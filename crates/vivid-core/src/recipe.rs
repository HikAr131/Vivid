//! 配方：按图形 API 生成固定目标、启动方式、支持标签。
//!
//! 这是「在线清单不能新增写入路径」的机器表达：清单只能提供版本、URL 和
//! **图形 API 这类元数据**，而**写哪个文件名、用什么参数启动**全部由这里的常量决定。
//! 清单里加一条新记录，最多只能让某款游戏换一种注入方式，绝不可能凭空多出一个路径。
//!
//! # 两款已验证游戏是特例，而且必须一直是
//!
//! GTA5 Enhanced 与 RDR2 经过 2026-08-31 的真机验收，它们的 API、exe、
//! ledger 目录名（`gta5-enhanced` / `rdr2`）**写在代码里，不参与任何推断**。
//!
//! 理由很实际：用户机器上这两款游戏**正装着**。命名方案一变，那两份安装就还原不回去。
//! 有测试钉着这个映射（`the_two_verified_games_keep_their_ledger_slug`）。
//!
//! 其余游戏的 slug 是 `app-<app_id>`，API 由 `graphics_api` 判定，
//! **判不出来就没有目标**——空目标集合会让白名单一律拒绝，也就装不了。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::rhi_manifest::RhiSnapshot;
use crate::steam::SteamApp;

/// 配方版本。写进 ledger，用来判断已装的是不是这一代配方。
/// 目标集合、启动参数或写入方式变了就 +1。
///
/// 泛化没有 +1：两款已验证游戏的目标列表、顺序、文件名**一个字节都没变**，
/// 只是生成它们的代码从枚举分支换成了按 API 查表。
pub const RECIPE_VERSION: u32 = 1;

// 三个固定的组件文件名。**这三个名字不随游戏变**，清单也改不了它们。
/// RenoDX DLSS5 addon 在游戏目录里的文件名。
pub const ADDON_FILE: &str = "renodx-dlss5.addon64";
/// DLSS 神经渲染运行时。
pub const DLSSNR_FILE: &str = "nvngx_dlssnr.dll";
/// DLSS 超分运行时。
pub const DLSS_FILE: &str = "nvngx_dlss.dll";

/// 一款游戏的标识。
///
/// 两款已验证游戏是具名变体，**序列化出来仍然是 `"Gta5Enhanced"` / `"RedDeadRedemption2"`**
/// ——用户机器上已有的 ledger 就是这么写的，改了它们就还原不回去。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum GameId {
    Gta5Enhanced,
    RedDeadRedemption2,
    /// 其余游戏，按 Steam App ID 标识。序列化为 `{"App": 1091500}`。
    App(u32),
}

impl GameId {
    /// 两款经过真机验收的游戏。
    pub const VERIFIED: [Self; 2] = [Self::Gta5Enhanced, Self::RedDeadRedemption2];

    /// Steam App ID，游戏发现的钥匙。
    pub const fn app_id(self) -> u32 {
        match self {
            Self::Gta5Enhanced => 3_240_220,
            Self::RedDeadRedemption2 => 1_174_180,
            Self::App(id) => id,
        }
    }

    /// 由 App ID 反推。两款已验证游戏走具名变体，其余落到 `App`。
    pub const fn from_app_id(app_id: u32) -> Self {
        match app_id {
            3_240_220 => Self::Gta5Enhanced,
            1_174_180 => Self::RedDeadRedemption2,
            other => Self::App(other),
        }
    }

    pub const fn is_verified(self) -> bool {
        matches!(self, Self::Gta5Enhanced | Self::RedDeadRedemption2)
    }

    /// 数据目录与 ledger 文件用的短名。
    ///
    /// **两款已验证游戏的短名钉死在 `gta5-enhanced` / `rdr2`。**
    /// 用户机器上这两份安装记录就叫这个名字，换了名字等于把它们弄丢。
    pub fn slug(self) -> String {
        match self {
            Self::Gta5Enhanced => "gta5-enhanced".to_string(),
            Self::RedDeadRedemption2 => "rdr2".to_string(),
            Self::App(id) => format!("app-{id}"),
        }
    }

    /// 兜底的显示名。真实名字来自 Steam 清单（见 [`GameProfile::name`]），
    /// 这里只在错误信息里当占位用。
    pub fn label(self) -> String {
        match self {
            Self::Gta5Enhanced => "Grand Theft Auto V Enhanced".to_string(),
            Self::RedDeadRedemption2 => "Red Dead Redemption 2".to_string(),
            Self::App(id) => format!("Steam 应用 {id}"),
        }
    }

