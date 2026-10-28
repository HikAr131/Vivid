//! 任务型中文单页：左边是可搜索的整库游戏列表，右边一个结论 + 一个主按钮。
//!
//! **动这个文件前先读 `docs/ui-style-guide.md`。** 三条最容易违反的：
//! 禁止用 `egui::Grid` 排内容列表、高饱和品牌色一屏只出现一处填充、
//! 常驻说明文字不用警告色。
//!
//! 版式判据（CLAUDE.md 第三节）：**一个不懂技术的玩家能不能三步之内完成
//! 「换新 DLSS 并确认生效」，出问题能不能一键回到原状。** 所以这里只有：
//!
//! 1. 左栏选游戏；2. 右栏看一句结论；3. 点那个唯一的主按钮。
//!
//! 左栏按裁决结果分三组：`可安装` / `未实测` / `不提供安装`。
//! **不给装的游戏照样列出来**——用户要知道 Vivid 看见了它，以及为什么不给装；
//! 悄悄从列表里抹掉只会让人以为是没扫到。那一组没有主按钮，只有一句为什么。
//!
//! 其余全部收进折叠详情——来源 URL、SHA-256、目标路径、备份位置都在那里，
//! 想核对的人找得到，不想看的人不会被它们淹没。
//!
//! GUI **不下载、不改文件、不碰注册表**：所有动作都走 `vivid_core::manager`，
//! 需要管理员权限的那几步走 `elevation` 的固定 helper。

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

use eframe::egui;
use vivid_core::evidence::{GameState, PrimaryAction};
use vivid_core::manager::{
    GameInspection, PreparedInstall, PreviewChange, VividManager, WindowsEnvironment,
};
use vivid_core::net::UreqHttpClient;
use vivid_core::paths::AppPaths;
use vivid_core::recipe::{GameId, Verification};
use vivid_core::rhi_manifest::SnapshotOrigin;
use vivid_core::steam::SteamApp;
use vivid_core::update;

use crate::elevation::{self, ElevationAction};
use crate::jobs::{JobEvent, JobKind, Jobs};
use crate::ui::{self, Tone};
use crate::{SLOGAN, tracked_text};

/// 等提权进程干完活的上限。装 4 个组件 + 一次 UAC，5 分钟足够；
/// 超时只说「等不到结果」，不假装成功。
const ELEVATION_TIMEOUT: Duration = Duration::from_secs(300);

type Manager = VividManager<UreqHttpClient, WindowsEnvironment>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Nav {
    Game(GameId),
    About,
}

/// 后台整库扫描的结果。**一次性送回来**，界面不做增量拼装——
/// 半张列表比没有列表更容易让人误判。
struct ScanOutcome {
    games: Vec<(SteamApp, GameInspection)>,
    /// 在线清单不可用之类的一句话提示。
    note: Option<String>,
}

pub struct Home {
    manager: Arc<Manager>,
    nav: Nav,
    /// 本机 Steam 库里的每一款游戏和对它的体检结果。**包括不给装的那些。**
    games: Vec<(SteamApp, GameInspection)>,
    /// 左栏的搜索词。库大了之后没有搜索就找不到东西。
    search: String,
    scan_rx: Option<Receiver<ScanOutcome>>,
    scanning: bool,
    /// 首次启动说明是否正在显示。
    welcome: bool,
    inspections: BTreeMap<GameId, GameInspection>,
    jobs: Jobs,
    tx: Sender<JobEvent>,
    rx: Receiver<JobEvent>,
    started: bool,
    details_open: bool,
    /// 已经生成好、正在等用户确认的安装计划。**确认之前不写任何东西。**
    preview: Option<PreviewPrompt>,
    /// 启动时收拾中断操作的结果，只说一次。
    recovered: Option<String>,
    /// 清单来源提示（在线不可用时给一句话）。
    manifest_note: Option<String>,
    /// 数据目录都定位不了时的兜底：只显示错误，不做任何写入。
    fatal: Option<String>,

    /// 版式自检：把渲染出来的那一帧存成 PNG 再退出。**只在 debug 构建里会有值。**
    shot: Option<crate::ScreenshotRequest>,
    shot_frames: u32,

    // ---- Vivid 自身的更新检查，和组件更新分开显示 ----
    update: Option<update::Available>,
    update_rx: Option<Receiver<Result<Option<update::Available>, update::UpdateError>>>,
    update_checked: bool,
    update_manual: bool,
    update_error: Option<String>,
    update_prompt: crate::update_prompt::UpdatePrompt,
    about: crate::about::About,
}

