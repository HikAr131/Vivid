//! 全库发现 + 每款游戏的可安装性裁决。
//!
//! 这是「游戏面铺开」的入口：把本机 Steam 库里的每一款游戏依次过一遍
//! **反作弊闸门 → 身份 → 图形 API → 裁决**，得到一张界面可以直接画的列表。
//!
//! 三条贯穿始终的规则：
//!
//! 1. **闸门排在最前面，顺序不能换。** 一款联机游戏无论 API 判得清不清楚、
//!    主程序埋得多深，得到的都必须是同一个答案。把闸门放在后面，等于让
//!    「判得出 API」「找得到主程序」变成绕过闸门的条件——2026-08-31 本机实测
//!    真的撞上了：CS2 的主程序在四层深处，身份检查先失败，界面上说的是
//!    「不是一款装好的游戏」而不是「联机游戏不给装」。
//! 2. **不给装的游戏仍然要列出来。** 用户得知道 Vivid 看见了它、以及为什么不给装；
//!    悄悄从列表里抹掉只会让人以为是没扫到。
//! 3. **判不出就不装。** 判不出图形 API 的游戏没有主按钮——猜错注入方式的代价是
//!    游戏闪退（2026-08-31 RDR2 实测）。

use std::collections::VecDeque;
use std::path::{Path, PathBuf};

use crate::anticheat::{self, AntiCheat};
use crate::graphics_api::{self, ApiSource, GraphicsApi};
use crate::recipe::{GameId, GameProfile, Verification};
use crate::rhi_manifest::RhiSnapshot;
use crate::steam::SteamApp;

/// 找主程序时最多进入多少个目录、下探几层。
///
/// 四层是**由真实布局定的**，不是拍脑袋：2026-08-31 本机实测，
/// CS2 的主程序在 `game\bin\win64\cs2.exe`、PUBG 在
/// `TslGame\Binaries\Win64\TslGame.exe`，都正好是第四层。
/// 只扫三层的话，这两款游戏会被判成「目录里没有可执行文件」。
const EXE_SCAN_DEPTH: usize = 4;
const EXE_SCAN_MAX_DIRS: usize = 256;

/// 这些目录里的 exe 是**安装器**，不是游戏主程序。
const SKIP_DIRS: [&str; 6] = [
    "_commonredist",
    "commonredist",
    "directx",
    "vcredist",
    "prerequisites",
    "support",
];

/// 一款游戏能不能装，以及为什么。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Eligibility {
    /// 可以装。是否已验证由档案里的 `verification` 携带。
    Installable(GameProfile),
    /// 检测到反作弊，且没有已验证的单机入口。
    BlockedByAntiCheat(AntiCheat),
    /// 图形 API 判不出来，**不猜**。
    UnknownApi,
    /// 目录里找不到任何可执行文件——这不像一款装好的游戏。
    NotAGame,
}

impl Eligibility {
    pub const fn profile(&self) -> Option<&GameProfile> {
        match self {
            Self::Installable(profile) => Some(profile),
            _ => None,
        }
    }

    pub const fn is_installable(&self) -> bool {
        matches!(self, Self::Installable(_))
    }
}

/// 一款被扫到的游戏和对它的裁决。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SurveyedGame {
    pub app: SteamApp,
    /// 图形 API 的判定依据，界面要能说清「我凭什么这么判」。
    pub api_source: ApiSource,
    pub eligibility: Eligibility,
}

impl SurveyedGame {
    pub fn name(&self) -> &str {
        &self.app.name
    }

    pub fn app_id(&self) -> u32 {
        self.app.app_id
    }

    pub fn id(&self) -> GameId {
        GameId::from_app_id(self.app.app_id)
    }

    pub const fn profile(&self) -> Option<&GameProfile> {
        self.eligibility.profile()
    }

    /// 界面上「为什么不给装」那一句。可以装时没有话说。
    pub fn blocked_reason(&self) -> Option<String> {
        match &self.eligibility {
            Eligibility::Installable(_) => None,
            Eligibility::BlockedByAntiCheat(found) => found.reason(),
            Eligibility::UnknownApi => Some(
                "判不出这款游戏用的是哪种图形 API。注入方式和游戏实际用的 API 对不上会让游戏闪退，\
                 所以 Vivid 不猜，也不提供安装。"
                    .to_string(),
            ),
            Eligibility::NotAGame => {
                Some("这个目录里没有可执行文件，看起来不是一款装好的游戏。".to_string())
            }
        }
    }
}

