//! **这是应用自己的组件库**：允许存在暂时没有调用方的原语，
//! 换掉一个页面不该逼着删掉一组通用组件。
#![allow(dead_code)]

//! 设计系统 —— 1U 设计语言的 egui 落地。规范见 `docs/ui-style-guide.md`。
//!
//! > **一屏只有一个卡层级；卡里的东西一律扁平，靠底色分轻重，不靠画线。**
//!
//! 用这里的函数，别自己搭。自己搭一份等价物就是「拼接感」的唯一来源——
//! 宽度、间距、材质会各漂各的，一屏看着就像从两个软件里拼出来的。
//!
//! **配色沿用 main.rs 那套 logo 品牌色**（青→紫），这里不另立一套。
//! 合并时把 main.rs 的颜色常量删掉、改成 `use crate::ui::color::*` 即可。

// egui 经 eframe 再导出，本 crate 没有直接依赖它。
use eframe::egui::{
    self, Color32, CornerRadius, Margin, Response, RichText, Sense, Stroke, Ui, UiBuilder, Vec2,
};

// ============================================================================
// 颜色：与 main.rs 的品牌色同源，数值一字不改
// ============================================================================

// 调色板提供完整的语义色，当前版式用不到全部——这不是死代码，
// 是设计系统该有的完整性。删掉某一个会让下次用到时又去猜色值。
// 调色板提供完整的语义色，当前版式用不到全部——这不是死代码，
// 是设计系统该有的完整性。删掉某一个会让下次用到时又去猜色值。
#[allow(dead_code)]
pub mod color {
    use eframe::egui::Color32;

    // ------------------------------------------------------------------
    // 背景：中性灰，靠明度递进分层。
    //
    // **刻意不带色相。** 早先用的是 GitHub Dark 那一系偏蓝的黑
    // （#0E1116 / #161B22 / #1A1F27），两个问题：色相让整屏发闷，
    // 相邻层之间明度差又太小，看不出层次。
    // macOS 的深色是纯中性灰，层次一眼可辨——这里照它走。
    // ------------------------------------------------------------------

    /// L0 侧栏底。比内容区更深，这样两栏不靠分隔线也分得开。
    pub const SIDEBAR: Color32 = Color32::from_rgb(0x17, 0x17, 0x19);
    /// L0 窗口 / 内容区底
    pub const BG: Color32 = Color32::from_rgb(0x1C, 0x1C, 0x1E);
    /// L1 区块面 —— 页面唯一的卡层级
    pub const PANEL: Color32 = Color32::from_rgb(0x2C, 0x2C, 0x2E);
    /// L2 内容块（分组容器、瓷砖）。**无边框**，只有底色。
    pub const CARD: Color32 = Color32::from_rgb(0x2C, 0x2C, 0x2E);
    /// L2 hover 底
    pub const CARD_HOVER: Color32 = Color32::from_rgb(0x3A, 0x3A, 0x3C);
    /// 边框。深色下面与面只差一点点，这圈线目前有功能，别单方面摘。
    pub const LINE: Color32 = Color32::from_rgb(0x38, 0x38, 0x3A);
    /// 行间分隔线。**刻意做得很弱**：它只提示「这是两行」，
    /// 不该在页面上形成一道道横杠。macOS 的 separator 也几乎看不见。
    pub const DIVIDER: Color32 = Color32::from_rgb(0x32, 0x32, 0x34);

    // ------------------------------------------------------------------
    // 文字：白色的不同强度，而不是几个不同的灰色值。
    // 同一个色相下拉开明度，层次才干净——这也是 macOS 的做法
    // （labelColor / secondaryLabelColor / tertiaryLabelColor）。
    // ------------------------------------------------------------------

    pub const FG: Color32 = Color32::from_rgb(0xFF, 0xFF, 0xFF);
    /// 次级正文
    pub const FG2: Color32 = Color32::from_rgb(0xD8, 0xD8, 0xDC);
    /// 说明、副行
    pub const DIM: Color32 = Color32::from_rgb(0x98, 0x98, 0x9D);
    /// 最弱一级：占位、禁用
    pub const DIM2: Color32 = Color32::from_rgb(0x63, 0x63, 0x66);

