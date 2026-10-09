use std::path::PathBuf;

use egui::{Key, Modifiers};
use little_clip_core::{AppConfig, palette::AppPalette};

use super::{FileManager, Navigation};
use crate::{icons::Icon, ui_appearance, ui_ext};

// egui 0.27 lets a CentralPanel frame grow with overflowing children. Bound
// both its background and child painting to the space left by the outer docks.
pub(super) fn central_panel<R>(
    ctx: &egui::Context,
    add_contents: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::InnerResponse<R> {
    let bounds = ctx.available_rect();
    let fill = ctx.style().visuals.panel_fill;
    egui::CentralPanel::default()
        .frame(egui::Frame::none().inner_margin(8.0))
        .show(ctx, |ui| {
            ui.set_clip_rect(ui.clip_rect().intersect(bounds));
            ui.painter().rect_filled(bounds, 0.0, fill);
            add_contents(ui)
        })
}

struct FileManagerApp {
    browser: FileManager,
    viewers: super::viewers::Viewers,
    palette: AppPalette,
    native: Option<super::native_drag::NativeDrag>,
    drop_destinations: std::collections::BTreeMap<u64, PathBuf>,
}

impl FileManagerApp {
    fn new(cc: &eframe::CreationContext<'_>, initial: Option<PathBuf>) -> Self {
        let ctx = &cc.egui_ctx;
        let config = AppConfig::load();
        let palette = ui_appearance::palette(config.workbench.appearance);
        ui_ext::configure_workbench_style(ctx);
        ui_appearance::apply_visuals(ctx, &palette);
        egui_extras::install_image_loaders(ctx);
        let mut browser = FileManager::default();
        browser.music_threshold = config.file_manager.music_threshold_percent.clamp(1, 100);
        browser.video_timeline_decimation = match config.file_manager.video_timeline_decimation {
            value @ (1 | 2 | 4 | 8 | 16) => value,
            _ => 1,
        };
        if let Some(path) = initial {
            browser.navigate(path, Navigation::New);
        }
        Self {
            browser,
            viewers: super::viewers::Viewers::new(config),
            palette,
            native: super::native_drag::NativeDrag::new(cc),
            drop_destinations: Default::default(),
        }
    }
}

impl FileManagerApp {
    fn native_tick(&mut self, ctx: &egui::Context) {
        let Some(native) = &mut self.native else {
            return;
        };
        for event in native.tick() {
            use super::native_drag::Event;
            match event {
                Event::Drop(token, point) => {
                    let point = point / ctx.zoom_factor();
                    if let Some((_, path)) = self
                        .browser
                        .drop_targets
                        .iter()
                        .rev()
                        .find(|(rect, _)| rect.contains(point))
                    {
                        self.drop_destinations.insert(token, path.clone());
                    }
                }
                Event::Files(token, paths, moving) => {
                    let accepted =
                        self.drop_destinations
                            .remove(&token)
                            .is_some_and(|destination| {
                                self.browser
                                    .queue_native_drop(token, paths, destination, moving)
                            });
                    if !accepted {
                        native.finish(token, false);
                    }
                }
                Event::Remove(expected, snapshot) => {
                    self.browser.native_move_cleanup(expected, snapshot)
                }
                Event::Failed(token, text) => {
                    self.drop_destinations.remove(&token);
                    self.browser.edits.report =
                        Some(super::edits::EditReport { text, failed: true });
                }
                Event::Refresh => self.browser.refresh_after_transfer = true,
                Event::Notice(text) => {
                    self.browser.edits.report =
                        Some(super::edits::EditReport { text, failed: true })
                }
            }
        }
        for (token, success) in self.browser.transfers.native_results.drain(..) {
            native.finish(token, success);
        }
        ctx.request_repaint_after(std::time::Duration::from_millis(if native.busy() {
            50
        } else {
            100
        }));
    }
    fn native_start(&mut self, ctx: &egui::Context) {
        let Some(native) = &mut self.native else {
            return;
        };
        let Some(payload) = egui::DragAndDrop::payload::<super::transfers::FileDrag>(ctx) else {
            return;
        };
        let identities = payload
            .paths
            .iter()
            .map(|path| {
                let identity = self
                    .browser
                    .directory
                    .as_ref()
                    .and_then(|dir| dir.entries.iter().find(|e| &e.path == path))
                    .and_then(|e| e.identity);
                (path.clone(), identity)
            })
            .collect();
        match native.start(identities, ctx.input(|i| i.modifiers.ctrl)) {
            Ok(true) => {
                egui::DragAndDrop::clear_payload(ctx);
            }
            Ok(false) => {}
            Err(text) => {
                self.browser.edits.report = Some(super::edits::EditReport { text, failed: true })
            }
        }
    }
}

impl FileManagerApp {
    fn main_content(&mut self, ctx: &egui::Context) {
        central_panel(ctx, |ui| {
            if self.viewers.active() {
                self.browser.video_previews.clear();
                if self.viewers.ui(ui, &self.palette) {
                    self.browser.refresh_after_transfer = true;
                    self.browser.restore_scroll = true;
                }
            } else {
                self.browser.ui(ui, &self.palette);
                if let Some(path) = self.browser.open_file.take() {
                    if little_clip_core::audio::is_audio_path(&path)
                        || little_clip_core::animation::is_video_path(&path)
                        || path.extension().is_some_and(|e| {
                            e.eq_ignore_ascii_case("mid") || e.eq_ignore_ascii_case("midi")
                        })
                    {
                        self.browser.music.stop();
                    }
                    let start = self
                        .browser
                        .video_start
                        .take()
                        .filter(|(source, _)| source == &path);
                    if let Some((_, seconds)) = start {
                        self.viewers.open_video_at(path, seconds);
                    } else {
                        let collection = self.browser.photo_collection(&path);
                        self.viewers.open_collection(path, collection);
                    }
                }
            }
        });
    }
    fn request_app_close(&mut self, ctx: &egui::Context) {
        if self.viewers.active() && !self.viewers.request_close() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            return;
        }
        if self.browser.filesystem_busy()
            || self.browser.music.copying()
            || self.browser.shelf.sending()
            || self.native.as_ref().is_some_and(|native| native.busy())
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.browser.edits.report = Some(super::edits::EditReport {
                text: "Wait for the file operation to finish before closing.".into(),
                failed: false,
            });
        } else {
            self.browser.music.stop();
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}

impl eframe::App for FileManagerApp {
    fn raw_input_hook(&mut self, ctx: &egui::Context, input: &mut egui::RawInput) {
        if let Some(native) = &mut self.native {
            native.raw_input(ctx, input);
        }
    }
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Handle preview keys before address/dialog handlers consume them;
        // typing keeps its normal behavior and stops the preview.
        if ctx.input(|input| input.key_pressed(Key::Escape)) {
            self.browser.music.stop();
        } else if ctx.input(|input| input.key_pressed(Key::Space)) {
            let range = self.browser.music_selection.clone().filter(|_| {
                self.browser.view == super::BrowserView::Music
                    && !self.viewers.active()
                    && !ctx
                        .memory(|memory| memory.focused())
                        .is_some_and(|id| egui::TextEdit::load_state(ctx, id).is_some())
            });
            if let Some(range) = range {
                ctx.input_mut(|input| {
                    input.consume_key(egui::Modifiers::NONE, Key::Space);
                });
                self.browser.music.play_range(range);
            } else {
                self.browser.music.stop();
            }
        }
        if self.viewers.uses_audio() {
            self.browser.music.stop();
        }
        self.browser
            .music
            .tick(ctx, &mut self.browser.music_selection);
        if self.viewers.active() {
            self.browser.drop_targets.clear();
        }
        self.native_tick(ctx);
        if self.viewers.active() {
            self.browser
                .tick(little_clip_core::BackgroundJobs::global());
        }
        self.browser.handle_address_escape(ctx);
        egui::TopBottomPanel::top("file_manager_title").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.add(
                    Icon::Folder.label(egui::RichText::new("Little Screen Clip — Files").strong()),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add(
                        egui::Label::new(if self.viewers.active() {
                            "Back to folder: browse  ·  Ctrl+Q: close"
                        } else {
                            "Ctrl+L: address  ·  F5: refresh  ·  Ctrl+Q: close"
                        })
                        .truncate(true),
                    );
                });
            });
        });
        self.main_content(ctx);
        self.native_start(ctx);
        let close = ctx.input_mut(|input| input.consume_key(Modifiers::CTRL, Key::Q))
            || ctx.input(|input| input.viewport().close_requested());
        if close {
            self.request_app_close(ctx);
        }
    }
}

pub fn run(initial: Option<PathBuf>) -> Result<(), eframe::Error> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_app_id("little-clip-file-manager")
            .with_title("Little Screen Clip — Files")
            .with_inner_size([1100.0, 720.0])
            .with_min_inner_size([640.0, 400.0])
            .with_resizable(true),
        persist_window: false,
        ..Default::default()
    };
    eframe::run_native(
        "clip-file-manager",
        options,
        Box::new(move |cc| Box::new(FileManagerApp::new(cc, initial))),
    )
}
