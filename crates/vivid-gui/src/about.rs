use eframe::egui::{self, Align2, Color32, Id, RichText, Sense, Stroke, Vec2};

use crate::{links, ui};

pub const TITLE: &str = "免费公开分享";
pub const NOTE: &str = "这个软件免费给大家使用。有好的建议或反馈，欢迎在下面这些平台告诉我。";
pub const COFFEE: &str = "请我喝杯咖啡";
pub const CAPTION: &str = "完全自愿，不解锁任何功能。点图片可以放大。";
pub const COFFEE_ID: &str = "about-coffee";
const POSTER_ID: &str = "coffee-poster";
const CLOSE_ID: &str = "coffee-close";

#[derive(Default)]
pub struct About {
    pub coffee_open: bool,
    pub zoomed: bool,
    legal_open: bool,
    just_opened: bool,
    return_focus: bool,
    opened_at: f64,
    texture: Option<egui::TextureHandle>,
    licenses: Option<Vec<License>>,
}

#[derive(serde::Deserialize)]
struct License {
    name: String,
    version: String,
    license: String,
    text: String,
}

impl About {
    pub fn has_dialog(&self) -> bool {
        self.coffee_open || self.legal_open
    }

    pub fn author(&mut self, ui: &mut egui::Ui) {
        ui.spacing_mut().item_spacing.y = 0.0;
        let p = ui::palette(ui);
        ui.label(RichText::new(TITLE).size(17.0).strong().color(p.text));
        ui.add_space(4.0);
        ui.label(RichText::new(NOTE).size(13.0).color(p.muted));
        ui.add_space(12.0);
        group(ui, |ui| {
            for (index, channel) in links::CHANNELS.iter().enumerate() {
                let response = channel_row(
                    ui,
                    Id::new(("channel", index)),
                    index,
                    channel.platform,
                    channel.account,
                    false,
                );
                if response.clicked() {
                    links::open(ui.ctx(), channel.url);
                }
                if index + 1 < links::CHANNELS.len() {
                    let y = response.rect.bottom();
                    ui.painter().line_segment(
                        [
                            egui::pos2(response.rect.left() + 44.0, y),
                            egui::pos2(response.rect.right(), y),
                        ],
                        Stroke::new(1.0, p.separator),
                    );
                }
            }
        });
        ui.add_space(16.0);
        group(ui, |ui| {
            let response = channel_row(ui, Id::new(COFFEE_ID), 3, COFFEE, "", true);
            if self.return_focus {
                response.request_focus();
                self.return_focus = false;
            }
            if response.clicked() {
                self.open_coffee(ui.ctx());
            }
        });
        ui.add_space(24.0);
        if ui
            .add(
                egui::Button::new(RichText::new("免责声明 ›").size(13.0).color(p.muted))
                    .frame(false),
            )
            .clicked()
        {
            self.legal_open = true;
        }
    }

    pub fn open_coffee(&mut self, ctx: &egui::Context) {
        self.coffee_open = true;
        self.zoomed = false;
        self.just_opened = true;
        self.opened_at = ctx.input(|i| i.time);
    }

    fn dismiss(&mut self) {
        if self.zoomed {
            self.zoomed = false;
        } else {
            self.coffee_open = false;
            self.return_focus = true;
        }
    }

    pub fn dialogs(&mut self, ctx: &egui::Context) {
        if self.coffee_open {
            self.coffee_dialog(ctx);
        }
        if self.legal_open {
            self.legal_dialog(ctx);
        }
    }

