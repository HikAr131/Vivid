//! 图形 API 判定：**清单优先，本机检测兜底，判不出就承认判不出。**
//!
//! 为什么这件事值得单独成模块、而且必须做得保守：
//!
//! > 2026-08-31 真机实测：RDR2 的图形 API 设为 DX12 时，套用 Vulkan 方案会**闪退**；
//! > 切成 Vulkan 后游戏正常。见 `docs/testing/2026-08-31-gta5-rdr2-acceptance.md`。
//!
//! **API 判错了比不装更糟。** 所以这里宁可返回 [`GraphicsApi::Unknown`]（界面上就是
//! 「不提供安装」），也不做任何一次「大概是 DX12 吧」的猜测。
//!
//! 判定顺序是固定的：
//!
//! 1. RHI 清单的 `graphicsApiOverrides`（上游维护的知识库，最可信）；
//! 2. 本机检测：游戏目录里的运行时依赖和主程序的 PE 导入表；
//! 3. 都判不出 → `Unknown`。
//!
//! 本机检测有一条贯穿始终的规则：**证据互相矛盾 = 判不出**。
//! 一款既导入 `d3d11.dll` 又导入 `d3d12.dll` 的游戏，我们确实不知道用户
//! 在游戏设置里选了哪个——这时候「不知道」才是正确答案。

use std::collections::{BTreeSet, VecDeque};
use std::path::Path;

use crate::rhi_manifest::RhiSnapshot;

/// 游戏实际使用的图形 API。
///
/// 取值与 RHI `graphicsApiOverrides` 的编码一一对应（`DX12` / `DX11` / `DX10` /
/// `DX9` / `VLK` / `OGL`），外加一个 Vivid 自己的 `Unknown`。
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub enum GraphicsApi {
    Dx12,
    Dx11,
    Dx10,
    Dx9,
    OpenGl,
    Vulkan,
    /// 判不出来。**这不是「大概率是某某」，是真的不知道。** 界面上不提供安装。
    Unknown,
}

impl GraphicsApi {
    /// 界面上显示的名字。
    pub const fn label(self) -> &'static str {
        match self {
            Self::Dx12 => "DirectX 12",
            Self::Dx11 => "DirectX 11",
            Self::Dx10 => "DirectX 10",
            Self::Dx9 => "DirectX 9",
            Self::OpenGl => "OpenGL",
            Self::Vulkan => "Vulkan",
            Self::Unknown => "判不出来",
        }
    }

    /// 这个 API 下 DLSS 5 神经渲染能不能生效。
    ///
    /// **只有 D3D12 能。** 2026-08-31 实测：RenoDX DLSS5 Generic v4.6 只挂
    /// `NVSDK_NGX_D3D12_*`，整份 Vulkan 日志里没有一条 NGX 入口，插件的开关
    /// 按得动但从来没被调用过。这条结论必须一路传到界面上。
    pub const fn neural_rendering_works(self) -> bool {
        matches!(self, Self::Dx12)
    }
}

/// 这次判定的结论是哪来的。界面要能说清「我凭什么这么判」。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiSource {
    /// RHI 在线清单的 `graphicsApiOverrides`。
    Manifest,
    /// 游戏目录里的文件与主程序的 PE 导入表。
    LocalFiles,
    /// 两条路都没结论。
    None,
}

impl ApiSource {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Manifest => "RHI 在线清单",
            Self::LocalFiles => "本机检测",
            Self::None => "没有可用的判断依据",
        }
    }
}

/// 最多读几个 exe 的导入表。
///
/// 游戏目录里除了主程序还有启动器、崩溃上报、反作弊壳。全看一遍没必要，
/// 但只看一个又可能挑错——取前几个够用，多了只是浪费时间。
const MAX_EXES: usize = 8;

/// 目录扫描的层数上限：游戏根目录往下三层的名字都会被看到。
///
/// 三层是**由真实布局决定的**，不是拍脑袋：虚幻引擎的主程序在
/// `<Game>\Binaries\Win64\`，育碧的在 `bin\`，都在三层以内。
const SCAN_DEPTH: usize = 3;

/// 最多进入多少个目录。防止某个游戏的资源目录把扫描拖垮。
const MAX_DIRS: usize = 256;