impl Home {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        let (paths, fatal) = match AppPaths::detect() {
            Ok(paths) => (paths, None),
            Err(e) => (
                // 定位不了数据目录时给一个绝不会被使用的占位路径：
                // `fatal` 一旦有值，界面只显示错误，不会发起任何操作。
                AppPaths::under(std::env::temp_dir().join("Vivid-unavailable")),
                Some(e.to_string()),
            ),
        };
        let receipt_dir = fatal.is_none().then(|| paths.root().join("update-prompts"));
        Self {
            manager: Arc::new(VividManager::new(
                paths,
                UreqHttpClient::default(),
                WindowsEnvironment,
            )),
            nav: Nav::About,
            games: Vec::new(),
            search: String::new(),
            scan_rx: None,
            scanning: false,
            welcome: false,
            inspections: BTreeMap::new(),
            jobs: Jobs::default(),
            tx,
            rx,
            started: false,
            details_open: false,
            preview: None,
            recovered: None,
            manifest_note: None,
            fatal,
            shot: None,
            shot_frames: 0,
            update: None,
            update_rx: None,
            update_checked: false,
            update_manual: false,
            update_error: None,
            update_prompt: crate::update_prompt::UpdatePrompt::new(receipt_dir),
            about: crate::about::About::default(),
        }
    }

    /// 版式自检：渲染稳定之后把这一帧存成 PNG，然后退出。
    pub fn with_screenshot(mut self, shot: Option<crate::ScreenshotRequest>) -> Self {
        if let Some(request) = &shot
            && request.theme.is_some()
        {
            self.started = true;
            self.update_checked = true;
            self.about.coffee_open = request.coffee || request.zoom;
            self.about.zoomed = request.zoom;
            self.update_prompt = crate::update_prompt::UpdatePrompt::new(None);
            if request.update_prompt {
                let available = update::Available {
                    current: update::current_version().to_string(),
                    manifest: update::Manifest {
                        version: "0.1.4".to_string(),
                        notes: "修复窗口缩到最小时，侧栏文字被截断的问题。\n启动速度更快。\n改进深色模式的文字对比度。\n调整键盘导航。\n优化状态提示。\n其他细节改进。".to_string(),
                        download_url: crate::links::RELEASE_URL.to_string(),
                        published: String::new(),
                    },
                };
                self.update_prompt.offer(&available, true);
                self.update = Some(available);
            }
            if request.update_checking {
                self.update_rx = Some(mpsc::channel().1);
            }
        }
        self.shot = shot;
        self
    }

    /// 等界面稳定下来（**含后台整库扫描**）再截。
    fn maybe_screenshot(&mut self, ctx: &egui::Context) {
        const SETTLE_FRAMES: u32 = 60;
        let Some(request) = &self.shot else {
            return;
        };
        let keep_welcome = request.welcome;
        self.shot_frames += 1;
        ctx.request_repaint(); // 自检期间必须持续出帧，否则等不到那一刻
        if !keep_welcome {
            self.welcome = false; // 默认拍主界面；要拍说明页加 --welcome
        }
        // **等整库扫描真的跑完再截**，否则拍到的永远是「正在扫描…」。
        if self.shot_frames >= SETTLE_FRAMES
            && !self.scanning
            && self.shot_frames.is_multiple_of(30)
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default()));
        }
        let captured = ctx.input(|input| {
            input.events.iter().find_map(|event| match event {
                egui::Event::Screenshot { image, .. } => Some(Arc::clone(image)),
                _ => None,
            })
        });
        if let Some(image) = captured {
            if let Some(request) = self.shot.take() {
                save_screenshot(&image, &request.path);
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    /// 首帧：先收拾上次被打断的操作，再刷新清单并体检两款游戏。
    fn start(&mut self, ctx: &egui::Context) {
        match self.manager.recover_pending() {
            Ok(games) if !games.is_empty() => {
                self.recovered = Some(format!(
                    "上次有 {} 个操作没做完，已经退回操作前的状态。",
                    games.len()
                ));
            }
            Ok(_) => {}
            Err(e) => self.recovered = Some(format!("收拾上次未完成的操作时出错：{e}")),
        }
        // 「不再显示」是一个空文件：存在即不再显示。
        self.welcome = !self.manager.paths.welcome_marker().is_file();
        self.spawn_scan(ctx);
        self.check_update(ctx, false);
    }

    /// 后台扫一遍整个 Steam 库：刷新清单 → 列出游戏 → 逐个体检。
    ///
    /// **全在工作线程里做。** 体检要走每款游戏的目录（反作弊特征、图形 API），
    /// 库大的话要几秒，放在 UI 线程上界面就卡住了。
    fn spawn_scan(&mut self, ctx: &egui::Context) {
        if self.scanning {
            return;
        }
        self.scanning = true;
        let manager = Arc::clone(&self.manager);
        let ctx = ctx.clone();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let note = match manager.refresh_snapshot() {
                Ok(snapshot) => {
                    let _ = manager.refresh_upstream();
                    match snapshot.origin {
                        SnapshotOrigin::Online => None,
                        SnapshotOrigin::Cache => {
                            Some("在线清单不可用，用的是本地缓存的上一次结果。".to_string())
                        }
                    }
                }
                Err(e) => Some(format!("拿不到组件清单：{e}")),
            };
            let _ = tx.send(ScanOutcome {
                games: manager.inspect_library(),
                note,
            });
            ctx.request_repaint();
        });
        self.scan_rx = Some(rx);
    }

    /// 后台查一次 Vivid 自身的更新。失败静默——连不上更新服务器不是用户的问题。
    fn check_update(&mut self, ctx: &egui::Context, manual: bool) {
        if self
            .shot
            .as_ref()
            .is_some_and(|request| request.theme.is_some())
        {
            return;
        }
        if self.update_rx.is_some() {
            return;
        }
        self.update_manual = manual;
        self.update_error = None;
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let found = update::check(update::DEFAULT_MANIFEST_URL, update::current_version());
            let _ = tx.send(found);
            ctx.request_repaint();
        });
        self.update_rx = Some(rx);
    }

    fn receive_update(&mut self, result: Result<Option<update::Available>, update::UpdateError>) {
        match result {
            Ok(found) => {
                if let Some(available) = &found {
                    self.update_prompt.offer(available, self.update_manual);
                }
                self.update = found;
                self.update_error = None;
                self.update_checked = true;
            }
            Err(error) => {
                self.update_checked = false;
                self.update_error = self.update_manual.then(|| error.to_string());
            }
        }
    }

    /// 把一个任务丢给工作线程。**同一款游戏同时只允许一个**，由 `Jobs` 把关。
    fn spawn(&mut self, ctx: &egui::Context, game: GameId, kind: JobKind) {
        self.run_in_background(ctx, game, kind, move |manager, _tx| {
            run_job(manager, game, kind)
        });
    }

    /// 下载并校验组件、生成计划，然后交给用户确认。**这一步不写游戏目录。**
    fn spawn_prepare(&mut self, ctx: &egui::Context, game: GameId, intent: JobKind) {
        self.run_in_background(ctx, game, JobKind::Prepare, move |manager, tx| {
            let backend = manager.backend_for(game).map_err(text)?;
            progress(tx, game, "正在下载并校验组件", 0.35);
            let prepared = manager.prepare(game, &backend).map_err(text)?;
            let count = prepared.plan.actions.len();
            let _ = tx.send(JobEvent::Prepared {
                game,
                intent,
                prepared: Box::new(prepared),
            });
            Ok(format!("组件已就绪，{count} 处改动等你确认"))
        });
    }

    /// 用户在预演里确认之后，才真正写入。
    fn spawn_apply(
        &mut self,
        ctx: &egui::Context,
        game: GameId,
        intent: JobKind,
        prepared: PreparedInstall,
    ) {
        self.run_in_background(ctx, game, intent, move |manager, tx| {
            apply_prepared(manager, tx, game, prepared)
        });
    }

    fn run_in_background<F>(&mut self, ctx: &egui::Context, game: GameId, kind: JobKind, work: F)
    where
        F: FnOnce(&Manager, &Sender<JobEvent>) -> Result<String, String> + Send + 'static,
    {
        if self.fatal.is_some() || self.jobs.start(game, kind).is_err() {
            return;
        }
        let manager = Arc::clone(&self.manager);
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = work(&manager, &tx);
            // 干完活重新体检这一款：状态和证据都以磁盘现状为准，不靠推测。
            // 别的游戏不重扫——整库扫描是启动时那一次，这里只更新被动过的那个。
            if let Ok(backend) = manager.backend_for(game)
                && let Ok(inspection) = manager.inspect(game, &backend)
            {
                let _ = tx.send(JobEvent::Inspected {
                    game,
                    inspection: Box::new(inspection),
                });
            }
            let _ = tx.send(JobEvent::Finished { game, result });
            ctx.request_repaint();
        });
    }

    fn drain_events(&mut self) {
        while let Ok(event) = self.rx.try_recv() {
            match event {
                JobEvent::Inspected { game, inspection } => {
                    // 左栏那一行的状态也要跟着变，否则装完了左边还写着「可安装」。
                    if let Some((_, report)) = self
                        .games
                        .iter_mut()
                        .find(|(app, _)| GameId::from_app_id(app.app_id) == game)
                    {
                        *report = (*inspection).clone();
                    }
                    self.inspections.insert(game, *inspection);
                }
                JobEvent::Prepared {
                    game,
                    intent,
                    prepared,
                } => {
                    self.preview = Some(PreviewPrompt {
                        game,
                        intent,
                        prepared,
                    });
                }
                JobEvent::Finished { game, result } => {
                    if let Ok(message) = &result
                        && self.jobs.kind(game) == Some(JobKind::Refresh)
                    {
                        self.manifest_note = message.contains("缓存").then(|| message.clone());
                    }
                    self.jobs.apply(JobEvent::Finished { game, result });
                }
                other => self.jobs.apply(other),
            }
        }
        if let Some(rx) = &self.update_rx
            && let Ok(found) = rx.try_recv()
        {
            self.receive_update(found);
            self.update_rx = None;
        }
        if let Some(rx) = &self.scan_rx
            && let Ok(outcome) = rx.try_recv()
        {
            self.manifest_note = outcome.note;
            self.inspections = outcome
                .games
                .iter()
                .map(|(app, report)| (GameId::from_app_id(app.app_id), report.clone()))
                .collect();
            self.games = outcome.games;
            self.scan_rx = None;
            self.scanning = false;
            // 默认落在第一款能装的游戏上；一款都没有就停在「关于」。
            if let Some(app_id) = self.shot.as_ref().and_then(|shot| shot.select) {
                self.nav = Nav::Game(GameId::from_app_id(app_id));
            } else if self.nav == Nav::About
                && let Some((app, _)) = self.games.iter().find(|(_, report)| {
                    report.primary_action() != PrimaryAction::None
                        || report.state == GameState::ReadyToInstall
                })
            {
                self.nav = Nav::Game(GameId::from_app_id(app.app_id));
            }
        }
    }

    /// 这款游戏在左栏属于哪一组。
    fn group_of(report: &GameInspection) -> NavGroup {
        match &report.state {
            GameState::BlockedByAntiCheat { .. }
            | GameState::UnknownApi
            | GameState::NotAGame
            | GameState::NotFound => NavGroup::Blocked,
            _ => match report.game.as_ref().map(|game| game.recipe.verification) {
                Some(Verification::Verified) => NavGroup::Verified,
                _ => NavGroup::Untested,
            },
        }
    }

    /// 左栏那一行的短状态。
    fn short_status(&self, game: GameId, report: &GameInspection) -> (Tone, &'static str) {
        if self.jobs.is_busy(game) {
            return (Tone::Brand, "处理中");
        }
        match &report.state {
            GameState::NotFound => (Tone::Neutral, "未找到"),
            GameState::UnsupportedGpu => (Tone::Neutral, "暂不支持"),
            GameState::ManifestUnavailable => (Tone::Neutral, "清单不可用"),
            GameState::BlockedByAntiCheat { .. } => (Tone::Neutral, "反作弊"),
            GameState::UnknownApi => (Tone::Neutral, "判不出 API"),
            GameState::NotAGame => (Tone::Neutral, "不是游戏"),
            GameState::ReadyToInstall => (Tone::Brand, "可安装"),
            GameState::Installed => (Tone::Ok, "已安装"),
            GameState::UpdateAvailable { .. } => (Tone::Brand, "可更新"),
            GameState::MissingFiles { .. } => (Tone::Warn, "文件异常"),
            GameState::Conflict { .. } => (Tone::Bad, "有冲突"),
        }
    }
}

