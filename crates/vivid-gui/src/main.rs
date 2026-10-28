//! 锐境 Vivid —— 图形界面的入口。
//!
//! 这个文件只留四件事：**提权 helper 的命令行入口**、窗口与图标、中文字体与主题、
//! 以及作者信息那几个常量。真正的界面在 `home.rs`。
//!
//! **动界面前先读 `docs/ui-style-guide.md` 和 `ui.rs`。** 三条最容易违反的：
//! 禁止用 `egui::Grid` 排内容列表、高饱和品牌色一屏只出现一处填充、
//! 常驻说明文字不用警告色。CLAUDE.md 第五·五节钉着这三条。
//!
//! 启动顺序是固定的：解析提权参数（命中就干完那件事直接退出）→ 收拾上次
//! 被打断的操作 → 后台刷新清单并体检两款游戏 → 渲染首页 → 轮询任务结果。

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod about;
mod elevation;
mod home;
mod jobs;
mod links;
mod social_icons;
mod ui;
mod update_prompt;

use std::sync::Arc;

use eframe::egui;
use vivid_core::{NAME_EN, WINDOW_TITLE};

fn main() -> eframe::Result {
    // 提权 helper 入口：只做 RDR2 全局 Vulkan layer 那一件事，不起 GUI。
    // 命令行形状不对就直接退出，绝不退化成「照着参数干点别的」。
    let argv: Vec<String> = std::env::args().collect();
    if argv.iter().any(|arg| arg == elevation::ARG_ELEVATED_APPLY) {
        let code = match elevation::parse_cli(&argv) {
            Some(request) => elevation::run_elevated_apply(&request),
            None => elevation::EXIT_BAD_ARGS,
        };
        std::process::exit(code);
    }

    // 版式自检入口（**只在 debug 构建里存在**，发布构建一行都编不进去）。
    let shot = screenshot_request(&argv);
    let size = shot.as_ref().map_or([1060.0, 720.0], |s| s.size);

    let mut viewport = egui::ViewportBuilder::default()
        .with_title(WINDOW_TITLE)
        .with_inner_size(size)
        .with_min_inner_size([860.0, 540.0])
        .with_drag_and_drop(true); // 导入 DLL 靠拖放，必须显式开

    if let Some(icon) = load_icon() {
        viewport = viewport.with_icon(icon);
    }

    eframe::run_native(
        // app id 用纯英文：它会落到窗口类名和配置目录上，中文在那些地方是麻烦。
        NAME_EN,
        eframe::NativeOptions {
            viewport,
            ..Default::default()
        },
        Box::new(|cc| {
            install_chinese_font(&cc.egui_ctx);
            ui::install_theme(&cc.egui_ctx);
            if let Some(request) = &shot {
                if let Some(theme) = request.theme {
                    cc.egui_ctx.set_theme(theme);
                }
                cc.egui_ctx.set_pixels_per_point(request.scale);
                cc.egui_ctx
                    .send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(
                        request.size[0],
                        request.size[1],
                    )));
            }
            Ok(Box::new(home::Home::new().with_screenshot(shot)))
        }),
    )
}

/// 版式自检的一次截图请求。
///
/// **为什么需要它**：这台开发机上的窗口内容截不到（wgpu 的交换链不进
/// `CopyFromScreen` 能拿到的那层），所以没法靠外部截图确认「1060×720 与
/// 860×540 都不重叠不截断」。让程序自己把帧缓冲存成 PNG 是唯一可靠的办法。
///
/// 只在 debug 构建里解析：发布的 exe 里**不存在**这条命令行。
#[derive(Debug, Clone)]
pub struct ScreenshotRequest {
    pub path: std::path::PathBuf,
    pub size: [f32; 2],
    /// 保留首次启动说明（默认关掉它，好看清主界面）。
    pub welcome: bool,
    /// 强制选中某个 Steam App ID 的游戏页（用来拍「不提供安装」那种页面）。
    pub select: Option<u32>,
    pub theme: Option<egui::ThemePreference>,
    pub coffee: bool,
    pub zoom: bool,
    pub scale: f32,
    pub update_prompt: bool,
    pub update_checking: bool,
}

