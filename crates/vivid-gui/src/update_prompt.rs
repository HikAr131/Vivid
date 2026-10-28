use std::{collections::BTreeSet, fs::OpenOptions, path::PathBuf};

use eframe::egui::{self, Align2, Id, RichText, Sense, Stroke, Vec2};
use vivid_core::update::{Available, SemVer};

use crate::{links, ui};

const CLOSE: &str = "update-prompt-close";
const MORE: &str = "update-prompt-more";
const LATER: &str = "update-prompt-later";
const DOWNLOAD: &str = "update-prompt-download";

struct Offer {
    available: Available,
    version: SemVer,
    manual: bool,
}

pub struct UpdatePrompt {
    receipt_dir: Option<PathBuf>,
    shown: BTreeSet<SemVer>,
    candidate: Option<Offer>,
    active: Option<Offer>,
    previous_focus: Option<Id>,
    return_focus: Option<Id>,
    persistence_error: Option<String>,
    opened_at: f64,
}

impl UpdatePrompt {
    pub fn new(receipt_dir: Option<PathBuf>) -> Self {
        Self {
            receipt_dir,
            shown: BTreeSet::new(),
            candidate: None,
            active: None,
            previous_focus: None,
            return_focus: None,
            persistence_error: None,
            opened_at: 0.0,
        }
    }

    pub fn offer(&mut self, available: &Available, manual: bool) {
        let Some(version) = SemVer::parse(&available.manifest.version) else {
            return;
        };
        let Some(current) = SemVer::parse(&available.current) else {
            return;
        };
        if version <= current || (!manual && self.shown.contains(&version)) {
            return;
        }
        if self
            .active
            .as_ref()
            .is_some_and(|open| open.version >= version)
            || self
                .candidate
                .as_ref()
                .is_some_and(|queued| queued.version > version)
        {
            return;
        }
        self.candidate = Some(Offer {
            available: available.clone(),
            version,
            manual,
        });
    }

    pub fn is_open(&self) -> bool {
        self.active.is_some() || self.candidate.is_some()
    }

    pub fn take_return_focus(&mut self) -> Option<Id> {
        self.return_focus.take()
    }

    pub fn take_persistence_error(&mut self) -> Option<String> {
        self.persistence_error.take()
    }