/// 左栏的分组。**不给装的游戏也有自己的一组**，不从列表里抹掉。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NavGroup {
    Verified,
    Untested,
    Blocked,
}

impl NavGroup {
    const ALL: [Self; 3] = [Self::Verified, Self::Untested, Self::Blocked];

    const fn label(self) -> &'static str {
        match self {
            Self::Verified => "可安装",
            Self::Untested => "未实测",
            Self::Blocked => "不提供安装",
        }
    }
}

impl Default for Home {
    fn default() -> Self {
        Self::new()
    }
}

impl eframe::App for Home {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if !self.started {
            self.started = true;
            self.start(&ctx);
        }
        self.drain_events();

        if let Some(fatal) = self.fatal.clone() {
            egui::CentralPanel::default().show(ui, |ui| {
                ui.add_space(40.0);
                ui::state_panel(ui, "Vivid 起不来", &fatal);
            });
            return;
        }

        let mut pending: Option<(GameId, JobKind)> = None;

        // ---------- 左栏：两款游戏 + 关于 ----------
        egui::Panel::left("nav")
            .resizable(false)
            .default_size(250.0)
            .frame(
                egui::Frame::new()
                    .fill(ui::palette(ui).sidebar)
                    .inner_margin(egui::Margin {
                        left: 8,
                        right: 8,
                        top: 0,
                        bottom: 0,
                    }),
            )
            .show(ui, |ui| {
                ui.add_space(14.0);
                ui.horizontal(|ui| {
                    ui.add_space(4.0);
                    ui.vertical(|ui| {
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new(vivid_core::NAME_ZH)
                                    .size(19.0)
                                    .strong()
                                    .color(ui::palette(ui).text),
                            );
                            ui.add_space(6.0);
                            ui.label(
                                egui::RichText::new("·")
                                    .size(12.0)
                                    .color(ui::palette(ui).muted),
                            );
                            ui.add_space(5.0);
                            tracked_text(ui, vivid_core::NAME_EN, 13.0, ui::palette(ui).muted, 1.5);
                        });
                        ui.add_space(3.0);
                        ui.label(
                            egui::RichText::new(SLOGAN)
                                .size(11.5)
                                .color(ui::palette(ui).muted),
                        );
                    });
                });
                ui.add_space(16.0);

                // 搜索：库大了之后没有搜索就找不到东西。
                ui::search_field(ui, "搜索游戏", &mut self.search);
                ui.add_space(12.0);

                let needle = self.search.trim().to_lowercase();
                let mut selected: Option<GameId> = None;
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let mut shown = 0usize;
                        for group in NavGroup::ALL {
                            let rows: Vec<(&SteamApp, &GameInspection)> = self
                                .games
                                .iter()
                                .filter(|(_, report)| Self::group_of(report) == group)
                                .filter(|(app, _)| name_matches(&app.name, &needle))
                                .map(|(app, report)| (app, report))
                                .collect();
                            if rows.is_empty() {
                                continue;
                            }
                            shown += rows.len();
                            ui::nav_section(ui, group.label());
                            for (app, report) in rows {
                                let id = GameId::from_app_id(app.app_id);
                                let (tone, status) = self.short_status(id, report);
                                if ui::nav_item(
                                    ui,
                                    self.nav == Nav::Game(id),
                                    &app.name,
                                    &short_path(&app.game_root),
                                    Some((tone, status)),
                                )
                                .clicked()
                                {
                                    selected = Some(id);
                                }
                            }
                            ui.add_space(8.0);
                        }
                        if shown == 0 {
                            ui.add_space(6.0);
                            ui::hint(
                                ui,
                                if self.scanning {
                                    "正在扫描 Steam 库…"
                                } else if self.games.is_empty() {
                                    "没有发现已安装的 Steam 游戏。"
                                } else {
                                    "没有名字匹配的游戏。"
                                },
                            );
                            ui.add_space(8.0);
                        }

                        ui::nav_section(ui, "其它");
                        let about_entry =
                            ui::nav_item(ui, self.nav == Nav::About, "关于与作者", "", None);
                        if self.update.is_some() {
                            ui.painter().circle_filled(
                                egui::pos2(
                                    about_entry.rect.right() - 14.0,
                                    about_entry.rect.center().y,
                                ),
                                3.0,
                                ui::palette(ui).accent,
                            );
                        }
                        if about_entry.clicked() {
                            self.nav = Nav::About;
                        }
                    });
                if let Some(id) = selected {
                    self.nav = Nav::Game(id);
                }
            });

        // ---------- 底部：常驻说明（灰字，不用警告色）----------
        egui::Panel::bottom("bottom").show(ui, |ui| {
            ui.add_space(9.0);
            if let Some(note) = &self.recovered {
                ui::hint(ui, note);
                ui.add_space(5.0);
            }
            if let Some(note) = &self.manifest_note {
                ui::hint(ui, note);
                ui.add_space(5.0);
            }
            ui::hint(
                ui,
                "这些文件只用于单机模式。装过的游戏请不要进联机——部分反作弊会因为文件改动判定异常；\
                 出问题点「一键还原」就能回到原状。",
            );
            ui.add_space(4.0);
            // 闸门的局限必须常驻：**沉默会被读成「Vivid 说这游戏能联机」**。
            ui::hint(ui, vivid_core::anticheat::CANNOT_BE_SURE);
            ui.add_space(9.0);
        });

        // ---------- 右栏：结论 + 唯一主按钮 + 折叠详情 ----------
        egui::CentralPanel::default().show(ui, |ui| {
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.add_space(6.0);
                    match self.nav {
                        Nav::About => self.about_page(ui),
                        Nav::Game(game) => pending = self.game_page(ui, game),
                    }
                    ui.add_space(12.0);
                });
        });

        // ---------- 首次启动：先把 Vivid 会做什么、必须知道什么说清楚 ----------
        self.welcome_dialog(&ctx);

        self.about.dialogs(&ctx);
        if self.update_prompt.is_open()
            && !self.welcome
            && self.preview.is_none()
            && !self.about.has_dialog()
        {
            self.update_prompt.show(&ctx);
        }
        if let Some(id) = self.update_prompt.take_return_focus() {
            ctx.memory_mut(|memory| memory.request_focus(id));
        }
        if let Some(error) = self.update_prompt.take_persistence_error() {
            self.update_error = Some(error);
        }
        self.maybe_screenshot(&ctx);

        // ---------- 预演：把要写什么摆出来，确认之后才动手 ----------
        self.preview_dialog(&ctx);

        if let Some((game, kind)) = pending {
            match kind {
                // 安装和更新都先生成计划给用户看，确认之后才写。
                JobKind::Install | JobKind::Update => self.spawn_prepare(&ctx, game, kind),
                other => self.spawn(&ctx, game, other),
            }
        }
    }
}