/// 主程序常见的存放位置。**只有这些目录（外加游戏根目录）里的 exe 会被读导入表**——
/// 满目录乱读 exe 既慢，又会把工具、崩溃上报的导入表混进证据里。
const BINARY_DIRS: [&str; 6] = ["bin", "bin64", "binaries", "win64", "x64", "retail"];

/// 这些目录里装的是**运行时安装包**（DirectX / VC++ / Vulkan Runtime），
/// 不是游戏自己的图形后端。进去扫只会捡到一堆互相矛盾的证据。
const REDIST_DIRS: [&str; 6] = [
    "_commonredist",
    "commonredist",
    "directx",
    "vcredist",
    "prerequisites",
    "support",
];

/// 单个 exe 的体积上限（512 MiB）。超过就跳过：那多半不是可执行主程序，
/// 而是被塞进 exe 的打包数据，mmap 它没有意义。
const MAX_EXE_BYTES: u64 = 512 * 1024 * 1024;

/// 这些文件名**不能**作为本机检测的证据：它们正是 ReShade 的注入 DLL 名字，
/// 也就是 Vivid（或用户装的别的工具）自己写进去的东西。
///
/// 拿自己写的文件当「这游戏是 DX11」的证据，是最典型的自证循环。
const OUR_OWN_INJECTION_DLLS: [&str; 5] = [
    "dxgi.dll",
    "d3d11.dll",
    "d3d10.dll",
    "d3d9.dll",
    "opengl32.dll",
];

/// 判定一款游戏的图形 API。清单优先，本机检测兜底，判不出就是 `Unknown`。
pub fn resolve(game_name: &str, game_root: &Path, snapshot: Option<&RhiSnapshot>) -> GraphicsApi {
    resolve_detailed(game_name, game_root, snapshot).0
}

/// 同 [`resolve`]，但一并给出结论的来源，供界面展示。
pub fn resolve_detailed(
    game_name: &str,
    game_root: &Path,
    snapshot: Option<&RhiSnapshot>,
) -> (GraphicsApi, ApiSource) {
    if let Some(api) = from_manifest(game_name, snapshot)
        && api != GraphicsApi::Unknown
    {
        return (api, ApiSource::Manifest);
    }
    match from_local_files(game_root) {
        GraphicsApi::Unknown => (GraphicsApi::Unknown, ApiSource::None),
        api => (api, ApiSource::LocalFiles),
    }
}

/// RHI 清单里的覆盖项。名字先精确匹配，再忽略大小写匹配一次。
fn from_manifest(game_name: &str, snapshot: Option<&RhiSnapshot>) -> Option<GraphicsApi> {
    let overrides = &snapshot?.games.graphics_api_overrides;
    let code = overrides.get(game_name).or_else(|| {
        overrides
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(game_name))
            .map(|(_, code)| code)
    })?;
    Some(parse_api_code(code))
}

/// RHI 的 API 编码 → [`GraphicsApi`]。
///
/// **多值（如 `"DX12, VLK"`）一律判成 `Unknown`。** 上游那么写的意思是
/// 「这款游戏两种都能跑」，而我们要的是「用户此刻实际在跑哪种」——
/// 那是游戏设置里的一个开关，清单答不了。猜错的代价是闪退。
pub fn parse_api_code(code: &str) -> GraphicsApi {
    if code.contains(',') {
        return GraphicsApi::Unknown;
    }
    match code.trim().to_ascii_uppercase().as_str() {
        "DX12" | "D3D12" => GraphicsApi::Dx12,
        "DX11" | "D3D11" => GraphicsApi::Dx11,
        "DX10" | "D3D10" => GraphicsApi::Dx10,
        "DX9" | "D3D9" => GraphicsApi::Dx9,
        "VLK" | "VULKAN" => GraphicsApi::Vulkan,
        "OGL" | "OPENGL" => GraphicsApi::OpenGl,
        _ => GraphicsApi::Unknown,
    }
}