    // ------------------------------------------------------------------
    // 品牌与强调
    // ------------------------------------------------------------------

    /// 品牌青（logo 上那个）。**只用于文字与细节，不做大面积填充**——
    /// 它亮度很高，整块铺开会盖过内容本身。
    pub const BRAND_CYAN: Color32 = Color32::from_rgb(0x22, 0xD3, 0xEE);
    pub const BRAND_PURPLE: Color32 = Color32::from_rgb(0xA8, 0x55, 0xF7);

    /// 主按钮填充。品牌青降到能稳稳配住白字的亮度。
    ///
    /// 为什么不直接用 `BRAND_CYAN` 填充：那个亮度在深色界面里像个警报器。
    /// macOS 的 accent 按钮也不是纯饱和色。
    pub const ACCENT: Color32 = Color32::from_rgb(0x0E, 0x9D, 0xBB);
    pub const ACCENT_HOVER: Color32 = Color32::from_rgb(0x14, 0xAF, 0xCF);
    /// 选中态填充：accent 压到底色附近，看得出选中又不抢内容。
    pub const ACCENT_SOFT: Color32 = Color32::from_rgb(0x16, 0x33, 0x3C);

    // ------------------------------------------------------------------
    // 状态：直接用 macOS 的系统色，它们在深色下的可读性是调过的
    // ------------------------------------------------------------------

    /// systemGreen (dark)
    pub const OK: Color32 = Color32::from_rgb(0x30, 0xD1, 0x58);
    /// systemRed (dark)
    pub const BAD: Color32 = Color32::from_rgb(0xFF, 0x45, 0x3A);
    /// systemOrange (dark)
    pub const WARN: Color32 = Color32::from_rgb(0xFF, 0x9F, 0x0A);

    /// 状态族淡底（徽章）。压得很暗，只做底衬不做强调。
    pub const CYAN_BG: Color32 = Color32::from_rgb(0x16, 0x33, 0x3C);
    pub const OK_BG: Color32 = Color32::from_rgb(0x14, 0x2E, 0x1B);
    pub const WARN_BG: Color32 = Color32::from_rgb(0x33, 0x27, 0x0E);
    pub const BAD_BG: Color32 = Color32::from_rgb(0x35, 0x18, 0x18);
}

#[derive(Clone, Copy)]
pub struct Palette {
    pub bg: Color32,
    pub panel: Color32,
    pub sidebar: Color32,
    pub text: Color32,
    pub muted: Color32,
    pub hover: Color32,
    pub separator: Color32,
    pub accent: Color32,
}

