//! 显卡与驱动检测。
//!
//! 走注册表而不是 WMI：WMI 要拉起 COM，启动慢、依赖重，而 Display Class 键里
//! 已经有我们要的全部信息（型号、驱动版本、厂商）。
//!
//! 这个模块的产出直接决定 UI 该对用户说什么话。口径要分清楚：
//!
//! - `supports_rhi_neural_rendering` 表达的是**RHI 上游的兼容声明**
//!   （RTX 20/30/40/50），UI 据此显示「RHI 上游兼容」；
//! - NVIDIA 官方的 DLSS 5 能力（RTX 50 独占）**不在这里表达**——把官方能力
//!   和上游兼容混在一个函数里，UI 迟早对用户说错话。

/// NVIDIA 显卡的架构代次。只覆盖跑得动 DLSS 的那几代。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Generation {
    /// RTX 20 系
    Turing,
    /// RTX 30 系
    Ampere,
    /// RTX 40 系
    Ada,
    /// RTX 50 系
    Blackwell,
}

impl Generation {
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Turing => "RTX 20 系（Turing）",
            Self::Ampere => "RTX 30 系（Ampere）",
            Self::Ada => "RTX 40 系（Ada）",
            Self::Blackwell => "RTX 50 系（Blackwell）",
        }
    }

    /// 是否支持 DLSS 4 的 transformer 超分模型。
    ///
    /// 20 系起全部支持——这正是本项目成立的基础：换新 DLL 对老卡同样有效。
    pub const fn supports_transformer(self) -> bool {
        true
    }

    /// 是否支持帧生成（DLSS-G）。Ada 起才有光流加速器。
    pub const fn supports_frame_generation(self) -> bool {
        matches!(self, Self::Ada | Self::Blackwell)
    }

    /// RHI 上游方案是否覆盖这一代。
    ///
    /// RHI 当前发布说明声明 DLSS 5 组件覆盖 RTX 20/30/40/50。
    /// 这是**上游兼容声明**，不是 NVIDIA 官方能力——UI 只能写「RHI 上游兼容」，
    /// 不能写成 Vivid 或 NVIDIA 已验证。
    pub const fn supports_rhi_neural_rendering(self) -> bool {
        true
    }

    /// 从显卡型号名里认出代次。认不出返回 `None`，不猜。
    pub fn from_model_name(name: &str) -> Option<Self> {
        let upper = name.to_uppercase();
        let after = &upper[upper.find("RTX")? + 3..];

        let digits: String = after
            .chars()
            .skip_while(|c| !c.is_ascii_digit())
            .take_while(|c| c.is_ascii_digit())
            .collect();

        match digits.parse::<u32>().ok()? {
            2000..=2999 => Some(Self::Turing),
            3000..=3999 => Some(Self::Ampere),
            4000..=4999 => Some(Self::Ada),
            5000..=5999 => Some(Self::Blackwell),
            _ => None,
        }
    }
}

/// 一块显卡的检测结果。
#[derive(Debug, Clone)]
pub struct GpuInfo {
    /// 型号名，如 `NVIDIA GeForce RTX 4070 Ti SUPER`。
    pub model: String,
    /// Windows 记的驱动版本，如 `32.0.15.8108`。
    pub driver_windows: String,
    /// NVIDIA 对外的驱动版本，如 `581.08`。换算失败时为 `None`。
    pub driver_nvidia: Option<String>,
    pub generation: Option<Generation>,
}

impl GpuInfo {
    /// 给玩家看的一句话总结。
    pub fn summary(&self) -> String {
        let driver = self
            .driver_nvidia
            .as_deref()
            .unwrap_or(&self.driver_windows);
        match self.generation {
            Some(g) => format!("{}（{}），驱动 {}", self.model, g.display_name(), driver),
            None => format!("{}，驱动 {}", self.model, driver),
        }
    }
}

/// 把 Windows 的驱动版本换算成 NVIDIA 对外的版本号。
///
/// Windows 里记的是 `32.0.15.8108`，而 NVIDIA 官网、论坛、GeForce Experience
/// 说的都是 `581.08`。用户只认后者，所以 UI 必须显示后者。
///
/// 规则：把后两段拼起来去掉点，取最后 5 位，在第 3 位后插小数点。
pub fn nvidia_driver_version(windows_version: &str) -> Option<String> {
    let parts: Vec<&str> = windows_version.split('.').collect();
    if parts.len() < 4 {
        return None;
    }
    let joined = format!("{}{}", parts[2], parts[3]);
    if joined.len() < 5 || !joined.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let tail = &joined[joined.len() - 5..];
    Some(format!("{}.{}", &tail[..3], &tail[3..]))
}

