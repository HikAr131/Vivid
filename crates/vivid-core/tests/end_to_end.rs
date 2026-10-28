//! 无真实写入的端到端流程：安装 → 更新 → 冲突 → 还原，以及中途失败与恢复。
//!
//! **这套测试一个字节都不碰真实游戏目录、真实 `C:\ProgramData` 和真实 HKLM。**
//! 游戏目录是临时目录里造的假 Steam 库，全局 layer 走假后端的虚拟 ProgramData
//! 映射，注册表只写内存 map，网络全部是排队好的假响应。
//!
//! 夹具复用 `vivid_core::testing` 里那三个基础假实现，不另建语义不同的第二套——
//! 两套假实现迟早会在某个细节上不一致，那时测试通过就不再说明任何事情。

use std::path::{Path, PathBuf};

use vivid_core::artifact::sha256_hex;
use vivid_core::deployment::{Deployment, ManagedTarget, OriginalState, recover_pending};
use vivid_core::evidence::{GameState, PrimaryAction};
use vivid_core::manager::{ManagerError, PreparedInstall, VividManager};
use vivid_core::paths::AppPaths;
use vivid_core::recipe::{GameId, GameProfile};
use vivid_core::rhi_manifest::{DLSS_MANIFEST_URL, GAME_MANIFEST_URL};
use vivid_core::steam::SteamApp;
use vivid_core::testing::{
    FakeEnvironment, FakeHttp, FakeSystemBackend, fake_payload, rtx_4070_ti_super, zip_with_files,
};
use vivid_core::upstream::{RENODX_RELEASES_URL, RESHADE_HOME_URL};

const GAME_MANIFEST: &str = include_str!("fixtures/rhi-manifest-min.json");
const DLSS_MANIFEST: &str = include_str!("fixtures/rhi-dlss-manifest-min.json");
const RELEASES_JSON: &str = include_str!("fixtures/rhi-releases-min.json");
const RESHADE_HTML: &str = include_str!("fixtures/reshade-download-min.html");

const RESHADE_URL: &str = "https://reshade.me/downloads/ReShade_Setup_6.8.1_Addon.exe";
const RENODX_URL: &str =
    "https://github.com/RankFTW/rhi-repo/releases/download/renodx-dlss5-4.60/renodx-dlss5_4.60.zip";
const DLSSNR_URL: &str = "https://github.com/RankFTW/rhi-repo/releases/download/dlssnr-310.8.SF-v2/nvngx_dlssnr_310.8.SF-v2.zip";
const DLSS_URL: &str =
    "https://github.com/RankFTW/rhi-repo/releases/download/dlss-310.8.0/nvngx_dlss_310.8.0.zip";

/// 虚拟的系统目录前缀。计划里的目标仍写真实路径，只有假后端把它映射到临时目录。
const PROGRAMDATA: &str = r"C:\ProgramData";

struct EndToEndFixture {
    _temp: tempfile::TempDir,
    manager: VividManager<FakeHttp, FakeEnvironment>,
    backend: FakeSystemBackend,
    game_id: GameId,
    game_root: PathBuf,
    programdata: PathBuf,
}

impl EndToEndFixture {
    fn gta() -> Self {
        Self::new(GameId::Gta5Enhanced)
    }

    fn rdr2() -> Self {
        Self::new(GameId::RedDeadRedemption2)
    }

    fn new(game_id: GameId) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().to_path_buf();
        let game_root = root
            .join("SteamLibrary")
            .join("steamapps")
            .join("common")
            .join(game_id.label());
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
        backend.map_programdata(Path::new(PROGRAMDATA), &programdata);

        let http = FakeHttp::new();
        push_manifests(&http, DLSS_MANIFEST);
        push_artifacts(&http);