/// 一份已经生成好、等用户确认的计划。
struct PreviewPrompt {
    game: GameId,
    intent: JobKind,
    prepared: Box<PreparedInstall>,
}

/// 「Vivid 会做什么」——三条，都是可验证的行为，不是承诺。
const WELCOME_DOES: [&str; 3] = [
    "只在你点安装时，按 RHI 的在线清单下载组件到本机缓存（不打包、不托管、不修改）",
    "只往你选的游戏目录写固定的几个文件，写之前先备份原文件",
    "出问题「一键还原」完整退回，注册表原值也一并恢复",
];

/// 「必须知道的」——**这一段不许删、不许软化**。
///
/// 它是整个软件唯一一次能把边界一次说清的地方：只用于单机、闸门抓不全、
/// 非 D3D12 装了也不变、组件各有各的许可、本体免费。
const WELCOME_MUST_KNOW: [&str; 5] = [
    "这些文件只用于单机。装过的游戏别进联机——部分反作弊会因文件改动判定异常",
    "Vivid 无法百分百判断一款游戏有没有反作弊（VAC 这类在游戏目录里没有痕迹）",
    "DLSS 5 神经渲染只对 D3D12 游戏有效；其它游戏能装 ReShade，但画面不会变",
    "组件来自 NVIDIA / ReShade / RenoDX，各自受自己的许可约束",
    "Vivid 本体采用 GPL-3.0-only，可按许可证使用、修改和再分发",
];