pub fn palette(ui: &Ui) -> Palette {
    if ui.visuals().dark_mode {
        Palette {
            bg: color::BG,
            panel: color::PANEL,
            sidebar: color::SIDEBAR,
            text: color::FG,
            muted: color::DIM,
            hover: color::CARD_HOVER,
            separator: color::DIVIDER,
            accent: color::BRAND_CYAN,
        }
    } else {
        Palette {
            bg: Color32::from_rgb(245, 245, 247),
            panel: Color32::WHITE,
            sidebar: Color32::from_rgb(236, 236, 238),
            text: Color32::from_rgb(29, 29, 31),
            muted: Color32::from_rgb(110, 110, 115),
            hover: Color32::from_rgb(237, 237, 240),
            separator: Color32::from_black_alpha(20),
            accent: color::ACCENT,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Neutral,
    Brand,
    Ok,
    Warn,
    Bad,
}

impl Tone {
    pub fn fg(self) -> Color32 {
        match self {
            Tone::Neutral => color::DIM,
            Tone::Brand => color::BRAND_CYAN,
            Tone::Ok => color::OK,
            Tone::Warn => color::WARN,
            Tone::Bad => color::BAD,
        }
    }
    pub fn bg(self) -> Color32 {
        match self {
            Tone::Neutral => color::CARD,
            Tone::Brand => color::CYAN_BG,
            Tone::Ok => color::OK_BG,
            Tone::Warn => color::WARN_BG,
            Tone::Bad => color::BAD_BG,
        }
    }
}

pub const RADIUS_CARD: u8 = 12;
pub const RADIUS_CONTROL: u8 = 8;
/// 行的上下内边距。行高由内容加这个padding决定，不设固定高度——
/// 规范 §3 说的「行高由内容决定」就是这个意思。
pub const ROW_PAD_Y: i8 = 9;

// ============================================================================
// L1：区块（页面唯一的卡层级）
// ============================================================================

/// **铁律：section 内部禁止再放 section。** 同色边框叠同色边框，零对比度增益。
/// 内容块一律用 [`group`]（无边框淡底）。
pub fn section<R>(ui: &mut Ui, content: impl FnOnce(&mut Ui) -> R) -> R {
    egui::Frame::new()
        .fill(palette(ui).panel)
        .stroke(Stroke::new(1.0, color::LINE))
        .corner_radius(CornerRadius::same(RADIUS_CARD))
        .inner_margin(Margin::same(14))
        .show(ui, content)
        .inner
}

// ============================================================================
// ① 结论层：整屏唯一强调，最多一个
// ============================================================================

/// 这一屏在回答用户什么问题——答案放这里，主操作放 `actions`（右侧）。
///
/// **整屏最多一个**：出现第二个，「哪里最重要」就没有答案了。
pub fn verdict(ui: &mut Ui, tone: Tone, title: &str, desc: &str, actions: impl FnOnce(&mut Ui)) {
    egui::Frame::new()
        // **一律中性底**，状态交给标题前那个小圆点。
        // 早先按 tone 给整块上色，结果是「青底 + 青标题 + 青按钮」三层叠在一起，
        // 视觉重量压过了内容。macOS 从不用大面积彩色背景表达状态。
        .fill(palette(ui).panel)
        .stroke(Stroke::new(1.0, color::LINE))
        .corner_radius(CornerRadius::same(RADIUS_CARD))
        .inner_margin(Margin::symmetric(16, 14))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        status_dot(ui, tone.fg());
                        ui.add_space(2.0);
                        ui.label(
                            RichText::new(title)
                                .size(15.0)
                                .strong()
                                .color(palette(ui).text),
                        );
                    });
                    if !desc.is_empty() {
                        ui.add_space(5.0);
                        ui.horizontal(|ui| {
                            ui.add_space(16.0); // 和圆点对齐
                            ui.label(RichText::new(desc).size(12.5).color(palette(ui).muted));
                        });
                    }
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), actions);
            });
        });
}

/// 状态圆点。
///
/// macOS 用小圆点表达状态，而不是给整块区域上色——8px 的一个点足够传达
/// 「这是什么状态」，又不会把周围的文字挤到次要位置。
fn status_dot(ui: &mut Ui, c: Color32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(10.0), Sense::hover());
    ui.painter().circle_filled(rect.center(), 4.0, c);
}

// ============================================================================
// ②③ 分组 + 行：替代 egui::Grid
// ============================================================================

/// 组前的小节标题。**不是卡，就是一行灰字。**
///
/// 改成左导航 + 右详情之后，游戏名成了导航项，这个函数暂时没有调用点。
/// 保留而不是删除：它是规范 §3「组 + 行」三件套之一，
/// 详情页里再出现需要分节的内容时直接用，不必重新发明。
#[allow(dead_code)]
pub fn group_label(ui: &mut Ui, text: &str, trailing: &str) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(text)
                .size(13.0)
                .strong()
                .color(palette(ui).text),
        );
        if !trailing.is_empty() {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(RichText::new(trailing).size(12.0).color(palette(ui).muted));
            });
        }
    });
    ui.add_space(6.0);
}

