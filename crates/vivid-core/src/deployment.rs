//! 分阶段安装：白名单 → baseline → operation snapshot → journal → 写入 → 回滚 → ledger。
//!
//! 这是整个项目最不能出错的一层：它是唯一会动游戏目录和注册表的代码。
//! 设计文档第 9 节把它拆成八步，这里逐条落地，每条都有测试守着：
//!
//! 1. **白名单先于一切。** 能被写的位置只有 `recipe` 输出的那几个，
//!    在线清单、ledger JSON、journal JSON 都不能新增路径。JSON 是可以被人改的，
//!    所以**反序列化回来的目标必须重新过一遍白名单**（`check_ledger_targets`）。
//! 2. **baseline 一经创建不被覆盖。** 用户更新三次，baseline 必须仍是 Vivid 接管前
//!    那份原始文件。第二次安装时把 baseline 刷成第一次装进去的文件，
//!    「还原」就会把用户带到一个他从没有过的状态——比不还原更糟。
//!    备份文件丢了也**不重新采集**：那样只会把 Vivid 装的文件记成「原始文件」。
//! 3. **写之前先落 journal。** 进程被杀、断电之后，下次启动 `recover_pending`
//!    能照着 operation snapshot 逆序退回操作前的状态。
//! 4. **失败逐个逆序回滚**，回滚也失败就保留 journal 并明说，不假装成功。
//! 5. **冲突不覆盖。** 目标被别的工具改过时，更新和还原都停下来让用户决定。
//!
//! 错误文案一律写成能直接念给玩家听的中文。

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::artifact::{ArtifactKind, CachedArtifact, sha256_hex};
use crate::paths::AppPaths;
use crate::recipe::{GameId, GameProfile, GraphicsApi, recipe_for_profile};

/// installation ledger 的格式版本。不认识的版本一律拒绝，不猜。
pub const LEDGER_SCHEMA: u32 = 1;
/// baseline 索引的格式版本。
pub const BASELINE_SCHEMA: u32 = 1;
/// pending journal 的格式版本。
pub const JOURNAL_SCHEMA: u32 = 1;

// ---------------------------------------------------------------------------
// 目标与计划
// ---------------------------------------------------------------------------

/// 允许写的注册表位置。**只有 HKLM 一种**，而且子键仍要过白名单。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum RegistryLocation {
    Hklm { subkey: String },
}

/// 一个可被 Vivid 管理的目标。
///
/// 只有「文件」和「固定 HKLM DWORD」两种——反序列化一份 ledger 回来，
/// 最多也只能得到这两种东西，拿不到任意写权限。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ManagedTarget {
    File(PathBuf),
    RegistryDword {
        location: RegistryLocation,
        value_name: String,
    },
}

impl ManagedTarget {
    /// 给用户看的一句话位置描述。
    pub fn describe(&self) -> String {
        match self {
            Self::File(path) => path.display().to_string(),
            Self::RegistryDword {
                location: RegistryLocation::Hklm { subkey },
                value_name,
            } => format!(r"HKEY_LOCAL_MACHINE\{subkey} 里的 {value_name}"),
        }
    }
}

/// 预演时拍下的「目标当前长什么样」。执行前重新比对一次，
/// 不一致说明用户看完预览之后目标又被改了 → 按冲突处理。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExpectedCurrent {
    Absent,
    FileSha256(String),
    RegistryAbsent,
    RegistryDword(u32),
}

impl ExpectedCurrent {
    fn matches(&self, current: &Current) -> bool {
        match (self, current) {
            (Self::Absent, Current::FileAbsent) => true,
            (Self::FileSha256(expected), Current::File { sha256, .. }) => expected == sha256,
            (Self::RegistryAbsent, Current::RegistryAbsent) => true,
            (Self::RegistryDword(expected), Current::RegistryDword(actual)) => expected == actual,
            _ => false,
        }
    }
}

/// 计划里的一步。只有「写文件」和「写固定 DWORD」两种动作，没有删除、没有执行命令。
#[derive(Debug, Clone)]
pub enum PlannedAction {
    WriteFile {
        source: PathBuf,
        source_sha256: String,
        target: PathBuf,
        expected_current: ExpectedCurrent,
    },
    WriteRegistryDword {
        location: RegistryLocation,
        value_name: String,
        value: u32,
        expected_current: ExpectedCurrent,
    },
}

impl PlannedAction {
    pub fn target(&self) -> ManagedTarget {
        match self {
            Self::WriteFile { target, .. } => ManagedTarget::File(target.clone()),
            Self::WriteRegistryDword {
                location,
                value_name,
                ..
            } => ManagedTarget::RegistryDword {
                location: location.clone(),
                value_name: value_name.clone(),
            },
        }
    }

    pub fn expected_current(&self) -> &ExpectedCurrent {
        match self {
            Self::WriteFile {
                expected_current, ..
            }
            | Self::WriteRegistryDword {
                expected_current, ..
            } => expected_current,
        }
    }
}

/// 一次安装/更新的完整计划。生成它不写任何磁盘（设计文档 9.1 的「预演」）。
#[derive(Debug, Clone)]
pub struct InstallPlan {
    pub game_id: GameId,
    pub recipe_version: u32,
    pub components: Vec<InstalledArtifact>,
    pub actions: Vec<PlannedAction>,
    pub requires_admin: bool,
}

/// 装进去的那份组件的来源与指纹。界面必须能展示来源 URL 与 SHA-256。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstalledArtifact {
    pub kind: ArtifactKind,
    pub version: String,
    pub source_url: String,
    pub final_url: String,
    pub sha256: String,
    pub size: u64,
}

impl From<&CachedArtifact> for InstalledArtifact {
    fn from(cached: &CachedArtifact) -> Self {
        Self {
            kind: cached.kind,
            version: cached.version.clone(),
            source_url: cached.source_url.clone(),
            final_url: cached.final_url.clone(),
            sha256: cached.sha256.clone(),
            size: cached.size,
        }
    }
}

/// Vivid 接管之前，这个位置原本是什么样。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum OriginalState {
    /// 原本没有这个文件——还原时要把 Vivid 新增的文件删掉。
    Absent,
    File {
        backup: PathBuf,
        sha256: String,
    },
    RegistryAbsent,
    RegistryDword(u32),
}

/// Vivid 装进去之后，这个位置应该是什么样。冲突检测拿它和现状比。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum InstalledState {
    File { sha256: String },
    RegistryDword(u32),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TargetRecord {
    pub target: ManagedTarget,
    pub original: OriginalState,
    pub installed: InstalledState,
    /// 内容由所装组件自己维护。**只作记录用**，冲突判定以当前配方为准——
    /// 这样旧的安装记录也能享受到规则修正，不必重装。
    #[serde(default)]
    pub component_owned: bool,
}

/// 一款游戏的安装记录。还原、更新、冲突检测全靠它。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallationLedger {
    pub schema: u32,
    pub game_id: GameId,
    /// 装进去的时候判定的图形 API。
    ///
    /// 记下来是为了**还原不受判定漂移影响**：清单更新、游戏改了设置之后
    /// 重新判定可能得出别的 API，而还原必须按当初装的那套来。
    ///
    /// 老 ledger 没有这个字段（用户机器上那两份就是），反序列化得到 `None`——
    /// 那只可能是两款已验证游戏，它们的 API 写在代码里，补得回来。
    #[serde(default)]
    pub api: Option<GraphicsApi>,
    pub recipe_version: u32,
    pub installed_at: String,
    pub components: Vec<InstalledArtifact>,
    pub targets: Vec<TargetRecord>,
}

/// 一个目标在本次操作**之前**的状态，用于中途失败时逆序回滚。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotEntry {
    pub target: ManagedTarget,
    pub previous: OriginalState,
}

/// 写入前落盘的待处理记录。进程被杀之后靠它把现场收拾干净。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingJournal {
    pub schema: u32,
    pub game_id: GameId,
    /// 恢复时要用它重建白名单，所以必须落盘。
    pub game_root: PathBuf,
    /// 重建白名单需要的其余事实（图形 API、exe）。
    ///
    /// 老 journal 没有这个字段——那只可能是两款已验证游戏，
    /// 它们的档案写在代码里，靠 `GameProfile::verified` 补得回来。
    /// 恢复时会核对它和 `game_id` / `game_root` 一致，对不上就当记录损坏。
    #[serde(default)]
    pub profile: Option<GameProfile>,
    pub started_at: String,
    pub entries: Vec<SnapshotEntry>,
}

