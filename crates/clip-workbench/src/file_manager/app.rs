use std::path::PathBuf;

use egui::{Key, Modifiers};
use little_clip_core::{AppConfig, palette::AppPalette};

use super::{FileManager, Navigation};
use crate::{icons::Icon, ui_appearance, ui_ext};

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

impl eframe::App for FileManagerApp {
    fn raw_input_hook(&mut self, ctx: &egui::Context, input: &mut egui::RawInput) {
        if let Some(native) = &mut self.native {
            native.raw_input(ctx, input);
        }
    }
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
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
        egui::CentralPanel::default().show(ctx, |ui| {
            if self.viewers.active() {
                if self.viewers.ui(ui, &self.palette) {
                    self.browser.refresh_after_transfer = true;
                    self.browser.restore_scroll = true;
                }
            } else {
                self.browser.ui(ui, &self.palette);
                if let Some(path) = self.browser.open_file.take() {
                    self.viewers.open(path);
                }
            }
        });
        self.native_start(ctx);
        let close = ctx.input_mut(|input| input.consume_key(Modifiers::CTRL, Key::Q))
            || ctx.input(|input| input.viewport().close_requested());
        if close {
            if self.viewers.active() && !self.viewers.request_close() {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                return;
            }
            if self.browser.filesystem_busy()
                || self.browser.shelf.sending()
                || self.native.as_ref().is_some_and(|native| native.busy())
            {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                self.browser.edits.report = Some(super::edits::EditReport {
                    text: "Wait for the file operation to finish before closing.".into(),
                    failed: false,
                });
            } else {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
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