/// 左栏的搜索框。
///
/// 主题里 `extreme_bg_color` 和左栏底色是同一个值，直接扔一个 `TextEdit` 进去
/// **完全看不见边界**（2026-09-01 版式自检截图抓到的）。按规范
/// 「靠底色分轻重，不靠画线」，给它一层 L2 底色，而不是画一圈边框。
pub fn search_field(ui: &mut Ui, hint: &str, text: &mut String) -> Response {
    egui::Frame::new()
        .fill(palette(ui).panel)
        .corner_radius(CornerRadius::same(RADIUS_CONTROL))
        .inner_margin(Margin::symmetric(10, 6))
        .show(ui, |ui| {
            ui.add(
                egui::TextEdit::singleline(text)
                    .background_color(palette(ui).panel)
                    .hint_text(hint)
                    .desired_width(f32::INFINITY)
                    .font(egui::FontId::proportional(12.5)),
            )
        })
        .inner
}

/// 一组行的淡底容器。**只有底色，不画边框**——上面已经有 L1 的边框负责区域感。
pub fn group<R>(ui: &mut Ui, content: impl FnOnce(&mut Ui) -> R) -> R {
    egui::Frame::new()
        .fill(palette(ui).panel)
        .corner_radius(CornerRadius::same(RADIUS_CARD))
        // 留 4px 内边距：行的 hover 底色是个带圆角的矩形，顶到容器边缘时
        // 两层圆角会互相咬，看着像把容器切开了。内缩一点，行就干净地浮在容器里。
        .inner_margin(Margin::same(4))
        .show(ui, content)
        .inner
}

/// 一行：左边主标题 + 副行小字，右边随便放（按钮 / 徽章 / 状态字）。
///
/// **整行都有 hover 反馈**，不是只有右边的按钮才有——这是核心手感。
///
/// # egui 即时模式的坑（这个函数存在的主要理由）
///
/// 想给一行画 hover 底色，得先知道行高；但行高要等内容画完才知道。
/// 直接在内容之后 `rect_filled` 会把背景盖在文字**上面**。
///
/// 解法是**占位再回填**：先 `painter().add(Shape::Noop)` 占一个绘制槽，内容画完拿到
/// rect，再 `painter().set(idx, ...)` 填回那个槽。槽在内容之前，所以背景永远在文字下面。
/// 自己写行时照抄这个模式，别用别的写法。
pub fn row(ui: &mut Ui, title: &str, sub: &str, right: impl FnOnce(&mut Ui)) -> Response {
    let bg_idx = ui.painter().add(egui::Shape::Noop);

    let inner = ui.scope_builder(UiBuilder::new().sense(Sense::hover()), |ui| {
        // 用上下对称的内边距撑高度。
        //
        // 两个写法都试错过，别再走回去：
        // - `set_min_height` + `horizontal`：内容贴在区域上沿，下面空一截。
        // - `set_min_height` + `horizontal_centered`：`horizontal_centered` 会占掉
        //   当前 Ui 的**全部剩余高度**再居中，在 ScrollArea 里一行就吃掉半个屏幕。
        //
        // 对称 margin 两个毛病都没有：`horizontal` 内部本来就是垂直居中对齐的，
        // 高度也只由内容决定。
        egui::Frame::new()
            .inner_margin(Margin::symmetric(0, ROW_PAD_Y))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.add_space(12.0);
                    ui.vertical(|ui| {
                        ui.label(RichText::new(title).size(13.5).color(palette(ui).text));
                        if !sub.is_empty() {
                            ui.add_space(2.0);
                            ui.label(RichText::new(sub).size(12.0).color(palette(ui).muted));
                        }
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.add_space(12.0);
                        right(ui);
                    });
                });
            });
    });

    if inner.response.hovered() {
        ui.painter().set(
            bg_idx,
            egui::epaint::RectShape::filled(
                inner.response.rect,
                CornerRadius::same(RADIUS_CONTROL),
                palette(ui).hover,
            ),
        );
    }
    inner.response
}