    /// 已验证游戏必须存在的可执行文件。
    pub const fn verified_exe(self) -> Option<&'static str> {
        match self {
            Self::Gta5Enhanced => Some("GTA5_Enhanced.exe"),
            Self::RedDeadRedemption2 => Some("RDR2.exe"),
            Self::App(_) => None,
        }
    }

    /// 已验证游戏的图形 API。**写在代码里，不做任何推断。**
    ///
    /// GTA5 Enhanced 的 DX12 和 RDR2 的 Vulkan 都是真机跑出来的结论，
    /// 比清单和本机检测都可靠——本机检测甚至一度判不出 GTA5。
    pub const fn verified_api(self) -> Option<GraphicsApi> {
        match self {
            Self::Gta5Enhanced => Some(GraphicsApi::Dx12),
            Self::RedDeadRedemption2 => Some(GraphicsApi::Vulkan),
            Self::App(_) => None,
        }
    }
}

// 图形 API 的判定（清单优先、本机检测兜底、判不出就承认）在 `graphics_api`；
// 这里只是把它转出去，配方按 API 决定注入方式。
pub use crate::graphics_api::GraphicsApi;

/// 这款游戏有没有被作者在真机上验证过。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Verification {
    /// 安装、载入、画面、还原四项都由作者本机实测过。
    Verified,
    /// 作者没有这款游戏。安装与还原逻辑是通用的，但**没人验证过画面和稳定性**。
    Untested,
}

/// 一款游戏的档案：配方需要知道的全部事实。
///
/// 这是泛化之后传递「是哪款游戏」的唯一载体。它可序列化，因为中断恢复用的
/// journal 要靠它重建白名单——**否则进程被杀之后就不知道该允许写哪些位置了**。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GameProfile {
    pub app_id: u32,
    /// Steam 清单里的游戏名，界面上显示的就是它。
    pub name: String,
    pub root: PathBuf,
    /// 目录里必须存在的可执行文件，用来证明这确实是这款游戏的安装目录。
    pub exe: String,
    pub api: GraphicsApi,
    pub verification: Verification,
}

impl GameProfile {
    pub const fn id(&self) -> GameId {
        GameId::from_app_id(self.app_id)
    }

    /// 两款已验证游戏的固定档案：API、exe、名字全部来自代码。
    ///
    /// **这就是「保留两款已验证游戏的特例」的落点。** 清单变了、本机检测判错了，
    /// 都影响不到它们——它们的行为必须和真机验收那天一模一样。
    pub fn verified(id: GameId, root: &Path) -> Option<Self> {
        Some(Self {
            app_id: id.app_id(),
            name: id.label(),
            root: root.to_path_buf(),
            exe: id.verified_exe()?.to_string(),
            api: id.verified_api()?,
            verification: Verification::Verified,
        })
    }
}

/// 这个 API 下，ReShade 要以什么文件名注入游戏目录。
///
/// **文件名写在这里，清单加不了新路径。**
///
/// Vulkan 返回 `None` 不是「不支持」，而是它根本不往游戏目录放注入 DLL——
/// 走的是 `C:\ProgramData\ReShade` 的全局 implicit layer。
/// `Unknown` 返回 `None` 才是真的「不装」：判不出 API 时**任何**注入方式都可能是错的，
/// 而错的代价是游戏闪退（2026-08-31 RDR2 实测）。
pub const fn injection_dll(api: GraphicsApi) -> Option<&'static str> {
    match api {
        GraphicsApi::Dx12 => Some("dxgi.dll"),
        GraphicsApi::Dx11 => Some("d3d11.dll"),
        GraphicsApi::Dx10 => Some("d3d10.dll"),
        GraphicsApi::Dx9 => Some("d3d9.dll"),
        GraphicsApi::OpenGl => Some("opengl32.dll"),
        GraphicsApi::Vulkan | GraphicsApi::Unknown => None,
    }
}

/// 游戏目录里那份 ReShade 配置的文件名。
///
/// 大小写的差别是**刻意保留**的：两款已验证游戏在真机验收时写进去的就是这两个名字
/// （DX12 那次是 `ReShade.ini`，Vulkan 全局层那次是 `reshade.ini`），
/// 沿用 RHI 的写法。Windows 路径大小写不敏感，改了不会坏，但没有理由去动它。
pub const fn config_file_name(api: GraphicsApi) -> &'static str {
    match api {
        GraphicsApi::Vulkan => "reshade.ini",
        _ => "ReShade.ini",
    }
}

/// 这个 API 需不需要管理员权限。
///
/// 只有 Vulkan 需要：全局 implicit layer 要写 `C:\ProgramData` 和 HKLM。
pub const fn requires_admin(api: GraphicsApi) -> bool {
    matches!(api, GraphicsApi::Vulkan)
}