/// baseline 索引：每个目标「Vivid 接管前」的原始状态。
#[derive(Debug, Clone, Serialize, Deserialize)]
struct BaselineIndex {
    schema: u32,
    game_id: GameId,
    entries: Vec<BaselineEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BaselineEntry {
    target: ManagedTarget,
    original: OriginalState,
}

// ---------------------------------------------------------------------------
// 错误
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum DeployError {
    #[error("这个位置不在 {game} 的允许范围内，Vivid 不会碰它：{target}")]
    NotAllowed { game: String, target: String },

    #[error("这份计划是给「{plan}」的，不能装到「{deployment}」上")]
    WrongGame { plan: String, deployment: String },

    #[error("找不到已下载的组件文件：{0}")]
    SourceMissing(PathBuf),

    #[error("组件文件校验不通过，可能没下完或被改过：{file}（期望 {expected}，实际 {actual}）")]
    SourceHashMismatch {
        file: PathBuf,
        expected: String,
        actual: String,
    },

    #[error("{target} 已经被别的程序改过（{detail}），Vivid 不会直接覆盖它")]
    Conflict { target: String, detail: String },

    #[error("写入 {target} 失败：{reason}。已经改动的部分都退回原样了")]
    ApplyFailed { target: String, reason: String },

    #[error("退回 {target} 时又失败了：{reason}。恢复记录已保留，下次启动 Vivid 会继续处理")]
    RollbackFailed { target: String, reason: String },

    #[error("{target} 的原始备份已经损坏，为免拿坏文件覆盖游戏，还原停止了")]
    BaselineCorrupted { target: String },

    #[error("恢复记录和目标类型对不上：{0}")]
    StateMismatch(String),

    #[error("这个游戏还没有 Vivid 的安装记录")]
    NotInstalled,

    #[error("Vivid 的安装记录是不认识的版本（{0}），请先还原再重新安装")]
    UnknownSchema(u32),

    #[error("待处理记录读不懂，文件已保留供排查：{0}")]
    PendingCorrupted(PathBuf),

    #[error("{0}")]
    Io(String),

    #[error("读写 Vivid 记录失败：{0}")]
    Json(String),

    #[error("当前系统不支持这个操作：{0}")]
    Unsupported(String),
}

// ---------------------------------------------------------------------------
// 系统边界
// ---------------------------------------------------------------------------

/// 真正会动系统的那几个动作。抽成 trait 是为了让测试能用临时目录 + 内存注册表跑，
/// 并且能在第 N 次写入时注入失败——回滚路径必须被真的执行过，不能只靠读代码相信它。
pub trait SystemBackend {
    fn read_file(&self, path: &Path) -> Result<Option<Vec<u8>>, DeployError>;
    fn replace_file(&mut self, path: &Path, bytes: &[u8]) -> Result<(), DeployError>;
    fn remove_file(&mut self, path: &Path) -> Result<(), DeployError>;
    /// 删掉一个**空**目录。非空、不存在、没权限一律当没发生——
    /// 这个方法只负责收走 Vivid 自己建出来的空壳，绝不做递归删除。
    fn remove_empty_dir(&mut self, path: &Path) -> Result<(), DeployError>;
    fn read_registry_dword(
        &self,
        location: &RegistryLocation,
        name: &str,
    ) -> Result<Option<u32>, DeployError>;
    fn write_registry_dword(
        &mut self,
        location: &RegistryLocation,
        name: &str,
        value: u32,
    ) -> Result<(), DeployError>;
    fn remove_registry_value(
        &mut self,
        location: &RegistryLocation,
        name: &str,
    ) -> Result<(), DeployError>;
}

/// 生产实现：**只允许白名单里的目标**，写文件走「同目录临时文件 → 校验 → 替换」。
pub struct RealSystemBackend {
    allowlist: TargetAllowlist,
}

impl RealSystemBackend {
    pub fn new(allowlist: TargetAllowlist) -> Self {
        Self { allowlist }
    }

    fn check_file(&self, path: &Path) -> Result<(), DeployError> {
        self.allowlist
            .check(&ManagedTarget::File(path.to_path_buf()))
    }

    fn check_registry(&self, location: &RegistryLocation, name: &str) -> Result<(), DeployError> {
        self.allowlist.check(&ManagedTarget::RegistryDword {
            location: location.clone(),
            value_name: name.to_string(),
        })
    }
}

impl SystemBackend for RealSystemBackend {
    fn read_file(&self, path: &Path) -> Result<Option<Vec<u8>>, DeployError> {
        self.check_file(path)?;
        match std::fs::read(path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(DeployError::Io(format!("读 {} 失败：{e}", path.display()))),
        }
    }

    fn replace_file(&mut self, path: &Path, bytes: &[u8]) -> Result<(), DeployError> {
        self.check_file(path)?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)
                .map_err(|e| DeployError::Io(format!("建目录 {} 失败：{e}", dir.display())))?;
        }
        // 同目录临时文件：跨目录 rename 可能跨盘，而且中途断电时游戏目录里
        // 至少不会留下半个文件。
        let tmp = path.with_extension("vivid-tmp");
        std::fs::write(&tmp, bytes)
            .map_err(|e| DeployError::Io(format!("写临时文件 {} 失败：{e}", tmp.display())))?;
        let written =
            std::fs::read(&tmp).map_err(|e| DeployError::Io(format!("回读临时文件失败：{e}")))?;
        if sha256_hex(&written) != sha256_hex(bytes) {
            let _ = std::fs::remove_file(&tmp);
            return Err(DeployError::Io(format!(
                "{} 写出来的内容校验不过，已放弃这次写入",
                path.display()
            )));
        }
        // Windows 上 rename 会覆盖同名文件。
        std::fs::rename(&tmp, path).map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            DeployError::Io(format!("替换 {} 失败：{e}", path.display()))
        })
    }

    fn remove_file(&mut self, path: &Path) -> Result<(), DeployError> {
        self.check_file(path)?;
        match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(DeployError::Io(format!(
                "删除 {} 失败：{e}",
                path.display()
            ))),
        }
    }

    fn remove_empty_dir(&mut self, path: &Path) -> Result<(), DeployError> {
        // remove_dir 只对空目录成功，非空会失败——这正是我们要的保险。
        let _ = std::fs::remove_dir(path);
        Ok(())
    }

    #[cfg(windows)]
    fn read_registry_dword(
        &self,
        location: &RegistryLocation,
        name: &str,
    ) -> Result<Option<u32>, DeployError> {
        use winreg::RegKey;
        use winreg::enums::HKEY_LOCAL_MACHINE;

        self.check_registry(location, name)?;
        let RegistryLocation::Hklm { subkey } = location;
        let Ok(key) = RegKey::predef(HKEY_LOCAL_MACHINE).open_subkey(subkey) else {
            return Ok(None);
        };
        Ok(key.get_value::<u32, _>(name).ok())
    }

    #[cfg(windows)]
    fn write_registry_dword(
        &mut self,
        location: &RegistryLocation,
        name: &str,
        value: u32,
    ) -> Result<(), DeployError> {
        use winreg::RegKey;
        use winreg::enums::HKEY_LOCAL_MACHINE;

        self.check_registry(location, name)?;
        let RegistryLocation::Hklm { subkey } = location;
        let (key, _) = RegKey::predef(HKEY_LOCAL_MACHINE)
            .create_subkey(subkey)
            .map_err(|e| DeployError::Io(format!("打开注册表项失败（可能需要管理员）：{e}")))?;
        key.set_value(name, &value)
            .map_err(|e| DeployError::Io(format!("写注册表值失败：{e}")))
    }

    #[cfg(windows)]
    fn remove_registry_value(
        &mut self,
        location: &RegistryLocation,
        name: &str,
    ) -> Result<(), DeployError> {
        use winreg::RegKey;
        use winreg::enums::{HKEY_LOCAL_MACHINE, KEY_ALL_ACCESS};

        self.check_registry(location, name)?;
        let RegistryLocation::Hklm { subkey } = location;
        let Ok(key) =
            RegKey::predef(HKEY_LOCAL_MACHINE).open_subkey_with_flags(subkey, KEY_ALL_ACCESS)
        else {
            return Ok(()); // 键都没了，值自然也没了
        };
        match key.delete_value(name) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(DeployError::Io(format!("删注册表值失败：{e}"))),
        }
    }

    #[cfg(not(windows))]
    fn read_registry_dword(
        &self,
        _location: &RegistryLocation,
        _name: &str,
    ) -> Result<Option<u32>, DeployError> {
        Err(DeployError::Unsupported("读注册表".to_string()))
    }

    #[cfg(not(windows))]
    fn write_registry_dword(
        &mut self,
        _location: &RegistryLocation,
        _name: &str,
        _value: u32,
    ) -> Result<(), DeployError> {
        Err(DeployError::Unsupported("写注册表".to_string()))
    }

    #[cfg(not(windows))]
    fn remove_registry_value(
        &mut self,
        _location: &RegistryLocation,
        _name: &str,
    ) -> Result<(), DeployError> {
        Err(DeployError::Unsupported("删注册表值".to_string()))
    }
}

