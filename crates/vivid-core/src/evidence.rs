//! 安装健康度与「到底生没生效」的证据。
//!
//! 这个模块存在的唯一理由，是**不让「文件复制成功」冒充「画面效果生效」**。
//! CLAUDE.md 第五节把证据分成三层，这里就照这三层来：
//!
//! 1. `文件已安装`——目标文件的哈希和 ledger 记的一致；
//! 2. `检测到组件载入`——`ReShade.log` 里有**本次安装之后**的成功载入行；
//! 3. `画面效果待实测`——程序无法从文件或日志证明最终画面变化。
//!
//! 两条刻意的保守规则：
//!
//! - 日志里只是**提到**了 addon 文件名（比如 `Scanning ...`）**不算载入成功**。
//!   把「提到」当成「成功」，用户就会以为生效了，实际什么都没发生。
//! - 日志修改时间早于本次安装 → `StaleLog`。上次装的证据不能给这次背书。

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::recipe::GameId;

/// 我们管理的那个 addon 的文件名。证据判定只认它。
pub const MANAGED_ADDON: &str = "renodx-dlss5.addon64";

/// 同一行里出现这些词才算载入成功。
const SUCCESS_WORDS: [&str; 4] = [
    "successfully loaded",
    "loaded add-on",
    "initialized",
    "registered",
];

/// 同一行里出现这些词就算失败。失败优先于成功，宁可少报喜。
const FAILURE_WORDS: [&str; 2] = ["failed", "error"];

/// ReShade 载入 addon 那一行的固定前缀（小写比较）。
const LOADING_PREFIX: &str = "loading add-on";
/// 注册成功那一行。
const REGISTERED_PREFIX: &str = "registered add-on";
/// 注销行也含 "registered add-on"，要排掉。
const UNREGISTERED_PREFIX: &str = "unregistered add-on";
/// 组件实际执行成功的记录。
const EVALUATED: &str = "evaluation succeeded";

/// ReShade 日志能提供的证据强度。**声明顺序即强弱顺序**，弱的可以被强的覆盖。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LoadEvidence {
    /// 没有日志，或者日志里根本没提过这个 addon。
    NoLog,
    /// 日志比本次安装还早，不能作为本次的证据。
    StaleLog,
    /// 提到了文件名，但还没看到注册成功。
    Mentioned,
    /// 载入并注册成功。
    Loaded,
    /// 不只是载入：日志里有它**实际执行**的记录。
    Running,
    /// 载入这个文件时失败了。
    Failed,
}

/// 一款游戏此刻的状态。界面上的「大结论」直接来自它。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GameState {
    NotFound,
    UnsupportedGpu,
    ManifestUnavailable,
    /// 检测到反作弊，且没有已验证的单机入口。**不提供安装。**
    ///
    /// 这个状态和「没找到」是两回事：Vivid 看见了这款游戏，只是不给装，
    /// 而且说得出为什么。悄悄把它从列表里抹掉只会让人以为是没扫到。
    BlockedByAntiCheat {
        reason: String,
    },
    /// 判不出这款游戏用的是哪种图形 API。**不猜，也不提供安装。**
    UnknownApi,
    /// 目录里连一个可执行文件都没有——这不像一款装好的游戏
    /// （Steam 库里那些运行库包就是这样）。
    ///
    /// 和 `NotFound` 分开是因为**说法不一样**：这个目录确实装着，
    /// 说它「没找到、请确认已经通过 Steam 安装」是错的
    /// （2026-09-01 版式自检时，Steamworks 运行库包就被这么误报了）。
    NotAGame,
    ReadyToInstall,
    Installed,
    UpdateAvailable {
        installed: String,
        available: String,
    },
    MissingFiles {
        files: Vec<PathBuf>,
    },
    Conflict {
        files: Vec<PathBuf>,
    },
}

