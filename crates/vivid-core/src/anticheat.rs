//! 反作弊检测与安装闸门。
//!
//! **这一步必须最先做完。** 游戏面一铺开，CS2、PUBG、Apex 会直接出现在界面的
//! 游戏列表里；红线 3「不提供多人入口」如果只写在文档里、不由代码强制，
//! 那它在全库列表面前就名存实亡了。
//!
//! 规则是「**带反作弊 且 没有已验证的单机入口 → 不装**」，
//! 而**不是**「带反作弊就不装」：GTA5 Enhanced 自己也带 BattlEye，
//! 但它有一条作者本机实测过的一次性 `-nobattleye` 单机启动，所以仍然可装。
//!
//! # 这个闸门抓不到什么——必须写在界面上
//!
//! **VAC 这类反作弊在游戏目录里没有任何痕迹。** CS2、L4D2、TF2 的安装目录
//! 和单机游戏长得一模一样，校验发生在服务端。文件检测对它们完全无效。
//!
//! 所以另加一层代码内的已知联机游戏小名单。**这份名单必然会过期**——
//! Steam 每天上新游戏，而 Vivid 不联网维护它。两层都没命中的游戏，
//! 界面必须常驻 [`CANNOT_BE_SURE`] 那句话，不能让沉默被读成「这游戏是安全的」。

use std::collections::VecDeque;
use std::path::{Path, PathBuf};

use crate::recipe::GameId;

/// 界面常驻的那一句。**不能删。**
///
/// 两层检测都没命中 ≠ 这款游戏没有反作弊。把这句话拿掉，用户就会把
/// 「Vivid 没拦我」读成「Vivid 说这游戏能联机」。
pub const CANNOT_BE_SURE: &str = "Vivid 无法判断一款游戏有没有反作弊\
    （VAC 这类在游戏目录里没有任何痕迹），请只在单机游戏上使用。";

/// 名字扫描的层数上限：游戏根目录往下**四层**的名字都会被看到。
///
/// 反作弊组件不是被藏起来的东西——它要在游戏启动时被加载，所以不是在游戏根目录，
/// 就是在 `<Game>\Binaries\Win64\EasyAntiCheat_x64.dll` 这种虚幻引擎的固定位置上，
/// 那正好是第四层（有测试钉着）。递归整个 3A 游戏目录（动辄十万个文件）
/// 换不来更多准确度，只会让界面卡住。
const MAX_DEPTH: usize = 4;

/// 单次检测最多看多少个目录项。
///
/// 这是**性能兜底，不是安全边界**：安全边界是 [`CANNOT_BE_SURE`] 那句常驻提示。
/// 扫描是广度优先的，所以即使预算用尽，浅层（最可能的位置）也一定已经看过了。
const MAX_ENTRIES: usize = 50_000;

/// 反作弊检测结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AntiCheat {
    /// 两层都没命中。**不等于「这款游戏没有反作弊」**，见模块头注释。
    NotFound,
    /// 游戏目录里找到了反作弊组件的文件或目录。
    Present {
        kind: &'static str,
        /// 命中的那个文件/目录，界面原样展示——不解释、不推断，让用户自己看。
        evidence: PathBuf,
    },
    /// 代码内小名单命中（VAC 这类在游戏目录里没有痕迹）。
    KnownOnline { app_id: u32, name: &'static str },
}

impl AntiCheat {
    /// 界面上「为什么不给装」那一句。放行时没有话说。
    pub fn reason(&self) -> Option<String> {
        match self {
            Self::NotFound => None,
            Self::Present { kind, evidence } => Some(format!(
                "游戏目录里有 {kind} 反作弊组件（{}），Vivid 不提供安装。",
                evidence.display()
            )),
            Self::KnownOnline { name, .. } => Some(format!(
                "{name} 是代码内名单里的联机游戏，Vivid 不提供安装。"
            )),
        }
    }

    /// 反作弊的名字，给界面分组和日志用。
    pub fn kind(&self) -> Option<&str> {
        match self {
            Self::NotFound => None,
            Self::Present { kind, .. } => Some(kind),
            Self::KnownOnline { .. } => Some("已知联机游戏"),
        }
    }
}