/// 相邻行之间的分隔线。**只在行与行之间调用**，组的首尾不画。
/// 左右各留 12px：顶到容器边缘的线看着像把容器切开了。
pub fn row_divider(ui: &mut Ui) {
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(w, 1.0), Sense::hover());
    ui.painter().hline(
        (rect.left() + 8.0)..=(rect.right() - 8.0),
        rect.center().y,
        Stroke::new(1.0, palette(ui).separator),
    );
}

// ============================================================================
// 小件
// ============================================================================

/// 状态标记。**它本身就是状态，别再在旁边写一遍文字**（状态只说一遍）。
pub fn badge(ui: &mut Ui, tone: Tone, text: &str) {
    egui::Frame::new()
        .fill(tone.bg())
        .corner_radius(CornerRadius::same(6))
        .inner_margin(Margin::symmetric(8, 3))
        .show(ui, |ui| {
            ui.label(RichText::new(text).size(11.5).color(tone.fg()));
        });
}

/// 只读状态文字。与 [`badge`] 的区别：**没有底色**。
///
/// 一行的右侧如果没有可点的操作，用这个而不是 badge——带底色的小方块会被当成按钮，
/// 用户点上去没反应，是很实的挫败感。底色留给真能点的东西。
pub fn status_text(ui: &mut Ui, tone: Tone, text: &str) {
    ui.label(RichText::new(text).size(12.5).color(tone.fg()));
}

/// 主操作按钮。**一屏之内只该有一处**，多了就没有主次。
pub fn primary_button(ui: &mut Ui, text: &str) -> Response {
    let r = ui.add(
        egui::Button::new(
            RichText::new(text)
                .size(12.5)
                .strong()
                .color(Color32::WHITE),
        )
        .fill(color::ACCENT)
        .corner_radius(CornerRadius::same(RADIUS_CONTROL)),
    );
    if r.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    r
}

/// 行内操作按钮：淡底 + 品牌青文字。**不填充高饱和色**——
/// 五个填充按钮排成一列就是一堵墙，主次全失。
pub fn row_button(ui: &mut Ui, text: &str) -> Response {
    let r = ui.add(
        egui::Button::new(RichText::new(text).size(12.0).color(color::BRAND_CYAN))
            .fill(color::CYAN_BG)
            .corner_radius(CornerRadius::same(RADIUS_CONTROL)),
    );
    if r.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    r
}

/// 次要按钮：淡底，hover 换底。
pub fn ghost_button(ui: &mut Ui, text: &str) -> Response {
    ui.add(
        egui::Button::new(RichText::new(text).size(12.0).color(palette(ui).text))
            .fill(palette(ui).hover)
            .corner_radius(CornerRadius::same(RADIUS_CONTROL)),
    )
}

/// 左侧导航项：主标题 + 副行，右侧可挂一个状态标记。
///
/// 选中态靠**底色 + 左侧竖条**表达，不用边框——边框会和外层容器的边框打架，
/// 而且深色下同色边框叠同色边框等于没有对比度增益（规范 §4 的铁律）。
///
/// 未选中且未悬停时**完全透明**：一列导航项如果各自带底色，
/// 就成了一堆并排的小卡片，视觉噪音比信息还多。
pub fn nav_item(
    ui: &mut Ui,
    selected: bool,
    title: &str,
    sub: &str,
    tag: Option<(Tone, &str)>,
) -> Response {
    let bg_idx = ui.painter().add(egui::Shape::Noop);

    let inner = ui.scope_builder(UiBuilder::new().sense(Sense::click()), |ui| {
        ui.set_width(ui.available_width());
        egui::Frame::new()
            .inner_margin(Margin::symmetric(10, 7))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.add_space(4.0);
                    ui.vertical(|ui| {
                        ui.label(
                            RichText::new(title)
                                .size(12.5)
                                .color(palette(ui).text)
                                .strong(),
                        );
                        if !sub.is_empty() {
                            ui.add_space(1.0);
                            ui.label(RichText::new(sub).size(11.0).color(palette(ui).muted));
                        }
                    });
                    if let Some((tone, text)) = tag {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(RichText::new(text).size(11.0).color(tone.fg()));
                        });
                    }
                });
            });
    });

    let rect = inner.response.rect;
    let bg = if selected {
        if ui.visuals().dark_mode {
            color::ACCENT_SOFT
        } else {
            Color32::from_rgb(218, 233, 237)
        }
    } else if inner.response.hovered() {
        palette(ui).hover
    } else {
        Color32::TRANSPARENT
    };
    // 选中就是一整块圆角填充，**不加左侧竖条**——竖条是 Web / Windows 的习惯，
    // macOS 侧栏（Finder、系统设置）一律是整块圆角高亮，更整、更安静。
    if bg != Color32::TRANSPARENT {
        ui.painter().set(
            bg_idx,
            egui::epaint::RectShape::filled(rect, CornerRadius::same(RADIUS_CONTROL), bg),
        );
    }

    if inner.response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    inner.response
}