    fn coffee_dialog(&mut self, ctx: &egui::Context) {
        let focus_ids = if self.zoomed {
            vec![Id::new(POSTER_ID)]
        } else {
            vec![Id::new(CLOSE_ID), Id::new(POSTER_ID)]
        };
        trap_focus(ctx, &focus_ids);
        if self.texture.is_none() {
            let image = image::load_from_memory(include_bytes!("../assets/donate-poster.png"))
                .expect("embedded coffee poster must be a valid PNG")
                .to_rgba8();
            self.texture = Some(ctx.load_texture(
                "donate-poster",
                egui::ColorImage::from_rgba_unmultiplied(
                    [image.width() as usize, image.height() as usize],
                    &image,
                ),
                egui::TextureOptions::LINEAR,
            ));
        }
        let zoomed = self.zoomed;
        let screen = ctx.content_rect();
        let height = if zoomed {
            screen.height() - 24.0
        } else {
            (screen.height() - 166.0).min(474.0)
        };
        let poster_size = Vec2::new(height * 2.0 / 3.0, height).max(Vec2::splat(1.0));
        let elapsed = ctx.input(|i| i.time) - self.opened_at;
        let progress = (elapsed / 0.16).clamp(0.0, 1.0) as f32;
        let ease = 1.0 - (1.0 - progress).powi(3);
        if progress < 1.0 {
            ctx.request_repaint();
        }
        let reduced = reduced_motion();
        let mut close = false;
        let mut toggle_zoom = false;
        let response = egui::Modal::new(Id::new("coffee-modal"))
            .backdrop_color(Color32::from_black_alpha(
                (if ctx.global_style().visuals.dark_mode {
                    128.0
                } else {
                    82.0
                } * ease) as u8,
            ))
            .frame(egui::Frame::NONE)
            .show(ctx, |ui| {
                let p = ui::palette(ui);
                ui.set_opacity(ease);
                let frame = if zoomed {
                    egui::Frame::NONE
                } else {
                    egui::Frame::new()
                        .fill(p.panel)
                        .corner_radius(12)
                        .inner_margin(16)
                };
                frame.show(ui, |ui| {
                    ui.spacing_mut().item_spacing = Vec2::new(0.0, 12.0);
                    let width = poster_size.x.max(if zoomed { 0.0 } else { 336.0 });
                    ui.set_width(width);
                    if !zoomed {
                        let (rect, _) =
                            ui.allocate_exact_size(Vec2::new(width, 28.0), Sense::hover());
                        ui.painter().text(
                            rect.left_center(),
                            Align2::LEFT_CENTER,
                            COFFEE,
                            egui::FontId::proportional(15.0),
                            p.text,
                        );
                        let close_rect = egui::Rect::from_min_size(
                            egui::pos2(rect.right() - 28.0, rect.top()),
                            Vec2::splat(28.0),
                        );
                        let r = ui.interact(close_rect, Id::new(CLOSE_ID), Sense::click());
                        if self.just_opened {
                            r.request_focus();
                        }
                        if r.hovered() || r.has_focus() {
                            ui.painter().rect_filled(close_rect, 8, p.hover);
                        }
                        if r.has_focus() {
                            ui.painter().rect_stroke(
                                close_rect,
                                8,
                                Stroke::new(1.5, p.accent),
                                egui::StrokeKind::Inside,
                            );
                        }
                        let c = close_rect.center();
                        for direction in [-1.0, 1.0] {
                            ui.painter().line_segment(
                                [
                                    c + Vec2::new(-5.0, -5.0 * direction),
                                    c + Vec2::new(5.0, 5.0 * direction),
                                ],
                                Stroke::new(1.6, p.muted),
                            );
                        }
                        close |= r.clicked();
                        r.widget_info(|| {
                            egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "关闭")
                        });
                    }
                    ui.horizontal(|ui| {
                        ui.add_space((width - poster_size.x) / 2.0);
                        let (rect, _) = ui.allocate_exact_size(poster_size, Sense::hover());
                        let r = ui.interact(rect, Id::new(POSTER_ID), Sense::click());
                        ui.painter().image(
                            self.texture.as_ref().unwrap().id(),
                            rect,
                            egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                            Color32::WHITE,
                        );
                        if r.has_focus() {
                            ui.painter().rect_stroke(
                                rect,
                                8,
                                Stroke::new(2.0, p.accent),
                                egui::StrokeKind::Outside,
                            );
                        }
                        r.widget_info(|| {
                            egui::WidgetInfo::labeled(
                                egui::WidgetType::Button,
                                true,
                                if zoomed {
                                    "缩小收款海报"
                                } else {
                                    "放大收款海报"
                                },
                            )
                        });
                        toggle_zoom |= r.clicked();
                    });
                    if !zoomed {
                        ui.with_layout(egui::Layout::top_down(egui::Align::Center), |ui| {
                            ui.label(RichText::new(CAPTION).size(12.0).color(p.muted));
                        });
                    }
                });
            });
        if !reduced && progress < 1.0 {
            let scale = 0.98 + 0.02 * ease;
            let center = response.response.rect.center();
            ctx.transform_layer_shapes(
                response.response.layer_id,
                egui::emath::TSTransform::from_translation(center.to_vec2())
                    * egui::emath::TSTransform::from_scaling(scale)
                    * egui::emath::TSTransform::from_translation(-center.to_vec2()),
            );
        }
        self.just_opened = false;
        if toggle_zoom {
            self.zoomed = !self.zoomed;
            ctx.memory_mut(|m| m.request_focus(Id::new(POSTER_ID)));
        } else if close {
            self.coffee_open = false;
            self.return_focus = true;
        } else if response.should_close() {
            self.dismiss();
        }
    }

    fn legal_dialog(&mut self, ctx: &egui::Context) {
        if self.licenses.is_none() {
            self.licenses = Some(
                serde_json::from_str(include_str!("../assets/third-party-licenses.json"))
                    .expect("generated license inventory must be valid"),
            );
        }
        let mut close = false;
        let modal = egui::Modal::new(Id::new("legal-modal")).show(ctx, |ui| {
            let p = ui::palette(ui);
            ui.set_width((ctx.content_rect().width() - 80.0).min(640.0));
            ui.horizontal(|ui| {
                ui.heading("免责声明");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    close |= ui.button("关闭").clicked();
                });
            });
            egui::ScrollArea::vertical().max_height(ctx.content_rect().height() - 140.0).show(ui, |ui| {
                for (title, text) in LEGAL_SECTIONS {
                    ui.add_space(12.0);
                    ui.label(RichText::new(*title).strong().color(p.text));
                    ui.label(RichText::new(*text).size(13.0).color(p.muted));
                }
                ui.add_space(12.0);
                ui.strong("第三方组件");
                if ui.add(egui::Button::new(RichText::new("Free icons from Streamline ↗").size(13.0).color(p.muted)).frame(false)).clicked() {
                    links::open(ui.ctx(), links::STREAMLINE_CREDIT_URL);
                }
                ui.label("本软件用到的第三方开源组件版权归原作者所有，按各自的许可证使用；下面逐个列出名称、版本与许可证，点开可看许可证全文。");
                for (index, item) in self.licenses.as_ref().unwrap().iter().enumerate() {
                    egui::CollapsingHeader::new(format!("{} · {} · {}", item.name, item.version, item.license))
                        .id_salt(("license", index)).show(ui, |ui| { ui.label(&item.text); });
                }
            });
        });
        if close || modal.should_close() {
            self.legal_open = false;
        }
    }
}

