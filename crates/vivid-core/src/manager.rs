//! 把清单、上游发布、组件缓存、固定配方和分阶段安装串成一条流程。
//!
//! 这是 GUI 唯一需要打交道的入口：GUI 不下载、不改文件、不碰注册表，
//! 只调用这里的 `prepare` / `apply` / `update` / `repair` / `restore`。
//!
//! 三条约束在这里汇合：
//!
//! 1. **在线清单只决定版本和来源**，写到哪、按什么顺序写，全部由代码内配方决定；
//! 2. **先在缓存里把四个组件凑齐并校验，再动游戏目录**——预演失败时磁盘零改动；
//! 3. **生成的 INI/JSON 也先落缓存**，再作为带 SHA-256 的普通 source 进计划，
//!    绝不把没记录过的内存内容直接写进游戏目录。

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::artifact::{ArtifactCache, ArtifactError, ArtifactKind, ArtifactSpec, sha256_hex};
use crate::catalog::{self, Eligibility};
use crate::deployment::{
    DeployError, Deployment, ExpectedCurrent, InstallPlan, InstallationLedger, InstalledArtifact,
    InstalledState, ManagedTarget, PlannedAction, RealSystemBackend, SystemBackend,
    TargetAllowlist,
};
use crate::evidence::{
    GameState, LoadEvidence, PrimaryAction, evidence_summary, failure_line, inspect_log,
    primary_action,
};
use crate::gpu::GpuInfo;
use crate::net::HttpClient;
use crate::paths::AppPaths;
use crate::recipe::{
    self, DiscoveredGame, GameId, GameProfile, GraphicsApi, RECIPE_VERSION, RecipeError,
    Verification, recipe_for_profile, validate_profile, validate_snapshot_support,
};
use crate::rhi_manifest::{ManifestError, RhiSnapshot, SnapshotOrigin};
use crate::steam::SteamApp;
use crate::upstream::{ContainerKind, UpstreamError, UpstreamSnapshot};
use crate::vulkan_layer::{self, VulkanError};

use crate::artifact::CachedArtifact;
use crate::rhi_manifest::LoadedSnapshot;
use crate::upstream::LoadedUpstreamSnapshot;

/// 通用的 ReShade 配置模板（RHI 稳定版口径）。文本模板，不是二进制。
///
/// 文件名还叫 `reshade.gta5.ini` 是历史原因——它的内容本来就不含任何
/// 游戏专有设置，所以 GTA5 之外的游戏也用它。RDR2 有自己那份带深度缓冲设置的，
/// 在 `vulkan_layer` 里。
const GENERIC_INI_TEMPLATE: &str = include_str!("../assets/reshade.gta5.ini");