/// 解析 `--screenshot <path.png> [--size <宽>x<高>]`。
#[cfg(debug_assertions)]
fn screenshot_request(argv: &[String]) -> Option<ScreenshotRequest> {
    let index = argv.iter().position(|arg| arg == "--screenshot")?;
    let path = std::path::PathBuf::from(argv.get(index + 1)?);
    let size = argv
        .iter()
        .position(|arg| arg == "--size")
        .and_then(|i| argv.get(i + 1))
        .and_then(|text| {
            let (w, h) = text.split_once('x')?;
            Some([w.parse().ok()?, h.parse().ok()?])
        })
        .unwrap_or([1060.0, 720.0]);
    Some(ScreenshotRequest {
        path,
        size,
        theme: argv
            .iter()
            .position(|a| a == "--theme")
            .and_then(|i| argv.get(i + 1))
            .map(|s| {
                if s == "light" {
                    egui::ThemePreference::Light
                } else {
                    egui::ThemePreference::Dark
                }
            }),
        coffee: argv.iter().any(|a| a == "--coffee"),
        zoom: argv.iter().any(|a| a == "--zoom"),
        update_prompt: argv.iter().any(|a| a == "--update-prompt"),
        update_checking: argv.iter().any(|a| a == "--update-checking"),
        scale: argv
            .iter()
            .position(|a| a == "--scale")
            .and_then(|i| argv.get(i + 1))
            .and_then(|s| s.parse().ok())
            .unwrap_or(1.0),
        welcome: argv.iter().any(|arg| arg == "--welcome"),
        select: argv
            .iter()
            .position(|arg| arg == "--select")
            .and_then(|i| argv.get(i + 1))
            .and_then(|text| text.parse().ok()),
    })
}

#[cfg(not(debug_assertions))]
fn screenshot_request(_argv: &[String]) -> Option<ScreenshotRequest> {
    None
}

/// 窗口与任务栏图标。
///
/// 内嵌的是我们**自己的** logo，不是第三方版权物——这跟不能内嵌 NVIDIA DLL、
/// 不能内嵌商业字体是两回事。
fn load_icon() -> Option<egui::IconData> {
    let rgba = image::load_from_memory(include_bytes!("../assets/icon.png"))
        .ok()?
        .to_rgba8();

    // 源图四周有透明留白（导出画布比图形本身大，还带一圈外发光）。
    // 不裁掉的话，缩进 32px 的任务栏格子里，真正的图形只占中间一小块
    // ——看起来就是「图标变小了」。
    let cropped = crop_to_opaque(&rgba);

    // 补成正方形再缩放：源图不是正方，直接等尺寸缩放会把圆角压成椭圆。
    let square = pad_to_square(&cropped);

    // 给 256 而不是 64：Windows 会按需缩到 16/24/32/48，从大图降采样比从小图放大清楚。
    let sized = image::imageops::resize(&square, 256, 256, image::imageops::FilterType::Lanczos3);
    let (width, height) = (sized.width(), sized.height());
    Some(egui::IconData {
        rgba: sized.into_raw(),
        width,
        height,
    })
}

/// 裁掉四周完全透明的边。
fn crop_to_opaque(img: &image::RgbaImage) -> image::RgbaImage {
    /// 极淡的外发光不算内容，否则等于没裁。
    const ALPHA_FLOOR: u8 = 8;

    let (w, h) = img.dimensions();
    let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0u32, 0u32);
    for y in 0..h {
        for x in 0..w {
            if img.get_pixel(x, y)[3] > ALPHA_FLOOR {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x + 1);
                y1 = y1.max(y + 1);
            }
        }
    }
    if x1 <= x0 || y1 <= y0 {
        return img.clone(); // 整张都是透明的，原样返回
    }
    image::imageops::crop_imm(img, x0, y0, x1 - x0, y1 - y0).to_image()
}

/// 居中补透明边成正方形。非等比缩放会把圆角压变形。
fn pad_to_square(img: &image::RgbaImage) -> image::RgbaImage {
    let (w, h) = img.dimensions();
    if w == h {
        return img.clone();
    }
    let side = w.max(h);
    let mut out = image::RgbaImage::from_pixel(side, side, image::Rgba([0, 0, 0, 0]));
    image::imageops::overlay(
        &mut out,
        img,
        ((side - w) / 2) as i64,
        ((side - h) / 2) as i64,
    );
    out
}