pub const LEGAL_SECTIONS: &[(&str, &str)] = &[
    (
        "软件性质",
        "作者免费公开分享本软件，不提供付费服务。Vivid 本体采用 GPL-3.0-only，允许按该许可证使用、修改和再分发，包括商业用途；分发时须遵守许可证的源码提供等义务。第三方组件与素材另受各自许可约束。关于页的「请我喝杯咖啡」完全自愿，不换取任何功能或服务。软件按现状提供，不承诺适用于任何特定目的，也不承诺持续更新或修复。",
    ),
    (
        "功能与风险",
        "本软件会按 RHI 在线清单下载第三方组件，向所选 Steam 游戏目录写入文件；实验性 Vulkan 方案还会修改全局 ReShade 文件与注册表，影响所有 Vulkan 游戏。这些操作可能导致游戏闪退、兼容性问题或游戏账号被处罚。Vivid 无法百分百判断反作弊，仅供单机使用；不同设备的效果与兼容性不能保证。组件来源、版本、下载地址与 SHA-256 见每款游戏的详情；本地 SHA-256 不是上游签名验证。",
    ),
    (
        "用户责任",
        "使用前请阅读功能说明并备份重要数据，不要带着修改后的文件进入联机模式。因未阅读说明、未备份、在特殊系统环境下使用、被安全软件拦截或强制中断造成的后果，由用户自行承担。使用第三方组件可能违反游戏用户协议，游戏方可能检测并处罚，是否使用由你自行决定；本软件不做任何规避检测的处理。",
    ),
    (
        "来源与联系",
        "方案参考 RHI（https://github.com/RankFTW/RHI）；NVIDIA、ReShade 与 RenoDX 组件在明确安装或更新后从上游下载，各自受其许可约束，不随 Vivid 分发。Vivid 本体使用 GPL-3.0-only。问题与建议欢迎到关于页列出的任一渠道告诉作者。",
    ),
];

