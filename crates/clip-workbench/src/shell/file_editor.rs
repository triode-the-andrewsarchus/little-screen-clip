//! Frame-scoped host chrome for editors opened by Files. Standalone studios use their own titles.
use egui::{Context, Id, Ui};
use std::path::PathBuf;

#[derive(Clone)]
pub(crate) struct PhotoFooter {
    pub index: usize,
    pub count: usize,
    pub rating: u8,
    pub enabled: bool,
    pub changed: bool,
}
#[derive(Clone, Default)]
pub(crate) struct FileEditor {
    pub path: PathBuf,
    pub rename_enabled: bool,
    pub rename_clicked: bool,
    pub suspend_keyboard: bool,
    pub photo: Option<PhotoFooter>,
}
impl FileEditor {
    fn id() -> Id {
        Id::new("files_editor_host_chrome")
    }
    pub fn current(ctx: &Context) -> Option<Self> {
        ctx.data(|data| data.get_temp(Self::id()))
    }
    /// Install only around this editor's render, then remove before another host can paint.
    pub fn scope<R>(self, ctx: &Context, render: impl FnOnce() -> R) -> (R, Self) {
        let keyboard = self.suspend_keyboard.then(|| {
            ctx.input_mut(|input| {
                (
                    std::mem::take(&mut input.events),
                    std::mem::take(&mut input.keys_down),
                    std::mem::take(&mut input.modifiers),
                )
            })
        });
        let previous = ctx.data_mut(|data| data.remove_temp::<Self>(Self::id()));
        ctx.data_mut(|data| data.insert_temp(Self::id(), self));
        let result = render();
        if let Some((events, keys_down, modifiers)) = keyboard {
            ctx.input_mut(|input| {
                input.events = events;
                input.keys_down = keys_down;
                input.modifiers = modifiers;
            });
        }
        let chrome = ctx
            .data_mut(|data| data.remove_temp::<Self>(Self::id()))
            .expect("file editor scope");
        if let Some(previous) = previous {
            ctx.data_mut(|data| data.insert_temp(Self::id(), previous));
        }
        (result, chrome)
    }
    pub fn title(&self) -> String {
        let name = self.path.file_name().unwrap_or_default().to_string_lossy();
        match self.path.parent() {
            Some(parent) => format!("{name} — {}", parent.display()),
            None => name.into_owned(),
        }
    }
    pub fn title_label(ui: &mut Ui, title: &str, tooltip: String, color: egui::Color32) {
        let can_rename = Self::current(ui.ctx()).is_some_and(|chrome| chrome.rename_enabled);
        let response = ui
            .add(
                egui::Label::new(egui::RichText::new(title).strong().size(22.0).color(color))
                    .truncate(true)
                    .selectable(!can_rename)
                    .sense(if can_rename {
                        egui::Sense::click()
                    } else {
                        egui::Sense::hover()
                    }),
            )
            .on_hover_text(if can_rename {
                format!("{tooltip}\nClick filename to rename")
            } else {
                tooltip
            });
        if can_rename
            && response
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .clicked()
        {
            Self::request_rename(ui.ctx());
        }
    }
    pub fn request_rename(ctx: &Context) {
        if let Some(mut chrome) = Self::current(ctx).filter(|chrome| chrome.rename_enabled) {
            chrome.rename_clicked = true;
            ctx.data_mut(|data| data.insert_temp(Self::id(), chrome));
        }
    }
    pub fn photo_controls(ui: &mut Ui) {
        let Some(mut chrome) = Self::current(ui.ctx()) else {
            return;
        };
        let Some(photo) = &mut chrome.photo else {
            return;
        };
        ui.separator();
        ui.label(egui::RichText::new(format!("{} / {}", photo.index + 1, photo.count)).size(12.0));
        ui.add_enabled_ui(photo.enabled, |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            ui.spacing_mut().button_padding = egui::vec2(2.0, 1.0);
            ui.spacing_mut().interact_size.y = 18.0;
            if ui
                .small_button("0")
                .on_hover_text("Clear rating [0]")
                .clicked()
            {
                photo.rating = 0;
                photo.changed = true;
            }
            for rating in 1..=5 {
                let color = if rating <= photo.rating {
                    egui::Color32::from_rgb(245, 196, 85)
                } else {
                    ui.visuals().weak_text_color()
                };
                if ui
                    .add(
                        egui::Button::image(crate::icons::Icon::Star.image_tinted(13.0, color))
                            .frame(false),
                    )
                    .on_hover_text(format!("{rating} stars [{rating}]"))
                    .clicked()
                {
                    photo.rating = rating;
                    photo.changed = true;
                }
            }
        });
        ui.ctx()
            .data_mut(|data| data.insert_temp(Self::id(), chrome));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::StudioShell;
    #[test]
    fn dialog_typing_is_hidden_from_editor_shortcuts_and_restored_for_the_dialog() {
        let ctx = Context::default();
        let _ = ctx.run(
            egui::RawInput {
                modifiers: egui::Modifiers::CTRL,
                events: vec![
                    egui::Event::Key {
                        key: egui::Key::R,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers: egui::Modifiers::CTRL,
                    },
                    egui::Event::Text("renamed.wav".into()),
                ],
                ..Default::default()
            },
            |ctx| {
                let mut editor_polled = false;
                FileEditor {
                    suspend_keyboard: true,
                    ..Default::default()
                }
                .scope(ctx, || {
                    editor_polled = true;
                    assert!(!ctx.input(
                        |input| input.key_pressed(egui::Key::R) || input.key_down(egui::Key::R)
                    ));
                    assert!(ctx.input(|input| input.events.is_empty() && !input.modifiers.any()));
                });
                assert!(editor_polled);
                assert!(ctx.input(|input| input.key_pressed(egui::Key::R)
                    && input.key_down(egui::Key::R)
                    && input.modifiers.ctrl));
                assert!(ctx.input(|input| input.events.iter().any(
                    |event| matches!(event, egui::Event::Text(text) if text == "renamed.wav")
                )));
            },
        );
    }
    #[test]
    fn photo_controls_keep_the_existing_footer_height_and_scope() {
        let palette = little_clip_core::AppConfig::default().workbench.appearance;
        let palette = crate::ui_appearance::palette(palette);
        for width in [640.0, 900.0, 1200.0] {
            let mut heights = Vec::new();
            for photo in [false, true] {
                let ctx = Context::default();
                egui_extras::install_image_loaders(&ctx);
                crate::ui_ext::configure_workbench_style(&ctx);
                let chrome = FileEditor {
                    path: "photos/photo.png".into(),
                    photo: photo.then_some(PhotoFooter {
                        index: 0,
                        count: 21,
                        rating: 3,
                        enabled: true,
                        changed: false,
                    }),
                    ..Default::default()
                };
                let mut height = 0.0;
                let _ = ctx.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(width, 300.0),
                        )),
                        ..Default::default()
                    },
                    |ctx| {
                        chrome.clone().scope(ctx, || {
                            egui::CentralPanel::default().show(ctx, |ui| {
                                let start = ui.cursor().top();
                                let mut close = false;
                                let shell = StudioShell::new(
                                    "Photo",
                                    crate::icons::Icon::Image,
                                    &palette,
                                    &mut close,
                                )
                                .with_footer_hints("Scroll to zoom; drag to pan");
                                shell.render_footer_with_center(ui, &None, 0.0, 184.0, |ui| {
                                    ui.add_sized(
                                        egui::vec2(56.0, 24.0),
                                        egui::Button::new("Reset").small(),
                                    );
                                    ui.add_sized(
                                        egui::vec2(40.0, 24.0),
                                        egui::Button::new("1:1").small(),
                                    );
                                });
                                height = ui.cursor().top() - start;
                            })
                        });
                        assert!(FileEditor::current(ctx).is_none());
                    },
                );
                heights.push(height);
            }
            assert!(
                (heights[0] - heights[1]).abs() < 0.1,
                "footer height at {width}: {heights:?}"
            );
        }
    }
}
