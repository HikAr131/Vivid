//! 识别一个文件是不是我们该管的 DLSS 组件——以及是不是**可疑的逆向产物**。
//!
//! 2026-08-31 范围调整后，`nvngx_dlssnr.dll`（神经渲染）成为第四个可管理组件：
//! 它由 RHI 的 `dlss_manifest.json` 指向、运行时下载，界面标注「RHI 上游版本」。
//! 因此旧的「NR DLL 一律拒绝、污染整个目录」规则撤销。
//!
//! 仍然保留的守门：`.ptx` 落在这类目录里才算逆向证据。**不能只看扩展名**——
//! 真机扫描教过：第一版把所有 `.ptx` 一律当破解件，结果把《永劫无间》录制模块
//! 自带的正常文件报成「绕过技术保护措施的产物」。错误的警告比不警告更伤信任。

use std::path::Path;

/// 本工具允许管理的 DLSS 组件。
///
/// 想往里加东西之前，先回去读 CLAUDE.md 的功能边界。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum DlssComponent {
    /// `nvngx_dlss.dll` —— 超分辨率主模型
    SuperResolution,
    /// `nvngx_dlssg.dll` —— 帧生成
    FrameGeneration,
    /// `nvngx_dlssd.dll` —— 光线重建
    RayReconstruction,
    /// `nvngx_dlssnr.dll` —— 神经渲染（RHI 上游版本，来源必须可见）
    NeuralRendering,
}

impl DlssComponent {
    /// 该组件的标准文件名（小写）。
    pub const fn file_name(self) -> &'static str {
        match self {
            Self::SuperResolution => "nvngx_dlss.dll",
            Self::FrameGeneration => "nvngx_dlssg.dll",
            Self::RayReconstruction => "nvngx_dlssd.dll",
            Self::NeuralRendering => "nvngx_dlssnr.dll",
        }
    }

    /// 给玩家看的中文名。UI 一律用这个，不暴露文件名。
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::SuperResolution => "超分辨率",
            Self::FrameGeneration => "帧生成",
            Self::RayReconstruction => "光线重建",
            Self::NeuralRendering => "神经渲染",
        }
    }

    /// 全部可管理组件，供扫描器与 UI 遍历。
    pub const ALL: [Self; 4] = [
        Self::SuperResolution,
        Self::FrameGeneration,
        Self::RayReconstruction,
        Self::NeuralRendering,
    ];
}

/// 判定为逆向产物时给玩家看的说明。
const REASON_REVERSED_PTX: &str =
    "这是逆向编译出来的显卡指令文件，且和魔改组件放在一起。本工具不处理。";

/// `.ptx` 只有落在这类目录里才算证据。**不能只看扩展名**——理由见 `tamper_evidence`。
const TAMPER_DIR_MARKERS: &[&str] = &["dlssnr", "sm86-patched", "sm89-patched"];

/// 对一个文件名的识别结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Recognized {
    /// 安全区内、本工具可以管理的组件。
    Managed(DlssComponent),
    /// 命中红线，本工具拒绝处理。`reason` 是直接给玩家看的中文说明。
    Forbidden { reason: &'static str },
    /// 与本工具无关的普通文件。
    Unrelated,
}

/// 按文件名判断该怎么对待一个文件。
pub fn recognize(file_name: &str) -> Recognized {
    match file_name.to_ascii_lowercase().as_str() {
        "nvngx_dlss.dll" => Recognized::Managed(DlssComponent::SuperResolution),
        "nvngx_dlssg.dll" => Recognized::Managed(DlssComponent::FrameGeneration),
        "nvngx_dlssd.dll" => Recognized::Managed(DlssComponent::RayReconstruction),
        "nvngx_dlssnr.dll" => Recognized::Managed(DlssComponent::NeuralRendering),
        _ => Recognized::Unrelated,
    }
}