/// 一款游戏的游戏目录内目标，按安装顺序排列。
///
/// 顺序固定为：注入 DLL → 配置 → addon → 神经渲染 → 超分。
/// Vulkan 方案没有注入 DLL（走全局层），它的全局文件由 `vulkan_layer` 生成。
///
/// **判不出 API 时返回空列表。** 空目标集合会让白名单一律拒绝——
/// 「不提供安装」这件事在这里就已经是机器强制的，不只是界面上不画按钮。
pub fn targets_for(profile: &GameProfile) -> Vec<PathBuf> {
    if profile.api == GraphicsApi::Unknown {
        return Vec::new();
    }
    let root = &profile.root;
    let mut targets = Vec::new();
    if let Some(dll) = injection_dll(profile.api) {
        targets.push(root.join(dll));
    }
    targets.push(root.join(config_file_name(profile.api)));
    targets.push(root.join(ADDON_FILE));
    targets.push(root.join(DLSSNR_FILE));
    targets.push(root.join(DLSS_FILE));
    targets
}

/// 这款游戏在 Vivid 数据目录里的短名。
pub fn slug_of(profile: &GameProfile) -> String {
    profile.id().slug()
}

/// 启动方式。首版只有固定的一次性 Steam 启动。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaunchMode {
    Steam { app_id: u32, args: Vec<String> },
}

/// 一款游戏的固定配方。
#[derive(Debug, Clone)]
pub struct GameRecipe {
    pub id: GameId,
    pub display_name: String,
    pub api: GraphicsApi,
    /// 界面上对支持程度的一句话标签。
    pub support_label: &'static str,
    pub game_root: PathBuf,
    pub exe_name: String,
    /// Vulkan 方案要装全局 layer，需要管理员权限；其余不需要。
    pub requires_admin: bool,
    pub launch: LaunchMode,
    pub verification: Verification,
}

/// 未实测游戏必须说的那一句。
const UNTESTED_CAVEAT: &str = "作者没有这款游戏，画面效果和是否闪退都没人验证过。\
     安装与还原逻辑是通用的，出问题点一键还原。";

/// 非 D3D12 游戏必须说的那一句。
///
/// **这是 2026-08-31 真机实测的结论，不是保守起见的免责声明**：
/// RenoDX DLSS5 Generic v4.6 只挂 `NVSDK_NGX_D3D12_*`，Vulkan 那份日志里
/// 一条 NGX 入口都没有，插件的开关按得动但从来没被调用过。
const NO_NEURAL_RENDERING_CAVEAT: &str = "这款游戏不是 D3D12，会装上 ReShade，\
     但 DLSS 5 神经渲染不会生效——插件只挂 D3D12 的入口（2026-08-31 实测）。";

/// Vulkan 方案必须说的那一句。
const GLOBAL_VULKAN_CAVEAT: &str = "Vulkan 方案会改全局状态\
     （C:\\ProgramData\\ReShade 与 HKLM 的 Vulkan 层注册项），\
     影响这台电脑上所有 Vulkan 游戏。一键还原可以完整退回。";

/// RDR2 的三条实测结论。**比按规则生成的那几句更具体**，所以原样保留：
/// 「DX12 下会闪退」是这款游戏特有的事实，任何规则都推导不出来。
const RDR2_CAVEATS: [&str; 3] = [
    "作者本机实测：游戏图形 API 设为 DX12 时，装完会闪退。",
    "设为 Vulkan 时游戏能正常运行，但 RenoDX DLSS5 插件只挂 D3D12 的 NGX 入口，\
     DLSS 5 神经渲染不会生效——也就是装了看不到画面变化。",
    "这套安装会改全局状态（C:\\ProgramData\\ReShade 与 HKLM 的 Vulkan 层注册项），\
     影响这台电脑上所有 Vulkan 游戏。一键还原可以完整退回。",
];

impl GameRecipe {
    /// 这款游戏的档案（配方就是从它生成的）。
    pub fn profile(&self) -> GameProfile {
        GameProfile {
            app_id: self.id.app_id(),
            name: self.display_name.clone(),
            root: self.game_root.clone(),
            exe: self.exe_name.clone(),
            api: self.api,
            verification: self.verification,
        }
    }