fn group(ui: &mut egui::Ui, body: impl FnOnce(&mut egui::Ui)) {
    let p = ui::palette(ui);
    egui::Frame::new()
        .fill(p.panel)
        .corner_radius(12)
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            ui.set_width(ui.available_width());
            body(ui);
        });
}

fn channel_row(
    ui: &mut egui::Ui,
    id: Id,
    icon: usize,
    title: &str,
    detail: &str,
    coffee: bool,
) -> egui::Response {
    let p = ui::palette(ui);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 44.0), Sense::hover());
    let response = ui
        .interact(rect, id, Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, true, format!("{title} {detail}"))
    });
    if response.hovered() || response.is_pointer_button_down_on() {
        ui.painter().rect_filled(rect, 12, p.hover);
    }
    if response.has_focus() {
        ui.painter().rect_stroke(
            rect,
            12,
            Stroke::new(2.0, p.accent),
            egui::StrokeKind::Inside,
        );
        response.scroll_to_me(Some(egui::Align::Center));
    }
    let center = rect.left_center() + Vec2::new(24.0, 0.0);
    if coffee {
        coffee_icon(ui.painter(), center, p.accent);
    } else {
        crate::social_icons::paint(ui, icon, center, p.muted);
    }
    ui.painter().text(
        rect.left_center() + Vec2::new(44.0, 0.0),
        Align2::LEFT_CENTER,
        title,
        egui::FontId::proportional(13.0),
        p.text,
    );
    ui.painter().text(
        rect.right_center() - Vec2::new(44.0, 0.0),
        Align2::RIGHT_CENTER,
        detail,
        egui::FontId::proportional(13.0),
        p.muted,
    );
    ui.painter().text(
        rect.right_center() - Vec2::new(20.0, 0.0),
        Align2::CENTER_CENTER,
        if coffee { "›" } else { "↗" },
        egui::FontId::proportional(16.0),
        p.muted,
    );
    response
}

fn coffee_icon(painter: &egui::Painter, center: egui::Pos2, color: Color32) {
    let point = |x: f32, y: f32| center + Vec2::new(x - 12.0, y - 12.0) * (16.0 / 24.0);
    let stroke = Stroke::new(1.3, color);
    let paths: &[&[(f32, f32)]] = &[
        &[
            (4.0, 9.5),
            (16.0, 9.5),
            (16.0, 16.0),
            (13.0, 19.0),
            (7.0, 19.0),
            (4.0, 16.0),
            (4.0, 9.5),
        ],
        &[(16.0, 11.0), (20.0, 11.0), (20.0, 16.0), (16.0, 16.0)],
        &[(8.0, 3.5), (9.0, 5.0), (8.0, 7.0)],
        &[(12.0, 3.5), (13.0, 5.0), (12.0, 7.0)],
    ];
    for path in paths {
        painter.add(egui::Shape::line(
            path.iter().map(|&(x, y)| point(x, y)).collect(),
            stroke,
        ));
    }
}

fn trap_focus(ctx: &egui::Context, ids: &[Id]) {
    let tab = ctx.input_mut(|i| {
        if i.consume_key(egui::Modifiers::SHIFT, egui::Key::Tab) {
            Some(true)
        } else if i.consume_key(egui::Modifiers::NONE, egui::Key::Tab) {
            Some(false)
        } else {
            None
        }
    });
    let current = ctx.memory(|m| m.focused());
    if let Some(backwards) = tab {
        let index = ids.iter().position(|id| Some(*id) == current).unwrap_or(0);
        let next = if backwards {
            (index + ids.len() - 1) % ids.len()
        } else {
            (index + 1) % ids.len()
        };
        ctx.memory_mut(|m| {
            m.move_focus(egui::FocusDirection::None);
            m.request_focus(ids[next]);
        });
    } else if !ids.iter().any(|id| Some(*id) == current) {
        ctx.memory_mut(|m| m.request_focus(ids[0]));
    }
}