        Self {
            manager: VividManager::new(
                AppPaths::under(root.join("Vivid")),
                http,
                FakeEnvironment::new(Some(rtx_4070_ti_super())).with_game(game_id, app),
            ),
            backend,
            game_id,
            game_root,
            programdata,
            _temp: temp,
        }
    }

    /// 游戏目录里本来就有的东西（Vivid 接管之前的原始文件）。
    fn seed_originals(&self, files: &[(&str, &[u8])]) {
        for (name, bytes) in files {
            std::fs::write(self.game_root.join(name), bytes).unwrap();
        }
    }

    fn prepare(&self) -> Result<PreparedInstall, ManagerError> {
        self.manager.prepare(self.game_id, &self.backend)
    }

    fn apply(&mut self, prepared: PreparedInstall) -> Result<(), ManagerError> {
        self.manager.apply(prepared, &mut self.backend).map(|_| ())
    }

    fn install(&mut self) -> Result<(), ManagerError> {
        let prepared = self.prepare()?;
        self.apply(prepared)
    }

    fn update(&mut self) -> Result<(), ManagerError> {
        self.manager
            .update(self.game_id, &mut self.backend)
            .map(|_| ())
    }

    fn restore(&mut self) -> Result<(), ManagerError> {
        self.manager.restore(self.game_id, &mut self.backend)
    }

    fn inspect(&self) -> GameState {
        self.manager
            .inspect(self.game_id, &self.backend)
            .unwrap()
            .state
    }

    fn deployment(&self) -> Deployment {
        Deployment::new(
            &self.manager.paths,
            &GameProfile::verified(self.game_id, &self.game_root).unwrap(),
        )
    }

    /// baseline 里存的那份「Vivid 接管之前」的内容。
    fn original_baseline(&self, name: &str) -> Vec<u8> {
        let target = ManagedTarget::File(self.game_root.join(name));
        match self.deployment().baseline_state(&target).unwrap().unwrap() {
            OriginalState::File { backup, .. } => std::fs::read(backup).unwrap(),
            other => panic!("baseline 不是文件：{other:?}"),
        }
    }

    fn game_file(&self, name: &str) -> Vec<u8> {
        std::fs::read(self.game_root.join(name)).unwrap()
    }

    fn game_file_exists(&self, name: &str) -> bool {
        self.game_root.join(name).is_file()
    }

    /// 模拟别的程序（Steam 校验、另一个 mod）改掉了我们管的文件。
    fn external_write(&self, name: &str, bytes: &[u8]) {
        std::fs::write(self.game_root.join(name), bytes).unwrap();
    }

    /// 把冲突文件改回 Vivid 装进去的内容，让还原可以继续。
    fn restore_managed_hash(&self, name: &str, bytes: &[u8]) {
        std::fs::write(self.game_root.join(name), bytes).unwrap();
    }

    /// 上游发布了新的 DLSS-NR：刷新清单缓存，并备好这一版的下载响应。
    fn publish_new_dlssnr(&self, version: &str, bytes: &[u8]) {
        let url = format!(
            "https://github.com/RankFTW/rhi-repo/releases/download/dlssnr-{version}/nvngx_dlssnr_{version}.zip"
        );
        let manifest = format!(
            r#"{{"dlss":[{{"version":"310.8.0","url":"{DLSS_URL}"}}],
                 "dlssnr":[{{"version":"{version}","url":"{url}"}}]}}"#
        );
        push_manifests(&self.manager.http, &manifest);
        self.manager.http.push(
            &url,
            Ok(fake_payload(
                &url,
                zip_with_files([("nvngx_dlssnr.dll", bytes)]),
            )),
        );
        // 刷新一次，让 last-known-good 变成新清单（inspect 只看缓存）。
        self.manager.refresh_snapshot().unwrap();
        self.manager.refresh_upstream().unwrap();
    }

    /// 全局 layer 在虚拟 ProgramData 里的落点。
    fn layer_file(&self, name: &str) -> PathBuf {
        self.programdata.join("ReShade").join(name)
    }
}