    /// 本机实测发现的已知后果。**安装之前必须原样摆给用户看。**
    ///
    /// 这不是「免责声明」，是实测结论：写在这里而不是文档里，是因为
    /// 用户在点安装那一刻才需要它。空列表表示这款游戏实测通过、没有已知问题。
    ///
    /// RDR2 的三条来自 2026-08-31 的真机验收（见
    /// `docs/testing/2026-08-31-gta5-rdr2-acceptance.md`）：两种图形 API 都试过，
    /// 一种闪退、一种装了不生效。用户仍然可以装——但要在知情的前提下装。
    ///
    /// 其余游戏的三句按档案生成：**没实测过就说没实测过、不是 D3D12 就说
    /// 神经渲染不生效、Vulkan 就说会改全局状态**。GTA5 三条规则一条都不命中
    /// （已验证 + D3D12 + 非 Vulkan），所以它的列表是空的——不是被特判成空的。
    pub fn known_caveats(&self) -> Vec<&'static str> {
        // RDR2 的实测结论比规则更具体（「DX12 下会闪退」推导不出来），原样保留。
        if self.id == GameId::RedDeadRedemption2 {
            return RDR2_CAVEATS.to_vec();
        }
        let mut lines = Vec::new();
        if self.verification == Verification::Untested {
            lines.push(UNTESTED_CAVEAT);
        }
        if !self.api.neural_rendering_works() {
            lines.push(NO_NEURAL_RENDERING_CAVEAT);
        }
        if self.api == GraphicsApi::Vulkan {
            lines.push(GLOBAL_VULKAN_CAVEAT);
        }
        lines
    }

    /// 装完之后**内容归所装组件自己维护**的目标。
    ///
    /// ReShade 启动时会把配置补成自己的完整格式、退出时存窗口布局——
    /// 那是它的正常行为，不是外部篡改。把这种文件的内容变化当冲突，
    /// 会让每个打开过 ReShade 面板的用户都还原不了（2026-08-31 真机验收撞到过）。
    /// 缺失仍然算文件异常，还原也照常把它删掉或写回原样。
    ///
    /// 泛化之后这一条**必须继续成立**：任何 API 下那份 ReShade 配置都归它自己管。
    pub fn component_owned_targets(&self) -> Vec<PathBuf> {
        if self.api == GraphicsApi::Unknown {
            return Vec::new();
        }
        vec![self.game_root.join(config_file_name(self.api))]
    }

    /// 游戏目录里的文件目标，按安装顺序排列。
    pub fn game_file_targets(&self) -> Vec<PathBuf> {
        targets_for(&self.profile())
    }
}

/// 一款已发现、可套用配方的游戏。
#[derive(Debug, Clone)]
pub struct DiscoveredGame {
    pub app: SteamApp,
    pub recipe: GameRecipe,
}

#[derive(Debug, thiserror::Error)]
pub enum RecipeError {
    #[error("目录里找不到 {exe}，这不是 {game} 的安装目录")]
    MissingExe { game: String, exe: String },

    #[error("目录不是来自 Steam App {app_id} 的 manifest（install dir 对不上）")]
    ForgedRoot { app_id: u32 },

    #[error("这不是作者验证过的两款游戏之一，不能用固定配方构造")]
    NotVerified,

    #[error("判不出{0}用的是哪种图形 API，Vivid 不提供安装")]
    UnknownApi(String),

    #[error("RHI 清单缺少 {0} 需要的映射")]
    SnapshotMissing(String),
}

/// 按档案构造配方。
///
/// **纯函数**：不查文件系统（测试与 GUI 预览都用固定路径构造）。
/// 目录真实性由 `validate_profile` 把关。
pub fn recipe_for_profile(profile: &GameProfile) -> GameRecipe {
    let id = profile.id();
    let support_label = match (id, profile.verification) {
        (GameId::Gta5Enhanced, _) => "方案可用",
        (GameId::RedDeadRedemption2, _) => "实验支持",
        (_, Verification::Verified) => "方案可用",
        (_, Verification::Untested) => "未实测",
    };
    let args = if id == GameId::Gta5Enhanced {
        // 一次性单机启动参数。**绝不写进 Steam 永久 Launch Options。**
        vec!["-nobattleye".to_string()]
    } else {
        Vec::new()
    };
    GameRecipe {
        id,
        display_name: profile.name.clone(),
        api: profile.api,
        support_label,
        game_root: profile.root.clone(),
        exe_name: profile.exe.clone(),
        requires_admin: requires_admin(profile.api),
        launch: LaunchMode::Steam {
            app_id: profile.app_id,
            args,
        },
        verification: profile.verification,
    }
}

/// 按已验证游戏的固定档案构造配方。
///
/// 只对 GTA5 Enhanced 与 RDR2 有效——其余游戏的 API 和 exe 要靠发现，
/// 走 [`recipe_for_profile`]。
pub fn recipe_for(id: GameId, game_root: &Path) -> Result<GameRecipe, RecipeError> {
    let profile = GameProfile::verified(id, game_root).ok_or(RecipeError::NotVerified)?;
    Ok(recipe_for_profile(&profile))
}