/// 导航区里的分组小标题。比 [`group_label`] 更弱，用于左栏分节。
pub fn nav_section(ui: &mut Ui, text: &str) {
    ui.add_space(10.0);
    ui.horizontal(|ui| {
        ui.add_space(14.0);
        ui.label(RichText::new(text).size(10.5).color(palette(ui).muted));
    });
    ui.add_space(4.0);
}

/// 可点击的入口卡片：圆形徽标 + 主标题 + 副标题 + 右侧箭头。
///
/// 整卡可点，hover 换底——和 [`row`] 同一套手感，只是横向排布、自带底色。
/// `enabled` 为 false 时置灰且不响应点击：**没有确切链接时宁可让它点不动，
/// 也不给一个跳到错误地址的入口**。
pub fn link_card(ui: &mut Ui, mark: &str, title: &str, sub: &str, enabled: bool) -> Response {
    let bg_idx = ui.painter().add(egui::Shape::Noop);

    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let inner = ui.scope_builder(UiBuilder::new().sense(sense), |ui| {
        egui::Frame::new()
            .inner_margin(Margin::symmetric(12, 10))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let (rect, _) = ui.allocate_exact_size(Vec2::splat(30.0), Sense::hover());
                    ui.painter()
                        .circle_filled(rect.center(), 15.0, palette(ui).hover);
                    ui.painter().text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        mark,
                        egui::FontId::proportional(13.0),
                        if enabled {
                            color::BRAND_CYAN
                        } else {
                            palette(ui).muted
                        },
                    );

                    ui.add_space(9.0);
                    ui.vertical(|ui| {
                        ui.label(RichText::new(title).size(13.0).color(if enabled {
                            palette(ui).text
                        } else {
                            palette(ui).muted
                        }));
                        if !sub.is_empty() {
                            ui.add_space(1.0);
                            ui.label(RichText::new(sub).size(11.5).color(palette(ui).muted));
                        }
                    });

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            RichText::new(if enabled { "→" } else { "…" })
                                .size(14.0)
                                .color(palette(ui).muted),
                        );
                    });
                });
            });
    });

    // 这里总是填背景（不像 row 只在 hover 时填）：卡片本身就该有底色。
    let bg = if enabled && inner.response.hovered() {
        palette(ui).hover
    } else {
        palette(ui).panel
    };
    ui.painter().set(
        bg_idx,
        egui::epaint::RectShape::filled(
            inner.response.rect,
            CornerRadius::same(RADIUS_CONTROL),
            bg,
        ),
    );

    if enabled && inner.response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    inner.response
}

/// 说明文字。常驻的静态说明**不是通知**，别用警告色——
/// 常驻黄字会让用户脱敏，真出事时反而看不见。
pub fn hint(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).size(12.0).color(palette(ui).muted));
}

/// 空态 / 错误态。只在**整块内容被它替换**时用。
pub fn state_panel(ui: &mut Ui, title: &str, desc: &str) {
    ui.add_space(48.0);
    ui.vertical_centered(|ui| {
        ui.label(RichText::new(title).size(15.0).color(palette(ui).text));
        if !desc.is_empty() {
            ui.add_space(6.0);
            ui.label(RichText::new(desc).size(12.5).color(palette(ui).muted));
        }
    });
}

