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
    subtitles_enabled: bool,
    seek_preview: Option<f64>,
    fullscreen: bool,
    stopped: bool,
    last_activity: f64,
    context: Option<egui::Context>,
}
impl Video {
    pub fn new(
        path: PathBuf,
        prepared: Prepared,
        start_seconds: f64,
        subtitles_enabled: bool,
    ) -> Self {
        let start_ms = if start_seconds.is_finite() {
            (start_seconds.max(0.0) * 1000.0) as u64
        } else {
            0
        };
        Self {
            path,
            subtitles_enabled,
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
            last_activity: 0.0,
            context: None,
        }
    }
    pub(super) fn fullscreen(&self) -> bool {
        self.fullscreen
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
                little_clip_core::media_backend::playback::Options {
                    subtitles_enabled: self.subtitles_enabled,
                    ..Default::default()
                },
            ));
        }
        let player = self.player.as_mut().unwrap();
        player.poll(ui.ctx());
        if !self.fullscreen {
            let mut shell =
                crate::shell::StudioShell::new("Video", crate::icons::Icon::Film, palette, close);
            shell.render_header(ui, |_| {});
        }
        let was_fullscreen = self.fullscreen;
        if ui.is_enabled() && !ui.ctx().wants_keyboard_input() {
            shortcuts(ui, player, &mut self.fullscreen);
        }
        let overlay_visible = overlay_visibility(
            ui,
            was_fullscreen != self.fullscreen,
            &mut self.last_activity,
        );
        if self.fullscreen && !overlay_visible {
            ui.ctx().set_cursor_icon(egui::CursorIcon::None);
        }
        let response = picture(ui, player, self.fullscreen);
        let rect = response.rect;
        response.context_menu(|ui| playback_menu(ui, player));
        if response.double_clicked() {
            self.fullscreen = !self.fullscreen;
            ui.ctx()
                .send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.fullscreen));
        } else if response.clicked() {
            player.set_playing(!player.playing());
        }
        if self.fullscreen {
            if overlay_visible || player.error.is_some() {
                let (back, exit) = fullscreen_controls(
                    ui,
                    rect,
                    &self.path,
                    player,
                    &mut self.seek_preview,
                    palette,
                );
                *close |= back;
                self.fullscreen &= !exit;
            }
        } else {
            controls(ui, player, &mut self.seek_preview, palette);
            if let Some(error) = &player.error {
                ui.colored_label(ui.visuals().error_fg_color, error);
            } else {
                ui.small(format!(
                    "{} · Space: play/pause · ←/→: seek · F: fullscreen",
                    self.prepared.description
                ));
            }
        }
        dismiss_key(ui, &mut self.fullscreen, close);
    }
}
fn dismiss_key(ui: &Ui, fullscreen: &mut bool, close: &mut bool) {
    if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
        if *fullscreen {
            *fullscreen = false;
            ui.ctx()
                .send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
        } else {
            *close = true;
        }
    }
}

fn overlay_visibility(ui: &Ui, fullscreen_changed: bool, last_activity: &mut f64) -> bool {
    let now = ui.input(|i| i.time);
    if fullscreen_changed
        || ui.input(|i| {
            i.pointer.delta() != egui::Vec2::ZERO || i.pointer.any_down() || !i.events.is_empty()
        })
    {
        *last_activity = now;
    }
    now - *last_activity < 2.5 || ui.ctx().memory(|m| m.any_popup_open())
}

