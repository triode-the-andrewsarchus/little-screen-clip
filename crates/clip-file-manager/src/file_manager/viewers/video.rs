//! Embedded video presentation; synchronized audio and decoding stay in the owned worker.
use super::super::{
    media::{VideoMetadata as Prepared, inspect_video},
    preview_copy::Copies,
    ui::video::paint_timelens,
    video_playback::{AudioMode, Session, Span},
    video_timeline::{Timeline, selection::FrameCopy},
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
    transport: Transport,
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
            transport: Transport::default(),
            fullscreen: false,
            stopped: false,
            last_activity: 0.0,
            context: None,
        }
    }
    pub(super) fn set_timeline(&mut self, timeline: Timeline) {
        let metadata = &timeline.source.metadata;
        if metadata.source == self.prepared.source && metadata.stream == self.prepared.stream {
            self.transport.timeline = Some(timeline);
        }
    }
    pub(super) fn notify_copy(&mut self, ctx: &egui::Context, text: &str) {
        self.transport.notice = Some((text.to_owned(), ctx.input(|input| input.time) + 4.0));
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
        self.transport.timeline = None;
        if let Some(player) = &self.player {
            player.relocate(source);
        }
    }
    pub(super) fn stop(&mut self) {
        self.player = None;
        self.stopped = true;
    }
    pub(in crate::file_manager) fn tick(&mut self, ctx: &egui::Context) {
        self.context = Some(ctx.clone());
        if self.stopped {
            return;
        }
        if self.player.is_none() {
            self.player = Some(Session::new(
                &self.prepared,
                Span::From(self.start_ms),
                AudioMode::Audible,
                1280,
                ctx.clone(),
                Default::default(),
                little_clip_core::media_backend::playback::Options {
                    subtitles_enabled: self.subtitles_enabled,
                    ..Default::default()
                },
            ));
        }
        if let Some(player) = &mut self.player {
            player.poll(ctx);
        }
    }

    pub fn ui(
        &mut self,
        ui: &mut Ui,
        close: &mut bool,
        palette: &little_clip_core::palette::AppPalette,
        copies: &mut Copies,
    ) {
        if self.stopped {
            let mut shell =
                crate::shell::StudioShell::new("Video", crate::icons::Icon::Film, palette, close);
            shell.render_header(ui, |_| {});
            if ui
                .add(ButtonRole::Primary.button("Play video", palette))
                .clicked()
            {
                self.stopped = false;
                ui.ctx().request_repaint();
            }
            *close |= ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
            return;
        }
        let Some(player) = &mut self.player else {
            ui.spinner();
            ui.label("Opening video…");
            return;
        };
        if !self.fullscreen {
            let mut shell =
                crate::shell::StudioShell::new("Video", crate::icons::Icon::Film, palette, close);
            shell.render_header(ui, |_| {});
        }
        let was_fullscreen = self.fullscreen;
        if ui.is_enabled() && !ui.ctx().egui_wants_keyboard_input() {
            shortcuts(ui, player, &mut self.fullscreen);
        }
        let response = picture(ui, player, self.fullscreen);
        let rect = response.rect;
        let menu_was_open = response.context_menu_opened() || egui::Popup::is_any_open(ui.ctx());
        response.context_menu(|ui| playback_menu(ui, player));
        let overlay_visible = overlay_visibility(
            ui,
            was_fullscreen != self.fullscreen,
            &mut self.last_activity,
        ) || response.context_menu_opened();
        if self.fullscreen && !overlay_visible {
            ui.ctx().set_cursor_icon(egui::CursorIcon::None);
        }
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
                    &mut self.transport,
                    palette,
                    copies,
                );
                *close |= back;
                self.fullscreen &= !exit;
            }
        } else {
            controls(ui, player, &mut self.transport, palette, copies);
            if let Some(error) = &player.error {
                ui.colored_label(ui.visuals().error_fg_color, error);
            } else {
                ui.small(format!(
                    "{} · Space: play/pause · ←/→: seek · F: fullscreen",
                    self.prepared.description
                ));
            }
        }
        dismiss_key(ui, &mut self.fullscreen, close, menu_was_open);
    }
}
fn dismiss_key(ui: &Ui, fullscreen: &mut bool, close: &mut bool, menu_was_open: bool) {
    if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
        if menu_was_open {
            egui::Popup::close_all(ui.ctx());
        } else if *fullscreen {
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
    now - *last_activity < 0.5 || egui::Popup::is_any_open(ui.ctx())
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
    transport: &mut Transport,
    palette: &little_clip_core::palette::AppPalette,
    copies: &mut Copies,
) -> (bool, bool) {
    let mut back = false;
    let mut exit = false;
    let ctx = ui.ctx().clone();
    egui::Area::new(egui::Id::unique("video_fullscreen_title"))
        .order(egui::Order::Middle)
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
    egui::Area::new(egui::Id::unique("video_fullscreen_transport"))
        .order(egui::Order::Middle)
        .fixed_pos(egui::pos2(rect.left() + 16.0, rect.bottom() - 64.0))
        .show(&ctx, |ui| {
            overlay_frame().show(ui, |ui| {
                ui.set_width((rect.width() - 48.0).max(100.0));
                ui.visuals_mut().override_text_color = Some(egui::Color32::WHITE);
                controls(ui, player, transport, palette, copies);
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
    transport: &mut Transport,
    palette: &little_clip_core::palette::AppPalette,
    copies: &mut Copies,
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
        let seconds = transport.seek_ui(ui, player, copies);
        ui.label(format!(
            "{} / {}",
            clock((seconds * 1000.0) as u64),
            clock(player.duration_ms)
        ));
    });
}

#[derive(Default)]
struct Transport {
    timeline: Option<Timeline>,
    seek_preview: Option<f64>,
    notice: Option<(String, f64)>,
}

impl Transport {
    fn seek_ui(&mut self, ui: &mut Ui, player: &mut Session, copies: &mut Copies) -> f64 {
        let mut seconds = self
            .seek_preview
            .unwrap_or(player.position_ms as f64 / 1000.0);
        let duration = (player.duration_ms as f64 / 1000.0).max(0.001);
        let width = (ui.available_width() - 125.0).max(40.0);
        let seek = if let Some(timeline) = &self.timeline {
            let (rect, response) =
                ui.allocate_exact_size(egui::vec2(width, 28.0), egui::Sense::click_and_drag());
            paint_timelens(ui, rect, &timeline.texture);
            let (seconds_at, cursor) =
                timeline_pointer(rect, response.interact_pointer_pos(), duration);
            if response.secondary_clicked() {
                let time_ms = (seconds_at.unwrap_or(seconds) * 1000.0).round() as u64;
                let frame = match (&player.image, player.frame_position_ms) {
                    (Some(image), Some(displayed_ms))
                        if displayed_ms.abs_diff(time_ms)
                            <= (duration * 1000.0 / f64::from(width)) as u64 =>
                    {
                        FrameCopy::Displayed(image.clone(), displayed_ms)
                    }
                    _ => FrameCopy::Position(timeline.source.clone(), time_ms),
                };
                copies.start(ui.ctx(), move |cancel| frame.copy(cancel));
            } else if let Some(value) = seconds_at.filter(|_| {
                response.clicked()
                    || response.dragged_by(egui::PointerButton::Primary)
                    || response.drag_stopped_by(egui::PointerButton::Primary)
            }) {
                seconds = value;
            }
            paint_playhead(ui, rect, (seconds / duration) as f32, cursor);
            response.surrender_focus();
            let notice = self
                .notice
                .as_ref()
                .filter(|(_, until)| ui.input(|input| input.time < *until))
                .map(|(text, _)| text.as_str());
            let hint = if copies.busy() {
                "Copying frame…"
            } else {
                "Click or drag to seek · Right-click to copy a frame"
            };
            let changed = response.clicked();
            let response = response.on_hover_text(notice.unwrap_or(hint));
            (response, changed)
        } else {
            let response = ui
                .scope(|ui| {
                    ui.spacing_mut().slider_width = width;
                    ui.add(egui::Slider::new(&mut seconds, 0.0..=duration).show_value(false))
                })
                .inner;
            let changed = response.changed();
            (response, changed)
        };
        let (response, changed) = seek;
        let dragging = response.dragged_by(egui::PointerButton::Primary);
        if dragging {
            self.seek_preview = Some(seconds);
        }
        if response.drag_stopped_by(egui::PointerButton::Primary) || (changed && !dragging) {
            player.seek((seconds * 1000.0).round() as u64);
            self.seek_preview = None;
        }
        seconds
    }
}

fn timeline_pointer(
    rect: egui::Rect,
    pointer: Option<egui::Pos2>,
    duration: f64,
) -> (Option<f64>, Option<f32>) {
    let fraction = pointer.map(|point| ((point.x - rect.left()) / rect.width()).clamp(0.0, 1.0));
    (fraction.map(|x| f64::from(x) * duration), fraction)
}

fn paint_playhead(ui: &Ui, rect: egui::Rect, fraction: f32, cursor: Option<f32>) {
    for (position, color, width) in cursor
        .map(|x| (x, egui::Color32::WHITE, 1.0))
        .into_iter()
        .chain([(fraction, ui.visuals().selection.stroke.color, 2.0)])
    {
        let x = egui::lerp(rect.x_range(), position.clamp(0.0, 1.0));
        ui.painter().line_segment(
            [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
            egui::Stroke::new(width, color),
        );
    }
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
    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        ui.close();
        return;
    }
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
    egui::Frame::NONE
        .fill(egui::Color32::from_black_alpha(160))
        .inner_margin(8.0)
        .corner_radius(6.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn escape_dismisses_the_menu_before_fullscreen_or_the_viewer() {
        let ctx = egui::Context::default();
        let mut fullscreen = true;
        let mut close = false;
        for (menu_was_open, expected_fullscreen, expected_close) in [
            (true, true, false),
            (false, false, false),
            (false, false, true),
        ] {
            let mut input = egui::RawInput::default();
            input.events.push(egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            });
            let _ = clip_ui::test_support::run_frame(&ctx, input, |ctx| {
                ctx.root_ui(|ui| {
                    egui::CentralPanel::default().show(ui, |ui| {
                        dismiss_key(ui, &mut fullscreen, &mut close, menu_was_open)
                    })
                });
            });
            assert_eq!((fullscreen, close), (expected_fullscreen, expected_close));
        }
    }
}