/// 这款游戏有没有**已验证的**单机入口。
///
/// 「已验证」的含义很窄：作者在本机实测过、并且写进配方的一次性单机启动。
/// 目前只有 GTA5 Enhanced 的 `-nobattleye`。**这不是给「带反作弊也想装」开的口子。**
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SinglePlayerEntry {
    Verified,
    None,
}

/// 一类反作弊的文件特征。都是事实性的文件名，不随版本过期。
struct Signature {
    kind: &'static str,
    /// 目录名（小写全等）。
    dirs: &'static [&'static str],
    /// 文件名前缀（小写）。
    file_prefixes: &'static [&'static str],
}

const SIGNATURES: [Signature; 4] = [
    Signature {
        kind: "EasyAntiCheat",
        dirs: &["easyanticheat", "easyanticheat_eos"],
        file_prefixes: &["easyanticheat", "start_protected_game"],
    },
    Signature {
        kind: "BattlEye",
        dirs: &["battleye"],
        file_prefixes: &["battleye", "beservice", "beclient", "belauncher"],
    },
    Signature {
        kind: "Denuvo Anti-Cheat",
        dirs: &["denuvo"],
        file_prefixes: &["denuvo"],
    },
    // Activision Ricochet 的内核驱动。2026-08-31 在本机的
    // `Call of Duty Modern Warfare II` 根目录里实测到 `Randgrid.sys`——
    // 这款游戏既没有 EAC 也没有 BattlEye 的痕迹，只有这一个文件能抓到它。
    Signature {
        kind: "Ricochet",
        dirs: &[],
        file_prefixes: &["randgrid.sys"],
    },
];

/// 代码内的已知联机游戏名单（App ID 升序）。
///
/// **存在的唯一理由**是文件检测对 VAC 无效。这份名单：
///
/// - **一定会过期**——Steam 每天上新游戏，Vivid 不联网维护它；
/// - **只做加法不做减法**——漏掉一款联机游戏的代价，比多拦一款单机游戏大得多；
/// - **绝不包含已验证的两款游戏**（GTA5 Enhanced `3240220` / RDR2 `1174180`），
///   有测试钉着这一条。
const KNOWN_ONLINE: [(u32, &str); 24] = [
    (10, "Counter-Strike"),
    (240, "Counter-Strike: Source"),
    (440, "Team Fortress 2"),
    (500, "Left 4 Dead"),
    (550, "Left 4 Dead 2"),
    (570, "Dota 2"),
    (730, "Counter-Strike 2"),
    (4_000, "Garry's Mod"),
    (230_410, "Warframe"),
    (236_390, "War Thunder"),
    (252_490, "Rust"),
    (271_590, "Grand Theft Auto V（Legacy，含 GTA Online）"),
    (359_550, "Tom Clancy's Rainbow Six Siege"),
    (553_850, "HELLDIVERS 2"),
    (578_080, "PUBG: BATTLEGROUNDS"),
    (1_085_660, "Destiny 2"),
    (1_172_470, "Apex Legends"),
    (1_203_220, "NARAKA: BLADEPOINT"),
    // 本机实测发现的两个：文件检测抓不到，或者抓到了也该有个更准确的说法。
    (1_402_480, "NARAKA: BLADEPOINT - Test Server"),
    (1_517_290, "Battlefield 2042"),
    (1_599_340, "Lost Ark"),
    (1_938_090, "Call of Duty"),
    (2_073_850, "THE FINALS"),
    (3_595_230, "Call of Duty: Modern Warfare II"),
];

/// 小名单查询。命中返回游戏名。
pub fn known_online(app_id: u32) -> Option<&'static str> {
    KNOWN_ONLINE
        .iter()
        .find(|(id, _)| *id == app_id)
        .map(|(_, name)| *name)
}

/// 这款游戏有没有已验证的单机入口。目前只有 GTA5 Enhanced。
pub fn single_player_entry(app_id: u32) -> SinglePlayerEntry {
    if app_id == GameId::Gta5Enhanced.app_id() {
        SinglePlayerEntry::Verified
    } else {
        SinglePlayerEntry::None
    }
}