/// 判断一个**完整路径**是不是「这里有逆向产物」的证据。
///
/// 为什么要看路径而不只是文件名——这是真机扫描教的：
/// `.ptx` 本身是完全正常的 CUDA 中间代码，正经游戏也带
/// （本机《永劫无间》的录制模块就有 `CCRecord/Bin/preproc32_lowlat.ptx`）。
/// 把所有 `.ptx` 一律报成破解件是**误报**，会让玩家以为自己的正版游戏被动过手脚。
///
/// 真正的信号是上下文：文件落在 `dlssnr-sm86-patched` 这类目录里。
pub fn tamper_evidence(path: &Path) -> Option<&'static str> {
    let name = path.file_name()?.to_string_lossy().to_ascii_lowercase();

    if name.ends_with(".ptx") {
        let suspicious = path.parent()?.components().any(|c| {
            let seg = c.as_os_str().to_string_lossy().to_ascii_lowercase();
            TAMPER_DIR_MARKERS.iter().any(|m| seg.contains(m))
        });
        if suspicious {
            return Some(REASON_REVERSED_PTX);
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn neural_rendering_is_a_managed_component() {
        assert_eq!(
            recognize("NVNGX_DLSSNR.DLL"),
            Recognized::Managed(DlssComponent::NeuralRendering)
        );
    }

    #[test]
    fn recognizes_all_managed_components() {
        for c in DlssComponent::ALL {
            assert_eq!(
                recognize(c.file_name()),
                Recognized::Managed(c),
                "{} 应被识别为可管理组件",
                c.file_name()
            );
        }
    }

    #[test]
    fn recognition_is_case_insensitive() {
        assert_eq!(
            recognize("NVNGX_DLSS.DLL"),
            Recognized::Managed(DlssComponent::SuperResolution)
        );
        assert_eq!(
            recognize("NvNgX_DlssNr.DLL"),
            Recognized::Managed(DlssComponent::NeuralRendering)
        );
    }

    /// `.ptx` 必须看上下文，不能看扩展名。
    ///
    /// 这两组路径都取自真机扫描：前者是《永劫无间》录制模块自带的正常文件，
    /// 后者是素材目录里的逆向产物。第一版实现把两者一视同仁，
    /// 等于告诉玩家「你的正版游戏被动过手脚」。
    #[test]
    fn ptx_is_judged_by_context_not_by_extension() {
        let innocent = Path::new(
            r"H:\Steam\steamapps\common\NARAKA BLADEPOINT\CCRecord\Bin\preproc32_lowlat.ptx",
        );
        assert_eq!(
            tamper_evidence(innocent),
            None,
            "游戏自带的 .ptx 不该被报成破解件"
        );

        let guilty = Path::new(r"G:\Dev\DLSS5\DLSS5-GTAV-DropIn\dlssnr-sm86-patched\mod00.ptx");
        assert!(
            tamper_evidence(guilty).is_some(),
            "落在 dlssnr-sm86-patched 里的 .ptx 必须被认出来"
        );
    }

    /// NR DLL 现在是普通可管理组件：它本身不该再是「有逆向产物」的证据。
    #[test]
    fn nr_dll_is_not_tamper_evidence_anymore() {
        assert_eq!(tamper_evidence(Path::new("nvngx_dlssnr.dll")), None);
        assert_eq!(tamper_evidence(Path::new("x64/nvngx_dlssnr.dll")), None);
    }

    /// 拦截说明必须是能直接念给玩家听的中文。
    #[test]
    fn tamper_reason_is_human_readable_chinese() {
        let p = Path::new(r"G:\Dev\DLSS5\DLSS5-GTAV-DropIn\dlssnr-sm86-patched\mod00.ptx");
        let reason = tamper_evidence(p).expect("应当被认出来");
        assert!(
            reason
                .chars()
                .any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c))
        );
        assert!(reason.len() > 10, "理由太短，玩家看不懂发生了什么");
    }

    #[test]
    fn unrelated_files_are_ignored() {
        for name in [
            "d3d12.dll",
            "GTA5.exe",
            "readme.txt",
            "nvngx.dll",
            "mod00.ptx",
        ] {
            assert_eq!(recognize(name), Recognized::Unrelated, "{name}");
        }
    }
}