impl Home {
    /// 首次启动说明。
    ///
    /// 「我知道了」只关掉这一次，「不再显示」写一个空标记文件。
    /// 用文件而不是配置项：想让它再出来一次，删掉那个文件就行。
    fn welcome_dialog(&mut self, ctx: &egui::Context) {
        if !self.welcome {
            return;
        }
        let mut close = false;
        let mut never = false;
        egui::Window::new("锐境 Vivid 会做什么")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.set_max_width(600.0);
                ui::section(ui, |ui| {
                    for line in WELCOME_DOES {
                        ui.label(
                            egui::RichText::new(format!("· {line}"))
                                .size(13.0)
                                .color(ui::palette(ui).text),
                        );
                        ui.add_space(5.0);
                    }
                });

                ui.add_space(14.0);
                ui.label(
                    egui::RichText::new("必须知道的")
                        .size(13.5)
                        .strong()
                        .color(ui::palette(ui).text),
                );
                ui.add_space(6.0);
                ui::section(ui, |ui| {
                    for line in WELCOME_MUST_KNOW {
                        ui.label(
                            egui::RichText::new(format!("· {line}"))
                                .size(13.0)
                                .color(ui::palette(ui).text),
                        );
                        ui.add_space(5.0);
                    }
                });

                ui.add_space(14.0);
                ui.horizontal(|ui| {
                    if ui::primary_button(ui, "我知道了").clicked() {
                        close = true;
                    }
                    ui.add_space(8.0);
                    if ui::ghost_button(ui, "不再显示").clicked() {
                        never = true;
                    }
                });
            });

        if never {
            let marker = self.manager.paths.welcome_marker();
            if let Some(dir) = marker.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = std::fs::write(&marker, b"");
        }
        if close || never {
            self.welcome = false;
        }
    }

    /// 安装前的只读预演（设计文档 9.1）。
    ///
    /// **这是「点了才知道装了什么」和「装之前就看得见」的分界。**
    /// 组件此时已经下载并校验完，但游戏目录一个字节都还没被碰过；
    /// 用户在这里看到每个目标、来源和新增/替换，确认之后才写。
    fn preview_dialog(&mut self, ctx: &egui::Context) {
        let mut confirmed = false;
        let mut cancelled = false;

        if let Some(prompt) = &self.preview {
            let rows = prompt.prepared.planned_targets();
            let title = if prompt.intent == JobKind::Update {
                "更新前请确认"
            } else {
                "安装前请确认"
            };
            egui::Window::new(title)
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.set_max_width(640.0);
                    ui.label(
                        egui::RichText::new(format!(
                            "下面 {} 处改动都还没有发生——点「确认」之前，游戏目录一个字节都没被碰过。",
                            rows.len()
                        ))
                        .size(13.0)
                        .color(ui::palette(ui).text),
                    );

                    // 实测发现的已知后果排在最前：用户在点确认之前必须看到它。
                    for caveat in prompt.prepared.known_caveats() {
                        ui.add_space(8.0);
                        ui.label(
                            egui::RichText::new(caveat)
                                .size(13.0)
                                .color(ui::color::WARN),
                        );
                    }

                    if prompt.prepared.plan.requires_admin {
                        ui.add_space(8.0);
                        // 偶发的、真有事发生的提示才用警告色（规范 §5.5）。
                        ui.label(
                            egui::RichText::new(
                                "这一步会安装全局 Vulkan ReShade layer，会影响这台电脑上所有 Vulkan 游戏。",
                            )
                            .size(13.0)
                            .strong()
                            .color(ui::color::WARN),
                        );
                        ui.add_space(3.0);
                        // 提前说清楚 UAC 会弹——不说的话，一个突然冒出来的
                        // 系统授权窗口本身就会把人吓退。
                        if elevation::is_process_elevated() {
                            ui::hint(
                                ui,
                                "Vivid 当前已经以管理员身份运行，不会再弹授权窗口；一键还原会把这些改动恢复原状。",
                            );
                        } else {
                            ui::hint(
                                ui,
                                "点确认后 Windows 会弹一次管理员授权窗口，那是正常的——写全局层和注册表需要它，点「是」才会继续。                                 只在这次操作请求一次，Vivid 不常驻管理员权限；一键还原会把这些改动恢复原状。",
                            );
                        }
                    }

                    ui.add_space(12.0);
                    egui::ScrollArea::vertical()
                        .max_height(320.0)
                        // 滚动条常驻：不显示的话，被切在半截的那一行看起来
                        // 像渲染缺陷，而不是「还能往下滚」。
                        .scroll_bar_visibility(
                            egui::scroll_area::ScrollBarVisibility::AlwaysVisible,
                        )
                        .show(ui, |ui| {
                            ui::group(ui, |ui| {
                                for row in &rows {
                                    ui.horizontal(|ui| {
                                        let tone = match row.change {
                                            PreviewChange::Add => Tone::Brand,
                                            PreviewChange::Replace => Tone::Warn,
                                        };
                                        ui::badge(ui, tone, row.change.label());
                                        ui.add_space(6.0);
                                        ui.label(
                                            egui::RichText::new(&row.target)
                                                .size(12.5)
                                                .strong()
                                                .color(ui::palette(ui).text),
                                        );
                                    });
                                    ui.label(
                                        egui::RichText::new(&row.origin)
                                            .size(11.5)
                                            .color(ui::palette(ui).muted),
                                    );
                                    if let Some(url) = &row.source_url {
                                        ui.label(
                                            egui::RichText::new(url)
                                                .size(11.0)
                                                .color(ui::palette(ui).muted),
                                        );
                                    }
                                    // 标「替换」的行要说清**被替换掉的是什么**：
                                    // 防的是把用户已装的 Special K / ENB / DXVK 顶掉。
                                    if let Some(current) = &row.current {
                                        ui.label(
                                            egui::RichText::new(format!("现在是：{current}"))
                                                .size(11.5)
                                                .color(ui::color::WARN),
                                        );
                                    }
                                    if let Some(sha) = &row.sha256 {
                                        ui.label(
                                            egui::RichText::new(format!(
                                                "SHA-256 {}",
                                                &sha[..sha.len().min(16)]
                                            ))
                                            .size(11.0)
                                            .color(ui::palette(ui).muted),
                                        );
                                    }
                                    ui.add_space(9.0);
                                }
                            });
                        });

                    ui.add_space(12.0);
                    ui::hint(
                        ui,
                        "版本和来源由 RHI 的在线清单决定；SHA-256 是 Vivid 下载后自己算的本地指纹，不是上游签名。",
                    );
                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        if ui::primary_button(ui, "确认，开始写入").clicked() {
                            confirmed = true;
                        }
                        ui.add_space(8.0);
                        if ui::ghost_button(ui, "取消").clicked() {
                            cancelled = true;
                        }
                    });
                });
        }

        if confirmed {
            if let Some(prompt) = self.preview.take() {
                self.spawn_apply(ctx, prompt.game, prompt.intent, *prompt.prepared);
            }
        } else if cancelled {
            // 取消就是真的什么都没发生：组件留在缓存里，游戏目录没动过。
            self.preview = None;
        }
    }

    /// 右栏的游戏页。返回用户点出来的任务（在借用结束之后再启动）。
    fn game_page(&mut self, ui: &mut egui::Ui, game: GameId) -> Option<(GameId, JobKind)> {
        let mut pending = None;
        let busy = self.jobs.is_busy(game);
        let Some(report) = self.inspections.get(&game) else {
            ui::state_panel(ui, "正在检查", "第一次打开要读一次清单和游戏目录。");
            return None;
        };
        // 名字取 Steam 清单里的那个：泛化之后大多数游戏在代码里没有固定名。
        let title = self
            .games
            .iter()
            .find(|(app, _)| GameId::from_app_id(app.app_id) == game)
            .map(|(app, _)| app.name.clone())
            .unwrap_or_else(|| game.label());

        // ---- 标题行：游戏名 + 支持标签 + 显卡 ----
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(&title)
                    .size(18.0)
                    .strong()
                    .color(ui::palette(ui).text),
            );
            ui.add_space(8.0);
            if let Some(found) = &report.game {
                // 「方案可用」「实验支持」「未实测」说的是**作者验证到哪一步**。
                ui::badge(ui, Tone::Neutral, found.recipe.support_label);
            }
        });
        ui.add_space(3.0);
        ui.label(
            egui::RichText::new(gpu_line(report))
                .size(12.0)
                .color(ui::palette(ui).muted),
        );
        ui.add_space(12.0);

        // ---- 结论层：整屏唯一强调，主按钮就在里面 ----
        let (tone, title, desc) = verdict_text(&report.state, busy, self.jobs.kind(game));
        let action = report.primary_action();
        let label = action.label();
        ui::verdict(ui, tone, &title, &desc, |ui| {
            if busy {
                ui.spinner();
                return;
            }
            if action != PrimaryAction::None
                && ui::primary_button(ui, label).clicked()
                && let Some(job) = job_for(action)
            {
                pending = Some((game, job));
            }
        });

        // ---- 一句下一步说明 + 证据分层 ----
        ui.add_space(8.0);
        for line in report.evidence_lines() {
            ui::hint(ui, line);
        }
        if action == PrimaryAction::LaunchSinglePlayer {
            ui::hint(
                ui,
                "「启动单机模式」只用于 Story Mode，不提供 GTA Online 入口。",
            );
        }
        // 常驻位置用灰字：警告色留给偶发提示（规范 §5.5）。
        // 真要动手时，预演框里会用警告色再说一遍。
        if let Some(found) = &report.game {
            for caveat in found.recipe.known_caveats() {
                ui::hint(ui, caveat);
            }
        }
        if let Some((message, percent)) = self.jobs.progress(game) {
            ui.add_space(6.0);
            // 不设 desired_height：条里带文字，压到 6px 会把字裁掉。
            ui.add(egui::ProgressBar::new(percent).text(message));
        }
        if let Some(done) = self.jobs.last_message(game) {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui::badge(ui, Tone::Ok, "完成");
                ui.add_space(2.0);
                ui::hint(ui, done);
            });
        }
        if let Some(summary) = self.jobs.last_summary(game) {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui::badge(ui, Tone::Bad, "出错");
                ui.add_space(2.0);
                ui::hint(ui, summary);
            });
        }

        // ---- 次级操作：一律淡底，不再出现第二个高饱和填充 ----
        ui.add_space(14.0);
        ui.horizontal(|ui| {
            if !busy && ui::row_button(ui, "重新检查").clicked() {
                pending = Some((game, JobKind::Refresh));
            }
            ui.add_space(8.0);
            if let Some(found) = &report.game {
                let log = found.recipe.game_root.join("ReShade.log");
                if log.is_file() && ui::row_button(ui, "查看日志").clicked() {
                    let _ = std::process::Command::new("explorer").arg(&log).spawn();
                }
                ui.add_space(8.0);
            }
            if !busy && report.files_installed && ui::ghost_button(ui, "一键还原").clicked() {
                pending = Some((game, JobKind::Restore));
            }
        });

        // ---- 折叠详情：来源 URL、SHA-256、目标、备份、日志 ----
        ui.add_space(14.0);
        // 用 ASCII 三角：中文字体里没有 ▸/▾ 的字形，会渲染成豆腐块
        // （2026-09-01 版式自检截图抓到的）。
        let arrow = if self.details_open { "v" } else { ">" };
        if ui::row_button(ui, &format!("{arrow} 组件、来源与备份")).clicked() {
            self.details_open = !self.details_open;
        }
        if self.details_open {
            ui.add_space(8.0);
            ui::section(ui, |ui| {
                for line in &report.detail_lines {
                    ui.label(
                        egui::RichText::new(line)
                            .size(12.0)
                            .color(ui::palette(ui).text),
                    );
                    ui.add_space(3.0);
                }
                if let Some(error) = self.jobs.last_error(game) {
                    ui.add_space(6.0);
                    ui.label(
                        egui::RichText::new(format!("上次出错的原文：{error}"))
                            .size(12.0)
                            .color(ui::palette(ui).muted),
                    );
                }
                match &report.state {
                    GameState::Conflict { files } | GameState::MissingFiles { files } => {
                        ui.add_space(6.0);
                        for file in files {
                            ui.label(
                                egui::RichText::new(file.display().to_string())
                                    .size(12.0)
                                    .color(ui::palette(ui).muted),
                            );
                        }
                    }
                    _ => {}
                }
            });
        }
        pending
    }

    // Version, update status, author channels and the legal entry.
    fn about_page(&mut self, ui: &mut egui::Ui) {
        let width = ui.available_width().min(560.0);
        let inset = (ui.available_width() - width) / 2.0;
        ui.horizontal(|ui| {
            ui.add_space(inset);
            ui.vertical(|ui| {
                ui.set_width(width);
                self.about_content(ui);
            });
        });
    }

    fn about_content(&mut self, ui: &mut egui::Ui) {
        ui.spacing_mut().item_spacing.y = 0.0;
        ui.label(
            egui::RichText::new("关于")
                .size(17.0)
                .strong()
                .color(ui::palette(ui).text),
        );
        ui.add_space(14.0);

        let checking = self.update_rx.is_some();
        let mut recheck = false;
        ui::group(ui, |ui| {
            let (title, sub) = match &self.update {
                Some(available) => (
                    format!("版本 {}", available.current),
                    format!(
                        "有新版本 v{}",
                        available.manifest.version.trim_start_matches(['v', 'V'])
                    ),
                ),
                None => (
                    format!("版本 {}", update::current_version()),
                    if checking {
                        "正在检查更新…".to_string()
                    } else if let Some(error) = &self.update_error {
                        error.clone()
                    } else if self.update_checked {
                        "已经是最新版本".to_string()
                    } else {
                        "点右边可以检查更新".to_string()
                    },
                ),
            };
            ui::row(ui, &title, &sub, |ui| {
                if self.update.is_some() {
                    if ui::row_button(ui, "去下载").clicked() {
                        crate::links::open(ui.ctx(), crate::links::RELEASE_URL);
                    }
                    if !checking && ui::ghost_button(ui, "检查更新").clicked() {
                        recheck = true;
                    }
                    if checking {
                        ui.spinner();
                    }
                } else if checking {
                    ui.spinner();
                } else if ui::ghost_button(ui, "检查更新").clicked() {
                    recheck = true;
                }
            });
        });
        if checking && self.update.is_some() {
            ui.add_space(4.0);
            ui::hint(ui, "正在检查更新…");
        }
        if self.update.is_some()
            && let Some(error) = &self.update_error
        {
            ui.add_space(4.0);
            ui::hint(ui, error);
        }
        ui.add_space(24.0);
        self.about.author(ui);

        if recheck {
            let ctx = ui.ctx().clone();
            self.update_checked = false;
            self.check_update(&ctx, true);
        }
    }
}