/// 装中文字体。
///
/// 主字体是**内嵌**的思源圆体（Resource Han Rounded CN Medium）：圆角字形与 logo 的
/// 圆角矩形、圆润山峰同气质，比微软雅黑更贴这个产品。
///
/// **内嵌它和「不能内嵌微软雅黑」并不矛盾**——区别不在内不内嵌，而在授权允不允许
/// 再分发。雅黑、苹方是商业字体，授权不含再分发；思源圆体是 SIL OFL 1.1，明确允许。
/// 授权声明与许可证全文见 `assets/FONT-LICENSE.md` 与 `assets/OFL.txt`，
/// **那两份文件必须随发行包一起分发**。
///
/// 系统字体接在后面作**缺字兜底**：思源圆体 CN 是简中子集，遇到繁体或生僻的游戏名
/// 可能缺字，那时 egui 会顺着字体链往后找。一个系统字体都读不到也不 panic——
/// 界面会有方块，但工具还能用，而崩溃会让用户连「出了什么事」都不知道。
fn install_chinese_font(ctx: &egui::Context) {
    /// 内嵌主字体。SIL OFL 1.1，见 `assets/FONT-LICENSE.md`。
    const EMBEDDED: &[u8] = include_bytes!("../assets/font.ttf");

    /// 缺字兜底用的系统字体，按优先级排列。**只读，不随发行包分发。**
    const SYSTEM_FALLBACK: &[&str] = &[
        r"C:\Windows\Fonts\msyh.ttc",   // 微软雅黑
        r"C:\Windows\Fonts\simhei.ttf", // 黑体
        r"C:\Windows\Fonts\Deng.ttf",   // 等线
        r"C:\Windows\Fonts\simsun.ttc", // 宋体，最后的退路
    ];

    let mut fonts = egui::FontDefinitions::default();
    let mut chain = Vec::new();

    fonts.font_data.insert(
        "vivid_rounded".to_owned(),
        Arc::new(egui::FontData::from_static(EMBEDDED)),
    );
    chain.push("vivid_rounded".to_owned());

    if let Some(bytes) = SYSTEM_FALLBACK.iter().find_map(|p| std::fs::read(p).ok()) {
        fonts.font_data.insert(
            "system_cjk".to_owned(),
            Arc::new(egui::FontData::from_owned(bytes)),
        );
        chain.push("system_cjk".to_owned());
    }

    // 正文：圆体优先。
    let prop = fonts
        .families
        .entry(egui::FontFamily::Proportional)
        .or_default();
    for (i, name) in chain.iter().enumerate() {
        prop.insert(i, name.clone());
    }

    // 等宽：**追加到末尾**，不是插到最前。
    // 版本号要等宽对齐（规范 §7），得让 egui 自带的等宽西文字体优先命中数字；
    // 中文字符它没有，才顺着链往后落到圆体上。插到最前会让数字失去等宽。
    let mono = fonts
        .families
        .entry(egui::FontFamily::Monospace)
        .or_default();
    mono.extend(chain.iter().cloned());

    ctx.set_fonts(fonts);
}

/// 带字间距的文字。
///
/// egui 没有 letter-spacing，逐字画 + 加大字间距是等价做法。
/// 用在英文名上——logo 品牌板里的英文名就是展开排的，跟着它走。
pub(crate) fn tracked_text(
    ui: &mut egui::Ui,
    text: &str,
    size: f32,
    color: egui::Color32,
    tracking: f32,
) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = tracking;
        for c in text.chars() {
            ui.label(egui::RichText::new(c).size(size).color(color));
        }
    });
}

/// 品牌 slogan。
///
/// 说的是**用户的心理状态**而不是功能清单：这个工具的价值不在于「有还原按钮」，
/// 而在于还原按钮让人敢动手（CLAUDE.md 第三节的判据）。
/// 「常新」也比「焕新」准——DLSS 版本会一直更新，这不是一次性动作。
const SLOGAN: &str = "画质常新，安心去试。";