/// 校验一个目录真是这款游戏的安装目录：
///
/// 1. 必须来自对应 Steam App manifest（`app.game_root` 与目录一致、App ID 相符）；
/// 2. 必须存在档案里记的那个 exe。
///
/// 伪造一个同名目录、或把配方套到别的目录上，都过不了这一关。
pub fn validate_profile(profile: &GameProfile, app: &SteamApp) -> Result<(), RecipeError> {
    if app.app_id != profile.app_id || app.game_root != profile.root {
        return Err(RecipeError::ForgedRoot {
            app_id: profile.app_id,
        });
    }
    if !profile.root.join(&profile.exe).is_file() {
        return Err(RecipeError::MissingExe {
            game: profile.name.clone(),
            exe: profile.exe.clone(),
        });
    }
    Ok(())
}

/// 已验证游戏的目录校验（exe 名来自代码内常量）。
pub fn validate_game_root(id: GameId, app: &SteamApp, game_root: &Path) -> Result<(), RecipeError> {
    let profile = GameProfile::verified(id, game_root).ok_or(RecipeError::NotVerified)?;
    validate_profile(&profile, app)
}

/// 用本机 Steam 发现两款已验证的游戏；目录不真实或 exe 缺失的跳过。
pub fn discover_supported_games() -> Vec<DiscoveredGame> {
    GameId::VERIFIED
        .iter()
        .filter_map(|&id| {
            let app = crate::steam::find_app(id.app_id())?;
            validate_game_root(id, &app, &app.game_root).ok()?;
            let recipe = recipe_for(id, &app.game_root).ok()?;
            Some(DiscoveredGame { app, recipe })
        })
        .collect()
}