// ---------------------------------------------------------------------------
// 白名单
// ---------------------------------------------------------------------------

/// 允许写入的位置集合。**这是「在线清单不能新增写入路径」的机器表达。**
///
/// 只能由代码内的固定配方构造：`for_profile` 走 `recipe_for_profile`，
/// `with_extra` 只接受调用方的代码内常量（全局 Vulkan layer 的目标从
/// `for_profile` 里加进来，这样 `recover_pending` 也自动认得它们）。
///
/// 泛化之后唯一的变量是**图形 API**——它决定注入 DLL 叫什么名字、要不要装全局层。
/// 但 API 只能取那七个枚举值，每个值对应一组写死在代码里的文件名，
/// 所以「清单不能新增路径」这条边界没有松动。
#[derive(Debug, Clone)]
pub struct TargetAllowlist {
    profile: GameProfile,
    keys: BTreeSet<String>,
    targets: Vec<ManagedTarget>,
    /// 内容归所装组件自己维护的目标键。见 `GameRecipe::component_owned_targets`。
    component_owned: BTreeSet<String>,
    /// Vivid 为了放组件而**自己建出来的**目录。还原时如果空了就收走，
    /// 不给用户留一个空壳（非空一律不动——里面可能有别人的东西）。
    created_dirs: Vec<PathBuf>,
}

impl TargetAllowlist {
    /// 按游戏档案构造。
    ///
    /// 判不出 API 的游戏没有任何目标，白名单为空 = 一律拒绝，
    /// **绝不退化成放行**。
    pub fn for_profile(profile: &GameProfile) -> Self {
        let recipe = recipe_for_profile(profile);
        let mut targets: Vec<ManagedTarget> = recipe
            .game_file_targets()
            .into_iter()
            .map(ManagedTarget::File)
            .collect();
        // Vulkan 方案还要写全局 layer。目标常量由 `vulkan_layer` 提供，
        // 白名单与实际写入共用同一份，也就自动认得它们。
        if profile.api == GraphicsApi::Vulkan && !targets.is_empty() {
            targets.extend(crate::vulkan_layer::global_layer_targets());
        }
        let component_owned = recipe
            .component_owned_targets()
            .into_iter()
            .map(|path| target_key(&ManagedTarget::File(path)))
            .collect();
        let mut allowlist = Self {
            profile: profile.clone(),
            keys: BTreeSet::new(),
            targets: Vec::new(),
            component_owned,
            created_dirs: crate::vulkan_layer::created_dirs(profile.api),
        };
        allowlist.extend(targets);
        allowlist
    }

    /// 按已验证游戏的固定档案构造（API 与 exe 都来自代码内常量）。
    pub fn for_verified_game(id: GameId, game_root: &Path) -> Self {
        match GameProfile::verified(id, game_root) {
            Some(profile) => Self::for_profile(&profile),
            // 不是已验证的两款游戏就构造不出档案 → 空白名单 = 一律拒绝。
            None => Self {
                profile: GameProfile {
                    app_id: id.app_id(),
                    name: id.label(),
                    root: game_root.to_path_buf(),
                    exe: String::new(),
                    api: GraphicsApi::Unknown,
                    verification: crate::recipe::Verification::Untested,
                },
                keys: BTreeSet::new(),
                targets: Vec::new(),
                component_owned: BTreeSet::new(),
                created_dirs: Vec::new(),
            },
        }
    }

    /// 追加固定的额外目标（只能来自代码内常量，不能来自清单或任何 JSON）。
    pub fn with_extra(mut self, targets: impl IntoIterator<Item = ManagedTarget>) -> Self {
        self.extend(targets);
        self
    }

    fn extend(&mut self, targets: impl IntoIterator<Item = ManagedTarget>) {
        for target in targets {
            if self.keys.insert(target_key(&target)) {
                self.targets.push(target);
            }
        }
    }

    pub fn game(&self) -> GameId {
        self.profile.id()
    }

    pub fn profile(&self) -> &GameProfile {
        &self.profile
    }

    pub fn game_root(&self) -> &Path {
        &self.profile.root
    }

    pub fn targets(&self) -> &[ManagedTarget] {
        &self.targets
    }

    pub fn permits(&self, target: &ManagedTarget) -> bool {
        self.keys.contains(&target_key(target))
    }

    /// 这个目标的内容是否由所装组件自己维护。
    ///
    /// ReShade 会把配置补成自己的完整格式、退出时存窗口布局。把这种变化
    /// 当成「别的程序改过」，会让每个开过它面板的用户都还原不了。
    /// **只放行内容变化**：白名单、缺失判定、还原写回一律照旧。
    pub fn is_component_owned(&self, target: &ManagedTarget) -> bool {
        self.component_owned.contains(&target_key(target))
    }

    /// Vivid 自己建出来的目录，还原后若为空则收走。
    pub fn created_dirs(&self) -> &[PathBuf] {
        &self.created_dirs
    }

    fn check(&self, target: &ManagedTarget) -> Result<(), DeployError> {
        if self.permits(target) {
            Ok(())
        } else {
            Err(DeployError::NotAllowed {
                game: self.profile.name.clone(),
                target: target.describe(),
            })
        }
    }
}

/// 目标的比较键。Windows 路径大小写不敏感、分隔符可能混用，统一成小写反斜杠。
fn target_key(target: &ManagedTarget) -> String {
    match target {
        ManagedTarget::File(path) => format!("file:{}", normalize_path_key(path)),
        ManagedTarget::RegistryDword {
            location: RegistryLocation::Hklm { subkey },
            value_name,
        } => format!(
            "hklm:{}|{}",
            subkey.replace('/', "\\").to_lowercase(),
            value_name.to_lowercase()
        ),
    }
}

fn normalize_path_key(path: &Path) -> String {
    path.to_string_lossy().replace('/', "\\").to_lowercase()
}

// ---------------------------------------------------------------------------
// 执行
// ---------------------------------------------------------------------------

/// 目标此刻的实际状态。
enum Current {
    FileAbsent,
    File { bytes: Vec<u8>, sha256: String },
    RegistryAbsent,
    RegistryDword(u32),
}

/// 一款游戏的安装执行器。
pub struct Deployment {
    paths: AppPaths,
    game_id: GameId,
    allowlist: TargetAllowlist,
}

impl Deployment {
    /// 按游戏档案构造（生产路径）。
    pub fn new(paths: &AppPaths, profile: &GameProfile) -> Self {
        Self::with_allowlist(paths, TargetAllowlist::for_profile(profile))
    }

    /// 用现成的白名单构造。游戏标识从白名单的档案里取，不另外传一份——
    /// 传两份就有对不上的可能。
    pub fn with_allowlist(paths: &AppPaths, allowlist: TargetAllowlist) -> Self {
        Self {
            paths: paths.clone(),
            game_id: allowlist.game(),
            allowlist,
        }
    }

    pub fn allowlist(&self) -> &TargetAllowlist {
        &self.allowlist
    }

    pub fn ledger_path(&self) -> PathBuf {
        self.paths
            .installations_dir()
            .join(format!("{}.json", self.game_id.slug()))
    }

    pub fn journal_path(&self) -> PathBuf {
        self.paths
            .pending_dir()
            .join(format!("{}.journal.json", self.game_id.slug()))
    }

    fn baseline_dir(&self) -> PathBuf {
        self.paths
            .backups_dir()
            .join(self.game_id.slug())
            .join("baseline")
    }

    fn snapshot_dir(&self) -> PathBuf {
        self.paths
            .pending_dir()
            .join(self.game_id.slug())
            .join("snapshot")
    }

    /// 读安装记录。没有记录返回 `None`（还没装过是正常情况，不是错误）。
    pub fn load_ledger(&self) -> Result<Option<InstallationLedger>, DeployError> {
        load_ledger_for(&self.paths, self.game_id)
    }

    /// 某个目标在 Vivid 接管之前的原始状态。界面「详情」和还原都用它。
    pub fn baseline_state(
        &self,
        target: &ManagedTarget,
    ) -> Result<Option<OriginalState>, DeployError> {
        let index = self.load_baseline()?;
        let key = target_key(target);
        Ok(index
            .entries
            .into_iter()
            .find(|entry| target_key(&entry.target) == key)
            .map(|entry| entry.original))
    }