/// 扫游戏目录找反作弊组件的痕迹。
///
/// 广度优先：根目录整层看完才往下一层。这样即使 [`MAX_ENTRIES`] 用尽，
/// 最可能的位置也一定已经看过——**深度优先做不到这个保证**。
pub fn detect(game_root: &Path) -> AntiCheat {
    let mut queue: VecDeque<(PathBuf, usize)> = VecDeque::new();
    queue.push_back((game_root.to_path_buf(), 0));
    let mut budget = MAX_ENTRIES;

    while let Some((dir, depth)) = queue.pop_front() {
        // 读不动的目录跳过（权限不足、正在被删）：这不是错误，也不该让检测失败。
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            if budget == 0 {
                return AntiCheat::NotFound;
            }
            budget -= 1;

            let name = entry.file_name().to_string_lossy().to_lowercase();
            let is_dir = entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false);
            if let Some(kind) = match_signature(&name, is_dir) {
                return AntiCheat::Present {
                    kind,
                    evidence: entry.path(),
                };
            }
            if is_dir && depth + 1 < MAX_DEPTH {
                queue.push_back((entry.path(), depth + 1));
            }
        }
    }
    AntiCheat::NotFound
}

fn match_signature(lower_name: &str, is_dir: bool) -> Option<&'static str> {
    SIGNATURES
        .iter()
        .find(|signature| {
            if is_dir {
                signature.dirs.contains(&lower_name)
            } else {
                signature
                    .file_prefixes
                    .iter()
                    .any(|prefix| lower_name.starts_with(prefix))
            }
        })
        .map(|signature| signature.kind)
}

/// 文件层闸门：目录里没有反作弊痕迹，或者这款游戏有已验证的单机入口。
pub fn installable(game_root: &Path, entry: SinglePlayerEntry) -> bool {
    match detect(game_root) {
        AntiCheat::NotFound => true,
        _ => entry == SinglePlayerEntry::Verified,
    }
}