/// 主按钮 → 后台任务。`ShowConflict` 只展开详情，不触发任何写入。
fn job_for(action: PrimaryAction) -> Option<JobKind> {
    match action {
        PrimaryAction::Install => Some(JobKind::Install),
        PrimaryAction::Update => Some(JobKind::Update),
        PrimaryAction::Repair => Some(JobKind::Repair),
        PrimaryAction::LaunchSinglePlayer | PrimaryAction::LaunchGame => Some(JobKind::Launch),
        PrimaryAction::None | PrimaryAction::ShowConflict => None,
    }
}

/// 大结论那一句。**只说程序能证明的事。**
fn verdict_text(state: &GameState, busy: bool, kind: Option<JobKind>) -> (Tone, String, String) {
    if busy {
        let label = kind.map(JobKind::busy_label).unwrap_or("正在处理");
        return (
            Tone::Brand,
            format!("{label}…"),
            "请等这一步做完，中途失败会自动退回操作前的状态。".to_string(),
        );
    }
    match state {
        GameState::NotFound => (
            Tone::Neutral,
            "没找到这款游戏".to_string(),
            "确认它已经通过 Steam 安装，然后点「重新检查」。".to_string(),
        ),
        GameState::UnsupportedGpu => (
            Tone::Neutral,
            "这张显卡暂不支持".to_string(),
            "RHI 上游声明覆盖 RTX 20/30/40/50，其它显卡 Vivid 不猜。".to_string(),
        ),
        GameState::ManifestUnavailable => (
            Tone::Neutral,
            "暂时拿不到组件清单".to_string(),
            "连上网之后点「重新检查」；已经装过的游戏仍然可以启动和还原。".to_string(),
        ),
        // 不给装的两种情况：**说清为什么**，而不是假装没看见这款游戏。
        GameState::BlockedByAntiCheat { reason } => (
            Tone::Neutral,
            "不提供安装".to_string(),
            reason.clone(),
        ),
        GameState::NotAGame => (
            Tone::Neutral,
            "不提供安装".to_string(),
            "这个目录里没有可执行文件，看起来不是一款装好的游戏（Steam 库里的运行库包就是这样）。"
                .to_string(),
        ),
        GameState::UnknownApi => (
            Tone::Neutral,
            "不提供安装".to_string(),
            "判不出这款游戏用的是哪种图形 API。注入方式和游戏实际用的 API 对不上会让游戏闪退，             所以 Vivid 不猜。"
                .to_string(),
        ),
        GameState::ReadyToInstall => (
            Tone::Brand,
            "可以安装".to_string(),
            "会先下载并校验组件，写入前自动备份原文件。".to_string(),
        ),
        GameState::Installed => (
            Tone::Ok,
            "文件已安装".to_string(),
            "进入游戏后再看实际画面——文件到位不等于画面已经变了。".to_string(),
        ),
        GameState::UpdateAvailable {
            installed,
            available,
        } => (
            Tone::Brand,
            "上游有新版本".to_string(),
            format!("已装 {installed}，可更新到 {available}。不点就不会替换。"),
        ),
        GameState::MissingFiles { files } => (
            Tone::Warn,
            "文件异常".to_string(),
            format!("有 {} 个文件不见了，可以按记录补回来。", files.len()),
        ),
        GameState::Conflict { files } => (
            Tone::Bad,
            "检测到外部改动".to_string(),
            format!(
                "有 {} 个文件被别的程序改过，Vivid 不会直接覆盖。展开详情看是哪些。",
                files.len()
            ),
        ),
    }
}

