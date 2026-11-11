//! Embedded video presentation; synchronized audio and decoding stay in the owned worker.
use super::super::{
    media::{VideoMetadata as Prepared, inspect_video},
    video_playback::Session,
};
use crate::file_source::FileSource;
use clip_ui::buttons::ButtonRole;
use egui::Ui;
use std::path::PathBuf;

pub(super) fn prepare(path: &std::path::Path) -> Result<Prepared, String> {
    inspect_video(path)
}

pub(super) struct Video {
    path: PathBuf,
    prepared: Prepared,
    player: Option<Session>,
    start_ms: u64,
    seek_preview: Option<f64>,
    fullscreen: bool,
    stopped: bool,
    context: Option<egui::Context>,
}
impl Video {
    pub fn new(path: PathBuf, prepared: Prepared, start_seconds: f64) -> Self {
        let start_ms = if start_seconds.is_finite() {
            (start_seconds.max(0.0) * 1000.0) as u64
        } else {
            0
        };
        Self {
            path,
            start_ms: start_ms.min(
                prepared
                    .duration_ms
                    .saturating_sub(prepared.frame_interval_ms),
            ),
            prepared,
            player: None,
            seek_preview: None,
            fullscreen: false,
            stopped: false,
            context: None,
        }
    }
    pub(super) fn file_source(&self) -> FileSource {
        self.prepared.source.clone()
    }
    pub(super) fn relocate_file(&mut self, path: PathBuf, source: FileSource) {
        self.path = path;
        self.prepared.source = source.clone();
        if let Some(player) = &self.player {
            player.relocate(source);
        }
    }
    pub(super) fn stop(&mut self) {
        self.player = None;
        self.stopped = true;
    }
    pub fn ui(
        &mut self,
        ui: &mut Ui,
        close: &mut bool,
        palette: &little_clip_core::palette::AppPalette,
    ) {
        self.context = Some(ui.ctx().clone());
        if self.stopped {
            let mut shell =
                crate::shell::StudioShell::new("Video", crate::icons::Icon::Film, palette, close);
            shell.render_header(ui, |_| {});
            if ui
                .add(ButtonRole::Primary.button("Play video", palette))
                .clicked()
            {
                self.stopped = false;
            }
            *close |= ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
            return;
        }
        if self.player.is_none() {
            self.player = Some(Session::new(
                &self.prepared,
                self.start_ms,
                false,
                1280,
                ui.ctx().clone(),
                Default::default(),
            ));
        }
        let player = self.player.as_mut().unwrap();
        player.poll(ui.ctx());
        let mut shell =
            crate::shell::StudioShell::new("Video", crate::icons::Icon::Film, palette, close);
        shell.render_header(ui, |_| {});
        if ui.is_enabled() && !ui.ctx().wants_keyboard_input() {
            if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Space)) {
                player.set_playing(!player.playing());
            }
            for (key, offset) in [
                (egui::Key::ArrowLeft, -5000_i64),
                (egui::Key::ArrowRight, 5000),
            ] {
                if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, key)) {
                    player.seek(player.position_ms.saturating_add_signed(offset));
                }
            }
            if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::F)) {
                self.fullscreen = !self.fullscreen;
                ui.ctx()
                    .send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.fullscreen));
            }
        }
        let size = egui::vec2(
            ui.available_width().max(1.0),
            (ui.available_height() - 62.0).max(1.0),
        );
        let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
        ui.painter().rect_filled(rect, 0.0, egui::Color32::BLACK);
        if let Some(texture) = &player.texture {
            let image = texture.size_vec2();
            let scale = (rect.width() / image.x).min(rect.height() / image.y);
            ui.painter().image(
                texture.id(),
                egui::Rect::from_center_size(rect.center(), image * scale),
                egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        } else if player.error.is_none() {
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "Opening video…",
                egui::FontId::proportional(16.0),
                egui::Color32::LIGHT_GRAY,
            );
        }
        if response.double_clicked() {
            self.fullscreen = !self.fullscreen;
            ui.ctx()
                .send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.fullscreen));
        } else if response.clicked() {
            player.set_playing(!player.playing());
        }
        ui.horizontal(|ui| {
            if ui
                .add(
                    ButtonRole::Primary
                        .button(if player.playing() { "Pause" } else { "Play" }, palette),
                )
                .clicked()
            {
                player.set_playing(!player.playing());
            }
            if ui
                .add(ButtonRole::Secondary.button("Stop", palette))
                .clicked()
            {
                player.set_playing(false);
                player.seek(0);
            }
            let mut seconds = self
                .seek_preview
                .unwrap_or(player.position_ms as f64 / 1000.0);
            let duration = player.duration_ms as f64 / 1000.0;
            let slider = ui.add_sized(
                [(ui.available_width() - 190.0).max(40.0), 22.0],
                egui::Slider::new(&mut seconds, 0.0..=duration.max(0.001)).show_value(false),
            );
            if slider.dragged() {
                self.seek_preview = Some(seconds);
            }
            if slider.drag_stopped() || (slider.changed() && !slider.dragged()) {
                player.seek((seconds * 1000.0) as u64);
                self.seek_preview = None;
            }
            ui.label(format!(
                "{} / {}",
                clock((seconds * 1000.0) as u64),
                clock(player.duration_ms)
            ));
            let mut volume = player.volume();
            if ui
                .add_sized(
                    [72.0, 22.0],
                    egui::Slider::new(&mut volume, 0.0..=1.0).show_value(false),
                )
                .on_hover_text("Volume")
                .changed()
            {
                player.set_volume(volume);
            }
        });
        if let Some(error) = &player.error {
            ui.colored_label(ui.visuals().error_fg_color, error);
        } else {
            ui.small(format!(
                "{} · Space: play/pause · ←/→: seek · F: fullscreen",
                self.prepared.description
            ));
        }
        if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            if self.fullscreen {
                self.fullscreen = false;
                ui.ctx()
                    .send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
            } else {
                *close = true;
            }
        }
    }
}
impl Drop for Video {
    fn drop(&mut self) {
        if self.fullscreen && let Some(context) = &self.context {
            context.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
        }
    }
}

fn clock(ms: u64) -> String {
    let seconds = ms / 1000;
    if seconds >= 3600 {
        format!(
            "{}:{:02}:{:02}",
            seconds / 3600,
            seconds / 60 % 60,
            seconds % 60
        )
    } else {
        format!("{}:{:02}", seconds / 60, seconds % 60)
    }
}