/// 对一批已发现的 Steam 游戏逐个裁决。
///
/// 纯函数（只读文件系统，不写任何东西），所以自动测试可以拿临时目录里的假库跑，
/// **不碰真实 Steam**。
pub fn survey(apps: Vec<SteamApp>, snapshot: Option<&RhiSnapshot>) -> Vec<SurveyedGame> {
    apps.into_iter().map(|app| judge(app, snapshot)).collect()
}

/// 扫本机 Steam 库并逐个裁决。
pub fn survey_local(snapshot: Option<&RhiSnapshot>) -> Vec<SurveyedGame> {
    survey(crate::steam::all_apps(), snapshot)
}

/// 一款游戏的完整裁决流程。**顺序固定，见模块头注释。**
fn judge(app: SteamApp, snapshot: Option<&RhiSnapshot>) -> SurveyedGame {
    let id = GameId::from_app_id(app.app_id);

    // 第一步：反作弊闸门，**排在所有别的判断之前**。
    //
    // 顺序是被真机数据改过的：闸门原本排在身份检查后面，结果 CS2 和 PUBG
    // 的主程序埋在四层深处、身份检查先失败，界面上显示的是
    // 「不是一款装好的游戏」而不是「联机游戏不给装」——拦是拦住了，但理由说错了。
    // 闸门放最前面，一款联机游戏无论目录长什么样，得到的都是同一个正确答案。
    if let Some(found) = anticheat::blocking_reason(
        app.app_id,
        &app.game_root,
        anticheat::single_player_entry(app.app_id),
    ) {
        return SurveyedGame {
            app,
            api_source: ApiSource::None,
            eligibility: Eligibility::BlockedByAntiCheat(found),
        };
    }

    // 第二步：身份。两款已验证游戏用代码内的固定档案，其余找一个主程序当凭据。
    let verified = GameId::VERIFIED
        .contains(&id)
        .then(|| GameProfile::verified(id, &app.game_root))
        .flatten()
        .filter(|profile| app.game_root.join(&profile.exe).is_file());

    let exe = match &verified {
        Some(profile) => Some(profile.exe.clone()),
        None => main_exe(&app.game_root),
    };
    let Some(exe) = exe else {
        return SurveyedGame {
            app,
            api_source: ApiSource::None,
            eligibility: Eligibility::NotAGame,
        };
    };

    // 第三步：图形 API。已验证游戏用代码内的结论，不参与推断。
    let (api, api_source) = match &verified {
        Some(profile) => (profile.api, ApiSource::Manifest),
        None => graphics_api::resolve_detailed(&app.name, &app.game_root, snapshot),
    };
    if api == GraphicsApi::Unknown {
        return SurveyedGame {
            app,
            api_source,
            eligibility: Eligibility::UnknownApi,
        };
    }

    // 第四步：裁决。
    let profile = GameProfile {
        app_id: app.app_id,
        name: app.name.clone(),
        root: app.game_root.clone(),
        exe,
        api,
        verification: match verified {
            Some(_) => Verification::Verified,
            None => Verification::Untested,
        },
    };
    SurveyedGame {
        app,
        api_source,
        eligibility: Eligibility::Installable(profile),
    }
}

/// 挑一个能代表这款游戏的可执行文件，返回**相对游戏根目录**的路径。
///
/// 选最大的那个 exe：游戏主程序几乎总是目录里体积最大的可执行文件，
/// 启动器、崩溃上报、卸载器都小得多。同样大小时按路径排序，
/// **结果必须是确定的**——它会被写进 journal，下次启动还要按它找回来。
pub fn main_exe(game_root: &Path) -> Option<String> {
    let mut best: Option<(u64, PathBuf)> = None;
    let mut queue: VecDeque<(PathBuf, usize)> = VecDeque::new();
    queue.push_back((game_root.to_path_buf(), 0));
    let mut visited = 0usize;

    while let Some((dir, depth)) = queue.pop_front() {
        visited += 1;
        if visited > EXE_SCAN_MAX_DIRS {
            break;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_lowercase();
            if entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false) {
                if depth + 1 < EXE_SCAN_DEPTH && !skip_dir(&name) {
                    queue.push_back((entry.path(), depth + 1));
                }
                continue;
            }
            if !name.ends_with(".exe") {
                continue;
            }
            let size = entry.metadata().map(|meta| meta.len()).unwrap_or(0);
            let path = entry.path();
            let better = match &best {
                None => true,
                Some((best_size, best_path)) => {
                    size > *best_size || (size == *best_size && &path < best_path)
                }
            };
            if better {
                best = Some((size, path));
            }
        }
    }

    let (_, path) = best?;
    let relative = path.strip_prefix(game_root).ok()?;
    Some(relative.to_string_lossy().into_owned())
}