    /// 设计文档 9.1–9.3 的八步，顺序固定。
    pub fn apply(
        &self,
        plan: &InstallPlan,
        backend: &mut dyn SystemBackend,
    ) -> Result<InstallationLedger, DeployError> {
        if plan.game_id != self.game_id {
            return Err(DeployError::WrongGame {
                plan: plan.game_id.label(),
                deployment: self.game_id.label(),
            });
        }

        // 第 1 步：校验来源哈希、配方目标、预演时的现场。任何一条不过就不碰磁盘。
        let mut prepared: Vec<(ManagedTarget, Current, &PlannedAction, Vec<u8>)> = Vec::new();
        for action in &plan.actions {
            let target = action.target();
            self.allowlist.check(&target)?;
            let bytes = match action {
                PlannedAction::WriteFile {
                    source,
                    source_sha256,
                    ..
                } => {
                    let bytes = std::fs::read(source)
                        .map_err(|_| DeployError::SourceMissing(source.clone()))?;
                    let actual = sha256_hex(&bytes);
                    if &actual != source_sha256 {
                        return Err(DeployError::SourceHashMismatch {
                            file: source.clone(),
                            expected: source_sha256.clone(),
                            actual,
                        });
                    }
                    bytes
                }
                PlannedAction::WriteRegistryDword { .. } => Vec::new(),
            };
            let current = self.read_current(&target, backend)?;
            if !action.expected_current().matches(&current) {
                return Err(DeployError::Conflict {
                    target: target.describe(),
                    detail: "预演之后这个位置又变了".to_string(),
                });
            }
            prepared.push((target, current, action, bytes));
        }

        // 第 2 步：已有安装记录时，先确认这些目标还是 Vivid 装的那份。
        let existing = self.load_ledger()?;
        if let Some(ledger) = &existing {
            self.check_ledger_targets(ledger)?;
            self.check_conflicts(ledger, backend)?;
        }

        // 第 3 步：建 baseline。已存在的一律不动——它是「Vivid 接管前」的唯一证据。
        let mut baseline = self.load_baseline()?;
        let mut baseline_changed = false;
        for (target, current, _, _) in &prepared {
            let key = target_key(target);
            if baseline
                .entries
                .iter()
                .any(|e| target_key(&e.target) == key)
            {
                continue;
            }
            let original = self.store_state(&self.baseline_dir(), target, current)?;
            baseline.entries.push(BaselineEntry {
                target: target.clone(),
                original,
            });
            baseline_changed = true;
        }
        if baseline_changed {
            atomic_write_json(&self.baseline_path(), &baseline)?;
        }

        // 第 4 步：本次操作的快照（回滚要用的是「操作前」，不是 baseline）。
        let snapshot_dir = self.snapshot_dir();
        let _ = std::fs::remove_dir_all(&snapshot_dir);
        let mut entries = Vec::new();
        for (target, current, _, _) in &prepared {
            entries.push(SnapshotEntry {
                target: target.clone(),
                previous: self.store_state(&snapshot_dir, target, current)?,
            });
        }

        // 第 5 步：原子写 pending journal。这之后每一步都能被下次启动收拾干净。
        let journal = PendingJournal {
            schema: JOURNAL_SCHEMA,
            game_id: self.game_id,
            game_root: self.allowlist.game_root().to_path_buf(),
            profile: Some(self.allowlist.profile().clone()),
            started_at: now_rfc3339(),
            entries,
        };
        let journal_path = self.journal_path();
        atomic_write_json(&journal_path, &journal)?;

        // 第 6 步：逐个写入；第 7 步：任一步失败就按快照逆序退回。
        let mut records = Vec::new();
        for (target, _, action, bytes) in &prepared {
            let outcome = match action {
                PlannedAction::WriteFile { target: path, .. } => {
                    self.write_file_checked(path, bytes, backend)
                }
                PlannedAction::WriteRegistryDword {
                    location,
                    value_name,
                    value,
                    ..
                } => backend
                    .write_registry_dword(location, value_name, *value)
                    .map(|()| InstalledState::RegistryDword(*value)),
            };
            match outcome {
                Ok(installed) => records.push((target.clone(), installed)),
                Err(e) => {
                    self.rollback(&journal, backend)?;
                    let _ = std::fs::remove_file(&journal_path);
                    self.clear_pending();
                    return Err(DeployError::ApplyFailed {
                        target: target.describe(),
                        reason: e.to_string(),
                    });
                }
            }
        }

        // 第 8 步：全部成功才写 ledger，写完再删 journal。
        let ledger = self.build_ledger(plan, &baseline, &records, existing.as_ref())?;
        atomic_write_json(&self.ledger_path(), &ledger)?;
        let _ = std::fs::remove_file(&journal_path);
        self.clear_pending();
        Ok(ledger)
    }

    /// 一键还原：先查冲突和备份完整性，全部恢复成功后才清记录。
    pub fn restore(&self, backend: &mut dyn SystemBackend) -> Result<(), DeployError> {
        let Some(ledger) = self.load_ledger()? else {
            return Err(DeployError::NotInstalled);
        };
        // ledger 是 JSON，可能被人改过：目标必须重新过白名单。
        self.check_ledger_targets(&ledger)?;
        self.check_conflicts(&ledger, backend)?;

        // 备份坏了宁可报错，也不能拿坏文件覆盖游戏。先全查一遍再动手。
        for record in &ledger.targets {
            if let OriginalState::File { backup, sha256 } = &record.original {
                let ok = std::fs::read(backup)
                    .map(|bytes| sha256_hex(&bytes) == *sha256)
                    .unwrap_or(false);
                if !ok {
                    return Err(DeployError::BaselineCorrupted {
                        target: record.target.describe(),
                    });
                }
            }
        }

        for record in ledger.targets.iter().rev() {
            self.restore_state(&record.target, &record.original, backend)?;
        }

        // 文件都删干净之后，把 Vivid 自己建的目录也收走（只删空的）。
        for dir in self.allowlist.created_dirs() {
            backend.remove_empty_dir(dir)?;
        }

        // 只有全部成功才清 ledger、pending 和 baseline。
        remove_file_if_present(&self.ledger_path())?;
        let _ = std::fs::remove_file(self.journal_path());
        self.clear_pending();
        let _ = std::fs::remove_dir_all(self.baseline_dir());
        // baseline 的父目录空了也一并收走，别在 backups 下留一堆空文件夹。
        let _ = std::fs::remove_dir(self.paths.backups_dir().join(self.game_id.slug()));
        Ok(())
    }

    /// 清掉本次操作的快照，顺带收走空掉的 pending 子目录，
    /// 不给用户留一堆空文件夹。
    fn clear_pending(&self) {
        let _ = std::fs::remove_dir_all(self.snapshot_dir());
        let _ = std::fs::remove_dir(self.paths.pending_dir().join(self.game_id.slug()));
    }

    fn baseline_path(&self) -> PathBuf {
        self.baseline_dir().join("baseline.json")
    }

    fn load_baseline(&self) -> Result<BaselineIndex, DeployError> {
        let path = self.baseline_path();
        let Ok(bytes) = std::fs::read(&path) else {
            return Ok(BaselineIndex {
                schema: BASELINE_SCHEMA,
                game_id: self.game_id,
                entries: Vec::new(),
            });
        };
        let index: BaselineIndex = serde_json::from_slice(&bytes)
            .map_err(|e| DeployError::Json(format!("{}：{e}", path.display())))?;
        if index.schema != BASELINE_SCHEMA {
            return Err(DeployError::UnknownSchema(index.schema));
        }
        Ok(index)
    }

    /// ledger 里的目标必须仍在白名单内，且属于同一款游戏。
    fn check_ledger_targets(&self, ledger: &InstallationLedger) -> Result<(), DeployError> {
        if ledger.game_id != self.game_id {
            return Err(DeployError::WrongGame {
                plan: ledger.game_id.label(),
                deployment: self.game_id.label(),
            });
        }
        for record in &ledger.targets {
            self.allowlist.check(&record.target)?;
        }
        Ok(())
    }

    /// 现状与 ledger 记录的「安装后」比对。目标缺失算「文件异常」不算冲突，
    /// 内容不同才是冲突——那说明有别的工具动过它。
    fn check_conflicts(
        &self,
        ledger: &InstallationLedger,
        backend: &dyn SystemBackend,
    ) -> Result<(), DeployError> {
        for record in &ledger.targets {
            if self.allowlist.is_component_owned(&record.target) {
                continue; // 内容归组件自己管，改了不算冲突
            }
            let current = self.read_current(&record.target, backend)?;
            let drifted = match (&record.installed, &current) {
                (InstalledState::File { sha256 }, Current::File { sha256: actual, .. }) => {
                    (sha256 != actual).then(|| "文件内容和 Vivid 装进去的不一样".to_string())
                }
                (InstalledState::RegistryDword(value), Current::RegistryDword(actual)) => {
                    (value != actual).then(|| format!("注册表值现在是 {actual}，不是 {value}"))
                }
                // 目标不见了：交给「修复文件」处理，不算冲突。
                (_, Current::FileAbsent) | (_, Current::RegistryAbsent) => None,
                _ => Some("这个位置的类型和记录对不上".to_string()),
            };
            if let Some(detail) = drifted {
                return Err(DeployError::Conflict {
                    target: record.target.describe(),
                    detail,
                });
            }
        }
        Ok(())
    }