fn gpu_line(report: &GameInspection) -> String {
    match &report.gpu {
        Some(gpu) => gpu.summary(),
        None => "没有读到显卡信息".to_string(),
    }
}

/// 搜索词能不能匹配上这个游戏名。
///
/// **空词放行所有**：搜索框是缩小范围用的，不是必填项。
/// 大小写不敏感——Steam 的游戏名大小写五花八门，让用户去猜是不合理的。
/// `needle` 由调用方先 trim + 转小写。
fn name_matches(name: &str, needle: &str) -> bool {
    needle.is_empty() || name.to_lowercase().contains(needle)
}

/// 左栏只放得下一小段路径，取最后两级。
fn short_path(path: &std::path::Path) -> String {
    let parts: Vec<String> = path
        .components()
        .rev()
        .take(2)
        .map(|c| c.as_os_str().to_string_lossy().to_string())
        .collect();
    parts.into_iter().rev().collect::<Vec<_>>().join("\\")
}

// ---------------------------------------------------------------------------
// 工作线程
// ---------------------------------------------------------------------------

/// 不需要预演确认的那几种任务。安装与更新不从这里进——
/// 它们走 `spawn_prepare` → 预演 → `spawn_apply`。
fn run_job(manager: &Manager, game: GameId, kind: JobKind) -> Result<String, String> {
    match kind {
        JobKind::Refresh => refresh(manager),
        // 安装和更新走「先预演、确认后再写」那条路，不从这里进。
        JobKind::Prepare | JobKind::Install | JobKind::Update => {
            Err("安装和更新要先经过预演确认".to_string())
        }
        JobKind::Repair => {
            let mut backend = manager.backend_for(game).map_err(text)?;
            manager.repair(game, &mut backend).map_err(text)?;
            Ok("已按记录补回缺失的文件".to_string())
        }
        JobKind::Restore => restore(manager, game),
        JobKind::Launch => {
            manager.launch(game).map_err(text)?;
            Ok("已经让 Steam 启动游戏".to_string())
        }
    }
}

fn refresh(manager: &Manager) -> Result<String, String> {
    let snapshot = manager.refresh_snapshot().map_err(text)?;
    manager.refresh_upstream().map_err(text)?;
    Ok(match snapshot.origin {
        SnapshotOrigin::Online => "已经检查过上游清单".to_string(),
        SnapshotOrigin::Cache => "在线清单不可用，用的是本地缓存的上一次结果。".to_string(),
    })
}