pub(crate) fn reduced_motion() -> bool {
    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            SPI_GETCLIENTAREAANIMATION, SystemParametersInfoW,
        };
        let mut enabled: i32 = 1;
        // The OS preference is the native equivalent of prefers-reduced-motion.
        unsafe {
            SystemParametersInfoW(
                SPI_GETCLIENTAREAANIMATION,
                0,
                (&mut enabled as *mut i32).cast(),
                0,
            );
        }
        enabled == 0
    }
    #[cfg(not(windows))]
    {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(key: egui::Key, shift: bool) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: if shift {
                egui::Modifiers::SHIFT
            } else {
                egui::Modifiers::NONE
            },
        }
    }

    fn frame(ctx: &egui::Context, about: &mut About, events: Vec<egui::Event>) -> egui::FullOutput {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                Vec2::new(1060.0, 720.0),
            )),
            time: Some(1.0),
            events,
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                about.author(ui);
            });
            about.dialogs(ctx);
        });
        output.textures_delta.clear();
        output
    }

    #[test]
    fn keyboard_channels_enter_and_modal_focus_round_trip() {
        let ctx = egui::Context::default();
        let mut about = About::default();
        let _ = frame(&ctx, &mut about, vec![]);
        for index in 0..3 {
            let _ = frame(&ctx, &mut about, vec![key(egui::Key::Tab, false)]);
            assert_eq!(
                ctx.memory(|m| m.focused()),
                Some(Id::new(("channel", index)))
            );
            let output = frame(&ctx, &mut about, vec![key(egui::Key::Enter, false)]);
            assert_eq!(output.platform_output.commands.len(), 1);
        }
        let _ = frame(&ctx, &mut about, vec![key(egui::Key::Tab, false)]);
        assert_eq!(ctx.memory(|m| m.focused()), Some(Id::new(COFFEE_ID)));
        let _ = frame(&ctx, &mut about, vec![key(egui::Key::Enter, false)]);
        assert!(about.coffee_open);
        let _ = frame(&ctx, &mut about, vec![]);
        assert_eq!(ctx.memory(|m| m.focused()), Some(Id::new(CLOSE_ID)));
        let _ = frame(&ctx, &mut about, vec![key(egui::Key::Tab, false)]);
        assert_eq!(ctx.memory(|m| m.focused()), Some(Id::new(POSTER_ID)));
        let _ = frame(&ctx, &mut about, vec![key(egui::Key::Tab, false)]);
        assert_eq!(ctx.memory(|m| m.focused()), Some(Id::new(CLOSE_ID)));
        let _ = frame(&ctx, &mut about, vec![key(egui::Key::Tab, true)]);
        assert_eq!(ctx.memory(|m| m.focused()), Some(Id::new(POSTER_ID)));
        let _ = frame(&ctx, &mut about, vec![key(egui::Key::Enter, false)]);
        assert!(about.zoomed);
        let _ = frame(&ctx, &mut about, vec![key(egui::Key::Tab, false)]);
        assert_eq!(ctx.memory(|m| m.focused()), Some(Id::new(POSTER_ID)));
        let _ = frame(&ctx, &mut about, vec![key(egui::Key::Escape, false)]);
        assert!(about.coffee_open && !about.zoomed);
        let _ = frame(&ctx, &mut about, vec![key(egui::Key::Escape, false)]);
        assert!(!about.coffee_open);
        let _ = frame(&ctx, &mut about, vec![]);
        assert_eq!(ctx.memory(|m| m.focused()), Some(Id::new(COFFEE_ID)));
    }

    #[test]
    fn poster_and_license_payloads_are_embedded_and_complete() {
        let image = image::load_from_memory(include_bytes!("../assets/donate-poster.png")).unwrap();
        assert_eq!((image.width(), image.height()), (1024, 1536));
        let entries: Vec<License> =
            serde_json::from_str(include_str!("../assets/third-party-licenses.json")).unwrap();
        assert!(entries.len() > 200);
        assert!(entries.iter().all(|item| !item.name.is_empty()
            && !item.version.is_empty()
            && !item.license.is_empty()
            && item.text.len() > 100));
        assert!(
            entries
                .iter()
                .any(|item| item.name == "Resource Han Rounded CN Medium"
                    && item.text.contains("SIL OPEN FONT LICENSE"))
        );
        assert!(
            LEGAL_SECTIONS[0]
                .1
                .contains("完全自愿，不换取任何功能或服务")
        );
        for (prefix, license) in [
            ("抖音图标", "CC-BY-4.0"),
            ("B站图标", "Apache-2.0"),
            ("小红书图标", "CC0-1.0"),
        ] {
            let entry = entries
                .iter()
                .find(|entry| entry.name.starts_with(prefix))
                .expect("social icon credit missing from legal inventory");
            assert_eq!(entry.license, license);
            assert!(
                entry.text.contains("https://")
                    && entry.text.contains("修改说明")
                    && entry.text.contains("2026-10-17")
            );
        }
    }

    #[test]
    fn pointer_poster_backdrop_and_close_follow_the_same_dismissal_order() {
        let ctx = egui::Context::default();
        let mut about = About::default();
        let _ = frame(&ctx, &mut about, vec![]);
        about.open_coffee(&ctx);
        for _ in 0..3 {
            let _ = frame(&ctx, &mut about, vec![]);
        }
        fn click(ctx: &egui::Context, about: &mut About, pos: egui::Pos2) {
            let event = |pressed| egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            };
            let _ = frame(
                ctx,
                about,
                vec![egui::Event::PointerMoved(pos), event(true)],
            );
            let _ = frame(ctx, about, vec![event(false)]);
            let _ = frame(ctx, about, vec![]);
        }
        let poster = ctx.read_response(Id::new(POSTER_ID)).unwrap().rect.center();
        click(&ctx, &mut about, poster);
        assert!(about.zoomed);
        click(&ctx, &mut about, egui::pos2(5.0, 5.0));
        assert!(about.coffee_open && !about.zoomed);
        let poster = ctx.read_response(Id::new(POSTER_ID)).unwrap().rect.center();
        click(&ctx, &mut about, poster);
        assert!(about.zoomed);
        let poster = ctx.read_response(Id::new(POSTER_ID)).unwrap().rect.center();
        click(&ctx, &mut about, poster);
        assert!(about.coffee_open && !about.zoomed);
        let close = ctx.read_response(Id::new(CLOSE_ID)).unwrap().rect.center();
        click(&ctx, &mut about, close);
        assert!(!about.coffee_open);
        assert_eq!(ctx.memory(|m| m.focused()), Some(Id::new(COFFEE_ID)));
    }

    #[test]
    fn author_rows_and_text_fit_both_window_sizes_and_themes() {
        for width in [1060.0_f32, 860.0] {
            for theme in [egui::ThemePreference::Light, egui::ThemePreference::Dark] {
                let ctx = egui::Context::default();
                crate::install_chinese_font(&ctx);
                crate::ui::install_theme(&ctx);
                ctx.set_theme(theme);
                let content_width = (width - 282.0).min(560.0);
                let mut about = About::default();
                let input = egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        Vec2::new(width, 720.0),
                    )),
                    ..Default::default()
                };
                let mut output = ctx.run_ui(input, |ui| {
                    ui.set_width(content_width);
                    about.author(ui);
                });
                for index in 0..3 {
                    let response = ctx.read_response(Id::new(("channel", index))).unwrap();
                    assert_eq!(response.rect.height(), 44.0);
                    assert!(response.rect.width() <= content_width);
                    let mut labels = Vec::new();
                    for clipped in &output.shapes {
                        if let egui::Shape::Text(text) = &clipped.shape {
                            let bounds = text.visual_bounding_rect();
                            if response.rect.contains(bounds.center()) {
                                assert!(
                                    response.rect.expand(1.0).contains_rect(bounds),
                                    "Text outside row: {:?}",
                                    bounds
                                );
                                labels.push(bounds);
                            }
                        }
                    }
                    labels.sort_by(|a, b| a.left().total_cmp(&b.left()));
                    assert_eq!(labels.len(), 3);
                    for pair in labels.windows(2) {
                        assert!(pair[0].right() <= pair[1].left(), "Overlapping row text");
                    }
                }
                output.textures_delta.clear();
            }
        }
    }
}