// ============================================================================
// 主题
// ============================================================================

/// 把上面这套层级装进 egui 的 Visuals。
///
/// 与 main.rs 现有的 `install_theme` 是同一件事——合并时保留其一即可，
/// 但**斑马纹必须关掉**（`faint_bg_color = TRANSPARENT`）：它是表格的产物，
/// 而我们不用表格。
pub fn install_theme(ctx: &egui::Context) {
    ctx.set_theme(egui::ThemePreference::System);
    let mut style = (*ctx.style_of(egui::Theme::Dark)).clone();
    let v = &mut style.visuals;

    v.panel_fill = color::BG;
    v.window_fill = color::PANEL;
    v.extreme_bg_color = color::SIDEBAR; // 输入框底
    v.faint_bg_color = Color32::TRANSPARENT; // 斑马纹：关（我们不用表格）
    v.window_stroke = Stroke::new(1.0, color::LINE);
    v.window_corner_radius = CornerRadius::same(RADIUS_CARD);

    v.widgets.noninteractive.bg_fill = color::PANEL;
    v.widgets.noninteractive.weak_bg_fill = color::PANEL;
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, color::DIVIDER);
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, color::FG);
    v.widgets.noninteractive.corner_radius = CornerRadius::same(RADIUS_CONTROL);

    // 摘边框（bg_stroke = NONE）之后，hover 反馈必须靠 bg_fill 补回来，
    // 否则控件对鼠标完全没反应——1U 实际踩过这个坑。
    v.widgets.inactive.bg_fill = color::PANEL;
    v.widgets.inactive.weak_bg_fill = color::PANEL;
    v.widgets.inactive.bg_stroke = Stroke::NONE;
    v.widgets.inactive.fg_stroke = Stroke::new(1.0, color::FG2);
    v.widgets.inactive.corner_radius = CornerRadius::same(RADIUS_CONTROL);

    v.widgets.hovered.bg_fill = color::CARD_HOVER;
    v.widgets.hovered.weak_bg_fill = color::CARD_HOVER;
    v.widgets.hovered.bg_stroke = Stroke::NONE;
    v.widgets.hovered.fg_stroke = Stroke::new(1.0, color::FG);
    v.widgets.hovered.corner_radius = CornerRadius::same(RADIUS_CONTROL);
    v.widgets.hovered.expansion = 0.0; // hover 不放大：临界阻尼、无 bounce

    v.widgets.active.bg_fill = color::CARD_HOVER;
    v.widgets.active.weak_bg_fill = color::CARD_HOVER;
    v.widgets.active.bg_stroke = Stroke::NONE;
    v.widgets.active.fg_stroke = Stroke::new(1.0, color::FG);
    v.widgets.active.corner_radius = CornerRadius::same(RADIUS_CONTROL);
    v.widgets.active.expansion = -1.0; // 按下即给反馈：轻微内缩

    v.selection.bg_fill = color::ACCENT_SOFT;
    v.selection.stroke = Stroke::new(1.0, color::ACCENT);

    // 8pt 网格：间距一律取 4 的倍数，说得出为什么（规范 §9）
    style.spacing.item_spacing = Vec2::new(8.0, 8.0);
    style.spacing.button_padding = Vec2::new(12.0, 7.0);

    ctx.set_style_of(egui::Theme::Dark, style);
    let mut light = (*ctx.style_of(egui::Theme::Light)).clone();
    light.visuals.panel_fill = Color32::from_rgb(245, 245, 247);
    light.visuals.window_fill = Color32::WHITE;
    light.visuals.window_corner_radius = CornerRadius::same(RADIUS_CARD);
    light.spacing.item_spacing = Vec2::new(8.0, 8.0);
    light.spacing.button_padding = Vec2::new(12.0, 7.0);
    ctx.set_style_of(egui::Theme::Light, light);
}