    fn record_display(&mut self, offer: &Offer) -> bool {
        let Some(directory) = &self.receipt_dir else {
            return true;
        };
        let result = (|| {
            std::fs::create_dir_all(directory)?;
            // Only parsed numeric components reach the filesystem.
            let path = directory.join(format!("prompted-v{}", offer.version));
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(file) => {
                    file.sync_all()?;
                    Ok(true)
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    let metadata = std::fs::symlink_metadata(&path)?;
                    if !metadata.file_type().is_file() || metadata.len() != 0 {
                        self.persistence_error =
                            Some("更新提醒记录无法读取，已保留原文件。".to_owned());
                    }
                    Ok(false)
                }
                Err(error) => Err(error),
            }
        })();
        match result {
            Ok(created) => offer.manual || created,
            Err(_) => {
                self.persistence_error =
                    Some("更新提醒记录未能保存，同一版本下次启动可能再次提醒。".to_owned());
                true
            }
        }
    }

    fn dismiss(&mut self) {
        self.active = None;
        self.return_focus = self.previous_focus.take();
    }

    pub fn show(&mut self, ctx: &egui::Context) {
        let mut just_opened = false;
        if self.active.is_none() {
            let Some(offer) = self.candidate.take() else {
                return;
            };
            if !self.record_display(&offer) {
                self.shown.insert(offer.version);
                return;
            }
            self.shown.insert(offer.version);
            self.previous_focus = ctx.memory(|memory| memory.focused());
            self.active = Some(offer);
            self.opened_at = ctx.input(|input| input.time);
            just_opened = true;
        }
        let offer = self.active.as_ref().unwrap();
        let notes = notes_preview(&offer.available.manifest.notes);
        let mut focus_ids = vec![Id::new(CLOSE)];
        if notes.has_more {
            focus_ids.push(Id::new(MORE));
        }
        focus_ids.extend([Id::new(LATER), Id::new(DOWNLOAD)]);
        trap_focus(ctx, &focus_ids, just_opened);
        let (escape, enter) = ctx.input_mut(|input| {
            (
                input.consume_key(egui::Modifiers::NONE, egui::Key::Escape),
                input.consume_key(egui::Modifiers::NONE, egui::Key::Enter),
            )
        });
        let screen = ctx.content_rect();
        let progress =
            ((ctx.input(|input| input.time) - self.opened_at) / 0.16).clamp(0.0, 1.0) as f32;
        let ease = 1.0 - (1.0 - progress).powi(3);
        if progress < 1.0 {
            ctx.request_repaint();
        }
        let width = (screen.width() - 64.0).clamp(160.0, 400.0) - 40.0;
        let mut close = escape;
        let mut download = enter && !just_opened && !escape;
        let modal = egui::Modal::new(Id::new("update-prompt"))
            .backdrop_color(egui::Color32::from_black_alpha(
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
                egui::Frame::new()
                    .fill(p.panel)
                    .corner_radius(ui::RADIUS_CARD)
                    .inner_margin(20)
                    .show(ui, |ui| {
                        ui.set_width(width);
                        ui.spacing_mut().item_spacing = Vec2::new(8.0, 12.0);
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 8.0;
                            let title_width = (width - 36.0).max(1.0);
                            let (title_rect, _) = ui
                                .allocate_exact_size(Vec2::new(title_width, 28.0), Sense::hover());
                            ui.painter().text(
                                title_rect.left_center(),
                                Align2::LEFT_CENTER,
                                format!("有新版本 v{}", offer.version),
                                egui::FontId::proportional(15.0),
                                p.text,
                            );
                            close |= button(ui, CLOSE, "×", Vec2::splat(28.0), false).clicked();
                        });
                        let current = SemVer::parse(&offer.available.current).unwrap();
                        ui.label(
                            RichText::new(format!(
                                "当前是 v{current}，之后也可以在「关于」里随时更新。"
                            ))
                            .size(13.0)
                            .color(p.muted),
                        );
                        if !notes.lines.is_empty() {
                            ui.label(RichText::new("这次更新").size(12.0).strong().color(p.muted));
                            egui::ScrollArea::vertical()
                                .id_salt("update-prompt-notes")
                                .max_height((screen.height() - 300.0).clamp(36.0, 240.0))
                                .auto_shrink([false, true])
                                .show(ui, |ui| {
                                    ui.spacing_mut().item_spacing.y = 4.0;
                                    for line in &notes.lines {
                                        ui.horizontal_top(|ui| {
                                            ui.label(RichText::new("•").size(13.0).color(p.muted));
                                            ui.add(
                                                egui::Label::new(
                                                    RichText::new(*line).size(13.0).color(p.text),
                                                )
                                                .wrap(),
                                            );
                                        });
                                    }
                                });
                            if notes.has_more
                                && button(ui, MORE, "查看完整说明 ↗", Vec2::new(120.0, 24.0), false)
                                    .clicked()
                            {
                                links::open(ctx, links::RELEASE_URL);
                            }
                        }
                        ui.label(
                            RichText::new("会在浏览器打开下载页。")
                                .size(12.0)
                                .color(p.muted),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let primary =
                                button(ui, DOWNLOAD, "去下载", Vec2::new(76.0, 28.0), true);
                            if just_opened {
                                primary.request_focus();
                            }
                            download |= primary.clicked();
                            close |= button(ui, LATER, "稍后再说", Vec2::new(88.0, 28.0), false)
                                .clicked();
                        });
                    });
            });
        if !crate::about::reduced_motion() && progress < 1.0 {
            let center = modal.response.rect.center();
            ctx.transform_layer_shapes(
                modal.response.layer_id,
                egui::emath::TSTransform::from_translation(center.to_vec2())
                    * egui::emath::TSTransform::from_scaling(0.98 + 0.02 * ease)
                    * egui::emath::TSTransform::from_translation(-center.to_vec2()),
            );
        }
        if download && !close {
            links::open(ctx, links::RELEASE_URL);
        }
        if close || download || modal.should_close() {
            self.dismiss();
        }
    }
}

struct NotesPreview<'a> {
    lines: Vec<&'a str>,
    has_more: bool,
}