/// 校验 RHI 快照是否包含该游戏需要的精确映射。
///
/// GTA5 与 RDR2 各要一个精确 wiki 名称覆盖；RDR2 额外要求 `VLK`。
/// 清单缺映射时，已有安装仍可检测/启动/还原，但新的安装与更新必须停止。
///
/// 未验证的游戏不要求任何映射：它们的 API 由 `graphics_api` 判定，
/// 清单只是其中一条依据，缺了不代表装不了。
pub fn validate_snapshot_support(id: GameId, snapshot: &RhiSnapshot) -> Result<(), RecipeError> {
    let wiki = &snapshot.games.wiki_name_overrides;
    match id {
        GameId::Gta5Enhanced => {
            if wiki.get("Grand Theft Auto V Enhanced").map(String::as_str)
                != Some("Grand Theft Auto V : Enhanced")
            {
                return Err(RecipeError::SnapshotMissing(
                    "Grand Theft Auto V Enhanced 的 wiki 映射".to_string(),
                ));
            }
        }
        GameId::RedDeadRedemption2 => {
            if wiki.get("Red Dead Redemption 2").map(String::as_str)
                != Some("Red Dead Redemption 2 (Vulkan)")
            {
                return Err(RecipeError::SnapshotMissing(
                    "Red Dead Redemption 2 的 wiki 映射".to_string(),
                ));
            }
            if snapshot
                .games
                .graphics_api_overrides
                .get("Red Dead Redemption 2")
                .map(String::as_str)
                != Some("VLK")
            {
                return Err(RecipeError::SnapshotMissing(
                    "Red Dead Redemption 2 的 VLK 覆盖".to_string(),
                ));
            }
        }
        GameId::App(_) => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gta_root() -> PathBuf {
        PathBuf::from(r"H:\Steam\steamapps\common\Grand Theft Auto V Enhanced")
    }

    fn other_profile(app_id: u32, api: GraphicsApi) -> GameProfile {
        GameProfile {
            app_id,
            name: "Some Game".to_string(),
            root: PathBuf::from(r"H:\Steam\steamapps\common\Some Game"),
            exe: "Some Game.exe".to_string(),
            api,
            verification: Verification::Untested,
        }
    }

    /// 注入方式由 API 决定，而且**文件名写在代码里**——清单加不了新路径。
    #[test]
    fn the_injection_dll_follows_the_graphics_api() {
        assert_eq!(injection_dll(GraphicsApi::Dx12), Some("dxgi.dll"));
        assert_eq!(injection_dll(GraphicsApi::Dx11), Some("d3d11.dll"));
        assert_eq!(injection_dll(GraphicsApi::Dx10), Some("d3d10.dll"));
        assert_eq!(injection_dll(GraphicsApi::Dx9), Some("d3d9.dll"));
        assert_eq!(injection_dll(GraphicsApi::OpenGl), Some("opengl32.dll"));
        // Vulkan 不往游戏目录放注入 DLL，走全局层
        assert_eq!(injection_dll(GraphicsApi::Vulkan), None);
        assert_eq!(injection_dll(GraphicsApi::Unknown), None);
    }

    /// 已经装好的两款游戏不能因为泛化而失效。
    ///
    /// **用户机器上这两份 ledger 正躺在 `%LOCALAPPDATA%\Vivid\installations`。**
    #[test]
    fn the_two_verified_games_keep_their_ledger_slug() {
        let gta = GameProfile::verified(GameId::Gta5Enhanced, &gta_root()).unwrap();
        let rdr2 = GameProfile::verified(
            GameId::RedDeadRedemption2,
            Path::new(r"H:\Steam\steamapps\common\Red Dead Redemption 2"),
        )
        .unwrap();
        assert_eq!(slug_of(&gta), "gta5-enhanced");
        assert_eq!(slug_of(&rdr2), "rdr2");
        assert_eq!(
            slug_of(&other_profile(1_091_500, GraphicsApi::Dx12)),
            "app-1091500"
        );
    }

    /// ledger 里的 `game_id` 字段必须继续序列化成老样子。
    #[test]
    fn the_verified_game_ids_serialise_the_way_existing_ledgers_spell_them() {
        assert_eq!(
            serde_json::to_string(&GameId::Gta5Enhanced).unwrap(),
            "\"Gta5Enhanced\""
        );
        assert_eq!(
            serde_json::to_string(&GameId::RedDeadRedemption2).unwrap(),
            "\"RedDeadRedemption2\""
        );
        // 反过来也读得回来——真实 ledger 里就是这两个字符串。
        assert_eq!(
            serde_json::from_str::<GameId>("\"Gta5Enhanced\"").unwrap(),
            GameId::Gta5Enhanced
        );
        assert_eq!(
            serde_json::from_str::<GameId>("{\"App\":1091500}").unwrap(),
            GameId::App(1_091_500)
        );
    }

    #[test]
    fn app_ids_round_trip_through_the_game_id() {
        assert_eq!(GameId::from_app_id(3_240_220), GameId::Gta5Enhanced);
        assert_eq!(GameId::from_app_id(1_174_180), GameId::RedDeadRedemption2);
        assert_eq!(GameId::from_app_id(1_091_500), GameId::App(1_091_500));
        for id in [
            GameId::Gta5Enhanced,
            GameId::RedDeadRedemption2,
            GameId::App(1_091_500),
        ] {
            assert_eq!(GameId::from_app_id(id.app_id()), id);
        }
    }

    /// GTA5 的目标列表、顺序和一次性启动参数一个字节都不能变。
    #[test]
    fn gta_recipe_has_fixed_targets_and_one_shot_launch() {
        let root = gta_root();
        let recipe = recipe_for(GameId::Gta5Enhanced, &root).unwrap();
        assert_eq!(recipe.api, GraphicsApi::Dx12);
        assert_eq!(recipe.support_label, "方案可用");
        assert_eq!(recipe.verification, Verification::Verified);
        assert!(!recipe.requires_admin);
        assert_eq!(
            recipe.launch,
            LaunchMode::Steam {
                app_id: 3_240_220,
                args: vec!["-nobattleye".into()],
            }
        );
        assert_eq!(
            recipe.game_file_targets(),
            vec![
                root.join("dxgi.dll"),
                root.join("ReShade.ini"),
                root.join("renodx-dlss5.addon64"),
                root.join("nvngx_dlssnr.dll"),
                root.join("nvngx_dlss.dll"),
            ]
        );
        assert_eq!(
            recipe.component_owned_targets(),
            vec![root.join("ReShade.ini")]
        );
    }

    /// RDR2 的目标列表同样不能变：没有注入 DLL，配置是小写的 `reshade.ini`。
    #[test]
    fn rdr2_recipe_requires_vulkan_and_admin() {
        let root = PathBuf::from(r"H:\Steam\steamapps\common\Red Dead Redemption 2");
        let recipe = recipe_for(GameId::RedDeadRedemption2, &root).unwrap();
        assert_eq!(recipe.api, GraphicsApi::Vulkan);
        assert_eq!(recipe.support_label, "实验支持");
        assert!(recipe.requires_admin);
        assert_eq!(
            recipe.launch,
            LaunchMode::Steam {
                app_id: 1_174_180,
                args: Vec::new(),
            }
        );
        assert_eq!(
            recipe.game_file_targets(),
            vec![
                root.join("reshade.ini"),
                root.join("renodx-dlss5.addon64"),
                root.join("nvngx_dlssnr.dll"),
                root.join("nvngx_dlss.dll"),
            ]
        );
        assert_eq!(
            recipe.component_owned_targets(),
            vec![root.join("reshade.ini")]
        );
    }

    /// 一款未实测的 DX11 游戏：注入 DLL 换成 `d3d11.dll`，其余三项不变。
    #[test]
    fn an_untested_dx11_game_gets_the_dx11_injection_dll() {
        let profile = other_profile(1_091_500, GraphicsApi::Dx11);
        let recipe = recipe_for_profile(&profile);
        assert_eq!(recipe.support_label, "未实测");
        assert!(!recipe.requires_admin);
        assert_eq!(
            recipe.game_file_targets(),
            vec![
                profile.root.join("d3d11.dll"),
                profile.root.join("ReShade.ini"),
                profile.root.join("renodx-dlss5.addon64"),
                profile.root.join("nvngx_dlssnr.dll"),
                profile.root.join("nvngx_dlss.dll"),
            ]
        );
        // 未实测的游戏必须有话说。
        assert!(!recipe.known_caveats().is_empty());
    }

    /// **判不出 API 就没有目标。** 空目标集合让白名单一律拒绝，
    /// 「不提供安装」由机器强制，不只是界面上不画按钮。
    #[test]
    fn an_unknown_api_produces_no_targets_at_all() {
        let profile = other_profile(1_091_500, GraphicsApi::Unknown);
        let recipe = recipe_for_profile(&profile);
        assert!(recipe.game_file_targets().is_empty());
        assert!(recipe.component_owned_targets().is_empty());
        assert!(targets_for(&profile).is_empty());
    }

    /// 已知后果必须跟着实测走：GTA5 验收全过所以是空的，
    /// RDR2 两种 API 都试过、都没有正收益，所以必须有话说。
    /// **这条测试是防止有人把实测结论从界面上悄悄拿掉。**
    #[test]
    fn only_the_game_with_known_problems_carries_caveats() {
        let gta = recipe_for(GameId::Gta5Enhanced, Path::new(r"H:\gta")).unwrap();
        assert!(gta.known_caveats().is_empty(), "GTA5 实测四项全过");

        let rdr2 = recipe_for(GameId::RedDeadRedemption2, Path::new(r"H:\rdr2")).unwrap();
        let text = rdr2.known_caveats().join("");
        assert_eq!(rdr2.known_caveats().len(), 3);
        assert!(text.contains("闪退"), "要说清 DX12 下会闪退");
        assert!(text.contains("不会生效"), "要说清 Vulkan 下装了也不生效");
        assert!(text.contains("所有 Vulkan 游戏"), "要说清全局影响范围");
        assert!(text.contains("还原"), "要给出退路");
    }

    /// 未实测的游戏**必须有话说**：作者没验证过这件事不能只写在文档里。
    #[test]
    fn an_untested_game_always_says_so() {
        let recipe = recipe_for_profile(&other_profile(1_091_500, GraphicsApi::Dx12));
        let text = recipe.known_caveats().join("");
        assert!(text.contains("没有这款游戏"), "{text}");
        assert!(text.contains("一键还原"), "要给出退路：{text}");
    }

    /// **非 D3D12 的游戏必须说清神经渲染不会生效。**
    ///
    /// 这是 2026-08-31 实测出来的结构性结论：插件只挂 D3D12 的 NGX 入口。
    /// 不说的话，用户会以为装上就有——装了却什么都没发生，比不装更糟。
    #[test]
    fn every_non_d3d12_api_says_neural_rendering_will_not_work() {
        for api in [
            GraphicsApi::Dx11,
            GraphicsApi::Dx10,
            GraphicsApi::Dx9,
            GraphicsApi::OpenGl,
            GraphicsApi::Vulkan,
        ] {
            let recipe = recipe_for_profile(&other_profile(1_091_500, api));
            let text = recipe.known_caveats().join("");
            assert!(
                text.contains("神经渲染不会生效"),
                "{api:?} 必须说清 NR 不生效：{text}"
            );
        }
        // D3D12 反过来不该出现这句话。
        let dx12 = recipe_for_profile(&other_profile(1_091_500, GraphicsApi::Dx12));
        assert!(!dx12.known_caveats().join("").contains("神经渲染不会生效"));
    }

    /// Vulkan 方案必须说清它改的是全局状态。
    #[test]
    fn a_vulkan_game_says_the_change_is_global() {
        let recipe = recipe_for_profile(&other_profile(1_091_500, GraphicsApi::Vulkan));
        let text = recipe.known_caveats().join("");
        assert!(text.contains("所有 Vulkan 游戏"), "{text}");
        assert!(text.contains("一键还原"), "{text}");
    }

    /// 伪造同名目录过不了校验：manifest 对不上、exe 缺失都不行。
    #[test]
    fn forged_directory_fails_validation() {
        let temp = tempfile::tempdir().unwrap();
        let forged = temp.path().join("Grand Theft Auto V Enhanced");
        std::fs::create_dir_all(&forged).unwrap();
        std::fs::write(forged.join("GTA5_Enhanced.exe"), b"MZ").unwrap();

        // 有 exe 但 manifest 的 game_root 指向别处 → 拒绝。
        let lying_app = SteamApp {
            app_id: 3_240_220,
            name: "Grand Theft Auto V Enhanced".into(),
            install_dir: "Grand Theft Auto V Enhanced".into(),
            game_root: temp.path().join("somewhere-else"),
            steam_exe: temp.path().join("steam.exe"),
        };
        assert!(validate_game_root(GameId::Gta5Enhanced, &lying_app, &forged).is_err());

        // App ID 对不上 → 拒绝。
        let wrong_id = SteamApp {
            app_id: 1_174_180,
            ..lying_app
        };
        assert!(validate_game_root(GameId::Gta5Enhanced, &wrong_id, &forged).is_err());
    }

    #[test]
    fn matching_manifest_and_exe_passes_validation() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("Red Dead Redemption 2");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("RDR2.exe"), b"MZ").unwrap();
        let app = SteamApp {
            app_id: 1_174_180,
            name: "Red Dead Redemption 2".into(),
            install_dir: "Red Dead Redemption 2".into(),
            game_root: root.clone(),
            steam_exe: temp.path().join("steam.exe"),
        };
        assert!(validate_game_root(GameId::RedDeadRedemption2, &app, &root).is_ok());
    }

    /// 未验证游戏的目录校验用它自己档案里的 exe。
    #[test]
    fn an_untested_game_is_validated_against_its_own_exe() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("Some Game");
        std::fs::create_dir_all(&root).unwrap();
        let app = SteamApp {
            app_id: 1_091_500,
            name: "Some Game".into(),
            install_dir: "Some Game".into(),
            game_root: root.clone(),
            steam_exe: temp.path().join("steam.exe"),
        };
        let profile = GameProfile {
            root: root.clone(),
            ..other_profile(1_091_500, GraphicsApi::Dx12)
        };
        // exe 还不存在 → 拒绝。
        assert!(validate_profile(&profile, &app).is_err());
        std::fs::write(root.join("Some Game.exe"), b"MZ").unwrap();
        assert!(validate_profile(&profile, &app).is_ok());
    }

    /// 清单缺映射时，两款已验证游戏的新安装与更新必须停。
    #[test]
    fn snapshot_without_exact_mappings_is_rejected() {
        let good_games = r#"{
            "version": 2,
            "wikiNameOverrides": {
                "Grand Theft Auto V Enhanced": "Grand Theft Auto V : Enhanced",
                "Red Dead Redemption 2": "Red Dead Redemption 2 (Vulkan)"
            },
            "graphicsApiOverrides": {"Red Dead Redemption 2": "VLK"}
        }"#;
        let good_dlss = r#"{
            "dlss": [{"version": "310.8.0", "url": "https://example.com/a.zip"}],
            "dlssnr": [{"version": "310.8.SF-v2", "url": "https://example.com/b.zip"}]
        }"#;
        let snapshot = RhiSnapshot::parse(good_games, good_dlss, "t").unwrap();
        assert!(validate_snapshot_support(GameId::Gta5Enhanced, &snapshot).is_ok());
        assert!(validate_snapshot_support(GameId::RedDeadRedemption2, &snapshot).is_ok());
        // 未验证的游戏不要求清单映射。
        assert!(validate_snapshot_support(GameId::App(1_091_500), &snapshot).is_ok());

        // wiki 映射被上游改掉 → 两款游戏都不能再用这份快照做新安装。
        let renamed = good_games.replace(
            "\"Red Dead Redemption 2\": \"Red Dead Redemption 2 (Vulkan)\"",
            "\"Red Dead Redemption 2\": \"Something Else\"",
        );
        let snapshot = RhiSnapshot::parse(&renamed, good_dlss, "t").unwrap();
        assert!(validate_snapshot_support(GameId::RedDeadRedemption2, &snapshot).is_err());
        // GTA5 的映射还在，不受影响。
        assert!(validate_snapshot_support(GameId::Gta5Enhanced, &snapshot).is_ok());
    }
}
