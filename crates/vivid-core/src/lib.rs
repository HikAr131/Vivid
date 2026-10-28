//! 锐境 Vivid —— 核心逻辑
//!
//! 边界见仓库根 `CLAUDE.md`。这里只做安全区的事：
//! 编目本机已有的 DLL、在游戏之间替换、备份与还原。
//! 不下载、不托管、不魔改任何 NVIDIA 二进制。

// ---------------------------------------------------------------------------
// 品牌名称。**改名时只改这一处**——UI、日志、窗口标题一律从这里取，
// 不要在别处写死字符串。
//
// 拆成四个而不是一个，是因为它们出现的位置对「中文/英文/是否带分隔符」
// 的要求不同，用一个常量迟早会在某个位置显得别扭。
//
// 注意备份文件后缀（backup::BACKUP_SUFFIX）**刻意不引用这里**：
// 产品可以改名，但改名不该让用户已有的备份失效。
// ---------------------------------------------------------------------------

/// 中文名。界面主标题、对用户说话时用它。
pub const NAME_ZH: &str = "锐境";

/// 英文名。可执行文件、crate、安装包、日志前缀用它。
pub const NAME_EN: &str = "Vivid";

/// 窗口标题栏显示的名字。
pub const WINDOW_TITLE: &str = "锐境 Vivid";

/// 主 Logo 位置的完整品牌显示。
pub const BRAND_FULL: &str = "锐境 · Vivid";

pub mod anticheat;
pub mod artifact;
pub mod backup;
pub mod catalog;
pub mod component;
pub mod deployment;
pub mod evidence;
pub mod gpu;
pub mod graphics_api;
pub mod launcher;
pub mod library;
pub mod manager;
pub mod net;
pub mod paths;
pub mod recipe;
pub mod rhi_manifest;
pub mod upstream;
pub mod vulkan_layer;

/// NVAPI 驱动设置。**这是全项目唯一用 unsafe 的模块**，理由见模块头注释。
#[cfg(windows)]
pub mod nvapi;
pub mod scan;
pub mod steam;
pub mod update;
pub mod version;

/// 测试共用的基础夹具（`FakeHttp`、`FakeSystemBackend`、`FakeEnvironment`）。
///
/// 单元测试直接靠 `cfg(test)`；集成测试链接普通 lib，要靠 `testing` 特性打开。
/// **发布构建两个条件都不成立，夹具一行都不会进产物。**
#[cfg(any(test, feature = "testing"))]
pub mod testing;

pub use component::{DlssComponent, Recognized, recognize};
pub use version::{DllVersion, VersionError};