fn picture(ui: &mut Ui, player: &Session, fullscreen: bool) -> egui::Response {
    let size = egui::vec2(
        ui.available_width().max(1.0),
        (ui.available_height() - if fullscreen { 0.0 } else { 76.0 }).max(1.0),
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
    response
}

fn fullscreen_controls(
    ui: &mut Ui,
    rect: egui::Rect,
    path: &std::path::Path,
    player: &mut Session,
    seek_preview: &mut Option<f64>,
    palette: &little_clip_core::palette::AppPalette,
) -> (bool, bool) {
    let mut back = false;
    let mut exit = false;
    let ctx = ui.ctx().clone();
    egui::Area::new(egui::Id::new("video_fullscreen_title"))
        .order(egui::Order::Foreground)
        .fixed_pos(rect.left_top() + egui::vec2(16.0, 16.0))
        .show(&ctx, |ui| {
            overlay_frame().show(ui, |ui| {
                ui.horizontal(|ui| {
                    if ui
                        .add(ButtonRole::Secondary.button("Back to folder", palette))
                        .clicked()
                    {
                        back = true;
                    }
                    ui.colored_label(
                        egui::Color32::WHITE,
                        path.file_name().unwrap_or_default().to_string_lossy(),
                    );
                    if ui
                        .add(ButtonRole::Secondary.button("Exit fullscreen", palette))
                        .clicked()
                    {
                        exit = true;
                        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
                    }
                });
            });
        });
    egui::Area::new(egui::Id::new("video_fullscreen_transport"))
        .order(egui::Order::Foreground)
        .fixed_pos(egui::pos2(rect.left() + 16.0, rect.bottom() - 64.0))
        .show(&ctx, |ui| {
            overlay_frame().show(ui, |ui| {
                ui.set_width((rect.width() - 48.0).max(100.0));
                ui.visuals_mut().override_text_color = Some(egui::Color32::WHITE);
                controls(ui, player, seek_preview, palette);
                if let Some(error) = &player.error {
                    ui.colored_label(egui::Color32::LIGHT_RED, error);
                }
            });
        });
    (back, exit)
}

fn shortcuts(ui: &mut Ui, player: &mut Session, fullscreen: &mut bool) {
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
        *fullscreen = !*fullscreen;
        ui.ctx()
            .send_viewport_cmd(egui::ViewportCommand::Fullscreen(*fullscreen));
    }
}

fn controls(
    ui: &mut Ui,
    player: &mut Session,
    seek_preview: &mut Option<f64>,
    palette: &little_clip_core::palette::AppPalette,
) {
    ui.horizontal(|ui| {
        let icon = if player.playing() {
            crate::icons::Icon::Pause
        } else {
            crate::icons::Icon::Play
        };
        if ui
            .add(crate::ui_ext::icon_button(
                icon,
                18.0,
                ui.visuals().widgets.inactive.bg_fill,
                None,
                palette,
            ))
            .on_hover_text(if player.playing() { "Pause" } else { "Play" })
            .clicked()
        {
            player.set_playing(!player.playing());
        }
        let mut seconds = (*seek_preview).unwrap_or(player.position_ms as f64 / 1000.0);
        let duration = player.duration_ms as f64 / 1000.0;
        let seek_width = (ui.available_width() - 125.0).max(40.0);
        let slider = ui
            .scope(|ui| {
                ui.spacing_mut().slider_width = seek_width;
                ui.add(egui::Slider::new(&mut seconds, 0.0..=duration.max(0.001)).show_value(false))
            })
            .inner;
        if slider.dragged() {
            *seek_preview = Some(seconds);
        }
        if slider.drag_stopped() || (slider.changed() && !slider.dragged()) {
            player.seek((seconds * 1000.0) as u64);
            *seek_preview = None;
        }
        ui.label(format!(
            "{} / {}",
            clock((seconds * 1000.0) as u64),
            clock(player.duration_ms)
        ));
    });
}

impl Drop for Video {
    fn drop(&mut self) {
        if self.fullscreen
            && let Some(context) = &self.context
        {
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

fn playback_menu(ui: &mut Ui, player: &mut Session) {
    let mut options = player.options().clone();
    ui.menu_button("Subtitles", |ui| {
        ui.checkbox(&mut options.subtitles_enabled, "Enable subtitles");
        if player.tracks.subtitles.is_empty() {
            ui.add_enabled(false, egui::Label::new("No embedded subtitle tracks"));
        }
        for track in &player.tracks.subtitles {
            let selected =
                options.subtitle_track.or(player.tracks.current_subtitle) == Some(track.index);
            if ui
                .selectable_label(selected, format!("{} · {}", track.index + 1, track.label))
                .clicked()
            {
                options.subtitle_track = Some(track.index);
                options.subtitles_enabled = true;
            }
        }
    });
    ui.menu_button("Audio track", |ui| {
        if player.tracks.audio.is_empty() {
            ui.add_enabled(false, egui::Label::new("No audio tracks"));
        }
        for track in &player.tracks.audio {
            let selected = options.audio_track.or(player.tracks.current_audio) == Some(track.index);
            if ui
                .selectable_label(selected, format!("{} · {}", track.index + 1, track.label))
                .clicked()
            {
                options.audio_track = Some(track.index);
            }
        }
    });
    ui.menu_button("Playback speed", |ui| {
        for rate in little_clip_core::media_backend::playback::RATES {
            ui.selectable_value(&mut options.rate, rate, format!("{rate}×"));
        }
    });
    ui.checkbox(&mut options.looping, "Loop video");
    player.set_options(options);
}

fn overlay_frame() -> egui::Frame {
    egui::Frame::none()
        .fill(egui::Color32::from_black_alpha(160))
        .inner_margin(8.0)
        .rounding(6.0)
}