fn push_manifests(http: &FakeHttp, dlss_manifest: &str) {
    for (url, body) in [
        (GAME_MANIFEST_URL, GAME_MANIFEST),
        (DLSS_MANIFEST_URL, dlss_manifest),
        (RENODX_RELEASES_URL, RELEASES_JSON),
        (RESHADE_HOME_URL, RESHADE_HTML),
    ] {
        http.push(url, Ok(fake_payload(url, body.as_bytes().to_vec())));
    }
}

fn push_artifacts(http: &FakeHttp) {
    let mut setup = b"MZ-setup-stub-".to_vec();
    setup.extend_from_slice(&zip_with_files([(
        "ReShade64.dll",
        b"MZ-reshade".as_slice(),
    )]));
    http.push(RESHADE_URL, Ok(fake_payload(RESHADE_URL, setup)));
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

#[test]
fn gta_install_update_conflict_and_restore_round_trip() {
    let mut fixture = EndToEndFixture::gta();
    println!("沙箱游戏目录：{}", fixture.game_root.display());
    fixture.seed_originals(&[("nvngx_dlss.dll", b"original-dlss")]);

    // ---- 安装 ----
    fixture.install().unwrap();
    assert_eq!(fixture.inspect(), GameState::Installed);
    assert_eq!(
        fixture.original_baseline("nvngx_dlss.dll"),
        b"original-dlss"
    );
    assert_eq!(fixture.game_file("nvngx_dlss.dll"), b"MZ-sr");
    assert!(fixture.game_file_exists("renodx-dlss5.addon64"));

    // ---- 上游出新版：只提示，不自动替换 ----
    fixture.publish_new_dlssnr("310.9.SF", b"MZ-new-nr");
    assert!(matches!(
        fixture.inspect(),
        GameState::UpdateAvailable { .. }
    ));
    assert_eq!(
        fixture.game_file("nvngx_dlssnr.dll"),
        b"MZ-nr",
        "没点更新之前，游戏里的文件不许被换掉"
    );

    // ---- 用户点更新：baseline 仍是最原始那份 ----
    fixture.update().unwrap();
    assert_eq!(fixture.game_file("nvngx_dlssnr.dll"), b"MZ-new-nr");
    assert_eq!(
        fixture.original_baseline("nvngx_dlss.dll"),
        b"original-dlss"
    );
    assert_eq!(fixture.inspect(), GameState::Installed);

    // ---- 别的程序动了我们管的文件：还原必须停下来 ----
    fixture.external_write("nvngx_dlss.dll", b"steam-or-another-mod");
    assert!(matches!(fixture.inspect(), GameState::Conflict { .. }));
    assert!(fixture.restore().is_err());
    assert_eq!(
        fixture.game_file("nvngx_dlss.dll"),
        b"steam-or-another-mod",
        "冲突时不许覆盖别人的改动"
    );

    // ---- 冲突解除之后还原：回到 Vivid 接管之前 ----
    fixture.restore_managed_hash("nvngx_dlss.dll", b"MZ-sr");
    fixture.restore().unwrap();
    assert_eq!(fixture.game_file("nvngx_dlss.dll"), b"original-dlss");
    assert!(!fixture.game_file_exists("renodx-dlss5.addon64"));
    assert!(!fixture.game_file_exists("dxgi.dll"));
    assert!(!fixture.game_file_exists("ReShade.ini"));
    assert!(!fixture.game_file_exists("nvngx_dlssnr.dll"));

    // 还原之后 Vivid 自己的数据目录也要收干净，不留空文件夹。
    for dir in ["installations", "backups", "pending"] {
        let path = fixture.manager.paths.root().join(dir);
        let left: Vec<_> = std::fs::read_dir(&path)
            .map(|entries| entries.flatten().map(|e| e.path()).collect())
            .unwrap_or_default();
        assert!(left.is_empty(), "{} 下还剩 {:?}", dir, left);
    }
}

#[test]
fn rdr2_global_layer_and_registry_are_restored_exactly() {
    let mut fixture = EndToEndFixture::rdr2();
    println!("沙箱 ProgramData：{}", fixture.programdata.display());

    let location = vivid_core::vulkan_layer::registry_location();
    let value_name = vivid_core::vulkan_layer::layer_json_path()
        .display()
        .to_string();
    assert_eq!(
        fixture.backend.registry_value(&location, &value_name),
        None,
        "装之前注册表里不该有这个值"
    );

    fixture.install().unwrap();
    assert_eq!(fixture.inspect(), GameState::Installed);
    assert!(fixture.layer_file("ReShade64.dll").is_file());
    assert!(fixture.layer_file("ReShade64.json").is_file());
    assert_eq!(
        fixture.backend.registry_value(&location, &value_name),
        Some(0)
    );
    assert!(fixture.game_file_exists("reshade.ini"));
    // 「没碰真实系统」不能靠「真实路径下没有这个文件」来证明——那个文件完全可能
    // 因为无关的原因存在（比如这台机器上真的用 Vivid 装过一次，2026-08-31 就撞到了）。
    // 真正的保证在假后端里：它对沙箱之外的任何读写直接 panic，
    // 所以只要这套测试跑得通，就说明一次都没越界。这里只断言内容确实落在沙箱里。
    assert!(
        fixture
            .layer_file("ReShade64.dll")
            .starts_with(&fixture.programdata)
    );

    fixture.restore().unwrap();
    assert!(!fixture.layer_file("ReShade64.dll").is_file());
    assert!(!fixture.layer_file("ReShade64.json").is_file());
    // Vivid 自己建的那个目录也要收走，别在 ProgramData 下留空壳。
    assert!(
        !fixture.programdata.join("ReShade").is_dir(),
        "还原后不该留下空的 ReShade 目录"
    );
    assert_eq!(
        fixture.backend.registry_value(&location, &value_name),
        None,
        "注册表值必须回到「原本不存在」"
    );
    assert!(!fixture.game_file_exists("reshade.ini"));
    assert!(!fixture.game_file_exists("renodx-dlss5.addon64"));
}

/// ReShade 装完就把配置写成自己的格式（补默认值、存窗口布局）。
/// 那是它的正常行为，**不能因此把还原卡死**——2026-08-31 GTA5 真机验收撞到过：
/// 用户开过一次 ReShade 面板，一键还原就被判成冲突拒绝了。
#[test]
fn a_config_rewritten_by_reshade_never_blocks_restore() {
    let mut fixture = EndToEndFixture::gta();
    fixture.seed_originals(&[("nvngx_dlss.dll", b"original-dlss")]);
    fixture.install().unwrap();

    // ReShade 自己改写配置
    fixture.external_write(
        "ReShade.ini",
        b"[GENERAL]
ReShade wrote its own full default set here
",
    );

    assert_eq!(
        fixture.inspect(),
        GameState::Installed,
        "组件自己改配置不是冲突"
    );
    fixture.restore().unwrap();
    assert_eq!(fixture.game_file("nvngx_dlss.dll"), b"original-dlss");
    assert!(!fixture.game_file_exists("ReShade.ini"), "还原要删掉它");

    // 但普通目标被改动仍然是冲突。
    let mut other = EndToEndFixture::gta();
    other.seed_originals(&[("nvngx_dlss.dll", b"original-dlss")]);
    other.install().unwrap();
    other.external_write("renodx-dlss5.addon64", b"changed-by-another-tool");
    assert!(matches!(other.inspect(), GameState::Conflict { .. }));
    assert!(other.restore().is_err());
}

#[test]
fn a_failed_write_leaves_no_partial_install() {
    let mut fixture = EndToEndFixture::gta();
    fixture.seed_originals(&[
        ("dxgi.dll", b"original-dxgi"),
        ("ReShade.ini", b"original-ini"),
    ]);
    fixture.backend.fail_on_applies([2]);

    let err = fixture.install().unwrap_err();
    assert!(
        matches!(err, ManagerError::Deploy(ref e) if e.to_string().contains("写入")),
        "{err:?}"
    );
    assert_eq!(fixture.game_file("dxgi.dll"), b"original-dxgi");
    assert_eq!(fixture.game_file("ReShade.ini"), b"original-ini");
    assert!(!fixture.game_file_exists("nvngx_dlss.dll"));
    assert!(!fixture.deployment().ledger_path().exists());
    assert!(!fixture.deployment().journal_path().exists());
    assert_eq!(fixture.inspect(), GameState::ReadyToInstall);
}

#[test]
fn an_interrupted_rollback_is_finished_on_the_next_start() {
    let mut fixture = EndToEndFixture::gta();
    fixture.seed_originals(&[
        ("dxgi.dll", b"original-dxgi"),
        ("ReShade.ini", b"original-ini"),
    ]);
    // 第 2 次写入失败触发回滚，回滚的第 1 次写入（第 3 次）也失败 → journal 留下。
    fixture.backend.fail_on_applies([2, 3]);

    assert!(fixture.install().is_err());
    let journal = fixture.deployment().journal_path();
    assert!(journal.exists(), "回滚失败必须保留待恢复记录");
    assert_eq!(
        fixture.game_file("dxgi.dll"),
        b"MZ-reshade",
        "第一步已经写进去了"
    );

    let recovered = recover_pending(&fixture.manager.paths, &mut fixture.backend).unwrap();
    assert_eq!(recovered, vec![GameId::Gta5Enhanced]);
    assert_eq!(fixture.game_file("dxgi.dll"), b"original-dxgi");
    assert_eq!(fixture.game_file("ReShade.ini"), b"original-ini");
    assert!(!journal.exists());
    assert!(!fixture.deployment().ledger_path().exists());
}

/// 所有落盘位置都必须在临时目录里。这条测试是这套沙箱的自检：
/// 一旦某个目标漏到真实系统上，它会先炸。
#[test]
fn every_planned_target_stays_inside_the_sandbox() {
    for fixture in [EndToEndFixture::gta(), EndToEndFixture::rdr2()] {
        let prepared = fixture.prepare().unwrap();
        let temp_root = fixture.game_root.ancestors().nth(4).unwrap();
        for action in &prepared.plan.actions {
            match action.target() {
                ManagedTarget::File(path) => {
                    let inside_sandbox = path.starts_with(temp_root);
                    let global_layer = path.starts_with(PROGRAMDATA);
                    assert!(
                        inside_sandbox || global_layer,
                        "目标跑到沙箱外面了：{}",
                        path.display()
                    );
                    if global_layer {
                        // 全局 layer 的目标是真实路径，但读写都被假后端映射走了。
                        println!("全局目标（映射到临时目录）：{}", path.display());
                    }
                }
                ManagedTarget::RegistryDword { .. } => {}
            }
        }
        // 组件缓存也在沙箱里。
        assert!(fixture.manager.paths.artifacts_dir().starts_with(temp_root));
        for artifact in &prepared.artifacts {
            assert_eq!(
                artifact.sha256,
                sha256_hex(&std::fs::read(&artifact.path).unwrap())
            );
        }
    }
}

// ---------------------------------------------------------------------------
// 多游戏：发现 → 裁决 → 预演 → 安装 → 体检 → 还原
// ---------------------------------------------------------------------------

/// 一个装了好几款游戏的假 Steam 库。
///
/// **全程不碰真实 Steam、真实游戏目录、真实注册表**：库在临时目录里造，
/// 每款游戏放哪些文件由参数决定，图形 API 和裁决结果因此完全由这些文件说了算。
struct LibraryFixture {
    _temp: tempfile::TempDir,
    manager: VividManager<FakeHttp, FakeEnvironment>,
    backend: FakeSystemBackend,
    roots: std::collections::BTreeMap<u32, PathBuf>,
}

impl LibraryFixture {
    fn new(games: &[(u32, &str, &[&str])]) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().to_path_buf();
        let common = root.join("SteamLibrary").join("steamapps").join("common");
        let steam_exe = root.join("steam.exe");
        std::fs::write(&steam_exe, b"MZ").unwrap();

        let mut environment = FakeEnvironment::new(Some(rtx_4070_ti_super()));
        let mut roots = std::collections::BTreeMap::new();
        for (app_id, name, files) in games {
            let game_root = common.join(name);
            std::fs::create_dir_all(&game_root).unwrap();
            for relative in *files {
                let path = game_root.join(relative);
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(&path, b"MZ").unwrap();
            }
            environment = environment.with_game(
                GameId::from_app_id(*app_id),
                SteamApp {
                    app_id: *app_id,
                    name: (*name).to_string(),
                    install_dir: (*name).to_string(),
                    game_root: game_root.clone(),
                    steam_exe: steam_exe.clone(),
                },
            );
            roots.insert(*app_id, game_root);
        }

        let mut backend = FakeSystemBackend::new(&root);
        backend.map_programdata(Path::new(PROGRAMDATA), &root.join("programdata"));
        let http = FakeHttp::new();
        push_manifests(&http, DLSS_MANIFEST);
        push_artifacts(&http);

        let fixture = Self {
            manager: VividManager::new(AppPaths::under(root.join("Vivid")), http, environment),
            backend,
            roots,
            _temp: temp,
        };
        fixture.manager.refresh_snapshot().unwrap();
        fixture.manager.refresh_upstream().unwrap();
        fixture
    }

    fn id(&self, app_id: u32) -> GameId {
        GameId::from_app_id(app_id)
    }

    fn state(&self, app_id: u32) -> GameState {
        self.manager
            .inspect(self.id(app_id), &self.backend)
            .unwrap()
            .state
    }

    fn primary_action(&self, app_id: u32) -> PrimaryAction {
        self.manager
            .inspect(self.id(app_id), &self.backend)
            .unwrap()
            .primary_action()
    }

    /// 一款游戏目录的完整指纹（相对路径 + 内容哈希），用来断言「回到基线」。
    fn fingerprint(&self, app_id: u32) -> Vec<String> {
        let root = &self.roots[&app_id];
        let mut out: Vec<String> = walkdir::WalkDir::new(root)
            .into_iter()
            .flatten()
            .filter(|entry| entry.file_type().is_file())
            .map(|entry| {
                let bytes = std::fs::read(entry.path()).unwrap_or_default();
                format!(
                    "{}={}",
                    entry.path().strip_prefix(root).unwrap().display(),
                    sha256_hex(&bytes)
                )
            })
            .collect();
        out.sort();
        out
    }
}