fn notes_preview(notes: &str) -> NotesPreview<'_> {
    let mut lines = notes.lines().map(str::trim).filter(|line| !line.is_empty());
    let preview = lines.by_ref().take(5).collect();
    NotesPreview {
        lines: preview,
        has_more: lines.next().is_some(),
    }
}

fn trap_focus(ctx: &egui::Context, ids: &[Id], just_opened: bool) {
    let backwards = ctx.input_mut(|input| {
        if input.consume_key(egui::Modifiers::SHIFT, egui::Key::Tab) {
            Some(true)
        } else if input.consume_key(egui::Modifiers::NONE, egui::Key::Tab) {
            Some(false)
        } else {
            None
        }
    });
    let focused = ctx.memory(|memory| memory.focused());
    let next = if just_opened {
        Some(Id::new(DOWNLOAD))
    } else if let Some(backwards) = backwards {
        let index = ids.iter().position(|id| Some(*id) == focused).unwrap_or(0);
        Some(ids[(index + if backwards { ids.len() - 1 } else { 1 }) % ids.len()])
    } else if !ids.iter().any(|id| Some(*id) == focused) {
        Some(Id::new(DOWNLOAD))
    } else {
        None
    };
    if let Some(next) = next {
        ctx.memory_mut(|memory| {
            memory.move_focus(egui::FocusDirection::None);
            memory.request_focus(next);
        });
    }
}