/// 本机检测：目录里的运行时文件 + 主程序的 PE 导入表。
///
/// **证据必须唯一。** 收集到零条或互相矛盾的多条，一律返回 `Unknown`。
///
/// 为什么要往下扫三层：本机实测（2026-08-31）发现主程序常常不在游戏根目录——
/// Watch_Dogs 2 在 `bin\`，虚幻引擎的游戏在 `<Game>\Binaries\Win64\`。
/// 只看根目录的话，绝大多数游戏都判不出来。
fn from_local_files(game_root: &Path) -> GraphicsApi {
    let mut evidence: BTreeSet<GraphicsApi> = BTreeSet::new();
    let mut exes: Vec<std::path::PathBuf> = Vec::new();
    let mut queue: VecDeque<(std::path::PathBuf, usize, bool)> = VecDeque::new();
    // 根目录本身算「放主程序的地方」。
    queue.push_back((game_root.to_path_buf(), 0, true));
    let mut visited = 0usize;

    while let Some((dir, depth, holds_binaries)) = queue.pop_front() {
        visited += 1;
        if visited > MAX_DIRS {
            break;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_lowercase();
            let is_dir = entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false);

            if is_dir {
                if let Some(api) = dir_marker(&name) {
                    evidence.insert(api);
                }
                // 运行时安装包目录不进：里面是 DirectX / Vulkan Runtime 的安装器，
                // 跟这款游戏用什么后端没关系。
                if depth + 1 < SCAN_DEPTH && !is_redist_dir(&name) {
                    queue.push_back((
                        entry.path(),
                        depth + 1,
                        BINARY_DIRS.contains(&name.as_str()),
                    ));
                }
                continue;
            }

            if let Some(api) = file_marker(&name) {
                evidence.insert(api);
            }
            if holds_binaries && name.ends_with(".exe") && exes.len() < MAX_EXES {
                exes.push(entry.path());
            }
        }
    }

    for exe in exes {
        for api in imported_apis(&exe) {
            evidence.insert(api);
        }
    }

    let mut found = evidence.into_iter();
    match (found.next(), found.next()) {
        // 唯一一条证据才作数。
        (Some(only), None) => only,
        // 零条、或互相矛盾的多条：**这时候「不知道」才是正确答案。**
        _ => GraphicsApi::Unknown,
    }
}

/// 目录名里能说明问题的（小写）。
///
/// `D3D12` 是 DirectX Agility SDK 的固定布局，`D3D12-REDIST` 是 GTA5 Enhanced
/// 的实际叫法（2026-08-31 本机实测）——两个都以 `d3d12` 开头。
fn dir_marker(lower_name: &str) -> Option<GraphicsApi> {
    lower_name.starts_with("d3d12").then_some(GraphicsApi::Dx12)
}

/// 这个目录里装的是运行时安装包，不是游戏的图形后端。
fn is_redist_dir(lower_name: &str) -> bool {
    lower_name.contains("redist") || REDIST_DIRS.contains(&lower_name)
}

/// 游戏目录里能说明问题的运行时文件名（小写）。
fn file_marker(lower_name: &str) -> Option<GraphicsApi> {
    if OUR_OWN_INJECTION_DLLS.contains(&lower_name) {
        return None;
    }
    // DirectX Agility SDK：只有 D3D12 应用才会带它。
    if lower_name == "d3d12core.dll" || lower_name == "d3d12.dll" {
        return Some(GraphicsApi::Dx12);
    }
    // DirectStorage 的 GPU 解压路径**要求一个 D3D12 设备**——这是接口约束，
    // 不是统计规律，所以随游戏发的 dstorage.dll 是一条硬证据。
    if lower_name == "dstorage.dll" || lower_name == "dstoragecore.dll" {
        return Some(GraphicsApi::Dx12);
    }
    if lower_name == "vulkan-1.dll" || lower_name.starts_with("sga_vulkan") {
        return Some(GraphicsApi::Vulkan);
    }
    // 育碧 Snowdrop / Anvil 的运行时后端命名。
    if lower_name.starts_with("sga_directx12") || lower_name.starts_with("sga_d3d12") {
        return Some(GraphicsApi::Dx12);
    }
    if lower_name.starts_with("sga_directx11") || lower_name.starts_with("sga_d3d11") {
        return Some(GraphicsApi::Dx11);
    }
    None
}