/// 执行用户已经确认过的计划。
///
/// RDR2 需要管理员的那三步交给提权 helper，游戏目录里的部分仍在普通权限下写——
/// **提权范围要尽可能小。**
fn apply_prepared(
    manager: &Manager,
    tx: &Sender<JobEvent>,
    game: GameId,
    prepared: PreparedInstall,
) -> Result<String, String> {
    let mut backend = manager.backend_for(game).map_err(text)?;

    if prepared.plan.requires_admin && !elevation::is_process_elevated() {
        progress(tx, game, "等待管理员授权", 0.55);
        let outcome = elevation::run_with_uac(
            &manager.paths,
            game,
            ElevationAction::Apply,
            ELEVATION_TIMEOUT,
        )
        .map_err(|e| e.to_string())?;
        if !outcome.ok {
            return Err(outcome.message);
        }
        progress(tx, game, "正在写入游戏目录", 0.85);
        manager
            .apply(prepared.without_elevated_actions(), &mut backend)
            .map_err(text)?;
    } else {
        progress(tx, game, "正在写入游戏目录", 0.7);
        manager.apply(prepared, &mut backend).map_err(text)?;
    }
    Ok("装好了，进入游戏后再看实际画面".to_string())
}

/// 全局 layer 和注册表值要管理员才能改回去，整份还原交给提权 helper。
///
/// **要不要提权由配方说了算**（只有 Vulkan 方案需要），不再按游戏名写死。
fn restore(manager: &Manager, game: GameId) -> Result<String, String> {
    let needs_admin = manager
        .profile_for(game)
        .map(|profile| vivid_core::recipe::requires_admin(profile.api))
        .unwrap_or(false);
    if needs_admin && !elevation::is_process_elevated() {
        let outcome = elevation::run_with_uac(
            &manager.paths,
            game,
            ElevationAction::Restore,
            ELEVATION_TIMEOUT,
        )
        .map_err(|e| e.to_string())?;
        return if outcome.ok {
            Ok(outcome.message)
        } else {
            Err(outcome.message)
        };
    }
    let mut backend = manager.backend_for(game).map_err(text)?;
    manager.restore(game, &mut backend).map_err(text)?;
    Ok("已经还原到 Vivid 接管之前的状态".to_string())
}

fn progress(tx: &Sender<JobEvent>, game: GameId, message: &str, percent: f32) {
    let _ = tx.send(JobEvent::Progress {
        game,
        message: message.to_string(),
        percent,
    });
}

/// 把 egui 交回来的帧缓冲存成 PNG。版式自检专用。
fn save_screenshot(image: &egui::ColorImage, path: &std::path::Path) {
    let [width, height] = [image.width() as u32, image.height() as u32];
    let mut bytes = Vec::with_capacity(image.pixels.len() * 4);
    for pixel in &image.pixels {
        bytes.extend_from_slice(&pixel.to_array());
    }
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Some(buffer) = image::RgbaImage::from_raw(width, height, bytes) {
        let _ = buffer.save(path);
    }
}

/// 错误原文原样带走。**这里没有任何把失败兜成成功的路径。**
fn text<E: std::fmt::Display>(error: E) -> String {
    error.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_checks_never_claim_the_app_is_current_and_manual_checks_explain_why() {
        let mut home = Home::new();
        home.update_prompt = crate::update_prompt::UpdatePrompt::new(None);
        home.update_manual = false;
        home.receive_update(Err(update::UpdateError::Network("offline".to_string())));
        assert!(home.update.is_none());
        assert!(!home.update_checked);
        assert!(home.update_error.is_none());
        home.update_manual = true;
        home.receive_update(Err(update::UpdateError::Malformed(
            "bad manifest".to_string(),
        )));
        assert!(home.update_error.as_ref().unwrap().contains("bad manifest"));
        assert!(!home.update_checked);
        assert!(!home.update_prompt.is_open());
        home.receive_update(Ok(None));
        assert!(home.update_error.is_none());
        assert!(home.update_checked);
    }

    #[test]
    fn failed_background_result_does_not_overwrite_error_status() {
        let mut home = Home::new();
        let (tx, rx) = mpsc::channel();
        home.update_manual = true;
        home.update_rx = Some(rx);
        tx.send(Err(update::UpdateError::Network("offline".to_string())))
            .unwrap();
        home.drain_events();
        assert!(!home.update_checked);
        assert!(home.update_rx.is_none());
        assert!(home.update_error.as_ref().unwrap().contains("offline"));
    }

    #[test]
    fn themed_screenshot_preview_cannot_start_network_jobs() {
        let argv: Vec<String> = ["Vivid", "--screenshot", "unused.png", "--theme", "light"]
            .into_iter()
            .map(str::to_owned)
            .collect();
        let mut home = Home::new().with_screenshot(crate::screenshot_request(&argv));
        assert!(home.started);
        home.check_update(&egui::Context::default(), false);
        assert!(home.update_rx.is_none());
        assert!(home.scan_rx.is_none());
        assert!(home.games.is_empty());
    }

    /// 搜索是缩小范围用的：空词放行所有，大小写不敏感。
    #[test]
    fn the_search_box_narrows_without_making_the_user_guess_case() {
        assert!(name_matches("Red Dead Redemption 2", ""));
        assert!(name_matches("Red Dead Redemption 2", "red dead"));
        assert!(name_matches("Counter-Strike 2", "strike"));
        assert!(!name_matches("Counter-Strike 2", "rdr"));
    }

    /// 左栏那一行只放得下一小段路径，取最后两级。
    #[test]
    fn the_nav_subtitle_keeps_the_last_two_path_segments() {
        let path = std::path::Path::new(r"H:\Steam\steamapps\common\Red Dead Redemption 2");
        assert_eq!(short_path(path), r"common\Red Dead Redemption 2");
    }

    /// 不给装的三种状态都归到「不提供安装」那一组，**不从列表里抹掉**。
    #[test]
    fn every_blocked_state_lands_in_the_blocked_group() {
        for state in [
            GameState::BlockedByAntiCheat {
                reason: "检测到 EasyAntiCheat".to_string(),
            },
            GameState::UnknownApi,
            GameState::NotAGame,
            GameState::NotFound,
        ] {
            let report = GameInspection {
                game: None,
                gpu: None,
                state,
                files_installed: false,
                load_evidence: vivid_core::evidence::LoadEvidence::NoLog,
                snapshot_origin: None,
                detail_lines: Vec::new(),
            };
            assert_eq!(Home::group_of(&report), NavGroup::Blocked);
            assert_eq!(report.primary_action(), PrimaryAction::None);
        }
    }
}