/// 界面上那个**唯一**的主按钮。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrimaryAction {
    None,
    Install,
    Update,
    Repair,
    /// GTA5 专用：一次性 `-nobattleye` 单机启动。
    LaunchSinglePlayer,
    LaunchGame,
    ShowConflict,
}

impl PrimaryAction {
    /// 按钮上的固定中文。
    pub const fn label(self) -> &'static str {
        match self {
            Self::None => "",
            Self::Install => "一键安装 DLSS 5",
            Self::Update => "更新组件",
            Self::Repair => "修复文件",
            Self::LaunchSinglePlayer => "启动单机模式",
            Self::LaunchGame => "启动游戏",
            Self::ShowConflict => "查看冲突",
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum EvidenceError {
    #[error("读 ReShade 日志失败：{0}")]
    Io(String),
}

/// 从日志文本判断证据强度。
///
/// 优先级：失败 > 正在运行 > 载入 > 只是提到 > 没提过。失败排在最前是刻意的——
/// 日志里既有失败又有成功时，报「成功」等于替用户下了乐观结论。
///
/// **真实 ReShade 日志把一次载入拆成两行**（2026-08-31 GTA5 真机抓到）：
///
/// ```text
/// Loading add-on from '<游戏目录>/renodx-dlss5.addon64' ...
/// Registered add-on "DLSS 5 Neural Rendering" v0.2026.828.517 using ReShade API version 18.
/// ```
///
/// 文件名只在第一行，成功词只在第二行。所以这里要记住「刚刚在载入我们这个文件」
/// 这个状态，再看下一条注册结果——只按同一行匹配会把成功的安装误判成「只是提到」。
///
/// 注册行里的引号内容是组件自报的名字，后续带这个名字且写着 `evaluation succeeded`
/// 的行说明它**在实际干活**，那比「载入了」更强一层。
pub fn parse_reshade_log(text: &str) -> LoadEvidence {
    let mut evidence = LoadEvidence::NoLog;
    // 刚看到「正在载入我们这个 addon」，等它的注册结果。
    let mut awaiting_registration = false;
    let mut registered_name: Option<String> = None;

    for line in text.lines() {
        let lower = line.to_lowercase();

        if lower.contains(MANAGED_ADDON) {
            if FAILURE_WORDS.iter().any(|word| lower.contains(word)) {
                return LoadEvidence::Failed;
            }
            evidence = evidence.max(LoadEvidence::Mentioned);
            if SUCCESS_WORDS.iter().any(|word| lower.contains(word)) {
                evidence = evidence.max(LoadEvidence::Loaded);
            }
            // 只有「正在载入这个文件」才值得等下一行的注册结果。
            awaiting_registration = lower.contains(LOADING_PREFIX);
            continue;
        }

        // 换成载入别的 addon 了，之前那次的等待作废。
        if lower.contains(LOADING_PREFIX) {
            awaiting_registration = false;
            continue;
        }

        if awaiting_registration
            && lower.contains(REGISTERED_PREFIX)
            && !lower.contains(UNREGISTERED_PREFIX)
        {
            evidence = evidence.max(LoadEvidence::Loaded);
            registered_name = quoted(line);
            awaiting_registration = false;
            continue;
        }

        // 组件自报的名字 + 执行成功 = 它不只是载入了，还在干活。
        if let Some(name) = &registered_name
            && line.contains(name.as_str())
            && lower.contains(EVALUATED)
        {
            evidence = evidence.max(LoadEvidence::Running);
        }
    }
    evidence
}

/// 取一行里第一对双引号之间的内容。
fn quoted(line: &str) -> Option<String> {
    let start = line.find('"')? + 1;
    let rest = line.get(start..)?;
    let end = rest.find('"')?;
    let name = rest.get(..end)?.trim();
    (!name.is_empty()).then(|| name.to_string())
}

/// 第一条失败日志的原文。界面直接展示它，不猜原因。
pub fn failure_line(text: &str) -> Option<String> {
    text.lines()
        .find(|line| {
            let lower = line.to_lowercase();
            lower.contains(MANAGED_ADDON) && FAILURE_WORDS.iter().any(|w| lower.contains(w))
        })
        .map(|line| line.trim().to_string())
}

/// 读日志文件并判断证据。
///
/// 日志不存在不是错误（游戏还没启动过很正常）；
/// 日志比 `installed_at` 还早，则只能算旧日志。
pub fn inspect_log(path: &Path, installed_at: SystemTime) -> Result<LoadEvidence, EvidenceError> {
    let Ok(meta) = std::fs::metadata(path) else {
        return Ok(LoadEvidence::NoLog);
    };
    let modified = meta
        .modified()
        .map_err(|e| EvidenceError::Io(e.to_string()))?;
    if modified < installed_at {
        return Ok(LoadEvidence::StaleLog);
    }
    let text = std::fs::read_to_string(path).map_err(|e| EvidenceError::Io(e.to_string()))?;
    Ok(parse_reshade_log(&text))
}

/// 状态 → 唯一主按钮。**一个状态只对应一个动作**，界面不做第二种解读。
pub const fn primary_action(state: &GameState, game: GameId) -> PrimaryAction {
    match state {
        // 不给装的三种情况都没有主按钮：**没有按钮就点不出安装来**，
        // 这比在按钮上写「不可用」更可靠。
        GameState::NotFound
        | GameState::UnsupportedGpu
        | GameState::ManifestUnavailable
        | GameState::BlockedByAntiCheat { .. }
        | GameState::UnknownApi
        | GameState::NotAGame => PrimaryAction::None,
        GameState::ReadyToInstall => PrimaryAction::Install,
        GameState::UpdateAvailable { .. } => PrimaryAction::Update,
        GameState::MissingFiles { .. } => PrimaryAction::Repair,
        // 冲突永远不映射成安装/更新/还原：那都会覆盖别人的改动。
        GameState::Conflict { .. } => PrimaryAction::ShowConflict,
        GameState::Installed => match game {
            // 只有 GTA5 有那条实测过的一次性单机启动，别的游戏一律普通启动。
            GameId::Gta5Enhanced => PrimaryAction::LaunchSinglePlayer,
            GameId::RedDeadRedemption2 | GameId::App(_) => PrimaryAction::LaunchGame,
        },
    }
}

/// 三层证据的固定文案。**没装就什么都不说**，装了也只说到能证明的那一层。
pub fn evidence_summary(files_installed: bool, evidence: LoadEvidence) -> Vec<&'static str> {
    if !files_installed {
        return Vec::new();
    }
    let mut lines = vec!["文件已安装"];
    match evidence {
        LoadEvidence::Running => {
            lines.push("检测到组件载入");
            lines.push("检测到组件正在运行");
            lines.push("画面效果待实测");
        }
        LoadEvidence::Loaded => {
            lines.push("检测到组件载入");
            lines.push("画面效果待实测");
        }
        LoadEvidence::Failed => lines.push("日志里有载入失败的记录"),
        LoadEvidence::NoLog | LoadEvidence::StaleLog | LoadEvidence::Mentioned => {
            lines.push("画面效果待实测");
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// 2026-08-31 GTA5 真机验收抓下来的日志片段。
    const REAL_LOG: &str = include_str!("../tests/fixtures/reshade-gta5-real.log");

    #[test]
    fn success_requires_a_positive_line_for_the_managed_addon() {
        let log = r#"
12:00:01 Loading add-on "renodx-dlss5.addon64"
12:00:02 Successfully loaded add-on "renodx-dlss5.addon64"
"#;
        assert_eq!(parse_reshade_log(log), LoadEvidence::Loaded);
    }

    /// 真实 ReShade 日志：**文件名和成功词不在同一行**。
    /// `Loading add-on from '...renodx-dlss5.addon64'` 之后另起一行
    /// `Registered add-on "DLSS 5 Neural Rendering"`，最初按同行匹配写的规则认不出来。
    /// 样本是 2026-08-31 GTA5 真机验收时抓的。
    #[test]
    fn the_real_reshade_log_is_recognised_as_running() {
        assert_eq!(parse_reshade_log(REAL_LOG), LoadEvidence::Running);
    }

    /// 只看到「正在载入」还不算成功，得等到注册那一行。
    #[test]
    fn a_load_without_registration_is_only_mentioned() {
        let log = "Loading add-on from 'C:/g/renodx-dlss5.addon64' ...";
        assert_eq!(parse_reshade_log(log), LoadEvidence::Mentioned);
    }

    /// 注册成功但没有执行记录 → 只能说「载入」，不能说「正在运行」。
    #[test]
    fn registration_without_evaluation_is_loaded_not_running() {
        let log = "Loading add-on from 'C:/g/renodx-dlss5.addon64' ...
                   Registered add-on \"DLSS 5 Neural Rendering\" v1 using ReShade API version 18.";
        assert_eq!(parse_reshade_log(log), LoadEvidence::Loaded);
    }

    /// 别的 addon 注册成功，不能算到我们头上。
    #[test]
    fn another_addons_registration_is_not_ours() {
        let log = "Loading add-on from 'C:/g/other.addon64' ...
                   Registered add-on \"Something Else\" v1 using ReShade API version 18.
                   Scanning renodx-dlss5.addon64";
        assert_eq!(parse_reshade_log(log), LoadEvidence::Mentioned);
    }

    #[test]
    fn a_filename_mention_alone_is_not_success() {
        let log = r#"Scanning renodx-dlss5.addon64"#;
        assert_eq!(parse_reshade_log(log), LoadEvidence::Mentioned);
    }

    #[test]
    fn failure_wins_over_a_plain_mention() {
        let log = r#"Failed to load renodx-dlss5.addon64: error 126"#;
        assert_eq!(parse_reshade_log(log), LoadEvidence::Failed);
    }

    /// 成功词必须和 addon 文件名在同一行；别的组件成功了不算数。
    #[test]
    fn a_success_line_about_another_addon_does_not_count() {
        let log =
            "Successfully loaded add-on \"something-else.addon64\"\nScanning renodx-dlss5.addon64";
        assert_eq!(parse_reshade_log(log), LoadEvidence::Mentioned);
    }

    /// 日志里完全没提过这个 addon → 没有证据，不是失败也不是成功。
    #[test]
    fn a_log_without_the_addon_has_no_evidence() {
        assert_eq!(
            parse_reshade_log("nothing to see here"),
            LoadEvidence::NoLog
        );
    }

    #[test]
    fn matching_ignores_case() {
        let log = "SUCCESSFULLY LOADED ADD-ON \"RENODX-DLSS5.ADDON64\"";
        assert_eq!(parse_reshade_log(log), LoadEvidence::Loaded);
    }

    /// 失败行要能原样拿出来给用户看，不猜原因。
    #[test]
    fn the_failing_line_is_reported_verbatim() {
        let log = "ok line\nFailed to load renodx-dlss5.addon64: error 126\n";
        assert_eq!(
            failure_line(log).unwrap(),
            "Failed to load renodx-dlss5.addon64: error 126"
        );
    }

    /// 安装之前的旧日志不能拿来当本次安装的证据。
    #[test]
    fn a_log_older_than_the_install_is_stale() {
        let temp = tempfile::tempdir().unwrap();
        let log = temp.path().join("ReShade.log");
        std::fs::write(&log, "Successfully loaded add-on \"renodx-dlss5.addon64\"").unwrap();

        let future = std::time::SystemTime::now() + Duration::from_secs(3600);
        assert_eq!(inspect_log(&log, future).unwrap(), LoadEvidence::StaleLog);

        let past = std::time::SystemTime::now() - Duration::from_secs(3600);
        assert_eq!(inspect_log(&log, past).unwrap(), LoadEvidence::Loaded);
    }

    #[test]
    fn a_missing_log_is_not_an_error() {
        let temp = tempfile::tempdir().unwrap();
        let missing = temp.path().join("ReShade.log");
        assert_eq!(
            inspect_log(&missing, std::time::UNIX_EPOCH).unwrap(),
            LoadEvidence::NoLog
        );
    }

    #[test]
    fn update_is_never_applied_without_a_user_action() {
        let state = GameState::UpdateAvailable {
            installed: "310.8.SF".into(),
            available: "310.8.SF-v2".into(),
        };
        assert_eq!(
            primary_action(&state, GameId::Gta5Enhanced),
            PrimaryAction::Update
        );
    }

    #[test]
    fn installed_gta_primary_action_is_single_player_launch() {
        assert_eq!(
            primary_action(&GameState::Installed, GameId::Gta5Enhanced),
            PrimaryAction::LaunchSinglePlayer
        );
    }

    #[test]
    fn installed_rdr2_primary_action_is_a_plain_launch() {
        assert_eq!(
            primary_action(&GameState::Installed, GameId::RedDeadRedemption2),
            PrimaryAction::LaunchGame
        );
    }

    #[test]
    fn conflict_never_maps_to_install_update_or_restore() {
        let state = GameState::Conflict {
            files: vec![PathBuf::from("nvngx_dlss.dll")],
        };
        assert_eq!(
            primary_action(&state, GameId::Gta5Enhanced),
            PrimaryAction::ShowConflict
        );
    }

    #[test]
    fn every_other_state_maps_to_exactly_one_button() {
        for (state, expected) in [
            (GameState::NotFound, PrimaryAction::None),
            (GameState::UnsupportedGpu, PrimaryAction::None),
            (GameState::ManifestUnavailable, PrimaryAction::None),
            (
                GameState::BlockedByAntiCheat {
                    reason: "检测到 EasyAntiCheat".to_string(),
                },
                PrimaryAction::None,
            ),
            (GameState::UnknownApi, PrimaryAction::None),
            (GameState::NotAGame, PrimaryAction::None),
            (GameState::ReadyToInstall, PrimaryAction::Install),
            (
                GameState::MissingFiles {
                    files: vec![PathBuf::from("nvngx_dlss.dll")],
                },
                PrimaryAction::Repair,
            ),
        ] {
            assert_eq!(
                primary_action(&state, GameId::Gta5Enhanced),
                expected,
                "{state:?}"
            );
        }
    }

    /// 三层证据的文案是固定的：文件装上了不等于画面生效了。
    #[test]
    fn evidence_wording_never_claims_more_than_it_knows() {
        assert_eq!(
            evidence_summary(true, LoadEvidence::Loaded),
            vec!["文件已安装", "检测到组件载入", "画面效果待实测"]
        );
        // 只是被提到过，不能升级成「已载入」
        assert_eq!(
            evidence_summary(true, LoadEvidence::Mentioned),
            vec!["文件已安装", "画面效果待实测"]
        );
        // 有实际执行记录时可以多说一层，但仍然不替用户判断画面。
        assert_eq!(
            evidence_summary(true, LoadEvidence::Running),
            vec![
                "文件已安装",
                "检测到组件载入",
                "检测到组件正在运行",
                "画面效果待实测"
            ]
        );
        assert_eq!(
            evidence_summary(true, LoadEvidence::StaleLog),
            vec!["文件已安装", "画面效果待实测"]
        );
        assert_eq!(
            evidence_summary(true, LoadEvidence::Failed),
            vec!["文件已安装", "日志里有载入失败的记录"]
        );
        assert!(evidence_summary(false, LoadEvidence::Loaded).is_empty());
    }
}