/// 一个 exe 的导入表里出现的图形 API。
///
/// 用 PE 解析而不是 Win32 API：纯 Rust、零 `unsafe`，非 Windows 上也能跑测试。
/// 读不动、不是 64 位 PE、没有导入表——一律当**没有证据**，不当成错误。
///
/// 两处查不到的情况，都会落到「没有证据」这条保守的路上：
/// 游戏用 `LoadLibrary` 动态加载图形运行时，以及延迟导入
/// （pelite 0.10 不解析延迟导入表）。查不到不等于没有，所以只做加法不做否定。
fn imported_apis(exe: &Path) -> Vec<GraphicsApi> {
    use pelite::FileMap;
    use pelite::pe64::{Pe, PeFile};

    if std::fs::metadata(exe).map(|m| m.len()).unwrap_or(u64::MAX) > MAX_EXE_BYTES {
        return Vec::new();
    }
    let Ok(map) = FileMap::open(exe) else {
        return Vec::new();
    };
    let Ok(pe) = PeFile::from_bytes(map.as_ref()) else {
        return Vec::new();
    };

    let mut found = Vec::new();
    if let Ok(imports) = pe.imports() {
        for descriptor in imports {
            if let Ok(name) = descriptor.dll_name()
                && let Some(api) = import_api(&name.to_string().to_lowercase())
            {
                found.push(api);
            }
        }
    }
    found
}