    fn read_current(
        &self,
        target: &ManagedTarget,
        backend: &dyn SystemBackend,
    ) -> Result<Current, DeployError> {
        match target {
            ManagedTarget::File(path) => match backend.read_file(path)? {
                None => Ok(Current::FileAbsent),
                Some(bytes) => Ok(Current::File {
                    sha256: sha256_hex(&bytes),
                    bytes,
                }),
            },
            ManagedTarget::RegistryDword {
                location,
                value_name,
            } => match backend.read_registry_dword(location, value_name)? {
                None => Ok(Current::RegistryAbsent),
                Some(value) => Ok(Current::RegistryDword(value)),
            },
        }
    }

    /// 把「当前状态」存成可恢复的记录：文件内容落到 `dir/files/`，注册表值直接记。
    fn store_state(
        &self,
        dir: &Path,
        target: &ManagedTarget,
        current: &Current,
    ) -> Result<OriginalState, DeployError> {
        match current {
            Current::FileAbsent => Ok(OriginalState::Absent),
            Current::RegistryAbsent => Ok(OriginalState::RegistryAbsent),
            Current::RegistryDword(value) => Ok(OriginalState::RegistryDword(*value)),
            Current::File { bytes, sha256 } => {
                let files = dir.join("files");
                std::fs::create_dir_all(&files)
                    .map_err(|e| DeployError::Io(format!("建备份目录失败：{e}")))?;
                let backup = files.join(backup_file_name(target));
                std::fs::write(&backup, bytes)
                    .map_err(|e| DeployError::Io(format!("写备份失败：{e}")))?;
                Ok(OriginalState::File {
                    backup,
                    sha256: sha256.clone(),
                })
            }
        }
    }

    fn write_file_checked(
        &self,
        path: &Path,
        bytes: &[u8],
        backend: &mut dyn SystemBackend,
    ) -> Result<InstalledState, DeployError> {
        backend.replace_file(path, bytes)?;
        // 写完立刻回读比对：只信实际落盘的内容，不信「写调用没报错」。
        let expected = sha256_hex(bytes);
        match backend.read_file(path)? {
            Some(written) if sha256_hex(&written) == expected => {
                Ok(InstalledState::File { sha256: expected })
            }
            _ => Err(DeployError::Io(format!(
                "{} 写完之后校验不过",
                path.display()
            ))),
        }
    }

    /// 按快照逆序退回操作前的状态。回滚本身失败就保留 journal 并如实报错。
    fn rollback(
        &self,
        journal: &PendingJournal,
        backend: &mut dyn SystemBackend,
    ) -> Result<(), DeployError> {
        for entry in journal.entries.iter().rev() {
            if let Err(e) = self.restore_state(&entry.target, &entry.previous, backend) {
                return Err(DeployError::RollbackFailed {
                    target: entry.target.describe(),
                    reason: e.to_string(),
                });
            }
        }
        Ok(())
    }

    fn restore_state(
        &self,
        target: &ManagedTarget,
        state: &OriginalState,
        backend: &mut dyn SystemBackend,
    ) -> Result<(), DeployError> {
        match (target, state) {
            (ManagedTarget::File(path), OriginalState::Absent) => backend.remove_file(path),
            (ManagedTarget::File(path), OriginalState::File { backup, sha256 }) => {
                let bytes = std::fs::read(backup).map_err(|_| DeployError::BaselineCorrupted {
                    target: target.describe(),
                })?;
                if sha256_hex(&bytes) != *sha256 {
                    return Err(DeployError::BaselineCorrupted {
                        target: target.describe(),
                    });
                }
                backend.replace_file(path, &bytes)
            }
            (
                ManagedTarget::RegistryDword {
                    location,
                    value_name,
                },
                OriginalState::RegistryAbsent,
            ) => backend.remove_registry_value(location, value_name),
            (
                ManagedTarget::RegistryDword {
                    location,
                    value_name,
                },
                OriginalState::RegistryDword(value),
            ) => backend.write_registry_dword(location, value_name, *value),
            _ => Err(DeployError::StateMismatch(target.describe())),
        }
    }

    /// 本次写入的记录 + 上一份 ledger 里**本次没碰到**的目标。
    ///
    /// 后者不能丢：配方换代之后旧目标仍然装在游戏里，只有 ledger 记着它，
    /// 丢了就再也还原不回去。
    fn build_ledger(
        &self,
        plan: &InstallPlan,
        baseline: &BaselineIndex,
        records: &[(ManagedTarget, InstalledState)],
        existing: Option<&InstallationLedger>,
    ) -> Result<InstallationLedger, DeployError> {
        let originals: BTreeMap<String, OriginalState> = baseline
            .entries
            .iter()
            .map(|entry| (target_key(&entry.target), entry.original.clone()))
            .collect();

        let mut targets = Vec::new();
        for (target, installed) in records {
            let key = target_key(target);
            let original = originals.get(&key).cloned().ok_or_else(|| {
                DeployError::Io(format!("{} 缺少原始备份记录", target.describe()))
            })?;
            targets.push(TargetRecord {
                target: target.clone(),
                original,
                installed: installed.clone(),
                component_owned: self.allowlist.is_component_owned(target),
            });
        }

        let touched: BTreeSet<String> = records
            .iter()
            .map(|(target, _)| target_key(target))
            .collect();
        if let Some(previous) = existing {
            for record in &previous.targets {
                if !touched.contains(&target_key(&record.target)) {
                    targets.push(record.clone());
                }
            }
        }

        Ok(InstallationLedger {
            schema: LEDGER_SCHEMA,
            game_id: self.game_id,
            api: Some(self.allowlist.profile().api),
            recipe_version: plan.recipe_version,
            installed_at: now_rfc3339(),
            components: plan.components.clone(),
            targets,
        })
    }
}

/// 只按游戏标识读安装记录，**不需要先有档案**。
///
/// 还原和体检要靠它：装过的东西必须能还原，哪怕这款游戏后来被反作弊闸门拦下、
/// 或者图形 API 判定漂移了。ledger 记着当初装的是什么 API，档案由它重建。
pub fn load_ledger_for(
    paths: &AppPaths,
    game_id: GameId,
) -> Result<Option<InstallationLedger>, DeployError> {
    let path = paths
        .installations_dir()
        .join(format!("{}.json", game_id.slug()));
    let Ok(bytes) = std::fs::read(&path) else {
        return Ok(None);
    };
    let ledger: InstallationLedger = serde_json::from_slice(&bytes)
        .map_err(|e| DeployError::Json(format!("{}：{e}", path.display())))?;
    if ledger.schema != LEDGER_SCHEMA {
        return Err(DeployError::UnknownSchema(ledger.schema));
    }
    Ok(Some(ledger))
}

/// 待处理 journal 指向的游戏。**读文件名和 JSON，不碰游戏目录。**
///
/// 启动路径上要靠它先问一句「有没有事要收拾」：绝大多数情况下答案是没有，
/// 那就一步都不该做。之前这里是无条件把整个 Steam 库扫一遍来拼白名单，
/// 结果首帧被堵住十几秒，界面一片空白——**启动路径上不做整库扫描**。
pub fn pending_games(paths: &AppPaths) -> Vec<GameId> {
    let Ok(dir) = std::fs::read_dir(paths.pending_dir()) else {
        return Vec::new();
    };
    let mut games: Vec<GameId> = dir
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .is_some_and(|name| name.to_string_lossy().ends_with(".journal.json"))
        })
        .filter_map(|path| {
            let bytes = std::fs::read(&path).ok()?;
            let journal: PendingJournal = serde_json::from_slice(&bytes).ok()?;
            Some(journal.game_id)
        })
        .collect();
    games.sort();
    games.dedup();
    games
}