/// 枚举本机的 NVIDIA 显卡。没有就返回空——没装 N 卡是正常情况，不是错误。
#[cfg(windows)]
pub fn detect() -> Vec<GpuInfo> {
    use winreg::RegKey;
    use winreg::enums::HKEY_LOCAL_MACHINE;

    // 显示适配器的 Device Class GUID，Windows 上是固定值。
    const DISPLAY_CLASS: &str =
        r"SYSTEM\CurrentControlSet\Control\Class\{4d36e968-e325-11ce-bfc1-08002be10318}";

    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    let Ok(class) = hklm.open_subkey(DISPLAY_CLASS) else {
        return Vec::new();
    };

    let mut out = Vec::new();
    for sub in class.enum_keys().flatten() {
        // 真正的适配器子键是 `0000`、`0001` 这种；其余（如 `Configuration`）跳过。
        if sub.is_empty() || !sub.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let Ok(key) = class.open_subkey(&sub) else {
            continue;
        };

        let provider: String = key.get_value("ProviderName").unwrap_or_default();
        if !provider.to_uppercase().contains("NVIDIA") {
            continue;
        }

        let model: String = key.get_value("DriverDesc").unwrap_or_default();
        let driver_windows: String = key.get_value("DriverVersion").unwrap_or_default();

        out.push(GpuInfo {
            driver_nvidia: nvidia_driver_version(&driver_windows),
            generation: Generation::from_model_name(&model),
            model,
            driver_windows,
        });
    }
    out
}

#[cfg(not(windows))]
pub fn detect() -> Vec<GpuInfo> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 本机实测值（2026-08-31）：Windows 记 32.0.15.8108，NVIDIA 叫它 581.08。
    #[test]
    fn converts_the_real_local_driver_version() {
        assert_eq!(
            nvidia_driver_version("32.0.15.8108").as_deref(),
            Some("581.08")
        );
    }

    #[test]
    fn converts_other_known_driver_versions() {
        // 31.0.15.5222 → 552.22
        assert_eq!(
            nvidia_driver_version("31.0.15.5222").as_deref(),
            Some("552.22")
        );
        // 30.0.14.9516 → 495.16（DriverStore 里那个老 nvngx.dll 的版本）
        assert_eq!(
            nvidia_driver_version("30.0.14.9516").as_deref(),
            Some("495.16")
        );
    }

    #[test]
    fn rejects_malformed_driver_versions() {
        for bad in ["", "32.0", "32.0.15", "abc.def.ghi.jkl", "1.2.3.4"] {
            assert_eq!(nvidia_driver_version(bad), None, "{bad} 不该被换算出结果");
        }
    }

    #[test]
    fn recognizes_generations_from_real_model_names() {
        let cases = [
            ("NVIDIA GeForce RTX 4070 Ti SUPER", Generation::Ada),
            ("NVIDIA GeForce RTX 3080 Ti", Generation::Ampere),
            ("NVIDIA GeForce RTX 2060", Generation::Turing),
            ("NVIDIA GeForce RTX 5090", Generation::Blackwell),
            ("NVIDIA GeForce RTX 4090 Laptop GPU", Generation::Ada),
        ];
        for (name, expected) in cases {
            assert_eq!(Generation::from_model_name(name), Some(expected), "{name}");
        }
    }

    /// 认不出就是认不出，不许瞎猜——猜错会让 UI 对用户说错话。
    #[test]
    fn does_not_guess_unknown_cards() {
        for name in [
            "NVIDIA GeForce GTX 1080 Ti",
            "AMD Radeon RX 7900 XTX",
            "Intel Arc A770",
            "",
        ] {
            assert_eq!(Generation::from_model_name(name), None, "{name}");
        }
    }

    /// RHI 上游兼容口径：RTX 20–50 全部覆盖。
    ///
    /// 注意这条**只**表达上游声明；官方能力不在此判断，避免 UI 把两者混为一谈。
    #[test]
    fn rhi_upstream_support_label_covers_rtx_20_through_50() {
        use Generation::*;
        for generation in [Turing, Ampere, Ada, Blackwell] {
            assert!(generation.supports_rhi_neural_rendering());
        }
    }

    /// 本项目成立的基础：换新 DLL 对老卡同样有效。
    #[test]
    fn every_supported_generation_benefits_from_transformer() {
        use Generation::*;
        for g in [Turing, Ampere, Ada, Blackwell] {
            assert!(g.supports_transformer(), "{}", g.display_name());
        }
    }

    #[test]
    fn frame_generation_starts_at_ada() {
        use Generation::*;
        assert!(!Turing.supports_frame_generation());
        assert!(!Ampere.supports_frame_generation());
        assert!(Ada.supports_frame_generation());
        assert!(Blackwell.supports_frame_generation());
    }

    #[test]
    fn summary_prefers_the_version_users_recognize() {
        let g = GpuInfo {
            model: "NVIDIA GeForce RTX 4070 Ti SUPER".into(),
            driver_windows: "32.0.15.8108".into(),
            driver_nvidia: Some("581.08".into()),
            generation: Some(Generation::Ada),
        };
        let s = g.summary();
        assert!(s.contains("581.08"), "要显示用户认得的版本号：{s}");
        assert!(
            !s.contains("32.0.15"),
            "不该把 Windows 内部版本号摆给用户看：{s}"
        );
    }

    /// 真机判据：这台机器上应当能认出至少一块 NVIDIA 卡，且信息完整。
    #[cfg(windows)]
    #[test]
    fn detects_a_real_nvidia_card_on_this_machine() {
        let gpus = detect();
        if gpus.is_empty() {
            return; // 没有 N 卡的机器上跳过，不算失败
        }
        let g = &gpus[0];
        assert!(!g.model.is_empty(), "型号不该为空");
        assert!(
            g.driver_nvidia.is_some(),
            "驱动版本 {} 应当能换算成 NVIDIA 的版本号",
            g.driver_windows
        );
    }
}