/// 导入的 DLL 名（小写）→ 图形 API。
///
/// `dxgi.dll` **刻意不算证据**：DX10/11/12 都用它，区分不了；而且它正好是
/// ReShade 装进去的那个名字。
fn import_api(lower_dll: &str) -> Option<GraphicsApi> {
    match lower_dll {
        "d3d12.dll" => Some(GraphicsApi::Dx12),
        "d3d11.dll" => Some(GraphicsApi::Dx11),
        "d3d10.dll" | "d3d10_1.dll" => Some(GraphicsApi::Dx10),
        "d3d9.dll" => Some(GraphicsApi::Dx9),
        "vulkan-1.dll" => Some(GraphicsApi::Vulkan),
        "opengl32.dll" => Some(GraphicsApi::OpenGl),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DLSS_MANIFEST: &str = r#"{
        "dlss": [{"version": "310.8.0", "url": "https://example.com/a.zip"}],
        "dlssnr": [{"version": "310.8.SF-v2", "url": "https://example.com/b.zip"}]
    }"#;

    /// 造一份带指定 API 覆盖的清单快照。
    ///
    /// 两款已验证游戏的映射是 `RhiSnapshot::validate` 的硬要求，所以固定带上。
    fn snapshot_with(game: &str, code: &str) -> RhiSnapshot {
        let games = format!(
            r#"{{
                "version": 2,
                "wikiNameOverrides": {{
                    "Grand Theft Auto V Enhanced": "Grand Theft Auto V : Enhanced",
                    "Red Dead Redemption 2": "Red Dead Redemption 2 (Vulkan)"
                }},
                "graphicsApiOverrides": {{
                    "Red Dead Redemption 2": "VLK",
                    "{game}": "{code}"
                }}
            }}"#
        );
        RhiSnapshot::parse(&games, DLSS_MANIFEST, "t").unwrap()
    }

    fn game_with(files: &[&str]) -> tempfile::TempDir {
        let temp = tempfile::tempdir().unwrap();
        for relative in files {
            let path = temp.path().join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, b"not a real pe").unwrap();
        }
        temp
    }

    #[test]
    fn the_manifest_override_wins() {
        let snapshot = snapshot_with("Red Dead Redemption 2", "VLK");
        // 目录里放着 DX12 的痕迹，清单仍然说了算。
        let root = game_with(&["D3D12Core.dll"]);
        assert_eq!(
            resolve("Red Dead Redemption 2", root.path(), Some(&snapshot)),
            GraphicsApi::Vulkan
        );
        assert_eq!(
            resolve_detailed("Red Dead Redemption 2", root.path(), Some(&snapshot)).1,
            ApiSource::Manifest
        );
    }

    /// **判不出来就说判不出来。** 猜错注入方式的代价是游戏闪退
    /// （2026-08-31 RDR2 实测：DX12 游戏套 Vulkan 方案会闪退）。
    #[test]
    fn an_undetectable_game_is_unknown_not_a_guess() {
        let empty = tempfile::tempdir().unwrap();
        assert_eq!(
            resolve("Whatever", empty.path(), None),
            GraphicsApi::Unknown
        );
        assert_eq!(
            resolve_detailed("Whatever", empty.path(), None).1,
            ApiSource::None
        );
    }

    /// 清单里没有这款游戏时退到本机检测。
    #[test]
    fn local_files_are_the_fallback() {
        let snapshot = snapshot_with("Some Other Game", "DX11");
        let root = game_with(&["D3D12Core.dll", "Game.dat"]);
        assert_eq!(
            resolve_detailed("Unlisted Game", root.path(), Some(&snapshot)),
            (GraphicsApi::Dx12, ApiSource::LocalFiles)
        );
    }

    /// Agility SDK 的 `D3D12\` 子目录同样算数。
    #[test]
    fn the_agility_sdk_directory_counts_as_dx12() {
        let root = game_with(&["D3D12/D3D12Core.dll"]);
        assert_eq!(resolve("X", root.path(), None), GraphicsApi::Dx12);
    }

    #[test]
    fn a_shipped_vulkan_runtime_is_evidence() {
        let root = game_with(&["vulkan-1.dll"]);
        assert_eq!(resolve("X", root.path(), None), GraphicsApi::Vulkan);
    }

    /// **证据互相矛盾 = 判不出。** 一款同时带 DX12 和 Vulkan 后端的游戏，
    /// 我们不知道用户在游戏设置里选了哪个——这正是 RDR2 那次闪退的成因。
    #[test]
    fn contradictory_evidence_resolves_to_unknown() {
        let root = game_with(&["D3D12Core.dll", "vulkan-1.dll"]);
        assert_eq!(resolve("X", root.path(), None), GraphicsApi::Unknown);
    }

    /// 清单里的多值（`"DX12, VLK"`）意思是「两种都能跑」，不是「就是 DX12」。
    #[test]
    fn a_multi_value_manifest_code_is_unknown() {
        assert_eq!(parse_api_code("DX12, VLK"), GraphicsApi::Unknown);
        assert_eq!(parse_api_code("DX11, DX12"), GraphicsApi::Unknown);

        let snapshot = snapshot_with("Dual API Game", "DX12, VLK");
        let empty = tempfile::tempdir().unwrap();
        assert_eq!(
            resolve("Dual API Game", empty.path(), Some(&snapshot)),
            GraphicsApi::Unknown
        );
    }

    /// 真实清单里出现过的全部编码都要认得（2026-08-31 抓的 39 条覆盖项）。
    #[test]
    fn every_code_seen_in_the_real_manifest_is_understood() {
        for (code, expected) in [
            ("DX12", GraphicsApi::Dx12),
            ("DX11", GraphicsApi::Dx11),
            ("DX10", GraphicsApi::Dx10),
            ("DX9", GraphicsApi::Dx9),
            ("VLK", GraphicsApi::Vulkan),
            ("OGL", GraphicsApi::OpenGl),
        ] {
            assert_eq!(parse_api_code(code), expected, "{code}");
        }
        // 上游哪天加了新编码，也是「判不出」，不是猜一个最像的。
        assert_eq!(parse_api_code("METAL"), GraphicsApi::Unknown);
        assert_eq!(parse_api_code(""), GraphicsApi::Unknown);
    }

    /// **不能拿自己写进去的文件当证据。** 装过 ReShade 的游戏目录里有
    /// `dxgi.dll` / `d3d11.dll`，那是 Vivid 放的，不说明游戏用什么 API。
    #[test]
    fn our_own_injection_dlls_are_never_evidence() {
        let root = game_with(&["dxgi.dll", "d3d11.dll", "opengl32.dll"]);
        assert_eq!(resolve("X", root.path(), None), GraphicsApi::Unknown);
    }

    /// 读不动的 exe 不是错误，只是没有证据。
    #[test]
    fn a_file_that_is_not_a_pe_yields_no_evidence() {
        let root = game_with(&["Game.exe"]);
        assert!(imported_apis(&root.path().join("Game.exe")).is_empty());
        assert_eq!(resolve("X", root.path(), None), GraphicsApi::Unknown);
    }

    /// 只有 D3D12 能让神经渲染生效——这条实测结论由测试钉住。
    #[test]
    fn neural_rendering_only_works_on_d3d12() {
        assert!(GraphicsApi::Dx12.neural_rendering_works());
        for api in [
            GraphicsApi::Dx11,
            GraphicsApi::Dx10,
            GraphicsApi::Dx9,
            GraphicsApi::OpenGl,
            GraphicsApi::Vulkan,
            GraphicsApi::Unknown,
        ] {
            assert!(!api.neural_rendering_works(), "{api:?}");
        }
    }

    /// 目录不存在时不 panic。
    #[test]
    fn a_missing_directory_is_unknown() {
        let temp = tempfile::tempdir().unwrap();
        assert_eq!(
            resolve("X", &temp.path().join("nope"), None),
            GraphicsApi::Unknown
        );
    }
}