/// **整库走一遍**：发现 → 裁决 → 预演 → 安装 → 体检 → 还原。
///
/// 覆盖计划要求的三类游戏：一款未实测的 D3D12 单机游戏、一款带 EAC 被拦的、
/// 一款判不出图形 API 的。三类都要出现在列表里，但只有第一类能装。
#[test]
fn a_mixed_library_is_surveyed_installed_and_restored() {
    const CLEAN: u32 = 1_091_500;
    const WITH_EAC: u32 = 1_222_670;
    const MYSTERY: u32 = 400_000;

    let mut fixture = LibraryFixture::new(&[
        (CLEAN, "Clean DX12 Game", &["Clean.exe", "D3D12Core.dll"]),
        (
            WITH_EAC,
            "Shooter",
            &[
                "Shooter.exe",
                "D3D12Core.dll",
                "EasyAntiCheat/EasyAntiCheat_x64.dll",
            ],
        ),
        (MYSTERY, "Mystery Game", &["Mystery.exe"]),
    ]);

    // ---- 发现：三款游戏都在列表里，不给装的也在 ----
    assert_eq!(
        fixture.manager.inspect_library().len(),
        3,
        "不给装的游戏也必须出现在列表里"
    );

    // ---- 裁决 ----
    assert_eq!(fixture.state(CLEAN), GameState::ReadyToInstall);
    assert!(matches!(
        fixture.state(WITH_EAC),
        GameState::BlockedByAntiCheat { .. }
    ));
    assert_eq!(fixture.state(MYSTERY), GameState::UnknownApi);

    // 不给装的两款：连预演都进不去，主按钮也是空的。
    for app_id in [WITH_EAC, MYSTERY] {
        let before = fixture.fingerprint(app_id);
        assert!(
            fixture
                .manager
                .prepare(fixture.id(app_id), &fixture.backend)
                .is_err()
        );
        assert_eq!(
            fixture.fingerprint(app_id),
            before,
            "被拦下的游戏目录一个字节都不许动"
        );
        assert_eq!(fixture.primary_action(app_id), PrimaryAction::None);
    }

    // ---- 预演：注入 DLL 跟着 API 走，目标全在这款游戏自己的目录里 ----
    let baseline = fixture.fingerprint(CLEAN);
    let blocked_before: Vec<Vec<String>> = [WITH_EAC, MYSTERY]
        .iter()
        .map(|id| fixture.fingerprint(*id))
        .collect();
    let clean = fixture.id(CLEAN);
    let clean_root = fixture.roots[&CLEAN].clone();
    let prepared = fixture.manager.prepare(clean, &fixture.backend).unwrap();
    let rows = prepared.planned_targets();
    assert_eq!(rows.len(), 5);
    assert!(rows[0].target.ends_with("dxgi.dll"), "{}", rows[0].target);
    for row in &rows {
        assert!(
            Path::new(&row.target).starts_with(&clean_root),
            "目标越界：{}",
            row.target
        );
    }
    // 未实测的游戏必须有话说。
    assert!(prepared.known_caveats().join("").contains("没有这款游戏"));

    // ---- 安装 ----
    fixture
        .manager
        .apply(prepared, &mut fixture.backend)
        .unwrap();
    assert_eq!(fixture.state(CLEAN), GameState::Installed);
    for name in [
        "dxgi.dll",
        "ReShade.ini",
        "renodx-dlss5.addon64",
        "nvngx_dlssnr.dll",
        "nvngx_dlss.dll",
    ] {
        assert!(clean_root.join(name).is_file(), "{name} 没装上");
    }
    // 别的两款游戏在整个安装过程里一个字节都没被碰过。
    for (index, app_id) in [WITH_EAC, MYSTERY].iter().enumerate() {
        assert_eq!(
            fixture.fingerprint(*app_id),
            blocked_before[index],
            "装 A 游戏不许动到 B 游戏"
        );
    }

    // ---- 还原：逐个文件回到基线 ----
    fixture
        .manager
        .restore(clean, &mut fixture.backend)
        .unwrap();
    assert_eq!(
        fixture.fingerprint(CLEAN),
        baseline,
        "还原之后必须和安装前一模一样"
    );
    assert_eq!(fixture.state(CLEAN), GameState::ReadyToInstall);
    assert!(
        !fixture
            .manager
            .paths
            .installations_dir()
            .join(format!("{}.json", clean.slug()))
            .exists(),
        "还原成功之后 ledger 必须清掉"
    );
}

/// Steam 库里的运行库包不是游戏：说得出为什么，而且**不说成「没找到」**。
#[test]
fn a_directory_without_an_exe_is_reported_as_not_a_game() {
    let fixture = LibraryFixture::new(&[(228_980, "Steamworks Shared", &["readme.txt"])]);
    assert_eq!(fixture.state(228_980), GameState::NotAGame);
    assert_eq!(fixture.primary_action(228_980), PrimaryAction::None);
}