/// 完整闸门：先查代码内的已知联机名单，再看游戏目录里的文件痕迹。
///
/// 返回 `Some(理由)` 表示**不提供安装**，`None` 表示放行。
///
/// 已知联机名单**不给单机入口开口子**：那份名单是刻意写死的
/// 「这就是一款联机游戏」，比在目录里找到什么更确定。
pub fn blocking_reason(
    app_id: u32,
    game_root: &Path,
    entry: SinglePlayerEntry,
) -> Option<AntiCheat> {
    if let Some(name) = known_online(app_id) {
        return Some(AntiCheat::KnownOnline { app_id, name });
    }
    match detect(game_root) {
        AntiCheat::NotFound => None,
        _ if entry == SinglePlayerEntry::Verified => None,
        found => Some(found),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 造一个只有指定相对路径的假游戏目录。
    fn fixture_with(paths: &[&str]) -> tempfile::TempDir {
        let temp = tempfile::tempdir().unwrap();
        for relative in paths {
            let path = temp.path().join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, b"MZ").unwrap();
        }
        temp
    }

    #[test]
    fn easy_anti_cheat_files_are_detected() {
        let game = fixture_with(&["EasyAntiCheat/EasyAntiCheat_x64.dll"]);
        assert!(matches!(
            detect(game.path()),
            AntiCheat::Present {
                kind: "EasyAntiCheat",
                ..
            }
        ));
    }

    #[test]
    fn battleye_files_are_detected() {
        let game = fixture_with(&["BattlEye/BEService_x64.exe"]);
        assert!(matches!(
            detect(game.path()),
            AntiCheat::Present {
                kind: "BattlEye",
                ..
            }
        ));
    }

    /// GTA5 也带 BattlEye，但它有一条**已验证的**一次性单机入口，所以仍然可装。
    /// 规则是「带反作弊 **且** 没有已验证的单机入口 → 不装」，
    /// 不是「带反作弊就不装」。
    #[test]
    fn a_verified_single_player_entry_overrides_the_gate() {
        let gta = fixture_with(&["BattlEye/BEService_x64.exe", "GTA5_Enhanced.exe"]);
        assert!(installable(gta.path(), SinglePlayerEntry::Verified));
        assert!(!installable(gta.path(), SinglePlayerEntry::None));
    }

    /// 干净的单机游戏不受影响。
    #[test]
    fn a_clean_single_player_game_passes() {
        let game = fixture_with(&["Cyberpunk2077.exe"]);
        assert_eq!(detect(game.path()), AntiCheat::NotFound);
    }

    /// Denuvo Anti-Cheat 也认。
    #[test]
    fn denuvo_anti_cheat_is_detected() {
        let game = fixture_with(&["denuvo-anti-cheat-runtime.dll"]);
        assert!(matches!(
            detect(game.path()),
            AntiCheat::Present {
                kind: "Denuvo Anti-Cheat",
                ..
            }
        ));
    }

    /// UE 游戏习惯把反作弊放在 `Binaries\Win64` 下，扫描要够得着。
    #[test]
    fn a_component_two_levels_down_is_still_found() {
        let game = fixture_with(&["ShooterGame/Binaries/Win64/EasyAntiCheat_x64.dll"]);
        assert!(matches!(detect(game.path()), AntiCheat::Present { .. }));
    }

    /// **诚实地记下扫描的边界**：埋得比 `MAX_DEPTH` 更深就抓不到。
    /// 这不是 bug，是取舍——所以界面必须常驻「Vivid 无法判断」那句话。
    #[test]
    fn the_scan_depth_limit_is_documented_by_a_test() {
        let game = fixture_with(&["a/b/c/d/EasyAntiCheat_x64.dll"]);
        assert_eq!(detect(game.path()), AntiCheat::NotFound);
        assert!(CANNOT_BE_SURE.contains("无法判断"));
    }

    /// **VAC 抓不到**：CS2 的目录干干净净，只有小名单能拦下它。
    #[test]
    fn a_vac_only_game_is_caught_by_the_list_not_by_files() {
        let cs2 = fixture_with(&["cs2.exe"]);
        assert_eq!(detect(cs2.path()), AntiCheat::NotFound);
        assert_eq!(
            blocking_reason(730, cs2.path(), SinglePlayerEntry::None),
            Some(AntiCheat::KnownOnline {
                app_id: 730,
                name: "Counter-Strike 2"
            })
        );
    }

    /// 已知联机名单**不给单机入口开口子**。
    #[test]
    fn the_known_online_list_is_not_overridden_by_a_single_player_entry() {
        let apex = fixture_with(&["r5apex.exe"]);
        assert!(blocking_reason(1_172_470, apex.path(), SinglePlayerEntry::Verified).is_some());
    }

    /// 两款已验证的游戏**绝不能**出现在名单里，否则现有安装会一夜之间不可用。
    #[test]
    fn the_two_verified_games_are_never_on_the_online_list() {
        assert_eq!(known_online(3_240_220), None, "GTA5 Enhanced");
        assert_eq!(known_online(1_174_180), None, "RDR2");
        assert_eq!(known_online(1_091_500), None, "Cyberpunk 2077 是单机游戏");
    }

    /// 只有 GTA5 Enhanced 有已验证的单机入口。
    #[test]
    fn only_gta5_enhanced_has_a_verified_single_player_entry() {
        assert_eq!(single_player_entry(3_240_220), SinglePlayerEntry::Verified);
        assert_eq!(single_player_entry(1_174_180), SinglePlayerEntry::None);
        assert_eq!(single_player_entry(1_091_500), SinglePlayerEntry::None);
    }

    /// 名单按 App ID 升序且无重复——人肉维护的表，让机器盯着它的形状。
    #[test]
    fn the_known_online_list_is_sorted_and_unique() {
        let ids: Vec<u32> = KNOWN_ONLINE.iter().map(|(id, _)| *id).collect();
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(ids, sorted, "KNOWN_ONLINE 必须按 App ID 升序且不重复");
    }

    /// 干净的单机游戏一路放行。
    #[test]
    fn a_clean_game_is_not_blocked() {
        let game = fixture_with(&["Cyberpunk2077.exe"]);
        assert_eq!(
            blocking_reason(1_091_500, game.path(), SinglePlayerEntry::None),
            None
        );
    }

    /// 不存在的目录不该 panic。
    #[test]
    fn a_missing_directory_is_not_an_error() {
        let temp = tempfile::tempdir().unwrap();
        assert_eq!(detect(&temp.path().join("nope")), AntiCheat::NotFound);
    }

    /// 拦截理由要能直接念给用户听。
    #[test]
    fn the_reason_is_plain_chinese() {
        let game = fixture_with(&["EasyAntiCheat/EasyAntiCheat_x64.dll"]);
        let reason = detect(game.path()).reason().unwrap();
        assert!(reason.contains("EasyAntiCheat"));
        assert!(reason.contains("不提供安装"));
        assert_eq!(AntiCheat::NotFound.reason(), None);
    }
}