/// 这款游戏用哪份 ReShade 配置模板。
///
/// **两款已验证游戏必须拿到真机验收时用的那一份**，一个字节都不能变；
/// 其余游戏用通用模板。
pub const fn ini_template(id: GameId) -> &'static str {
    match id {
        GameId::RedDeadRedemption2 => vulkan_layer::rdr2_ini_template(),
        _ => GENERIC_INI_TEMPLATE,
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ManagerError {
    #[error("没找到{game}的安装目录，请确认它已经通过 Steam 安装")]
    GameNotFound { game: String },

    #[error("判不出这款游戏用的是哪种图形 API，Vivid 不猜，也不提供安装")]
    UnknownApi,

    #[error("{0}")]
    BlockedByAntiCheat(String),

    #[error("这张显卡（{model}）不在 RHI 上游声明的范围内，Vivid 暂不支持")]
    UnsupportedGpu { model: String },

    #[error("RHI 清单里没有{0}，暂时不能安装或更新")]
    ManifestMissing(&'static str),

    #[error("{0}")]
    Recipe(#[from] RecipeError),

    #[error("{0}")]
    Manifest(#[from] ManifestError),

    #[error("{0}")]
    Upstream(#[from] UpstreamError),

    #[error("{0}")]
    Artifact(#[from] ArtifactError),

    #[error("{0}")]
    Vulkan(#[from] VulkanError),

    #[error("{0}")]
    Deploy(#[from] DeployError),

    #[error("{0}")]
    Launch(#[from] crate::launcher::LaunchError),

    #[error("这个游戏还没有 Vivid 的安装记录")]
    NotInstalled,

    #[error("准备安装文件失败：{0}")]
    Io(String),
}

/// 本机环境探测。抽成 trait 是为了让自动测试不依赖真实 Steam 和真实显卡。
pub trait EnvironmentProbe: Send + Sync {
    fn find_game(&self, game_id: GameId) -> Option<SteamApp>;
    /// 本机 Steam 库里所有已安装的游戏。游戏面铺开之后界面列的就是它。
    fn all_games(&self) -> Vec<SteamApp>;
    fn primary_gpu(&self) -> Option<GpuInfo>;
}

/// 生产实现：读真实注册表和真实 Steam 库。
pub struct WindowsEnvironment;

impl EnvironmentProbe for WindowsEnvironment {
    fn find_game(&self, game_id: GameId) -> Option<SteamApp> {
        crate::steam::find_app(game_id.app_id())
    }

    fn all_games(&self) -> Vec<SteamApp> {
        crate::steam::all_apps()
    }

    fn primary_gpu(&self) -> Option<GpuInfo> {
        crate::gpu::detect().into_iter().next()
    }
}

/// 预演结果：装什么、从哪来、写到哪、是新增还是替换。
#[derive(Debug, Clone)]
pub struct PreparedInstall {
    pub game: DiscoveredGame,
    pub snapshot_origin: SnapshotOrigin,
    pub artifacts: Vec<CachedArtifact>,
    pub plan: InstallPlan,
    /// 只列**从网络取来的组件**。Vivid 自己生成的配置文件不在这里——
    /// 它们没有来源 URL，混进来会让「来源」这一列变得可疑。
    pub preview: Vec<PreviewRow>,
}

impl PreparedInstall {
    /// 这款游戏的已知后果，安装前原样展示。
    pub fn known_caveats(&self) -> Vec<&'static str> {
        self.game.recipe.known_caveats()
    }

    /// 去掉需要管理员权限的那几步。
    ///
    /// RDR2 的全局 Vulkan layer 由提权 helper 单独执行，普通进程只装游戏目录里的部分——
    /// 提权范围要尽可能小，不该为了几个文件把整个安装都拉进管理员权限。
    pub fn without_elevated_actions(mut self) -> Self {
        let elevated: Vec<ManagedTarget> = crate::vulkan_layer::global_layer_targets();
        self.plan
            .actions
            .retain(|action| !elevated.contains(&action.target()));
        self.preview
            .retain(|row| !elevated.contains(&ManagedTarget::File(row.target.clone())));
        self
    }

    pub fn artifact_kinds(&self) -> Vec<ArtifactKind> {
        self.artifacts
            .iter()
            .map(|artifact| artifact.kind)
            .collect()
    }

    /// 按写入顺序列出**全部**目标，供安装前的预演确认。
    pub fn planned_targets(&self) -> Vec<PlannedSummary> {
        self.plan
            .actions
            .iter()
            .map(|action| match action {
                PlannedAction::WriteFile {
                    source,
                    target,
                    expected_current,
                    ..
                } => {
                    let artifact = self.artifacts.iter().find(|a| &a.path == source);
                    let change = change_of(expected_current);
                    PlannedSummary {
                        target: target.display().to_string(),
                        origin: match artifact {
                            Some(a) => format!("{} {}", component_label(a.kind), a.version),
                            None => "Vivid 生成的配置".to_string(),
                        },
                        source_url: artifact.map(|a| a.source_url.clone()),
                        sha256: artifact.map(|a| a.sha256.clone()),
                        current: (change == PreviewChange::Replace)
                            .then(|| describe_current(target)),
                        change,
                    }
                }
                PlannedAction::WriteRegistryDword {
                    location,
                    value_name,
                    value,
                    expected_current,
                } => PlannedSummary {
                    target: ManagedTarget::RegistryDword {
                        location: location.clone(),
                        value_name: value_name.clone(),
                    }
                    .describe(),
                    origin: format!("Vulkan 全局层注册项，值 {value}"),
                    source_url: None,
                    sha256: None,
                    current: None,
                    change: change_of(expected_current),
                },
            })
            .collect()
    }
}

#[derive(Debug, Clone)]
pub struct PreviewRow {
    pub component: ArtifactKind,
    pub version: String,
    pub source_url: String,
    pub target: PathBuf,
    pub change: PreviewChange,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreviewChange {
    Add,
    Replace,
}

impl PreviewChange {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Add => "新增",
            Self::Replace => "替换",
        }
    }
}

/// 预演对话框里的一行：**每个目标都有一行**，包括 Vivid 自己生成的配置
/// 和注册表值——设计文档 9.1 要求「列出来源、版本、每个目标、将新增还是替换」，
/// 少列一个目标，用户就是在对一份不完整的清单点确认。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedSummary {
    /// 写到哪（文件路径或注册表位置）。
    pub target: String,
    /// 这份内容是什么、哪来的（组件名 + 版本，或者「Vivid 生成」）。
    pub origin: String,
    /// 下载来源。Vivid 自己生成的内容没有 URL——**这一格宁可空着也不编**。
    pub source_url: Option<String>,
    /// 下载物的 SHA-256（本地指纹，不是上游签名）。
    pub sha256: Option<String>,
    /// 这个位置**现在**是什么文件。只有标「替换」的行才有值。
    ///
    /// 防的是把用户已经装好的 Special K / ENB / DXVK 顶掉而他自己不知道：
    /// 预演清单上写着「替换 dxgi.dll」，用户得看得见被替换掉的是哪一个。
    pub current: Option<String>,
    pub change: PreviewChange,
}

/// 目标位置现在那个文件的一句话描述。
///
/// 认得出就说名字和版本（由文件自己的版本资源提供，不做映射表）；
/// 认不出就说清「认不出」并给出大小——**不编一个名字出来**。
fn describe_current(target: &Path) -> String {
    if let Some(described) = crate::version::describe_pe_file(target) {
        return described;
    }
    match std::fs::metadata(target).map(|meta| meta.len()) {
        Ok(size) => format!("认不出是什么（{size} 字节）"),
        Err(_) => "认不出是什么".to_string(),
    }
}

/// 一款游戏的只读体检结果。**不下载、不写游戏目录、不碰注册表。**
#[derive(Debug, Clone)]
pub struct GameInspection {
    pub game: Option<DiscoveredGame>,
    pub gpu: Option<GpuInfo>,
    pub state: GameState,
    /// 目标文件的哈希与 ledger 一致。**这只证明文件到位了，不证明画面变了。**
    pub files_installed: bool,
    pub load_evidence: LoadEvidence,
    /// 本次判断依据的清单来源。`inspect` 只读本地缓存，所以取到就是 `Cache`；
    /// 「在线清单不可用」由 GUI 按自己那次 `refresh_snapshot` 的结果显示。
    pub snapshot_origin: Option<SnapshotOrigin>,
    /// 折叠详情里逐行展示的中文说明（显卡、组件、来源、日志）。
    pub detail_lines: Vec<String>,
}

impl GameInspection {
    /// 这个状态对应的唯一主按钮。
    pub fn primary_action(&self) -> PrimaryAction {
        match &self.game {
            Some(game) => primary_action(&self.state, game.recipe.id),
            None => PrimaryAction::None,
        }
    }

    /// 三层证据的固定文案。
    pub fn evidence_lines(&self) -> Vec<&'static str> {
        evidence_summary(self.files_installed, self.load_evidence)
    }
}

/// 一次「这款游戏现在是什么情况」的解析结果。
struct Resolution {
    app: SteamApp,
    surveyed: catalog::SurveyedGame,
    ledger: Option<InstallationLedger>,
    /// 可以拿来构造配方的档案。判不出 API 又没装过时为 `None`。
    profile: Option<GameProfile>,
}

/// 裁决结果 → 「不提供安装」的状态。可装时返回 `None`。
fn blocked_state(eligibility: &Eligibility) -> Option<GameState> {
    match eligibility {
        Eligibility::Installable(_) => None,
        Eligibility::BlockedByAntiCheat(found) => Some(GameState::BlockedByAntiCheat {
            reason: found.reason().unwrap_or_else(|| "检测到反作弊".to_string()),
        }),
        Eligibility::UnknownApi => Some(GameState::UnknownApi),
        Eligibility::NotAGame => Some(GameState::NotAGame),
    }
}

/// 按已有的安装记录重建档案。
///
/// **API 取 ledger 记的那一个**：装进去的是什么就按什么还原。
/// 老 ledger 没记 API（泛化之前写的），那只可能是两款已验证游戏，从代码补。
fn installed_profile(
    game_id: GameId,
    app: &SteamApp,
    ledger: &InstallationLedger,
    fresh: Option<&GameProfile>,
) -> Option<GameProfile> {
    let api = ledger
        .api
        .or_else(|| game_id.verified_api())
        .or_else(|| fresh.map(|profile| profile.api))?;
    let exe = game_id
        .verified_exe()
        .map(str::to_string)
        .or_else(|| fresh.map(|profile| profile.exe.clone()))
        .or_else(|| catalog::main_exe(&app.game_root))?;
    Some(GameProfile {
        app_id: app.app_id,
        name: app.name.clone(),
        root: app.game_root.clone(),
        exe,
        api,
        verification: if game_id.is_verified() {
            Verification::Verified
        } else {
            Verification::Untested
        },
    })
}

/// 安装编排器。GUI 持有一个，所有联网与写盘都从这里发起。
pub struct VividManager<C: HttpClient, E: EnvironmentProbe> {
    pub paths: AppPaths,
    pub http: C,
    pub environment: E,
}

impl<C: HttpClient, E: EnvironmentProbe> VividManager<C, E> {
    pub fn new(paths: AppPaths, http: C, environment: E) -> Self {
        Self {
            paths,
            http,
            environment,
        }
    }

    /// 刷新 RHI 清单，失败回退 last-known-good。
    pub fn refresh_snapshot(&self) -> Result<LoadedSnapshot, ManagerError> {
        Ok(crate::rhi_manifest::refresh_or_cached(
            &self.http,
            &self.paths.manifest_cache(),
        )?)
    }

    /// 刷新 ReShade / RenoDX 上游发布，失败回退 last-known-good。
    pub fn refresh_upstream(&self) -> Result<LoadedUpstreamSnapshot, ManagerError> {
        Ok(crate::upstream::refresh_or_cached(
            &self.http,
            &self.paths.upstream_cache(),
        )?)
    }

    /// 生成只读安装计划。**任何一步不确定就失败，且不写游戏目录一个字节。**
    ///
    /// `backend` 只用来读当前状态（决定新增还是替换、填 `ExpectedCurrent`）。
    pub fn prepare(
        &self,
        game_id: GameId,
        backend: &dyn SystemBackend,
    ) -> Result<PreparedInstall, ManagerError> {
        let game = self.discover_installable(game_id)?;
        self.check_gpu()?;

        let snapshot = self.refresh_snapshot()?;
        validate_snapshot_support(game_id, &snapshot.snapshot)?;
        let upstream = self.refresh_upstream()?;

        // 四个组件全部下载并校验完成之后，才开始生成动作。
        let cache = ArtifactCache::new(self.paths.artifacts_dir());
        let mut artifacts = Vec::new();
        for spec in artifact_specs(&snapshot.snapshot, &upstream.snapshot)? {
            artifacts.push(cache.ensure(&self.http, &spec)?);
        }

        let actions = self.actions_for(&game, &artifacts, backend)?;
        let plan = InstallPlan {
            game_id,
            recipe_version: RECIPE_VERSION,
            components: artifacts.iter().map(InstalledArtifact::from).collect(),
            actions,
            requires_admin: game.recipe.requires_admin,
        };
        let preview = preview_rows(&plan, &artifacts);

        Ok(PreparedInstall {
            game,
            snapshot_origin: snapshot.origin,
            artifacts,
            plan,
            preview,
        })
    }

    /// 执行预演好的计划。
    pub fn apply(
        &self,
        prepared: PreparedInstall,
        backend: &mut dyn SystemBackend,
    ) -> Result<InstallationLedger, ManagerError> {
        Ok(self
            .deployment(&prepared.game)
            .apply(&prepared.plan, backend)?)
    }

    /// 更新组件：重新预演一次，再按新的 operation snapshot 执行。
    /// **只在用户点击时调用**，Vivid 不自动替换游戏文件。
    pub fn update(
        &self,
        game_id: GameId,
        backend: &mut dyn SystemBackend,
    ) -> Result<InstallationLedger, ManagerError> {
        let prepared = self.prepare(game_id, &*backend)?;
        self.apply(prepared, backend)
    }

    /// 修复：只补回 ledger 记着、但现在不见了的目标。有冲突时由 `apply` 拒绝。
    pub fn repair(
        &self,
        game_id: GameId,
        backend: &mut dyn SystemBackend,
    ) -> Result<InstallationLedger, ManagerError> {
        let mut prepared = self.prepare(game_id, &*backend)?;
        let deployment = self.deployment(&prepared.game);
        let ledger = deployment
            .load_ledger()?
            .ok_or(ManagerError::NotInstalled)?;

        prepared.plan.actions.retain(|action| {
            let target = action.target();
            let recorded = ledger.targets.iter().any(|record| record.target == target);
            let missing = matches!(
                action.expected_current(),
                ExpectedCurrent::Absent | ExpectedCurrent::RegistryAbsent
            );
            recorded && missing
        });
        if prepared.plan.actions.is_empty() {
            return Ok(ledger); // 没有缺失目标，什么都不用做
        }
        self.apply(prepared, backend)
    }

    /// 一键还原。恢复文件与注册表原值，全部成功后才清记录。
    pub fn restore(
        &self,
        game_id: GameId,
        backend: &mut dyn SystemBackend,
    ) -> Result<(), ManagerError> {
        let game = self.discover(game_id)?;
        Ok(self.deployment(&game).restore(backend)?)
    }

    /// 一次性启动游戏。参数来自固定配方，**不写 Steam 的任何配置**。
    pub fn launch(&self, game_id: GameId) -> Result<(), ManagerError> {
        let game = self.discover(game_id)?;
        crate::launcher::launch(&game.app.steam_exe, &game.recipe.launch)?;
        Ok(())
    }

    /// 这款游戏对应的生产系统边界（只允许配方白名单里的目标）。
    pub fn backend_for(&self, game_id: GameId) -> Result<RealSystemBackend, ManagerError> {
        let game = self.discover(game_id)?;
        Ok(RealSystemBackend::new(TargetAllowlist::for_profile(
            &game.recipe.profile(),
        )))
    }

    /// 已装记录，供界面显示状态。没装过返回 `None`。
    pub fn installed_ledger(
        &self,
        game_id: GameId,
    ) -> Result<Option<InstallationLedger>, ManagerError> {
        let game = self.discover(game_id)?;
        Ok(self.deployment(&game).load_ledger()?)
    }

    /// 只读体检：Steam、显卡、本地清单缓存、ledger、目标现状和 ReShade 日志。
    ///
    /// 优先级固定为：未找到游戏 → 不支持显卡 → 冲突 → 缺文件 → 可更新 →
    /// 已安装 → 清单不可用 → 可安装。**冲突排在很前面**，因为一旦有别的工具
    /// 动过文件，任何自动操作都可能覆盖用户的改动。
    ///
    /// 不联网：清单只读 `refresh_snapshot` 写下的 last-known-good，
    /// 这样启动时刷新一次、两款游戏各体检一次，也只发生一次网络请求。
    pub fn inspect(
        &self,
        game_id: GameId,
        backend: &dyn SystemBackend,
    ) -> Result<GameInspection, ManagerError> {
        let gpu = self.environment.primary_gpu();
        let Ok(resolution) = self.resolve(game_id) else {
            return Ok(self.inspection(
                None,
                gpu,
                GameState::NotFound,
                false,
                LoadEvidence::NoLog,
                None,
                None,
            ));
        };

        // 不给装的两种情况有自己的状态，**不能退化成「没找到」**：
        // 用户要知道 Vivid 看见了这款游戏，以及为什么不给装。
        // 已经装过的游戏例外——它必须还能还原，所以照常往下走。
        if resolution.ledger.is_none()
            && let Some(state) = blocked_state(&resolution.surveyed.eligibility)
        {
            let game = resolution.profile.as_ref().map(|profile| DiscoveredGame {
                app: resolution.app.clone(),
                recipe: recipe_for_profile(profile),
            });
            return Ok(self.inspection(game, gpu, state, false, LoadEvidence::NoLog, None, None));
        }

        let Ok(game) = self.discover(game_id) else {
            return Ok(self.inspection(
                None,
                gpu,
                GameState::NotFound,
                false,
                LoadEvidence::NoLog,
                None,
                None,
            ));
        };
        if self.check_gpu().is_err() {
            return Ok(self.inspection(
                Some(game),
                gpu,
                GameState::UnsupportedGpu,
                false,
                LoadEvidence::NoLog,
                None,
                None,
            ));
        }

        let snapshot = crate::rhi_manifest::load_cached(&self.paths.manifest_cache());
        let upstream = crate::upstream::load_cached_upstream(&self.paths.upstream_cache());
        let origin = snapshot.as_ref().map(|_| SnapshotOrigin::Cache);

        let ledger = self.deployment(&game).load_ledger()?;
        let Some(ledger) = ledger else {
            // 没装过：能不能装取决于清单和上游发布都在。
            let installable = snapshot
                .as_ref()
                .is_some_and(|s| validate_snapshot_support(game_id, s).is_ok())
                && upstream.is_some();
            let state = if installable {
                GameState::ReadyToInstall
            } else {
                GameState::ManifestUnavailable
            };
            return Ok(self.inspection(
                Some(game),
                gpu,
                state,
                false,
                LoadEvidence::NoLog,
                origin,
                None,
            ));
        };

        // 逐个目标比对：内容不同是冲突，不见了是缺文件。
        let mut conflicts = Vec::new();
        let mut missing = Vec::new();
        let deployment = self.deployment(&game);
        for record in &ledger.targets {
            // 内容归组件自己维护的配置，改了不算冲突（缺失仍然算文件异常）。
            let component_owned = deployment.allowlist().is_component_owned(&record.target);
            let ManagedTarget::File(path) = &record.target else {
                continue; // 注册表值的缺失/改动由 deployment 在执行前再判一次
            };
            let InstalledState::File { sha256 } = &record.installed else {
                continue;
            };
            match backend.read_file(path)? {
                None => missing.push(path.clone()),
                Some(bytes) if !component_owned && &sha256_hex(&bytes) != sha256 => {
                    conflicts.push(path.clone())
                }
                Some(_) => {}
            }
        }

        if !conflicts.is_empty() {
            let state = GameState::Conflict { files: conflicts };
            return Ok(self.inspection(
                Some(game),
                gpu,
                state,
                false,
                LoadEvidence::NoLog,
                origin,
                Some(ledger),
            ));
        }
        if !missing.is_empty() {
            let state = GameState::MissingFiles { files: missing };
            return Ok(self.inspection(
                Some(game),
                gpu,
                state,
                false,
                LoadEvidence::NoLog,
                origin,
                Some(ledger),
            ));
        }

        // 文件都对得上，再看日志能给到哪一层证据。
        let evidence = self.log_evidence(&game, &ledger);
        let state = match available_update(&ledger, snapshot.as_ref(), upstream.as_ref()) {
            Some((installed, available)) => GameState::UpdateAvailable {
                installed,
                available,
            },
            // 清单读不到时不谎报「有更新」，也不拦着启动和还原。
            None => GameState::Installed,
        };
        Ok(self.inspection(Some(game), gpu, state, true, evidence, origin, Some(ledger)))
    }

    /// 体检本机 Steam 库里的**每一款**游戏。只读：不下载、不写游戏目录、不碰注册表。
    ///
    /// 界面启动时调一次。不给装的游戏也在结果里——用户要知道 Vivid 看见了它，
    /// 以及为什么不给装。体检失败的（读不动记录之类）跳过，不让一款游戏拖垮整张列表。
    pub fn inspect_library(&self) -> Vec<(SteamApp, GameInspection)> {
        self.environment
            .all_games()
            .into_iter()
            .filter_map(|app| {
                let game_id = GameId::from_app_id(app.app_id);
                let backend = match self.backend_for(game_id) {
                    Ok(backend) => backend,
                    // 构造不出白名单（判不出 API、被闸门拦下、目录里没有 exe）时
                    // 用一个空白名单：那条路上 `inspect` 直接返回「不提供安装」，
                    // 本来就不会去读任何目标。
                    Err(_) => RealSystemBackend::new(TargetAllowlist::for_profile(&GameProfile {
                        app_id: app.app_id,
                        name: app.name.clone(),
                        root: app.game_root.clone(),
                        exe: String::new(),
                        api: GraphicsApi::Unknown,
                        verification: Verification::Untested,
                    })),
                };
                let inspection = self.inspect(game_id, &backend).ok()?;
                Some((app, inspection))
            })
            .collect()
    }

    /// 本次安装之后的 ReShade 日志证据。
    ///
    /// 只看游戏目录里的 `ReShade.log`：RDR2 的全局 layer 到底把日志写在哪，
    /// 要等 Task 12 真机验收看过再说，**不在这里猜一个路径**。
    fn log_evidence(&self, game: &DiscoveredGame, ledger: &InstallationLedger) -> LoadEvidence {
        let Some(installed_at) = parse_rfc3339(&ledger.installed_at) else {
            return LoadEvidence::NoLog;
        };
        let log = game.recipe.game_root.join("ReShade.log");
        inspect_log(&log, installed_at).unwrap_or(LoadEvidence::NoLog)
    }

    #[allow(clippy::too_many_arguments)]
    fn inspection(
        &self,
        game: Option<DiscoveredGame>,
        gpu: Option<GpuInfo>,
        state: GameState,
        files_installed: bool,
        load_evidence: LoadEvidence,
        snapshot_origin: Option<SnapshotOrigin>,
        ledger: Option<InstallationLedger>,
    ) -> GameInspection {
        let mut detail_lines = Vec::new();
        detail_lines.push(match &gpu {
            Some(info) => format!("显卡：{}", info.summary()),
            None => "显卡：没读到显卡信息".to_string(),
        });
        if let Some(game) = &game {
            detail_lines.push(format!("游戏目录：{}", game.recipe.game_root.display()));
            detail_lines.push(format!("支持程度：{}", game.recipe.support_label));
        }
        if let Some(ledger) = &ledger {
            detail_lines.push(format!("安装时间：{}", ledger.installed_at));
            for component in &ledger.components {
                detail_lines.push(format!(
                    "{}：{}，来源 {}，SHA-256 {}",
                    component_label(component.kind),
                    component.version,
                    component.source_url,
                    &component.sha256[..component.sha256.len().min(12)]
                ));
            }
            for record in &ledger.targets {
                detail_lines.push(format!("目标：{}", record.target.describe()));
            }
        }
        if load_evidence == LoadEvidence::Failed
            && let Some(game) = &game
            && let Ok(text) = std::fs::read_to_string(game.recipe.game_root.join("ReShade.log"))
            && let Some(line) = failure_line(&text)
        {
            detail_lines.push(format!("日志：{line}"));
        }

        GameInspection {
            game,
            gpu,
            state,
            files_installed,
            load_evidence,
            snapshot_origin,
            detail_lines,
        }
    }

    /// 启动时收拾上次被打断的操作。
    ///
    /// 白名单取两款已发现游戏的并集：journal 里可能是任意一款游戏的记录。
    /// 一款游戏都没发现时返回空列表——journal 指向的目录必须真有那款游戏的 exe，
    /// 否则 `deployment::recover_pending` 本来也会拒绝。
    pub fn recover_pending(&self) -> Result<Vec<GameId>, ManagerError> {
        // 没有待处理记录就什么都不做。**这条早退让启动路径保持零开销。**
        if crate::deployment::pending_games(&self.paths).is_empty() {
            return Ok(Vec::new());
        }
        let Some(mut backend) = self.recovery_backend() else {
            return Ok(Vec::new());
        };
        Ok(crate::deployment::recover_pending(
            &self.paths,
            &mut backend,
        )?)
    }

    /// 覆盖**待处理 journal 指向的那几款游戏**的合并白名单。
    ///
    /// 只看 journal 里点到名的游戏，不扫整库：`pending` 目录绝大多数时候是空的，
    /// 那就应该一步都不做。白名单仍然走 `discover` 这条可信路径生成
    /// （Steam 清单 + 代码内配方），**不采信 journal 自己写的档案**——
    /// 那份档案是不可信输入，只在 `recover_pending` 里当作待校验的数据用。
    pub fn recovery_backend(&self) -> Option<RealSystemBackend> {
        let mut allowlist: Option<TargetAllowlist> = None;
        for game_id in crate::deployment::pending_games(&self.paths) {
            let Ok(game) = self.discover(game_id) else {
                continue;
            };
            let next = TargetAllowlist::for_profile(&game.recipe.profile());
            allowlist = Some(match allowlist {
                None => next,
                Some(existing) => existing.with_extra(next.targets().to_vec()),
            });
        }
        allowlist.map(RealSystemBackend::new)
    }

    fn deployment(&self, game: &DiscoveredGame) -> Deployment {
        Deployment::new(&self.paths, &game.recipe.profile())
    }

    /// 这款游戏此刻的全部事实：Steam 记录、裁决结果、已有安装记录、可用档案。
    ///
    /// **档案优先按 ledger 记的 API 重建。** 装过的东西必须还原得回来，
    /// 哪怕这款游戏后来被反作弊闸门拦下、或者 API 判定漂移了——
    /// 那时候按新判定生成的白名单会和装进去的目标对不上，还原就卡住了。
    fn resolve(&self, game_id: GameId) -> Result<Resolution, ManagerError> {
        let app =
            self.environment
                .find_game(game_id)
                .ok_or_else(|| ManagerError::GameNotFound {
                    game: game_id.label(),
                })?;
        let snapshot = crate::rhi_manifest::load_cached(&self.paths.manifest_cache());
        let surveyed = catalog::survey(vec![app.clone()], snapshot.as_ref())
            .pop()
            .expect("一进一出");
        let ledger = crate::deployment::load_ledger_for(&self.paths, game_id)?;

        let fresh = surveyed.profile().cloned();
        let profile = match &ledger {
            Some(ledger) => installed_profile(game_id, &app, ledger, fresh.as_ref()).or(fresh),
            None => fresh,
        };
        Ok(Resolution {
            app,
            surveyed,
            ledger,
            profile,
        })
    }

    /// 这款游戏此刻可用的档案。
    ///
    /// 提权 helper 和 GUI 都靠它判断「要不要管理员权限」——
    /// **按 ledger 记的 API 优先**，装进去时是什么方案，还原就按什么方案。
    pub fn profile_for(&self, game_id: GameId) -> Result<GameProfile, ManagerError> {
        self.resolve(game_id)?
            .profile
            .ok_or(ManagerError::UnknownApi)
    }

    /// 按 Steam App ID 发现游戏，并确认目录真是它的安装目录。
    ///
    /// 拿不到可用档案（判不出 API、被闸门拦下、目录里没有 exe）就失败——
    /// 这条路上的调用方都是要动磁盘的，**不确定就不动手**。
    fn discover(&self, game_id: GameId) -> Result<DiscoveredGame, ManagerError> {
        let resolution = self.resolve(game_id)?;
        let profile = resolution
            .profile
            .ok_or_else(|| match &resolution.surveyed.eligibility {
                Eligibility::BlockedByAntiCheat(found) => ManagerError::BlockedByAntiCheat(
                    found.reason().unwrap_or_else(|| "检测到反作弊".to_string()),
                ),
                Eligibility::UnknownApi => ManagerError::UnknownApi,
                _ => ManagerError::GameNotFound {
                    game: game_id.label(),
                },
            })?;
        validate_profile(&profile, &resolution.app)?;
        Ok(DiscoveredGame {
            app: resolution.app,
            recipe: recipe_for_profile(&profile),
        })
    }

    /// 只用于要写磁盘的操作：必须**现在**就通得过闸门与 API 判定。
    ///
    /// `discover` 会拿 ledger 里的档案兜底（那是为了还原），
    /// 但「装新东西」不能靠一份旧记录绕过闸门。
    fn discover_installable(&self, game_id: GameId) -> Result<DiscoveredGame, ManagerError> {
        let resolution = self.resolve(game_id)?;
        match &resolution.surveyed.eligibility {
            Eligibility::BlockedByAntiCheat(found) => {
                return Err(ManagerError::BlockedByAntiCheat(
                    found.reason().unwrap_or_else(|| "检测到反作弊".to_string()),
                ));
            }
            Eligibility::UnknownApi => return Err(ManagerError::UnknownApi),
            Eligibility::NotAGame => {
                return Err(ManagerError::GameNotFound {
                    game: game_id.label(),
                });
            }
            Eligibility::Installable(_) => {}
        }
        // 已经装过就沿用装进去时的档案，免得更新换了一套注入方式。
        let profile = resolution.profile.ok_or(ManagerError::UnknownApi)?;
        validate_profile(&profile, &resolution.app)?;
        Ok(DiscoveredGame {
            app: resolution.app,
            recipe: recipe_for_profile(&profile),
        })
    }

    /// 显卡是否在 RHI 上游声明的范围内。**认不出代次就不猜**。
    fn check_gpu(&self) -> Result<(), ManagerError> {
        let Some(gpu) = self.environment.primary_gpu() else {
            return Ok(()); // 读不到显卡信息时不拦，也不对用户下任何结论
        };
        match gpu.generation {
            Some(generation) if generation.supports_rhi_neural_rendering() => Ok(()),
            _ => Err(ManagerError::UnsupportedGpu { model: gpu.model }),
        }
    }

    /// 按图形 API 生成安装动作。
    ///
    /// 顺序固定：**注入方式 → 配置 → addon → 神经渲染 → 超分**。
    ///
    /// - Vulkan 走全局 implicit layer（`C:\ProgramData\ReShade` + HKLM 注册项），
    ///   不往游戏目录放注入 DLL；
    /// - 其余 API 把 ReShade 以固定的注入文件名写进游戏目录。
    ///
    /// 文件名全部来自 `recipe` 里的常量表，**清单一个路径都新增不了**。
    fn actions_for(
        &self,
        game: &DiscoveredGame,
        artifacts: &[CachedArtifact],
        backend: &dyn SystemBackend,
    ) -> Result<Vec<PlannedAction>, ManagerError> {
        let profile = game.recipe.profile();
        let root = &profile.root;
        let mut actions = Vec::new();

        if profile.api == GraphicsApi::Vulkan {
            let vulkan = vulkan_layer::build_vulkan_actions(
                root,
                &artifacts[0],
                &self.generated_dir(),
                backend,
                ini_template(profile.id()),
            )?;
            actions.extend(vulkan.elevated_actions()); // 需要管理员的那三样
            actions.extend(vulkan.user_actions()); // 游戏目录里的 reshade.ini
        } else {
            let dll = recipe::injection_dll(profile.api).ok_or(ManagerError::UnknownApi)?;
            actions.push(self.artifact_action(&artifacts[0], root.join(dll), backend)?);

            let ini_name = recipe::config_file_name(profile.api);
            let ini_target = root.join(ini_name);
            let existing = backend.read_file(&ini_target)?;
            let merged = vulkan_layer::merge_ini(
                read_text(existing.as_deref(), ini_name)?,
                ini_template(profile.id()),
            )?;
            let ini_source = self.stage_generated(ini_name, &merged)?;
            actions.push(file_action(
                &ini_source,
                &sha256_hex(&merged),
                ini_target,
                backend,
            )?);
        }

        actions.push(self.artifact_action(
            &artifacts[1],
            root.join(recipe::ADDON_FILE),
            backend,
        )?);
        actions.push(self.artifact_action(
            &artifacts[2],
            root.join(recipe::DLSSNR_FILE),
            backend,
        )?);
        actions.push(self.artifact_action(&artifacts[3], root.join(recipe::DLSS_FILE), backend)?);
        Ok(actions)
    }

    fn artifact_action(
        &self,
        artifact: &CachedArtifact,
        target: PathBuf,
        backend: &dyn SystemBackend,
    ) -> Result<PlannedAction, ManagerError> {
        file_action(&artifact.path, &artifact.sha256, target, backend)
    }

    fn generated_dir(&self) -> PathBuf {
        self.paths.cache_dir().join("generated")
    }

    /// 生成的文本按内容哈希落缓存：同样的内容永远是同一个文件，
    /// 重新预演也不会把还没执行的旧计划的 source 改掉。
    fn stage_generated(&self, name: &str, bytes: &[u8]) -> Result<PathBuf, ManagerError> {
        let dir = self.generated_dir().join(&sha256_hex(bytes)[..16]);
        std::fs::create_dir_all(&dir).map_err(|e| ManagerError::Io(e.to_string()))?;
        let path = dir.join(name);
        std::fs::write(&path, bytes).map_err(|e| ManagerError::Io(e.to_string()))?;
        Ok(path)
    }
}

/// 四个组件的固定来源。顺序即安装顺序：ReShade → RenoDX → DLSS-NR → DLSS。
fn artifact_specs(
    snapshot: &RhiSnapshot,
    upstream: &UpstreamSnapshot,
) -> Result<Vec<ArtifactSpec>, ManagerError> {
    let dlssnr = snapshot
        .dlss
        .dlssnr
        .first()
        .ok_or(ManagerError::ManifestMissing("DLSS 神经渲染组件"))?;
    let dlss = snapshot
        .dlss
        .dlss
        .first()
        .ok_or(ManagerError::ManifestMissing("DLSS 超分组件"))?;

    Ok(vec![
        ArtifactSpec {
            kind: ArtifactKind::ReShade,
            version: upstream.reshade.version.clone(),
            url: upstream.reshade.url.clone(),
            // setup 是壳，要的是里面那个 DLL。
            expected_file: "ReShade64.dll".to_string(),
            container: upstream.reshade.container,
        },
        ArtifactSpec {
            kind: ArtifactKind::RenoDxDlss5,
            version: upstream.renodx_dlss5.version.clone(),
            url: upstream.renodx_dlss5.url.clone(),
            expected_file: upstream.renodx_dlss5.file_name.clone(),
            container: upstream.renodx_dlss5.container,
        },
        ArtifactSpec {
            kind: ArtifactKind::DlssNr,
            version: dlssnr.version.clone(),
            url: dlssnr.url.clone(),
            expected_file: "nvngx_dlssnr.dll".to_string(),
            container: ContainerKind::Zip,
        },
        ArtifactSpec {
            kind: ArtifactKind::Dlss,
            version: dlss.version.clone(),
            url: dlss.url.clone(),
            expected_file: "nvngx_dlss.dll".to_string(),
            container: ContainerKind::Zip,
        },
    ])
}

/// 目标当前是空的就是「新增」，否则是「替换」。
fn change_of(expected: &ExpectedCurrent) -> PreviewChange {
    match expected {
        ExpectedCurrent::Absent | ExpectedCurrent::RegistryAbsent => PreviewChange::Add,
        _ => PreviewChange::Replace,
    }
}

/// 预演表格：一行一个从网络取来的组件。
fn preview_rows(plan: &InstallPlan, artifacts: &[CachedArtifact]) -> Vec<PreviewRow> {
    plan.actions
        .iter()
        .filter_map(|action| {
            let PlannedAction::WriteFile {
                source,
                target,
                expected_current,
                ..
            } = action
            else {
                return None;
            };
            let artifact = artifacts.iter().find(|a| &a.path == source)?;
            Some(PreviewRow {
                component: artifact.kind,
                version: artifact.version.clone(),
                source_url: artifact.source_url.clone(),
                target: target.clone(),
                change: change_of(expected_current),
            })
        })
        .collect()
}

/// 现有配置必须是文本才敢合并；不是文本就拒绝，不覆盖用户的东西。
fn read_text<'a>(bytes: Option<&'a [u8]>, name: &str) -> Result<Option<&'a str>, ManagerError> {
    match bytes {
        None => Ok(None),
        Some(raw) => match std::str::from_utf8(raw) {
            Ok(text) => Ok(Some(text)),
            Err(_) => Err(ManagerError::Vulkan(VulkanError::NotText(name.to_string()))),
        },
    }
}

/// 按目标当前状态生成一步「写文件」动作。
///
/// `ExpectedCurrent` 就是在这里拍的照：用户看完预演之后目标再被改，
/// `deployment::apply` 会照出不一致并按冲突处理。
fn file_action(
    source: &Path,
    source_sha256: &str,
    target: PathBuf,
    backend: &dyn SystemBackend,
) -> Result<PlannedAction, ManagerError> {
    let current = backend.read_file(&target)?;
    Ok(PlannedAction::WriteFile {
        source: source.to_path_buf(),
        source_sha256: source_sha256.to_string(),
        expected_current: match current {
            None => ExpectedCurrent::Absent,
            Some(bytes) => ExpectedCurrent::FileSha256(sha256_hex(&bytes)),
        },
        target,
    })
}

/// 已装版本和当前首项对不上就算「可更新」。返回第一处差异（旧版, 新版）。
///
/// 比的是**清单标签**，不是 PE 文件版本号：`310.8.SF-v2` 这种标签根本
/// 转不成纯数字版本，硬转只会比错。
fn available_update(
    ledger: &InstallationLedger,
    snapshot: Option<&RhiSnapshot>,
    upstream: Option<&UpstreamSnapshot>,
) -> Option<(String, String)> {
    for component in &ledger.components {
        let available = match component.kind {
            ArtifactKind::ReShade => upstream.map(|u| u.reshade.version.clone()),
            ArtifactKind::RenoDxDlss5 => upstream.map(|u| u.renodx_dlss5.version.clone()),
            ArtifactKind::DlssNr => snapshot
                .and_then(|s| s.dlss.dlssnr.first())
                .map(|entry| entry.version.clone()),
            ArtifactKind::Dlss => snapshot
                .and_then(|s| s.dlss.dlss.first())
                .map(|entry| entry.version.clone()),
        };
        let Some(available) = available else {
            continue; // 读不到就不下结论
        };
        if available != component.version {
            return Some((component.version.clone(), available));
        }
    }
    None
}

/// 界面上的组件中文名。
fn component_label(kind: ArtifactKind) -> &'static str {
    match kind {
        ArtifactKind::ReShade => "ReShade",
        ArtifactKind::RenoDxDlss5 => "RenoDX DLSS5 插件",
        ArtifactKind::DlssNr => "DLSS 神经渲染",
        ArtifactKind::Dlss => "DLSS 超分",
    }
}

/// ledger 里的 RFC 3339 时间 → `SystemTime`。解析不了就返回 `None`，不猜。
fn parse_rfc3339(text: &str) -> Option<SystemTime> {
    let parsed = chrono::DateTime::parse_from_rfc3339(text).ok()?;
    let seconds = parsed.timestamp();
    if seconds < 0 {
        return None;
    }
    Some(std::time::UNIX_EPOCH + std::time::Duration::from_secs(seconds as u64))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::artifact::sha256_hex;
    use crate::deployment::ManagedTarget;
    use crate::rhi_manifest::{DLSS_MANIFEST_URL, GAME_MANIFEST_URL};
    use crate::testing::{
        FakeEnvironment, FakeHttp, FakeSystemBackend, fake_payload, rtx_4070_ti_super,
        zip_with_files,
    };
    use crate::upstream::{RENODX_RELEASES_URL, RESHADE_HOME_URL};
    use std::path::Path;

    const GAME_MANIFEST: &str = include_str!("../tests/fixtures/rhi-manifest-min.json");
    const DLSS_MANIFEST: &str = include_str!("../tests/fixtures/rhi-dlss-manifest-min.json");
    const RELEASES_JSON: &str = include_str!("../tests/fixtures/rhi-releases-min.json");
    const RESHADE_HTML: &str = include_str!("../tests/fixtures/reshade-download-min.html");

    const RESHADE_URL: &str = "https://reshade.me/downloads/ReShade_Setup_6.8.1_Addon.exe";
    const RENODX_URL: &str = "https://github.com/RankFTW/rhi-repo/releases/download/renodx-dlss5-4.60/renodx-dlss5_4.60.zip";
    const DLSSNR_URL: &str = "https://github.com/RankFTW/rhi-repo/releases/download/dlssnr-310.8.SF-v2/nvngx_dlssnr_310.8.SF-v2.zip";
    const DLSS_URL: &str =
        "https://github.com/RankFTW/rhi-repo/releases/download/dlss-310.8.0/nvngx_dlss_310.8.0.zip";

    /// ReShade setup 是 PE 自解压壳 + 尾部 ZIP。
    fn reshade_setup() -> Vec<u8> {
        let mut payload = b"MZ-setup-stub-".to_vec();
        payload.extend_from_slice(&zip_with_files([(
            "ReShade64.dll",
            b"MZ-reshade".as_slice(),
        )]));
        payload
    }

    /// 四份清单 + 四个组件下载都答得上来的假网络。
    fn full_http() -> FakeHttp {
        let http = manifest_http(GAME_MANIFEST, DLSS_MANIFEST, RELEASES_JSON, RESHADE_HTML);
        push_artifacts(&http);
        http
    }

    fn manifest_http(games: &str, dlss: &str, releases: &str, reshade: &str) -> FakeHttp {
        FakeHttp::with_texts([
            (GAME_MANIFEST_URL, games),
            (DLSS_MANIFEST_URL, dlss),
            (RENODX_RELEASES_URL, releases),
            (RESHADE_HOME_URL, reshade),
        ])
    }

    fn push_artifacts(http: &FakeHttp) {
        http.push(RESHADE_URL, Ok(fake_payload(RESHADE_URL, reshade_setup())));
        http.push(
            RENODX_URL,
            Ok(fake_payload(
                RENODX_URL,
                zip_with_files([("renodx-dlss5.addon64", b"MZ-addon".as_slice())]),
            )),
        );
        http.push(
            DLSSNR_URL,
            Ok(fake_payload(
                DLSSNR_URL,
                zip_with_files([("nvngx_dlssnr.dll", b"MZ-nr".as_slice())]),
            )),
        );
        http.push(
            DLSS_URL,
            Ok(fake_payload(
                DLSS_URL,
                zip_with_files([("nvngx_dlss.dll", b"MZ-sr".as_slice())]),
            )),
        );
    }

    struct ManagerFixture {
        _temp: tempfile::TempDir,
        manager: VividManager<FakeHttp, FakeEnvironment>,
        backend: FakeSystemBackend,
        game_id: GameId,
        app: SteamApp,
        game_root: PathBuf,
        programdata: PathBuf,
    }

    impl ManagerFixture {
        fn gta() -> Self {
            Self::new(GameId::Gta5Enhanced, full_http())
        }

        fn rdr2() -> Self {
            Self::new(GameId::RedDeadRedemption2, full_http())
        }

        fn new(game_id: GameId, http: FakeHttp) -> Self {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().to_path_buf();
            let game_root = root.join(game_id.label());
            std::fs::create_dir_all(&game_root).unwrap();
            std::fs::write(game_root.join(game_id.verified_exe().unwrap()), b"MZ").unwrap();
            let steam_exe = root.join("steam.exe");
            std::fs::write(&steam_exe, b"MZ").unwrap();

            let app = SteamApp {
                app_id: game_id.app_id(),
                name: game_id.label().to_string(),
                install_dir: game_id.label().to_string(),
                game_root: game_root.clone(),
                steam_exe,
            };
            let programdata = root.join("programdata");
            let mut backend = FakeSystemBackend::new(&root);
            backend.map_programdata(Path::new("C:/ProgramData"), &programdata);

            Self {
                manager: VividManager::new(
                    AppPaths::under(root.join("vivid-data")),
                    http,
                    environment(game_id, &app, Some(rtx_4070_ti_super())),
                ),
                backend,
                game_id,
                app,
                game_root,
                programdata,
                _temp: temp,
            }
        }

        /// 换一套网络 / 显卡，其余（数据目录、游戏目录、缓存）全部保留。
        fn swap(mut self, http: FakeHttp, gpu: Option<GpuInfo>) -> Self {
            let paths = self.manager.paths.clone();
            let env = environment(self.game_id, &self.app, gpu);
            self.manager = VividManager::new(paths, http, env);
            self
        }

        fn prepare(&self) -> Result<PreparedInstall, ManagerError> {
            self.manager.prepare(self.game_id, &self.backend)
        }

        /// 游戏目录、虚拟 ProgramData 和注册表的完整指纹。
        fn system_snapshot(&self) -> Vec<String> {
            let mut out = Vec::new();
            for dir in [&self.game_root, &self.programdata] {
                for entry in walkdir::WalkDir::new(dir).into_iter().flatten() {
                    if entry.file_type().is_file() {
                        let bytes = std::fs::read(entry.path()).unwrap_or_default();
                        out.push(format!("{}={}", entry.path().display(), sha256_hex(&bytes)));
                    }
                }
            }
            out.extend(self.backend.registry_dump());
            out.sort();
            out
        }
    }

    fn environment(game_id: GameId, app: &SteamApp, gpu: Option<GpuInfo>) -> FakeEnvironment {
        FakeEnvironment::new(gpu).with_game(game_id, app.clone())
    }

    /// 造一款**未实测**的假游戏：目录里放哪些文件由参数决定，
    /// 图形 API 和裁决结果因此完全由这些文件说了算。
    fn untested_fixture(app_id: u32, name: &str, files: &[&str]) -> ManagerFixture {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().to_path_buf();
        let game_root = root.join(name);
        std::fs::create_dir_all(&game_root).unwrap();
        for relative in files {
            let path = game_root.join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, b"MZ").unwrap();
        }
        let steam_exe = root.join("steam.exe");
        std::fs::write(&steam_exe, b"MZ").unwrap();

        let game_id = GameId::from_app_id(app_id);
        let app = SteamApp {
            app_id,
            name: name.to_string(),
            install_dir: name.to_string(),
            game_root: game_root.clone(),
            steam_exe,
        };
        let programdata = root.join("programdata");
        let mut backend = FakeSystemBackend::new(&root);
        backend.map_programdata(Path::new("C:/ProgramData"), &programdata);

        ManagerFixture {
            manager: VividManager::new(
                AppPaths::under(root.join("vivid-data")),
                full_http(),
                environment(game_id, &app, Some(rtx_4070_ti_super())),
            ),
            backend,
            game_id,
            app,
            game_root,
            programdata,
            _temp: temp,
        }
    }

    /// 一款干净的未实测 DX12 游戏可以走完预演，注入 DLL 是 `dxgi.dll`。
    #[test]
    fn an_untested_dx12_game_can_be_prepared() {
        let fixture = untested_fixture(
            1_091_500,
            "Cyberpunk 2077",
            &["Cyberpunk2077.exe", "D3D12Core.dll"],
        );
        fixture.manager.refresh_snapshot().unwrap();
        let prepared = fixture.prepare().expect("干净的 DX12 单机游戏应该能预演");
        let targets: Vec<String> = prepared
            .plan
            .actions
            .iter()
            .map(|action| action.target().describe())
            .collect();
        assert!(targets[0].ends_with("dxgi.dll"), "{targets:?}");
        assert!(targets[1].ends_with("ReShade.ini"), "{targets:?}");
        assert!(targets[4].ends_with("nvngx_dlss.dll"), "{targets:?}");
        // 未实测的游戏必须有话说。
        let caveats = prepared.known_caveats().join("");
        assert!(caveats.contains("没有这款游戏"), "{caveats}");
    }

    /// 注入方式跟着 API 走：DX11 的游戏写的是 `d3d11.dll`。
    #[test]
    fn an_untested_dx11_game_plans_the_dx11_injection_dll() {
        let fixture =
            untested_fixture(1_091_501, "Old Game", &["OldGame.exe", "sga_directx11.dll"]);
        fixture.manager.refresh_snapshot().unwrap();
        let prepared = fixture.prepare().unwrap();
        let first = prepared.plan.actions[0].target().describe();
        assert!(first.ends_with("d3d11.dll"), "{first}");
        // 非 D3D12 必须说清神经渲染不生效。
        assert!(
            prepared
                .known_caveats()
                .join("")
                .contains("神经渲染不会生效"),
            "{:?}",
            prepared.known_caveats()
        );
    }

    /// **带反作弊的游戏既装不了，也要在体检里说清为什么。**
    #[test]
    fn an_anti_cheat_game_is_refused_and_inspects_as_blocked() {
        let fixture = untested_fixture(
            1_091_502,
            "Some Shooter",
            &[
                "Shooter.exe",
                "D3D12Core.dll",
                "EasyAntiCheat/EasyAntiCheat_x64.dll",
            ],
        );
        fixture.manager.refresh_snapshot().unwrap();
        assert!(matches!(
            fixture.prepare(),
            Err(ManagerError::BlockedByAntiCheat(_))
        ));

        let report = fixture
            .manager
            .inspect(fixture.game_id, &fixture.backend)
            .unwrap();
        assert!(matches!(report.state, GameState::BlockedByAntiCheat { .. }));
        assert_eq!(report.primary_action(), PrimaryAction::None);
        // 磁盘一个字节都没被碰过。
        assert_eq!(
            std::fs::read_dir(&fixture.game_root).unwrap().count(),
            3,
            "预演被拒绝时不该写任何东西"
        );
    }

    /// **整库体检把每一款游戏都列出来，包括不给装的那些。**
    #[test]
    fn the_library_inspection_lists_blocked_games_too() {
        let fixture = untested_fixture(
            1_091_504,
            "Some Shooter",
            &[
                "Shooter.exe",
                "D3D12Core.dll",
                "EasyAntiCheat/EasyAntiCheat_x64.dll",
            ],
        );
        fixture.manager.refresh_snapshot().unwrap();
        let library = fixture.manager.inspect_library();
        assert_eq!(library.len(), 1);
        let (app, report) = &library[0];
        assert_eq!(app.name, "Some Shooter");
        assert!(matches!(report.state, GameState::BlockedByAntiCheat { .. }));
        assert_eq!(report.primary_action(), PrimaryAction::None);
    }

    /// 判不出 API 的游戏同样不给装，状态说得出为什么。
    #[test]
    fn a_game_with_an_undetectable_api_inspects_as_unknown_api() {
        let fixture = untested_fixture(1_091_503, "Mystery Game", &["Mystery.exe"]);
        fixture.manager.refresh_snapshot().unwrap();
        assert!(matches!(fixture.prepare(), Err(ManagerError::UnknownApi)));

        let report = fixture
            .manager
            .inspect(fixture.game_id, &fixture.backend)
            .unwrap();
        assert_eq!(report.state, GameState::UnknownApi);
        assert_eq!(report.primary_action(), PrimaryAction::None);
    }

    fn file_names(plan: &InstallPlan) -> Vec<String> {
        plan.actions
            .iter()
            .map(|action| match action.target() {
                ManagedTarget::File(path) => {
                    path.file_name().unwrap().to_string_lossy().to_string()
                }
                ManagedTarget::RegistryDword { .. } => "<registry>".to_string(),
            })
            .collect()
    }

    #[test]
    fn prepare_gta_downloads_four_artifacts_and_builds_fixed_plan() {
        let fixture = ManagerFixture::gta();
        let prepared = fixture.prepare().unwrap();
        assert_eq!(
            prepared.artifact_kinds(),
            [
                ArtifactKind::ReShade,
                ArtifactKind::RenoDxDlss5,
                ArtifactKind::DlssNr,
                ArtifactKind::Dlss,
            ]
        );
        assert_eq!(prepared.plan.actions.len(), 5);
        assert!(!prepared.plan.requires_admin);
        assert!(
            prepared
                .preview
                .iter()
                .all(|row| row.source_url.starts_with("https://"))
        );
        assert_eq!(
            file_names(&prepared.plan),
            vec![
                "dxgi.dll",
                "ReShade.ini",
                "renodx-dlss5.addon64",
                "nvngx_dlssnr.dll",
                "nvngx_dlss.dll",
            ]
        );
        // 全新安装：每一行都是「新增」
        assert!(
            prepared
                .preview
                .iter()
                .all(|row| row.change == PreviewChange::Add)
        );
    }

    /// 标「替换」的行必须说清**现在那个文件是什么**。
    ///
    /// 防的是把用户已经装好的 Special K / ENB / DXVK 顶掉而他自己不知道：
    /// 清单上写着「替换 dxgi.dll」，用户得看得见被替换掉的是哪一个。
    #[test]
    fn a_replace_row_says_what_is_being_replaced() {
        let fixture = ManagerFixture::gta();
        // 游戏目录里先放一个别的工具留下的 dxgi.dll。
        std::fs::write(fixture.game_root.join("dxgi.dll"), b"someone-elses-dxgi").unwrap();
        let prepared = fixture.prepare().unwrap();
        let rows = prepared.planned_targets();

        let replaced: Vec<&PlannedSummary> = rows
            .iter()
            .filter(|row| row.change == PreviewChange::Replace)
            .collect();
        assert_eq!(replaced.len(), 1, "只有 dxgi.dll 那一行是替换");
        assert!(replaced[0].target.ends_with("dxgi.dll"));
        // 认不出来的文件**不编名字**，但要说出大小，用户至少能自己判断。
        let current = replaced[0].current.as_deref().unwrap();
        assert!(current.contains("认不出是什么"), "{current}");
        assert!(current.contains("18"), "要给出大小：{current}");

        // 新增的行没有「当前是什么」——那个位置本来就是空的。
        assert!(
            rows.iter()
                .filter(|row| row.change == PreviewChange::Add)
                .all(|row| row.current.is_none())
        );
    }

    /// 预演清单必须**覆盖每个目标**，包括 Vivid 自己生成的配置——
    /// 少列一个，用户就是在对一份不完整的清单点确认。
    #[test]
    fn the_preview_lists_every_target_including_generated_files() {
        let fixture = ManagerFixture::gta();
        let prepared = fixture.prepare().unwrap();
        let rows = prepared.planned_targets();
        assert_eq!(rows.len(), prepared.plan.actions.len());
        assert_eq!(rows.len(), 5);

        // 下载来的组件有来源和指纹
        let downloaded: Vec<&PlannedSummary> =
            rows.iter().filter(|r| r.source_url.is_some()).collect();
        assert_eq!(downloaded.len(), 4);
        assert!(
            downloaded
                .iter()
                .all(|r| r.source_url.as_deref().unwrap().starts_with("https://")
                    && r.sha256.as_deref().unwrap().len() == 64)
        );

        // Vivid 自己生成的那一行没有 URL，也不许编一个出来
        let generated: Vec<&PlannedSummary> =
            rows.iter().filter(|r| r.source_url.is_none()).collect();
        assert_eq!(generated.len(), 1);
        assert!(generated[0].target.ends_with("ReShade.ini"));
        assert!(generated[0].sha256.is_none());

        // 全新安装：每一行都是新增
        assert!(rows.iter().all(|r| r.change == PreviewChange::Add));
    }

    /// RDR2 的清单里必须能看到那条注册表改动——它是影响面最大的一步。
    #[test]
    fn the_rdr2_preview_shows_the_registry_change() {
        let fixture = ManagerFixture::rdr2();
        let prepared = fixture.prepare().unwrap();
        let rows = prepared.planned_targets();
        assert_eq!(rows.len(), 7);
        let registry: Vec<&PlannedSummary> = rows
            .iter()
            .filter(|r| r.target.contains("HKEY_LOCAL_MACHINE"))
            .collect();
        assert_eq!(registry.len(), 1);
        assert!(
            registry[0].target.contains("ImplicitLayers"),
            "{:?}",
            registry[0]
        );
        assert_eq!(registry[0].change, PreviewChange::Add);
    }

    #[test]
    fn prepare_rdr2_never_writes_before_elevation() {
        let fixture = ManagerFixture::rdr2();
        let before = fixture.system_snapshot();
        let prepared = fixture.prepare().unwrap();
        assert!(prepared.plan.requires_admin);
        assert_eq!(fixture.system_snapshot(), before);
    }

    /// RDR2 的动作顺序：全局 DLL → layer JSON → 注册表 → 游戏 ini → addon → NR → 超分。
    #[test]
    fn rdr2_plan_has_seven_actions_in_the_fixed_order() {
        let fixture = ManagerFixture::rdr2();
        let prepared = fixture.prepare().unwrap();
        assert_eq!(
            file_names(&prepared.plan),
            vec![
                "ReShade64.dll",
                "ReShade64.json",
                "<registry>",
                "reshade.ini",
                "renodx-dlss5.addon64",
                "nvngx_dlssnr.dll",
                "nvngx_dlss.dll",
            ]
        );
    }

    /// 清单里没有所需组件 → 新的安装必须停。
    #[test]
    fn manifest_missing_required_entry_stops_new_installs() {
        let http = manifest_http(
            GAME_MANIFEST,
            r#"{"dlss": [], "dlssnr": []}"#,
            RELEASES_JSON,
            RESHADE_HTML,
        );
        push_artifacts(&http);
        let fixture = ManagerFixture::new(GameId::Gta5Enhanced, http);
        assert!(fixture.prepare().is_err());
    }

    /// ReShade 页面变了、又没有 last-known-good → 停。
    #[test]
    fn reshade_page_change_stops_prepare() {
        let http = manifest_http(
            GAME_MANIFEST,
            DLSS_MANIFEST,
            RELEASES_JSON,
            "<html>页面改版了</html>",
        );
        push_artifacts(&http);
        let fixture = ManagerFixture::new(GameId::Gta5Enhanced, http);
        assert!(matches!(fixture.prepare(), Err(ManagerError::Upstream(_))));
    }

    /// RenoDX 发布里没有可用资产 → 停。
    #[test]
    fn renodx_release_without_asset_stops_prepare() {
        let http = manifest_http(
            GAME_MANIFEST,
            DLSS_MANIFEST,
            r#"[{"tag_name":"other-1.0.0","assets":[]}]"#,
            RESHADE_HTML,
        );
        push_artifacts(&http);
        let fixture = ManagerFixture::new(GameId::Gta5Enhanced, http);
        assert!(matches!(fixture.prepare(), Err(ManagerError::Upstream(_))));
    }

    /// 缓存完整时，断网也能生成计划。
    #[test]
    fn offline_with_complete_cache_can_still_prepare() {
        let fixture = ManagerFixture::gta();
        fixture.prepare().unwrap();

        let offline = fixture.swap(FakeHttp::new(), Some(rtx_4070_ti_super()));
        let prepared = offline.prepare().unwrap();
        assert_eq!(prepared.snapshot_origin, SnapshotOrigin::Cache);
        assert_eq!(prepared.artifacts.len(), 4);
        assert_eq!(prepared.plan.actions.len(), 5);
    }

    /// 断网且缓存缺文件 → 只能检测和还原，不能装。
    #[test]
    fn offline_with_incomplete_cache_refuses_to_prepare() {
        let fixture = ManagerFixture::gta();
        fixture.prepare().unwrap();
        std::fs::remove_dir_all(fixture.manager.paths.artifacts_dir().join("dlss")).unwrap();

        let offline = fixture.swap(FakeHttp::new(), Some(rtx_4070_ti_super()));
        assert!(matches!(offline.prepare(), Err(ManagerError::Artifact(_))));
    }

    /// 认不出代次的显卡（非 RTX）→ 不猜，直接说暂不支持。
    #[test]
    fn prepare_refuses_when_the_gpu_is_not_an_rtx_card() {
        let gtx = GpuInfo {
            model: "NVIDIA GeForce GTX 1080 Ti".to_string(),
            driver_windows: "31.0.15.5222".to_string(),
            driver_nvidia: None,
            generation: None,
        };
        let fixture = ManagerFixture::gta().swap(full_http(), Some(gtx));
        let err = fixture.prepare().unwrap_err();
        assert!(
            matches!(err, ManagerError::UnsupportedGpu { .. }),
            "{err:?}"
        );
    }

    /// 装完再还原，游戏目录回到装之前。
    #[test]
    fn apply_then_restore_returns_the_game_to_its_original_state() {
        let mut fixture = ManagerFixture::gta();
        std::fs::write(fixture.game_root.join("nvngx_dlss.dll"), b"MZ-original").unwrap();
        let before = fixture.system_snapshot();

        let prepared = fixture.prepare().unwrap();
        let ledger = fixture
            .manager
            .apply(prepared, &mut fixture.backend)
            .unwrap();
        assert_eq!(ledger.targets.len(), 5);
        assert_eq!(
            std::fs::read(fixture.game_root.join("nvngx_dlss.dll")).unwrap(),
            b"MZ-sr"
        );

        fixture
            .manager
            .restore(fixture.game_id, &mut fixture.backend)
            .unwrap();
        assert_eq!(fixture.system_snapshot(), before);
    }

    /// 修复只补回 ledger 里记着、但现在不见了的目标。
    #[test]
    fn repair_only_fills_back_missing_targets() {
        let mut fixture = ManagerFixture::gta();
        let prepared = fixture.prepare().unwrap();
        fixture
            .manager
            .apply(prepared, &mut fixture.backend)
            .unwrap();

        std::fs::remove_file(fixture.game_root.join("nvngx_dlssnr.dll")).unwrap();
        let before = fixture.backend.apply_count();
        fixture
            .manager
            .repair(fixture.game_id, &mut fixture.backend)
            .unwrap();
        assert_eq!(
            fixture.backend.apply_count() - before,
            1,
            "只该补那一个缺失的文件"
        );
        assert_eq!(
            std::fs::read(fixture.game_root.join("nvngx_dlssnr.dll")).unwrap(),
            b"MZ-nr"
        );
    }

    /// 目标被别的工具改过 → 更新停下来，不覆盖。
    #[test]
    fn update_refuses_when_a_target_was_changed_outside() {
        let mut fixture = ManagerFixture::gta();
        let prepared = fixture.prepare().unwrap();
        fixture
            .manager
            .apply(prepared, &mut fixture.backend)
            .unwrap();

        let addon = fixture.game_root.join("renodx-dlss5.addon64");
        std::fs::write(&addon, b"MZ-changed-by-another-tool").unwrap();
        let err = fixture
            .manager
            .update(fixture.game_id, &mut fixture.backend)
            .unwrap_err();
        assert!(
            matches!(err, ManagerError::Deploy(DeployError::Conflict { .. })),
            "{err:?}"
        );
        assert_eq!(
            std::fs::read(&addon).unwrap(),
            b"MZ-changed-by-another-tool"
        );
    }

    /// 找不到游戏时说人话，不 panic。
    #[test]
    fn missing_game_is_reported_in_chinese() {
        let temp = tempfile::tempdir().unwrap();
        let manager = VividManager::new(
            AppPaths::under(temp.path().join("vivid-data")),
            full_http(),
            FakeEnvironment::new(None),
        );
        let backend = FakeSystemBackend::new(temp.path());
        let err = manager.prepare(GameId::Gta5Enhanced, &backend).unwrap_err();
        assert!(matches!(err, ManagerError::GameNotFound { .. }));
        assert!(err.to_string().contains("Steam"));
    }

    /// 装过一次之后，第二次预演每一行都是「替换」。
    #[test]
    fn a_second_prepare_reports_replacements() {
        let mut fixture = ManagerFixture::gta();
        let prepared = fixture.prepare().unwrap();
        fixture
            .manager
            .apply(prepared, &mut fixture.backend)
            .unwrap();

        let again = fixture.prepare().unwrap();
        assert!(
            again
                .preview
                .iter()
                .all(|row| row.change == PreviewChange::Replace)
        );
    }

    #[test]
    fn inspect_reports_ready_to_install_before_anything_is_installed() {
        let fixture = ManagerFixture::gta();
        // 先刷新一次清单，让本地有 last-known-good。
        fixture.manager.refresh_snapshot().unwrap();
        fixture.manager.refresh_upstream().unwrap();

        let report = fixture
            .manager
            .inspect(fixture.game_id, &fixture.backend)
            .unwrap();
        assert_eq!(report.state, GameState::ReadyToInstall);
        assert_eq!(report.primary_action(), PrimaryAction::Install);
        assert!(!report.files_installed);
        assert!(report.evidence_lines().is_empty());
    }

    /// 没有任何清单缓存时，只能说「清单不可用」，不能假装可以装。
    #[test]
    fn inspect_reports_manifest_unavailable_without_cache() {
        let fixture = ManagerFixture::gta();
        let report = fixture
            .manager
            .inspect(fixture.game_id, &fixture.backend)
            .unwrap();
        assert_eq!(report.state, GameState::ManifestUnavailable);
        assert_eq!(report.primary_action(), PrimaryAction::None);
    }

    #[test]
    fn inspect_reports_installed_after_apply() {
        let mut fixture = ManagerFixture::gta();
        let prepared = fixture.prepare().unwrap();
        fixture
            .manager
            .apply(prepared, &mut fixture.backend)
            .unwrap();

        let report = fixture
            .manager
            .inspect(fixture.game_id, &fixture.backend)
            .unwrap();
        assert_eq!(report.state, GameState::Installed);
        assert_eq!(report.primary_action(), PrimaryAction::LaunchSinglePlayer);
        assert!(report.files_installed);
        // 没有日志就只能说到「待实测」，不能说已载入。
        assert_eq!(report.load_evidence, LoadEvidence::NoLog);
        assert_eq!(
            report.evidence_lines(),
            vec!["文件已安装", "画面效果待实测"]
        );
    }

    #[test]
    fn inspect_reports_missing_files_when_a_target_is_deleted() {
        let mut fixture = ManagerFixture::gta();
        let prepared = fixture.prepare().unwrap();
        fixture
            .manager
            .apply(prepared, &mut fixture.backend)
            .unwrap();
        std::fs::remove_file(fixture.game_root.join("nvngx_dlss.dll")).unwrap();

        let report = fixture
            .manager
            .inspect(fixture.game_id, &fixture.backend)
            .unwrap();
        assert!(matches!(report.state, GameState::MissingFiles { .. }));
        assert_eq!(report.primary_action(), PrimaryAction::Repair);
        assert!(!report.files_installed);
    }

    /// 冲突优先于缺文件：两种都发生时，先让用户看冲突。
    #[test]
    fn inspect_reports_conflict_and_conflict_wins_over_missing() {
        let mut fixture = ManagerFixture::gta();
        let prepared = fixture.prepare().unwrap();
        fixture
            .manager
            .apply(prepared, &mut fixture.backend)
            .unwrap();
        std::fs::write(fixture.game_root.join("dxgi.dll"), b"MZ-someone-else").unwrap();
        std::fs::remove_file(fixture.game_root.join("nvngx_dlss.dll")).unwrap();

        let report = fixture
            .manager
            .inspect(fixture.game_id, &fixture.backend)
            .unwrap();
        let GameState::Conflict { files } = &report.state else {
            panic!("应该是冲突：{:?}", report.state);
        };
        assert_eq!(files, &vec![fixture.game_root.join("dxgi.dll")]);
        assert_eq!(report.primary_action(), PrimaryAction::ShowConflict);
    }

    /// 组件自己改写配置 → 仍然是「已安装」，不是冲突，还原照样能用。
    #[test]
    fn a_config_rewritten_by_the_component_is_not_a_conflict() {
        let mut fixture = ManagerFixture::gta();
        let prepared = fixture.prepare().unwrap();
        fixture
            .manager
            .apply(prepared, &mut fixture.backend)
            .unwrap();

        std::fs::write(
            fixture.game_root.join("ReShade.ini"),
            b"[GENERAL]
ReShade wrote its own defaults here
",
        )
        .unwrap();

        let report = fixture
            .manager
            .inspect(fixture.game_id, &fixture.backend)
            .unwrap();
        assert_eq!(report.state, GameState::Installed, "{:?}", report.state);
        assert!(report.files_installed);
        fixture
            .manager
            .restore(fixture.game_id, &mut fixture.backend)
            .unwrap();
        assert!(!fixture.game_root.join("ReShade.ini").exists());
    }

    /// 上游出了新版本 → 只提示，不自动替换。
    #[test]
    fn inspect_reports_update_available_but_changes_nothing() {
        let mut fixture = ManagerFixture::gta();
        let prepared = fixture.prepare().unwrap();
        fixture
            .manager
            .apply(prepared, &mut fixture.backend)
            .unwrap();
        let before = fixture.system_snapshot();

        // 清单里的 DLSS 换了新版本标签
        let newer = DLSS_MANIFEST.replace("310.8.0", "310.9.0");
        let snapshot =
            crate::rhi_manifest::RhiSnapshot::parse(GAME_MANIFEST, &newer, "2026-08-31T12:00:00Z")
                .unwrap();
        crate::rhi_manifest::save_snapshot(&fixture.manager.paths.manifest_cache(), &snapshot)
            .unwrap();

        let report = fixture
            .manager
            .inspect(fixture.game_id, &fixture.backend)
            .unwrap();
        assert_eq!(
            report.state,
            GameState::UpdateAvailable {
                installed: "310.8.0".to_string(),
                available: "310.9.0".to_string(),
            }
        );
        assert_eq!(report.primary_action(), PrimaryAction::Update);
        assert_eq!(fixture.system_snapshot(), before, "体检不许改任何文件");
    }

    #[test]
    fn inspect_reports_not_found_without_the_game() {
        let temp = tempfile::tempdir().unwrap();
        let manager = VividManager::new(
            AppPaths::under(temp.path().join("vivid-data")),
            full_http(),
            FakeEnvironment::new(Some(rtx_4070_ti_super())),
        );
        let backend = FakeSystemBackend::new(temp.path());
        let report = manager.inspect(GameId::Gta5Enhanced, &backend).unwrap();
        assert_eq!(report.state, GameState::NotFound);
        assert_eq!(report.primary_action(), PrimaryAction::None);
    }

    /// 清单没了也不能挡住「已装 → 启动 / 还原」这条路。
    #[test]
    fn an_offline_manifest_does_not_block_launch_or_restore() {
        let mut fixture = ManagerFixture::gta();
        let prepared = fixture.prepare().unwrap();
        fixture
            .manager
            .apply(prepared, &mut fixture.backend)
            .unwrap();
        std::fs::remove_file(fixture.manager.paths.manifest_cache()).unwrap();
        std::fs::remove_file(fixture.manager.paths.upstream_cache()).unwrap();

        let report = fixture
            .manager
            .inspect(fixture.game_id, &fixture.backend)
            .unwrap();
        assert_eq!(report.state, GameState::Installed);
        assert_eq!(report.primary_action(), PrimaryAction::LaunchSinglePlayer);

        // 还原仍然可用
        fixture
            .manager
            .restore(fixture.game_id, &mut fixture.backend)
            .unwrap();
    }

    /// 安装之后写的日志才算数。
    #[test]
    fn inspect_uses_the_log_only_when_it_is_newer_than_the_install() {
        let mut fixture = ManagerFixture::gta();
        let prepared = fixture.prepare().unwrap();
        fixture
            .manager
            .apply(prepared, &mut fixture.backend)
            .unwrap();

        std::fs::write(
            fixture.game_root.join("ReShade.log"),
            "Successfully loaded add-on \"renodx-dlss5.addon64\"",
        )
        .unwrap();
        let report = fixture
            .manager
            .inspect(fixture.game_id, &fixture.backend)
            .unwrap();
        assert_eq!(report.load_evidence, LoadEvidence::Loaded);
        assert_eq!(
            report.evidence_lines(),
            vec!["文件已安装", "检测到组件载入", "画面效果待实测"]
        );
    }

    /// 详情里必须能看到来源 URL 和 SHA-256——用户得知道装的是什么、从哪来。
    #[test]
    fn details_show_where_every_component_came_from() {
        let mut fixture = ManagerFixture::gta();
        let prepared = fixture.prepare().unwrap();
        fixture
            .manager
            .apply(prepared, &mut fixture.backend)
            .unwrap();

        let report = fixture
            .manager
            .inspect(fixture.game_id, &fixture.backend)
            .unwrap();
        let joined = report.detail_lines.join("\n");
        assert!(joined.contains("https://reshade.me/"), "{joined}");
        assert!(joined.contains("SHA-256"), "{joined}");
        assert!(joined.contains("DLSS 神经渲染"), "{joined}");
    }
}