/// 启动时收拾上次被打断的操作：按快照逆序退回，成功后删 journal。
///
/// journal 也是 JSON，同样不可信：目录必须真是那款游戏的安装目录，
/// 目标必须过白名单，否则宁可报错保留文件，也不照着它写任何东西。
pub fn recover_pending(
    paths: &AppPaths,
    backend: &mut dyn SystemBackend,
) -> Result<Vec<GameId>, DeployError> {
    let Ok(dir) = std::fs::read_dir(paths.pending_dir()) else {
        return Ok(Vec::new());
    };
    let mut journals: Vec<PathBuf> = dir
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .is_some_and(|name| name.to_string_lossy().ends_with(".journal.json"))
        })
        .collect();
    journals.sort();

    let mut recovered = Vec::new();
    for path in journals {
        let bytes = std::fs::read(&path)
            .map_err(|e| DeployError::Io(format!("读 {} 失败：{e}", path.display())))?;
        let journal: PendingJournal = serde_json::from_slice(&bytes)
            .map_err(|_| DeployError::PendingCorrupted(path.clone()))?;
        if journal.schema != JOURNAL_SCHEMA {
            return Err(DeployError::PendingCorrupted(path));
        }
        // journal 里的档案必须和它自己的 game_id / game_root 对得上，
        // 否则这份记录说不清自己是谁，指向哪里都不能信。
        let profile = match &journal.profile {
            Some(profile) => {
                if profile.app_id != journal.game_id.app_id() || profile.root != journal.game_root {
                    return Err(DeployError::PendingCorrupted(path));
                }
                profile.clone()
            }
            // 老 journal 只可能属于两款已验证游戏，档案从代码里补。
            None => match GameProfile::verified(journal.game_id, &journal.game_root) {
                Some(profile) => profile,
                None => return Err(DeployError::PendingCorrupted(path)),
            },
        };
        // 目录里必须真有那款游戏的 exe，否则这份记录指向哪里都不能信。
        if !journal.game_root.join(&profile.exe).is_file() {
            return Err(DeployError::PendingCorrupted(path));
        }
        let allowlist = TargetAllowlist::for_profile(&profile);
        for entry in &journal.entries {
            allowlist.check(&entry.target)?;
        }

        let deployment = Deployment::with_allowlist(paths, allowlist);
        deployment.rollback(&journal, backend)?;
        remove_file_if_present(&path)?;
        let _ = std::fs::remove_dir_all(deployment.snapshot_dir());
        recovered.push(journal.game_id);
    }
    Ok(recovered)
}

// ---------------------------------------------------------------------------
// 小工具
// ---------------------------------------------------------------------------

/// 备份文件名：目标键的哈希前缀 + 原文件名。同名不同目录的目标不会互相覆盖。
fn backup_file_name(target: &ManagedTarget) -> String {
    let digest = sha256_hex(target_key(target).as_bytes());
    let name = match target {
        ManagedTarget::File(path) => path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "file".to_string()),
        ManagedTarget::RegistryDword { .. } => "registry".to_string(),
    };
    format!("{}-{name}", &digest[..16])
}

fn atomic_write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), DeployError> {
    let bytes = serde_json::to_vec_pretty(value).map_err(|e| DeployError::Json(e.to_string()))?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| DeployError::Io(format!("建目录 {} 失败：{e}", dir.display())))?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, &bytes)
        .map_err(|e| DeployError::Io(format!("写 {} 失败：{e}", tmp.display())))?;
    std::fs::rename(&tmp, path)
        .map_err(|e| DeployError::Io(format!("落位 {} 失败：{e}", path.display())))
}

/// 删除文件；本来就不存在算成功，别的失败要如实报。
fn remove_file_if_present(path: &Path) -> Result<(), DeployError> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(DeployError::Io(format!(
            "删除 {} 失败：{e}",
            path.display()
        ))),
    }
}

fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::FakeSystemBackend;

    /// 计划里的示例用 `a.dll` / `b.dll`，那两个名字不在 GTA5 配方白名单里，
    /// 会被 `NotAllowed` 提前拦下——测不到回滚。这里换成配方真有的两个目标。
    const FIRST: &str = "nvngx_dlss.dll";
    const SECOND: &str = "nvngx_dlssnr.dll";

    /// 组合三样东西：临时目录、Vivid 数据目录、假系统边界。
    /// 真实游戏目录、真实 HKLM、真实 `C:\ProgramData` 一概不碰。
    struct Fixture {
        _temp: tempfile::TempDir,
        paths: AppPaths,
        backend: FakeSystemBackend,
        game_root: PathBuf,
        sources: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().to_path_buf();
            let game_root = root.join("Grand Theft Auto V Enhanced");
            std::fs::create_dir_all(&game_root).unwrap();
            std::fs::write(game_root.join("GTA5_Enhanced.exe"), b"MZ").unwrap();
            let sources = root.join("cache");
            std::fs::create_dir_all(&sources).unwrap();
            let backend = FakeSystemBackend::new(&root);
            Self {
                paths: AppPaths::under(root.join("vivid-data")),
                backend,
                game_root,
                sources,
                _temp: temp,
            }
        }

        fn fail_on_apply_number(mut self, n: usize) -> Self {
            self.backend.fail_on_applies([n]);
            self
        }

        fn fail_on_apply_numbers(mut self, ns: impl IntoIterator<Item = usize>) -> Self {
            self.backend.fail_on_applies(ns);
            self
        }

        fn deployment(&self) -> Deployment {
            Deployment::new(&self.paths, &gta_profile(&self.game_root))
        }

        fn write_game(&self, name: &str, bytes: &[u8]) {
            std::fs::write(self.game_root.join(name), bytes).unwrap();
        }

        fn read_game(&self, name: &str) -> Vec<u8> {
            std::fs::read(self.game_root.join(name)).unwrap()
        }

        fn game_has(&self, name: &str) -> bool {
            self.game_root.join(name).is_file()
        }

        /// baseline 里存的那份「Vivid 接管之前」的内容。
        fn read_baseline(&self, name: &str) -> Vec<u8> {
            let target = ManagedTarget::File(self.game_root.join(name));
            match self.deployment().baseline_state(&target).unwrap().unwrap() {
                OriginalState::File { backup, .. } => std::fs::read(backup).unwrap(),
                other => panic!("baseline 不是文件：{other:?}"),
            }
        }

        /// 把 `bytes` 落成缓存来源文件，再按当前磁盘状态生成一个单目标计划。
        fn plan_with(&self, name: &str, bytes: &[u8]) -> InstallPlan {
            self.plan_for(&[(name, bytes)])
        }

        fn two_file_plan(&self) -> InstallPlan {
            self.plan_for(&[(FIRST, b"new-first"), (SECOND, b"new-second")])
        }

        fn plan_for(&self, items: &[(&str, &[u8])]) -> InstallPlan {
            let actions = items
                .iter()
                .map(|(name, bytes)| self.write_action(name, bytes))
                .collect();
            InstallPlan {
                game_id: GameId::Gta5Enhanced,
                recipe_version: 1,
                components: vec![fake_artifact()],
                actions,
                requires_admin: false,
            }
        }

        fn write_action(&self, name: &str, bytes: &[u8]) -> PlannedAction {
            let source = self.sources.join(name);
            std::fs::write(&source, bytes).unwrap();
            let target = self.game_root.join(name);
            PlannedAction::WriteFile {
                source_sha256: sha256_hex(bytes),
                source,
                expected_current: current_of(&target),
                target,
            }
        }

        fn deploy(&mut self, plan: InstallPlan) -> Result<InstallationLedger, DeployError> {
            let deployment = Deployment::new(&self.paths, &gta_profile(&self.game_root));
            deployment.apply(&plan, &mut self.backend)
        }

        /// 用户点「更新组件」：换一份新内容重跑一遍。
        fn update(&mut self) -> Result<InstallationLedger, DeployError> {
            let plan = self.plan_with(FIRST, b"updated");
            self.deploy(plan)
        }

        fn restore(&mut self) -> Result<(), DeployError> {
            let deployment = Deployment::new(&self.paths, &gta_profile(&self.game_root));
            deployment.restore(&mut self.backend)
        }

        fn ledger_path(&self) -> PathBuf {
            self.deployment().ledger_path()
        }

        fn journal_path(&self) -> PathBuf {
            self.deployment().journal_path()
        }
    }

    /// 目标当前状态 → 计划里记录的 `ExpectedCurrent`（预演时拍的照）。
    fn current_of(target: &std::path::Path) -> ExpectedCurrent {
        match std::fs::read(target) {
            Ok(bytes) => ExpectedCurrent::FileSha256(sha256_hex(&bytes)),
            Err(_) => ExpectedCurrent::Absent,
        }
    }

    /// 测试固定用 GTA5 Enhanced 的档案：API 与 exe 都来自代码内常量。
    fn gta_profile(game_root: &Path) -> GameProfile {
        GameProfile::verified(GameId::Gta5Enhanced, game_root).unwrap()
    }

    fn fake_artifact() -> InstalledArtifact {
        InstalledArtifact {
            kind: ArtifactKind::Dlss,
            version: "310.8.SF-v2".to_string(),
            source_url: "https://example.com/dlss.zip".to_string(),
            final_url: "https://cdn.example.com/dlss.zip".to_string(),
            sha256: "ab".repeat(32),
            size: 4096,
        }
    }

    /// 装好一份、ledger 已落盘的夹具。
    fn installed_fixture() -> Fixture {
        let mut fixture = Fixture::new();
        fixture.write_game(FIRST, b"original");
        let plan = fixture.plan_with(FIRST, b"installed");
        fixture.deploy(plan).unwrap();
        fixture
    }

    #[test]
    fn baseline_keeps_the_pre_vivid_state_across_updates() {
        let mut fixture = Fixture::new();
        fixture.write_game(FIRST, b"original");
        let first = fixture.plan_with(FIRST, b"first");
        fixture.deploy(first).unwrap();
        let second = fixture.plan_with(FIRST, b"second");
        fixture.deploy(second).unwrap();
        assert_eq!(fixture.read_baseline(FIRST), b"original");
        fixture.restore().unwrap();
        assert_eq!(fixture.read_game(FIRST), b"original");
    }

    #[test]
    fn a_failure_rolls_back_every_target_to_pre_operation_state() {
        let mut fixture = Fixture::new().fail_on_apply_number(2);
        fixture.write_game(FIRST, b"old-a");
        fixture.write_game(SECOND, b"old-b");
        let plan = fixture.two_file_plan();
        let err = fixture.deploy(plan).unwrap_err();
        assert!(matches!(err, DeployError::ApplyFailed { .. }), "{err:?}");
        assert_eq!(fixture.read_game(FIRST), b"old-a");
        assert_eq!(fixture.read_game(SECOND), b"old-b");
        assert!(!fixture.ledger_path().exists());
        // 回滚干净收尾：journal 不留。
        assert!(!fixture.journal_path().exists());
    }

    #[test]
    fn external_change_blocks_update_and_restore() {
        let mut fixture = installed_fixture();
        fixture.write_game(FIRST, b"changed-by-another-tool");
        assert!(matches!(
            fixture.update(),
            Err(DeployError::Conflict { .. })
        ));
        assert!(matches!(
            fixture.restore(),
            Err(DeployError::Conflict { .. })
        ));
        // 冲突不许顺手覆盖别人的文件。
        assert_eq!(fixture.read_game(FIRST), b"changed-by-another-tool");
        assert!(fixture.ledger_path().exists());
    }

    /// ReShade 装完会把配置写成自己的格式（补全默认值、存窗口布局）。
    /// **那是它自己的正常行为，不是外部篡改**——不能因此把更新和还原卡死，
    /// 否则每个打开过 ReShade 面板的用户都还原不了。2026-08-31 真机验收撞到过。
    #[test]
    fn a_component_owned_config_can_drift_without_blocking_restore() {
        const CONFIG: &str = "ReShade.ini";
        let mut fixture = Fixture::new();
        let plan = fixture.plan_with(CONFIG, b"seeded-by-vivid");
        fixture.deploy(plan).unwrap();

        // 组件自己改写了这个文件
        fixture.write_game(CONFIG, b"rewritten-by-reshade-itself");

        // 更新不该被拦
        let plan = fixture.plan_with(CONFIG, b"newer-seed");
        fixture.deploy(plan).unwrap();

        // 还原也不该被拦，而且要把 Vivid 新增的配置删掉
        fixture.restore().unwrap();
        assert!(!fixture.game_has(CONFIG), "还原要删掉 Vivid 新增的配置");
    }

    /// 原目标不存在时，还原要把 Vivid 新增的文件删掉，而不是留一个空壳。
    #[test]
    fn restore_deletes_files_vivid_added() {
        let mut fixture = Fixture::new();
        assert!(!fixture.game_has(FIRST));
        let plan = fixture.plan_with(FIRST, b"added-by-vivid");
        fixture.deploy(plan).unwrap();
        assert_eq!(fixture.read_game(FIRST), b"added-by-vivid");

        fixture.restore().unwrap();
        assert!(!fixture.game_has(FIRST), "Vivid 新增的文件必须删掉");
        assert!(!fixture.ledger_path().exists());
    }

    /// 备份坏了宁可报错，也不能拿坏文件覆盖游戏。
    #[test]
    fn corrupt_baseline_refuses_restore() {
        let mut fixture = installed_fixture();
        let target = ManagedTarget::File(fixture.game_root.join(FIRST));
        let OriginalState::File { backup, .. } = fixture
            .deployment()
            .baseline_state(&target)
            .unwrap()
            .unwrap()
        else {
            panic!("baseline 应该是文件");
        };
        std::fs::write(&backup, b"corrupted-backup").unwrap();

        let err = fixture.restore().unwrap_err();
        assert!(
            matches!(err, DeployError::BaselineCorrupted { .. }),
            "{err:?}"
        );
        // 游戏里仍是安装后的文件，没被坏备份覆盖。
        assert_eq!(fixture.read_game(FIRST), b"installed");
        assert!(fixture.ledger_path().exists());
    }

    /// 回滚中途再挂一次 → journal 留下；下次启动 `recover_pending` 收尾。
    #[test]
    fn recover_pending_rolls_back_interrupted_operation() {
        // 第 2 次写入失败触发回滚，回滚的第 1 次写入（第 3 次写入）也失败。
        let mut fixture = Fixture::new().fail_on_apply_numbers([2, 3]);
        fixture.write_game(FIRST, b"old-a");
        fixture.write_game(SECOND, b"old-b");
        let plan = fixture.two_file_plan();
        let err = fixture.deploy(plan).unwrap_err();
        assert!(matches!(err, DeployError::RollbackFailed { .. }), "{err:?}");
        assert!(fixture.journal_path().exists(), "回滚失败必须保留 journal");
        assert_eq!(fixture.read_game(FIRST), b"new-first", "第一个目标已被写入");

        let recovered = recover_pending(&fixture.paths, &mut fixture.backend).unwrap();
        assert_eq!(recovered, vec![GameId::Gta5Enhanced]);
        assert_eq!(fixture.read_game(FIRST), b"old-a");
        assert_eq!(fixture.read_game(SECOND), b"old-b");
        assert!(!fixture.journal_path().exists());
    }

    /// 坏 journal 不能被静默吞掉，也不能被删——留着给人诊断。
    #[test]
    fn corrupt_journal_reports_and_keeps_the_file() {
        let fixture = Fixture::new();
        let journal = fixture.journal_path();
        std::fs::create_dir_all(journal.parent().unwrap()).unwrap();
        std::fs::write(&journal, b"{ not json").unwrap();

        let mut backend = FakeSystemBackend::new(fixture.game_root.parent().unwrap());
        let err = recover_pending(&fixture.paths, &mut backend).unwrap_err();
        assert!(matches!(err, DeployError::PendingCorrupted(_)), "{err:?}");
        assert!(journal.exists(), "坏 journal 必须保留");
    }

    /// **泛化之前写下的 ledger 必须还读得回来。**
    ///
    /// 这份 JSON 的形状照抄 2026-08-31 真机验收留在
    /// `%LOCALAPPDATA%\Vivid\installations\gta5-enhanced.json` 里的那一份
    /// （URL 与哈希缩短了，字段一个不少，也一个不多——特别是没有 `api`）。
    /// 用户机器上 GTA5 与 RDR2 **正装着**：这条测试挂了，就等于那两份安装还原不回去。
    #[test]
    fn a_ledger_written_before_the_generalisation_still_loads() {
        const REAL_SHAPE: &str = r#"{
          "schema": 1,
          "game_id": "Gta5Enhanced",
          "recipe_version": 1,
          "installed_at": "2026-09-01T04:30:45.605496600+00:00",
          "components": [
            {
              "kind": "ReShade",
              "version": "6.8.0",
              "source_url": "https://reshade.me/downloads/ReShade_Setup_6.8.0_Addon.exe",
              "final_url": "https://reshade.me/downloads/ReShade_Setup_6.8.0_Addon.exe",
              "sha256": "0cee63f9c9f13f3ac909c5b4903f4dbb4b719a7ab3b4f13b0deaf83c814b94f7",
              "size": 5592064
            }
          ],
          "targets": [
            {
              "target": {"File": "H:\\Steam\\steamapps\\common\\GTA V\\ReShade.ini"},
              "original": "Absent",
              "installed": {"File": {"sha256": "aa"}},
              "component_owned": true
            },
            {
              "target": {"File": "H:\\Steam\\steamapps\\common\\GTA V\\nvngx_dlss.dll"},
              "original": {"File": {"backup": "C:\\V\\backups\\x.bin", "sha256": "bb"}},
              "installed": {"File": {"sha256": "cc"}},
              "component_owned": false
            }
          ]
        }"#;
        let ledger: InstallationLedger = serde_json::from_str(REAL_SHAPE).unwrap();
        assert_eq!(ledger.game_id, GameId::Gta5Enhanced);
        assert_eq!(ledger.game_id.slug(), "gta5-enhanced");
        // 老记录没有 api 字段；补得回来的地方是代码里的已验证档案。
        assert_eq!(ledger.api, None);
        assert_eq!(
            GameProfile::verified(ledger.game_id, Path::new(r"H:\gta"))
                .unwrap()
                .api,
            GraphicsApi::Dx12
        );
        assert_eq!(ledger.targets.len(), 2);
        assert!(ledger.targets[0].component_owned);
    }

    /// RDR2 那份 ledger 里有注册表目标和全局层路径，形状也要读得回来。
    #[test]
    fn the_rdr2_ledger_shape_with_a_registry_target_still_loads() {
        const REAL_SHAPE: &str = r#"{
          "schema": 1,
          "game_id": "RedDeadRedemption2",
          "recipe_version": 1,
          "installed_at": "2026-09-01T05:09:59.185693100+00:00",
          "components": [],
          "targets": [
            {
              "target": {"RegistryDword": {
                "location": {"Hklm": {"subkey": "SOFTWARE\\Khronos\\Vulkan\\ImplicitLayers"}},
                "value_name": "C:\\ProgramData\\ReShade\\ReShade64.json"
              }},
              "original": "RegistryAbsent",
              "installed": {"RegistryDword": 0},
              "component_owned": false
            }
          ]
        }"#;
        let ledger: InstallationLedger = serde_json::from_str(REAL_SHAPE).unwrap();
        assert_eq!(ledger.game_id.slug(), "rdr2");
        assert_eq!(
            GameProfile::verified(ledger.game_id, Path::new(r"H:\rdr2"))
                .unwrap()
                .api,
            GraphicsApi::Vulkan
        );
        assert!(matches!(
            ledger.targets[0].target,
            ManagedTarget::RegistryDword { .. }
        ));
    }

    /// 老 journal（没有 profile 字段）仍然能被收拾干净。
    #[test]
    fn a_journal_written_before_the_generalisation_still_recovers() {
        let mut fixture = Fixture::new();
        fixture.write_game(FIRST, b"vivid-wrote-this");
        let journal_path = fixture
            .paths
            .pending_dir()
            .join("gta5-enhanced.journal.json");
        std::fs::create_dir_all(journal_path.parent().unwrap()).unwrap();
        let old_shape = format!(
            r#"{{
              "schema": 1,
              "game_id": "Gta5Enhanced",
              "game_root": {},
              "started_at": "2026-08-31T12:00:00+00:00",
              "entries": [
                {{
                  "target": {{"File": {}}},
                  "previous": "Absent"
                }}
              ]
            }}"#,
            serde_json::to_string(&fixture.game_root).unwrap(),
            serde_json::to_string(&fixture.game_root.join(FIRST)).unwrap(),
        );
        std::fs::write(&journal_path, old_shape).unwrap();

        let recovered = recover_pending(&fixture.paths, &mut fixture.backend).unwrap();
        assert_eq!(recovered, vec![GameId::Gta5Enhanced]);
        assert!(
            !fixture.game_root.join(FIRST).exists(),
            "老 journal 也要能退回操作前的状态"
        );
        assert!(!journal_path.exists());
    }

    #[test]
    fn ledger_json_round_trip_keeps_every_field() {
        let ledger = InstallationLedger {
            schema: LEDGER_SCHEMA,
            game_id: GameId::RedDeadRedemption2,
            api: Some(GraphicsApi::Vulkan),
            recipe_version: 3,
            installed_at: "2026-08-31T12:00:00+00:00".to_string(),
            components: vec![fake_artifact()],
            targets: vec![
                TargetRecord {
                    target: ManagedTarget::File(PathBuf::from(r"C:\Games\RDR2\nvngx_dlss.dll")),
                    original: OriginalState::File {
                        backup: PathBuf::from(r"C:\Vivid\backups\rdr2\baseline\files\a.bin"),
                        sha256: "cd".repeat(32),
                    },
                    installed: InstalledState::File {
                        sha256: "ef".repeat(32),
                    },
                    component_owned: false,
                },
                TargetRecord {
                    target: ManagedTarget::RegistryDword {
                        location: RegistryLocation::Hklm {
                            subkey: r"SOFTWARE\Khronos\Vulkan\ImplicitLayers".to_string(),
                        },
                        value_name: r"C:\ProgramData\ReShade\ReShade64.json".to_string(),
                    },
                    original: OriginalState::RegistryAbsent,
                    installed: InstalledState::RegistryDword(0),
                    component_owned: true,
                },
            ],
        };
        let json = serde_json::to_string(&ledger).unwrap();
        let back: InstallationLedger = serde_json::from_str(&json).unwrap();
        assert_eq!(back, ledger);
    }

    /// 白名单是红线：配方之外的目标一个字节都不许写。
    #[test]
    fn targets_outside_the_recipe_are_rejected() {
        let mut fixture = Fixture::new();
        let outside = fixture.game_root.parent().unwrap().join("evil.dll");
        std::fs::write(fixture.sources.join("evil.dll"), b"MZ-evil").unwrap();
        let plan = InstallPlan {
            game_id: GameId::Gta5Enhanced,
            recipe_version: 1,
            components: Vec::new(),
            actions: vec![PlannedAction::WriteFile {
                source: fixture.sources.join("evil.dll"),
                source_sha256: sha256_hex(b"MZ-evil"),
                target: outside.clone(),
                expected_current: ExpectedCurrent::Absent,
            }],
            requires_admin: false,
        };
        let err = fixture.deploy(plan).unwrap_err();
        assert!(matches!(err, DeployError::NotAllowed { .. }), "{err:?}");
        assert!(!outside.exists());
        assert_eq!(
            fixture.backend.apply_count(),
            0,
            "拦截必须发生在任何写入之前"
        );
    }

    /// ledger 是 JSON，可能被人改过；载入后必须重新过白名单。
    #[test]
    fn tampered_ledger_targets_are_rejected_on_restore() {
        let mut fixture = installed_fixture();
        let outside = fixture.game_root.parent().unwrap().join("outside.dll");
        let path = fixture.ledger_path();
        let mut ledger: InstallationLedger =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        ledger.targets[0].target = ManagedTarget::File(outside.clone());
        std::fs::write(&path, serde_json::to_vec(&ledger).unwrap()).unwrap();

        let err = fixture.restore().unwrap_err();
        assert!(matches!(err, DeployError::NotAllowed { .. }), "{err:?}");
        assert!(!outside.exists());
    }

    /// 来源文件与计划记录的哈希对不上 → 停，不写游戏目录。
    #[test]
    fn source_hash_mismatch_is_rejected() {
        let mut fixture = Fixture::new();
        fixture.write_game(FIRST, b"original");
        let mut plan = fixture.plan_with(FIRST, b"good-source");
        if let PlannedAction::WriteFile { source_sha256, .. } = &mut plan.actions[0] {
            *source_sha256 = "00".repeat(32);
        }
        let err = fixture.deploy(plan).unwrap_err();
        assert!(
            matches!(err, DeployError::SourceHashMismatch { .. }),
            "{err:?}"
        );
        assert_eq!(fixture.read_game(FIRST), b"original");
        assert_eq!(fixture.backend.apply_count(), 0);
    }

    /// 用户看完预演之后目标又被改了 → 按冲突处理，不照着旧照片写。
    #[test]
    fn preview_drift_is_rejected() {
        let mut fixture = Fixture::new();
        fixture.write_game(FIRST, b"original");
        let plan = fixture.plan_with(FIRST, b"installed");
        fixture.write_game(FIRST, b"changed-after-preview");
        let err = fixture.deploy(plan).unwrap_err();
        assert!(matches!(err, DeployError::Conflict { .. }), "{err:?}");
        assert_eq!(fixture.read_game(FIRST), b"changed-after-preview");
    }

    /// 注册表目标：写入走内存 map，还原回到「原本不存在」。
    /// GTA5 配方没有注册表目标，这里用固定的 Vulkan 键构造白名单——
    /// Task 7 的全局 layer 会走同一条路径。
    #[test]
    fn registry_dword_is_written_and_restored() {
        let mut fixture = Fixture::new();
        let location = RegistryLocation::Hklm {
            subkey: r"SOFTWARE\Khronos\Vulkan\ImplicitLayers".to_string(),
        };
        let value_name = r"C:\ProgramData\ReShade\ReShade64.json".to_string();
        let allowlist =
            TargetAllowlist::for_verified_game(GameId::Gta5Enhanced, &fixture.game_root)
                .with_extra([ManagedTarget::RegistryDword {
                    location: location.clone(),
                    value_name: value_name.clone(),
                }]);
        let plan = InstallPlan {
            game_id: GameId::Gta5Enhanced,
            recipe_version: 1,
            components: Vec::new(),
            actions: vec![PlannedAction::WriteRegistryDword {
                location: location.clone(),
                value_name: value_name.clone(),
                value: 0,
                expected_current: ExpectedCurrent::RegistryAbsent,
            }],
            requires_admin: true,
        };

        let deployment = Deployment::with_allowlist(&fixture.paths, allowlist.clone());
        deployment.apply(&plan, &mut fixture.backend).unwrap();
        assert_eq!(
            fixture.backend.registry_value(&location, &value_name),
            Some(0)
        );

        let deployment = Deployment::with_allowlist(&fixture.paths, allowlist);
        deployment.restore(&mut fixture.backend).unwrap();
        assert_eq!(fixture.backend.registry_value(&location, &value_name), None);
    }
}