fn skip_dir(lower_name: &str) -> bool {
    lower_name.contains("redist") || SKIP_DIRS.contains(&lower_name)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DLSS_MANIFEST: &str = r#"{
        "dlss": [{"version": "310.8.0", "url": "https://example.com/a.zip"}],
        "dlssnr": [{"version": "310.8.SF-v2", "url": "https://example.com/b.zip"}]
    }"#;

    const GAME_MANIFEST: &str = r#"{
        "version": 2,
        "wikiNameOverrides": {
            "Grand Theft Auto V Enhanced": "Grand Theft Auto V : Enhanced",
            "Red Dead Redemption 2": "Red Dead Redemption 2 (Vulkan)"
        },
        "graphicsApiOverrides": {"Red Dead Redemption 2": "VLK"}
    }"#;

    fn snapshot() -> RhiSnapshot {
        RhiSnapshot::parse(GAME_MANIFEST, DLSS_MANIFEST, "t").unwrap()
    }

    /// 造一个假 Steam 库，返回 `(临时目录, 库根)`。
    fn library() -> (tempfile::TempDir, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("Steam");
        std::fs::create_dir_all(root.join("steamapps").join("common")).unwrap();
        std::fs::write(temp.path().join("steam.exe"), b"MZ").unwrap();
        (temp, root)
    }

    /// 往假库里放一款游戏：写清单 + 造目录 + 放指定文件。
    fn install(root: &Path, app_id: u32, name: &str, files: &[(&str, usize)]) -> SteamApp {
        let steamapps = root.join("steamapps");
        std::fs::write(
            steamapps.join(format!("appmanifest_{app_id}.acf")),
            format!(
                "\"AppState\"\n{{\n\t\"appid\"\t\t\"{app_id}\"\n\
                 \t\"name\"\t\t\"{name}\"\n\t\"installdir\"\t\t\"{name}\"\n}}"
            ),
        )
        .unwrap();
        let game_root = steamapps.join("common").join(name);
        std::fs::create_dir_all(&game_root).unwrap();
        for (relative, size) in files {
            let path = game_root.join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, vec![b'M'; *size]).unwrap();
        }
        SteamApp {
            app_id,
            name: name.to_string(),
            install_dir: name.to_string(),
            game_root,
            steam_exe: root.parent().unwrap().join("steam.exe"),
        }
    }

    fn judge_one(app: SteamApp) -> SurveyedGame {
        let snapshot = snapshot();
        survey(vec![app], Some(&snapshot)).pop().unwrap()
    }

    /// 干净的 DX12 单机游戏可以装，而且标成「未实测」。
    #[test]
    fn a_clean_dx12_game_is_installable_but_untested() {
        let (_temp, root) = library();
        let app = install(
            &root,
            1_091_500,
            "Cyberpunk 2077",
            &[("Cyberpunk2077.exe", 4096), ("D3D12Core.dll", 8)],
        );
        let surveyed = judge_one(app);
        let profile = surveyed.profile().expect("应该可以装");
        assert_eq!(profile.api, GraphicsApi::Dx12);
        assert_eq!(profile.verification, Verification::Untested);
        assert_eq!(profile.exe, "Cyberpunk2077.exe");
        assert_eq!(surveyed.api_source, ApiSource::LocalFiles);
        assert_eq!(surveyed.blocked_reason(), None);
    }

    /// 带 EasyAntiCheat 的游戏被拦下，**但仍然出现在列表里**，并且说得出为什么。
    #[test]
    fn an_eac_game_is_blocked_and_still_listed() {
        let (_temp, root) = library();
        let app = install(
            &root,
            1_222_670,
            "Some Shooter",
            &[
                ("Shooter.exe", 4096),
                ("D3D12Core.dll", 8),
                ("EasyAntiCheat/EasyAntiCheat_x64.dll", 8),
            ],
        );
        let surveyed = judge_one(app);
        assert!(matches!(
            surveyed.eligibility,
            Eligibility::BlockedByAntiCheat(AntiCheat::Present { .. })
        ));
        assert!(surveyed.profile().is_none());
        assert!(surveyed.blocked_reason().unwrap().contains("EasyAntiCheat"));
    }

    /// VAC 类游戏（目录里干干净净）靠代码内名单拦下。
    #[test]
    fn a_known_online_game_is_blocked_even_with_a_clean_directory() {
        let (_temp, root) = library();
        let app = install(&root, 730, "Counter-Strike 2", &[("cs2.exe", 4096)]);
        let surveyed = judge_one(app);
        assert!(matches!(
            surveyed.eligibility,
            Eligibility::BlockedByAntiCheat(AntiCheat::KnownOnline { .. })
        ));
    }

    /// 判不出 API 的游戏不提供安装。
    #[test]
    fn a_game_with_an_undetectable_api_is_not_installable() {
        let (_temp, root) = library();
        let app = install(&root, 400_000, "Mystery Game", &[("Mystery.exe", 4096)]);
        let surveyed = judge_one(app);
        assert_eq!(surveyed.eligibility, Eligibility::UnknownApi);
        assert!(surveyed.blocked_reason().unwrap().contains("闪退"));
    }

    /// **GTA5 是特例：带 BattlEye 也能装**，因为它有一条实测过的一次性单机入口。
    /// API 也走代码内的结论，不参与推断。
    #[test]
    fn gta5_is_installable_despite_battleye() {
        let (_temp, root) = library();
        let app = install(
            &root,
            3_240_220,
            "Grand Theft Auto V Enhanced",
            &[
                ("GTA5_Enhanced.exe", 4096),
                ("BattlEye/BEService_x64.exe", 8),
            ],
        );
        let surveyed = judge_one(app);
        let profile = surveyed.profile().expect("GTA5 必须可装");
        assert_eq!(profile.api, GraphicsApi::Dx12);
        assert_eq!(profile.verification, Verification::Verified);
        assert_eq!(profile.exe, "GTA5_Enhanced.exe");
    }

    /// RDR2 走代码内的 Vulkan 结论。
    #[test]
    fn rdr2_keeps_its_verified_vulkan_profile() {
        let (_temp, root) = library();
        let app = install(
            &root,
            1_174_180,
            "Red Dead Redemption 2",
            &[("RDR2.exe", 4096)],
        );
        let profile = judge_one(app).eligibility.profile().cloned().unwrap();
        assert_eq!(profile.api, GraphicsApi::Vulkan);
        assert_eq!(profile.verification, Verification::Verified);
    }

    /// 已验证游戏的 exe 不在了 → 退回普通游戏那条路（不能拿一个空壳目录冒充它）。
    #[test]
    fn a_verified_game_without_its_exe_is_not_treated_as_verified() {
        let (_temp, root) = library();
        let app = install(
            &root,
            3_240_220,
            "Grand Theft Auto V Enhanced",
            &[("Launcher.exe", 16)],
        );
        let surveyed = judge_one(app);
        // BattlEye 不在了，但也没有已验证档案；API 判不出 → 不给装。
        assert_eq!(surveyed.eligibility, Eligibility::UnknownApi);
    }

    /// 目录里一个 exe 都没有：这不像一款装好的游戏。
    #[test]
    fn a_directory_without_any_exe_is_not_a_game() {
        let (_temp, root) = library();
        let app = install(&root, 500_000, "Just Data", &[("data.pak", 32)]);
        assert_eq!(judge_one(app).eligibility, Eligibility::NotAGame);
    }

    /// 主程序取最大的那个 exe，而且结果必须确定——它会被写进 journal。
    #[test]
    fn the_main_exe_is_the_largest_one_and_the_pick_is_stable() {
        let (_temp, root) = library();
        let app = install(
            &root,
            600_000,
            "Deep Game",
            &[
                ("Launcher.exe", 16),
                ("bin/DeepGame.exe", 4096),
                ("_CommonRedist/vcredist_x64.exe", 999_999),
            ],
        );
        let picked = main_exe(&app.game_root).unwrap();
        assert_eq!(
            picked,
            PathBuf::from("bin").join("DeepGame.exe").to_string_lossy()
        );
        // 跑两次结果一样。
        assert_eq!(main_exe(&app.game_root).unwrap(), picked);
    }

    /// 一次裁决一整个库：可装的、被拦的、判不出的各归各位。
    #[test]
    fn a_whole_library_is_surveyed_at_once() {
        let (_temp, root) = library();
        let apps = vec![
            install(
                &root,
                1_091_500,
                "Cyberpunk 2077",
                &[("Cyberpunk2077.exe", 4096), ("D3D12Core.dll", 8)],
            ),
            install(&root, 730, "Counter-Strike 2", &[("cs2.exe", 4096)]),
            install(&root, 400_000, "Mystery Game", &[("Mystery.exe", 4096)]),
        ];
        let snapshot = snapshot();
        let surveyed = survey(apps, Some(&snapshot));
        assert_eq!(surveyed.len(), 3);
        assert!(surveyed[0].eligibility.is_installable());
        assert!(!surveyed[1].eligibility.is_installable());
        assert!(!surveyed[2].eligibility.is_installable());
        // 不给装的也都在列表里，而且每个都说得出为什么。
        for game in &surveyed[1..] {
            assert!(game.blocked_reason().is_some(), "{}", game.name());
        }
    }

    /// **闸门排在身份检查之前。**
    ///
    /// CS2 的主程序埋在 `game\bin\win64\` 四层深处（2026-08-31 本机实测）。
    /// 闸门要是排在身份检查后面，界面上会显示「不是一款装好的游戏」——
    /// 拦是拦住了，理由却说错了。
    #[test]
    fn a_blocked_game_reports_the_anti_cheat_even_when_its_exe_is_buried() {
        let (_temp, root) = library();
        let app = install(
            &root,
            730,
            "Counter-Strike Global Offensive",
            &[("game/bin/win64/cs2.exe", 4096)],
        );
        assert!(matches!(
            judge_one(app).eligibility,
            Eligibility::BlockedByAntiCheat(AntiCheat::KnownOnline { .. })
        ));
    }

    /// 主程序埋在第四层也要找得到（PUBG 的 `TslGame\Binaries\Win64\`）。
    #[test]
    fn a_main_exe_four_levels_down_is_found() {
        let (_temp, root) = library();
        let app = install(
            &root,
            700_000,
            "Deep UE Game",
            &[("DeepGame/Binaries/Win64/DeepGame.exe", 4096)],
        );
        let picked = main_exe(&app.game_root).unwrap();
        assert!(picked.ends_with("DeepGame.exe"), "{picked}");
    }

    /// Ricochet 的内核驱动也算反作弊痕迹（CoD 既没有 EAC 也没有 BattlEye）。
    #[test]
    fn the_ricochet_driver_is_detected() {
        let (_temp, root) = library();
        let app = install(
            &root,
            999_999,
            "Some Shooter",
            &[("Shooter.exe", 4096), ("Randgrid.sys", 8)],
        );
        let surveyed = judge_one(app);
        assert!(
            matches!(
                &surveyed.eligibility,
                Eligibility::BlockedByAntiCheat(AntiCheat::Present {
                    kind: "Ricochet",
                    ..
                })
            ),
            "{:?}",
            surveyed.eligibility
        );
    }

    /// **闸门排在 API 判定之前**：一款 API 判得清清楚楚的联机游戏照样不给装。
    #[test]
    fn the_anti_cheat_gate_runs_before_the_api_check() {
        let (_temp, root) = library();
        let app = install(
            &root,
            730,
            "Counter-Strike 2",
            &[("cs2.exe", 4096), ("D3D12Core.dll", 8)],
        );
        let surveyed = judge_one(app);
        assert!(matches!(
            surveyed.eligibility,
            Eligibility::BlockedByAntiCheat(_)
        ));
    }
}
