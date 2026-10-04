use std::path::PathBuf;

use egui::{Key, Modifiers};
use little_clip_core::{AppConfig, palette::AppPalette};

use super::{FileManager, Navigation};
use crate::{icons::Icon, ui_appearance, ui_ext};

struct FileManagerApp {
    browser: FileManager,
    palette: AppPalette,
}

impl FileManagerApp {
    fn new(ctx: &egui::Context, initial: Option<PathBuf>) -> Self {
        let config = AppConfig::load();
        let palette = ui_appearance::palette(config.workbench.appearance);
        ui_ext::configure_workbench_style(ctx);
        ui_appearance::apply_visuals(ctx, &palette);
        egui_extras::install_image_loaders(ctx);
        let mut browser = FileManager::default();
        if let Some(path) = initial {
            browser.navigate(path, Navigation::New);
        }
        Self { browser, palette }
    }
}

impl eframe::App for FileManagerApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.browser.handle_address_escape(ctx);
        egui::TopBottomPanel::top("file_manager_title").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.add(
                    Icon::Folder.label(egui::RichText::new("Little Screen Clip — Files").strong()),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label("Ctrl+L: address  ·  F5: refresh  ·  Ctrl+Q: close");
                });
            });
        });
        egui::CentralPanel::default().show(ctx, |ui| self.browser.ui(ui, &self.palette));
        if ctx.input_mut(|input| input.consume_key(Modifiers::CTRL, Key::Q)) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
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
        Box::new(move |cc| Box::new(FileManagerApp::new(&cc.egui_ctx, initial))),
    )
}