fn button(ui: &mut egui::Ui, name: &str, text: &str, size: Vec2, primary: bool) -> egui::Response {
    let p = ui::palette(ui);
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let response = ui
        .interact(rect, Id::new(name), Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    let fill = if primary {
        if response.hovered() {
            ui::color::ACCENT_HOVER
        } else {
            ui::color::ACCENT
        }
    } else if response.hovered() || response.is_pointer_button_down_on() {
        p.hover
    } else if name == MORE || name == CLOSE {
        egui::Color32::TRANSPARENT
    } else {
        p.separator
    };
    ui.painter().rect_filled(rect, ui::RADIUS_CONTROL, fill);
    if response.has_focus() {
        ui.painter().rect_stroke(
            rect,
            ui::RADIUS_CONTROL,
            Stroke::new(1.5, p.accent),
            egui::StrokeKind::Outside,
        );
    }
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        text,
        egui::FontId::proportional(13.0),
        if primary {
            egui::Color32::WHITE
        } else if name == MORE {
            p.accent
        } else {
            p.text
        },
    );
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, text));
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    fn available(version: &str, notes: &str) -> Available {
        Available {
            manifest: vivid_core::update::Manifest {
                version: version.to_owned(),
                notes: notes.to_owned(),
                download_url: "https://evil.invalid/not-allowed".to_owned(),
                published: String::new(),
            },
            current: "0.1.0".to_owned(),
        }
    }

    fn temp_dir() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

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

    fn frame(
        ctx: &egui::Context,
        prompt: &mut UpdatePrompt,
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        frame_at(ctx, prompt, events, Vec2::new(960.0, 640.0))
    }

    fn frame_at(
        ctx: &egui::Context,
        prompt: &mut UpdatePrompt,
        events: Vec<egui::Event>,
        size: Vec2,
    ) -> egui::FullOutput {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                events,
                ..Default::default()
            },
            |_ui| prompt.show(ctx),
        );
        output.textures_delta.clear();
        output
    }

    fn click(ctx: &egui::Context, prompt: &mut UpdatePrompt, pos: egui::Pos2) -> egui::FullOutput {
        let event = |pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        let _ = frame(
            ctx,
            prompt,
            vec![egui::Event::PointerMoved(pos), event(true)],
        );
        frame(ctx, prompt, vec![event(false)])
    }

    #[test]
    fn records_only_when_displayed_and_once_across_instances() {
        let directory = temp_dir();
        let receipts = directory.path().join("receipts");
        let ctx = egui::Context::default();
        let update = available("v1.2.3", "");
        let mut first = UpdatePrompt::new(Some(receipts.clone()));
        first.offer(&update, false);
        assert!(first.is_open());
        assert!(!receipts.exists());
        let mut replacement = UpdatePrompt::new(Some(receipts.clone()));
        replacement.offer(&update, false);
        let _ = frame(&ctx, &mut replacement, vec![]);
        assert!(replacement.is_open());
        assert!(receipts.join("prompted-v1.2.3").is_file());
        drop(replacement);
        let mut restarted = UpdatePrompt::new(Some(receipts.clone()));
        restarted.offer(&update, false);
        let _ = frame(&ctx, &mut restarted, vec![]);
        assert!(!restarted.is_open());
        restarted.offer(&update, true);
        let _ = frame(&ctx, &mut restarted, vec![]);
        assert!(restarted.is_open());
        let _ = frame(&ctx, &mut restarted, vec![key(egui::Key::Escape, false)]);
        restarted.offer(&available("1.2.4", ""), false);
        let _ = frame(&ctx, &mut restarted, vec![]);
        assert!(restarted.is_open());
        assert!(receipts.join("prompted-v1.2.4").is_file());
    }

    #[test]
    fn canonical_version_names_never_copy_manifest_path_text() {
        let directory = temp_dir();
        let ctx = egui::Context::default();
        for version in ["../../escape", "C:/escape", "v9.8.7-../../escape"] {
            let mut prompt = UpdatePrompt::new(Some(directory.path().to_owned()));
            prompt.offer(&available(version, ""), false);
            let _ = frame(&ctx, &mut prompt, vec![]);
        }
        let names: Vec<_> = std::fs::read_dir(directory.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(names, [std::ffi::OsString::from("prompted-v9.8.7")]);
    }

    #[test]
    fn corrupt_receipt_and_unwritable_directory_are_preserved() {
        let directory = temp_dir();
        let marker = directory.path().join("prompted-v1.2.3");
        std::fs::write(&marker, b"other data").unwrap();
        let ctx = egui::Context::default();
        let mut prompt = UpdatePrompt::new(Some(directory.path().to_owned()));
        prompt.offer(&available("1.2.3", ""), false);
        let _ = frame(&ctx, &mut prompt, vec![]);
        assert!(!prompt.is_open());
        assert!(prompt.take_persistence_error().is_some());
        assert_eq!(std::fs::read(&marker).unwrap(), b"other data");
        let mut blocked = UpdatePrompt::new(Some(marker.clone()));
        blocked.offer(&available("1.2.4", ""), false);
        let _ = frame(&ctx, &mut blocked, vec![]);
        assert!(blocked.is_open());
        assert!(blocked.take_persistence_error().is_some());
        assert_eq!(std::fs::read(&marker).unwrap(), b"other data");
    }

    #[test]
    fn notes_are_nonempty_lines_with_five_item_limit() {
        let notes = notes_preview("\n one \n\n two\nthree\nfour\nfive\nsix\n");
        assert_eq!(notes.lines, ["one", "two", "three", "four", "five"]);
        assert!(notes.has_more);
        assert!(!notes_preview("\n \n").has_more);
        assert!(notes_preview("\n \n").lines.is_empty());
        assert!(!notes_preview("1\n2\n3\n4\n5\n\n").has_more);
    }

    #[test]
    fn keyboard_traps_focus_and_enter_opens_only_fixed_release() {
        let ctx = egui::Context::default();
        let mut prompt = UpdatePrompt::new(None);
        prompt.offer(&available("1.2.3", "1\n2\n3\n4\n5\n6"), false);
        let _ = frame(&ctx, &mut prompt, vec![]);
        assert_eq!(
            ctx.memory(|memory| memory.focused()),
            Some(Id::new(DOWNLOAD))
        );
        for name in [CLOSE, MORE, LATER, DOWNLOAD, CLOSE] {
            let _ = frame(&ctx, &mut prompt, vec![key(egui::Key::Tab, false)]);
            assert_eq!(ctx.memory(|memory| memory.focused()), Some(Id::new(name)));
        }
        let _ = frame(&ctx, &mut prompt, vec![key(egui::Key::Tab, true)]);
        assert_eq!(
            ctx.memory(|memory| memory.focused()),
            Some(Id::new(DOWNLOAD))
        );
        let output = frame(&ctx, &mut prompt, vec![key(egui::Key::Enter, false)]);
        assert!(!prompt.is_open());
        assert_eq!(output.platform_output.commands.len(), 1);
        let egui::OutputCommand::OpenUrl(url) = &output.platform_output.commands[0] else {
            panic!("expected the release page");
        };
        assert_eq!(url.url, links::RELEASE_URL);
    }

    #[test]
    fn escape_closes_and_returns_opening_focus() {
        let ctx = egui::Context::default();
        let origin = Id::new("update-origin");
        let mut prompt = UpdatePrompt::new(None);
        let mut initial = ctx.run_ui(Default::default(), |ui| {
            let (rect, _) = ui.allocate_exact_size(Vec2::splat(30.0), Sense::hover());
            let response = ui.interact(rect, origin, Sense::click());
            response.request_focus();
        });
        initial.textures_delta.clear();
        prompt.offer(&available("1.2.3", ""), true);
        let _ = frame(&ctx, &mut prompt, vec![]);
        let output = frame(&ctx, &mut prompt, vec![key(egui::Key::Escape, false)]);
        assert!(!prompt.is_open());
        assert!(output.platform_output.commands.is_empty());
        assert_eq!(prompt.take_return_focus(), Some(origin));
        assert_eq!(prompt.take_return_focus(), None);
    }

    #[test]
    fn pointer_later_close_and_backdrop_dismiss_without_opening_browser() {
        for control in [Some(LATER), Some(CLOSE), None] {
            let ctx = egui::Context::default();
            let mut prompt = UpdatePrompt::new(None);
            let update = available("1.2.3", "");
            prompt.offer(&update, false);
            for _ in 0..3 {
                let _ = frame(&ctx, &mut prompt, vec![]);
            }
            let point = control
                .map(|name| ctx.read_response(Id::new(name)).unwrap().rect.center())
                .unwrap_or_else(|| egui::pos2(5.0, 5.0));
            let output = click(&ctx, &mut prompt, point);
            assert!(!prompt.is_open(), "{control:?}");
            assert!(output.platform_output.commands.is_empty());
            prompt.offer(&update, false);
            let _ = frame(&ctx, &mut prompt, vec![]);
            assert!(!prompt.is_open());
        }
    }

    #[test]
    fn more_notes_opens_fixed_release_and_empty_notes_hide_more_control() {
        let ctx = egui::Context::default();
        let mut prompt = UpdatePrompt::new(None);
        prompt.offer(&available("1.2.3", "1\n2\n3\n4\n5\n6"), true);
        for _ in 0..3 {
            let _ = frame(&ctx, &mut prompt, vec![]);
        }
        let point = ctx.read_response(Id::new(MORE)).unwrap().rect.center();
        let output = click(&ctx, &mut prompt, point);
        assert!(prompt.is_open());
        assert_eq!(output.platform_output.commands.len(), 1);
        let egui::OutputCommand::OpenUrl(url) = &output.platform_output.commands[0] else {
            panic!("expected the release page");
        };
        assert_eq!(url.url, links::RELEASE_URL);
        let _ = frame(&ctx, &mut prompt, vec![key(egui::Key::Escape, false)]);
        prompt.offer(&available("1.2.3", " \n\n "), true);
        let _ = frame(&ctx, &mut prompt, vec![]);
        let _ = frame(&ctx, &mut prompt, vec![]);
        assert!(ctx.read_response(Id::new(MORE)).is_none());
    }

    #[test]
    fn long_notes_keep_actions_inside_small_viewport() {
        for notes in ["long notes ".repeat(2000), "long notes\n".repeat(6)] {
            let ctx = egui::Context::default();
            crate::install_chinese_font(&ctx);
            crate::ui::install_theme(&ctx);
            let mut prompt = UpdatePrompt::new(None);
            prompt.offer(&available("1.2.3", &notes), true);
            let size = Vec2::new(480.0, 320.0);
            for _ in 0..3 {
                let _ = frame_at(&ctx, &mut prompt, vec![], size);
            }
            for name in [CLOSE, LATER, DOWNLOAD] {
                let response = ctx.read_response(Id::new(name)).unwrap();
                assert!(
                    egui::Rect::from_min_size(egui::Pos2::ZERO, size).contains_rect(response.rect),
                    "{name}: {:?}",
                    response.rect
                );
            }
        }
    }
}
