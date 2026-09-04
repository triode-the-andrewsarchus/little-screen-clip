use eframe::egui::{self, Color32, Pos2, Rect, Rounding, Stroke, Vec2};
use little_clip_core::audio::{compute_full_waveform, read_audio_metadata, AudioMetadata, AudioWaveform};
use little_clip_core::ipc::IpcRequest;
use rodio::stream::{DeviceSinkBuilder, MixerDeviceSink};
use rodio::{Decoder, Player};
use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq)]
enum DragAction {
    ScrubRuler,
    DragLeftHandle { other_anchor: f32 },
    DragRightHandle { other_anchor: f32 },
    CreateSelection {
        start_time: f32,
        start_pos_x: f32,
        has_moved: bool,
    },
}

pub struct AudioPreviewState {
    pub path: PathBuf,
    pub title: String,
    pub metadata: Option<AudioMetadata>,
    pub waveform: Option<AudioWaveform>,
    pub waveform_rx: Option<Receiver<Result<AudioWaveform, String>>>,
    pub is_loading_waveform: bool,
    pub loading_start_time: Instant,
    pub duration_sec: f32,

    // Audio Playback Engine (rodio)
    player: Option<Player>,
    _sink: Option<MixerDeviceSink>,
    pub is_playing: bool,
    playback_start_instant: Option<Instant>,
    playback_start_offset_sec: f32,
    pub volume: f32,

    // Playhead & Region Selection
    pub current_time_sec: f32,
    pub selection: Option<(f32, f32)>, // (start_sec, end_sec)
    active_drag: Option<DragAction>,
    pub loop_selection: bool,

    // Toast Notification
    pub status_toast: Option<(String, f64)>,
}

impl AudioPreviewState {
    pub fn new(path: PathBuf, title: String) -> Self {
        let meta = read_audio_metadata(&path);
        let duration_sec = meta
            .as_ref()
            .map(|m| m.duration_ms as f32 / 1000.0)
            .unwrap_or(0.0);

        // Spawn background worker for full-length waveform generation
        let (tx, rx) = channel();
        let path_clone = path.clone();
        std::thread::spawn(move || {
            let num_bars = 1200;
            if let Some(wf) = compute_full_waveform(&path_clone, num_bars) {
                let _ = tx.send(Ok(wf));
            } else {
                let _ = tx.send(Err("Failed to decode audio waveform".to_string()));
            }
        });

        // Initialize rodio playback engine
        let mut sink = DeviceSinkBuilder::open_default_sink().ok();
        if let Some(ref mut s) = sink {
            s.log_on_drop(false);
        }
        let player = sink.as_ref().map(|s| Player::connect_new(s.mixer()));
        if let Some(ref p) = player {
            p.set_volume(0.8);
        }

        Self {
            path,
            title,
            metadata: meta,
            waveform: None,
            waveform_rx: Some(rx),
            is_loading_waveform: true,
            loading_start_time: Instant::now(),
            duration_sec,
            player,
            _sink: sink,
            is_playing: false,
            playback_start_instant: None,
            playback_start_offset_sec: 0.0,
            volume: 0.8,
            current_time_sec: 0.0,
            selection: None,
            active_drag: None,
            loop_selection: true,
            status_toast: None,
        }
    }

    pub fn toggle_play(&mut self) {
        if self.is_playing {
            self.pause();
        } else {
            self.play();
        }
    }

    pub fn play(&mut self) {
        if self.player.is_none() {
            return;
        }

        // If selection exists and playhead is outside, jump to selection start
        if let Some((start, end)) = self.selection {
            if self.current_time_sec < start || self.current_time_sec >= end {
                self.current_time_sec = start;
            }
        }

        // If at end of track, wrap to start
        if self.current_time_sec >= self.duration_sec - 0.05 {
            self.current_time_sec = self.selection.map(|(s, _)| s).unwrap_or(0.0);
        }

        if let Some(ref p) = self.player {
            p.clear();
            if let Ok(file) = File::open(&self.path) {
                if let Ok(decoder) = Decoder::new(BufReader::new(file)) {
                    p.append(decoder);
                    let target_pos = Duration::from_secs_f32(self.current_time_sec);
                    let _ = p.try_seek(target_pos);
                    p.play();
                    self.is_playing = true;
                    self.playback_start_instant = Some(Instant::now());
                    self.playback_start_offset_sec = self.current_time_sec;
                }
            }
        }
    }

    pub fn pause(&mut self) {
        if let Some(ref p) = self.player {
            p.pause();
        }
        self.is_playing = false;
        self.playback_start_instant = None;
    }

    pub fn stop(&mut self) {
        if let Some(ref p) = self.player {
            p.stop();
        }
        self.is_playing = false;
        self.playback_start_instant = None;
        self.current_time_sec = self.selection.map(|(s, _)| s).unwrap_or(0.0);
    }

    pub fn seek_to(&mut self, target_sec: f32) {
        let clamped = target_sec.clamp(0.0, self.duration_sec);
        self.current_time_sec = clamped;
        self.playback_start_offset_sec = clamped;
        self.playback_start_instant = Some(Instant::now());

        if let Some(ref p) = self.player {
            if self.is_playing {
                let target_pos = Duration::from_secs_f32(clamped);
                if p.try_seek(target_pos).is_err() {
                    // Fallback reload and seek
                    p.clear();
                    if let Ok(file) = File::open(&self.path) {
                        if let Ok(decoder) = Decoder::new(BufReader::new(file)) {
                            p.append(decoder);
                            let _ = p.try_seek(target_pos);
                            p.play();
                        }
                    }
                }
            }
        }
    }

    pub fn copy_selected_segment(&mut self, now: f64) {
        let (start, end) = self
            .selection
            .unwrap_or((0.0, self.duration_sec.max(0.1)));

        let duration = (end - start).max(0.001);
        let base_name = self
            .path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("audio");

        let staging_dir = std::env::temp_dir().join("little_clip_audio_segments");
        let _ = std::fs::create_dir_all(&staging_dir);
        let out_filename = format!("{}_segment_{:.3}s_{:.3}s.wav", base_name, start, end);
        let out_path = staging_dir.join(out_filename);

        // Extract slice using ffmpeg
        let status = std::process::Command::new("ffmpeg")
            .arg("-y")
            .arg("-ss")
            .arg(format!("{:.3}", start))
            .arg("-to")
            .arg(format!("{:.3}", end))
            .arg("-i")
            .arg(&self.path)
            .arg("-c:a")
            .arg("pcm_s16le")
            .arg(&out_path)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();

        if let Ok(s) = status {
            if s.success() && out_path.exists() {
                // 1. Copy to Wayland/X11 system clipboard
                let path_str = out_path.to_string_lossy().to_string();
                let uri_list = format!("file://{}\n", path_str);
                let _ = std::process::Command::new("wl-copy")
                    .arg("-t")
                    .arg("text/uri-list")
                    .arg(&uri_list)
                    .spawn();
                let _ = std::process::Command::new("wl-copy").arg(&path_str).spawn();

                // 2. Dispatch IPC Push to daemon so Left Hand updates with the new audio item
                let req = IpcRequest::PushFile {
                    path: out_path.clone(),
                    hand: little_clip_core::HandSide::Left,
                };
                let _ = crate::send_ipc_request(&req);

                self.status_toast = Some((
                    format!(
                        "📋 Copied slice ({:02}:{:04.1} - {:02}:{:04.1}, {:.3}s) to Clipboard & Left Hand!",
                        (start / 60.0) as u32,
                        start % 60.0,
                        (end / 60.0) as u32,
                        end % 60.0,
                        sel_duration_label(duration)
                    ),
                    now,
                ));
            } else {
                self.status_toast = Some(("⚠️ Failed to extract audio segment via ffmpeg".to_string(), now));
            }
        } else {
            self.status_toast = Some(("⚠️ ffmpeg process failed to execute".to_string(), now));
        }
    }

    pub fn render_ui(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        close_requested: &mut bool,
    ) {
        let now = ctx.input(|i| i.time);

        // Check for completed waveform background task
        if self.is_loading_waveform {
            ctx.request_repaint();
            if let Some(ref rx) = self.waveform_rx {
                if let Ok(res) = rx.try_recv() {
                    match res {
                        Ok(wf) => {
                            self.duration_sec = wf.duration_ms as f32 / 1000.0;
                            self.waveform = Some(wf);
                            self.is_loading_waveform = false;
                            self.waveform_rx = None;
                        }
                        Err(err) => {
                            self.status_toast = Some((err, now));
                            self.is_loading_waveform = false;
                            self.waveform_rx = None;
                        }
                    }
                }
            }
        }

        // Advance playhead if playing
        if self.is_playing {
            ctx.request_repaint();
            if let Some(start_inst) = self.playback_start_instant {
                let elapsed = start_inst.elapsed().as_secs_f32();
                let new_time = self.playback_start_offset_sec + elapsed;

                // Handle looping within region selection
                if let Some((sel_start, sel_end)) = self.selection {
                    if self.loop_selection && new_time >= sel_end {
                        self.seek_to(sel_start);
                    } else if new_time >= self.duration_sec {
                        self.pause();
                        self.current_time_sec = self.duration_sec;
                    } else {
                        self.current_time_sec = new_time;
                    }
                } else if new_time >= self.duration_sec {
                    self.pause();
                    self.current_time_sec = self.duration_sec;
                } else {
                    self.current_time_sec = new_time;
                }
            }
        }

        // Clear focus from background palette inputs so they cannot swallow shortcuts
        ctx.memory_mut(|m| {
            if let Some(id) = m.focused() {
                m.surrender_focus(id);
            }
        });

        // Global Keybindings for Audio Preview
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.stop();
            *close_requested = true;
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Space)) {
            self.toggle_play();
        }

        let copy_requested = ctx.input(|i| {
            i.events.iter().any(|e| matches!(e, egui::Event::Copy))
                || i.raw.events.iter().any(|e| matches!(e, egui::Event::Copy))
                || (i.key_pressed(egui::Key::C) && (i.modifiers.command || i.modifiers.ctrl || i.modifiers.mac_cmd))
        }) || ctx.input_mut(|i| {
            i.consume_key(egui::Modifiers::COMMAND, egui::Key::C)
                || i.consume_key(egui::Modifiers::CTRL, egui::Key::C)
        });

        if copy_requested {
            self.copy_selected_segment(now);
        }
        if ctx.input(|i| i.key_pressed(egui::Key::ArrowLeft)) {
            let step = if ctx.input(|i| i.modifiers.shift) { 5.0 } else { 1.0 };
            self.seek_to(self.current_time_sec - step);
        }
        if ctx.input(|i| i.key_pressed(egui::Key::ArrowRight)) {
            let step = if ctx.input(|i| i.modifiers.shift) { 5.0 } else { 1.0 };
            self.seek_to(self.current_time_sec + step);
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Home) || i.key_pressed(egui::Key::Num0)) {
            self.seek_to(0.0);
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Backspace) || i.key_pressed(egui::Key::Delete)) {
            self.selection = None;
        }

        let full_rect = ui.available_rect_before_wrap();
        ui.allocate_ui_at_rect(full_rect, |ui| {
            // Dark studio background surface
            ui.painter().rect_filled(
                full_rect,
                Rounding::ZERO,
                Color32::from_rgba_unmultiplied(11, 15, 25, 252),
            );

            ui.add_space(10.0);

            // 1. Top Control Header
            ui.horizontal(|ui| {
                ui.add_space(14.0);
                ui.label(
                    egui::RichText::new("🎵 Audio Waveform Preview & Slicer")
                        .heading()
                        .strong()
                        .color(Color32::WHITE),
                );

                ui.add_space(10.0);

                // File metadata badge
                let mins = (self.duration_sec / 60.0) as u32;
                let secs = self.duration_sec % 60.0;
                let meta_str = if let Some(meta) = &self.metadata {
                    format!(
                        "{} • {:02}:{:04.1} • {:.1} kHz • {}",
                        self.title,
                        mins,
                        secs,
                        meta.sample_rate as f32 / 1000.0,
                        meta.format
                    )
                } else {
                    format!("{} • {:02}:{:04.1}", self.title, mins, secs)
                };

                let meta_badge = egui::Button::new(
                    egui::RichText::new(meta_str)
                        .small()
                        .strong()
                        .color(Color32::from_rgb(148, 163, 184)),
                )
                .fill(Color32::from_rgb(30, 41, 59))
                .stroke(Stroke::NONE)
                .rounding(Rounding::same(4.0));
                ui.add(meta_badge);

                // Right-aligned close button
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add_space(14.0);
                    let close_btn = egui::Button::new(
                        egui::RichText::new("✕ Close (Esc)")
                            .strong()
                            .color(Color32::WHITE),
                    )
                    .fill(Color32::from_rgb(220, 38, 38))
                    .rounding(Rounding::same(6.0));

                    if ui.add(close_btn).clicked() {
                        self.stop();
                        *close_requested = true;
                    }

                    if let Some((msg, ts)) = &self.status_toast {
                        if now - ts < 4.0 {
                            ui.label(
                                egui::RichText::new(msg)
                                    .color(Color32::from_rgb(52, 211, 153))
                                    .small()
                                    .strong(),
                            );
                        }
                    }
                });
            });

            ui.add_space(10.0);

            // 2. Transport & Slicing Control Bar
            ui.horizontal(|ui| {
                ui.add_space(14.0);

                // Play / Pause Button
                let play_icon = if self.is_playing { "⏸ Pause [Space]" } else { "▶ Play [Space]" };
                let play_color = if self.is_playing {
                    Color32::from_rgb(245, 158, 11) // Amber
                } else {
                    Color32::from_rgb(16, 185, 129) // Emerald
                };

                let play_btn = egui::Button::new(
                    egui::RichText::new(play_icon)
                        .strong()
                        .color(Color32::WHITE),
                )
                .fill(play_color)
                .rounding(Rounding::same(6.0));

                if ui.add(play_btn).clicked() {
                    self.toggle_play();
                }

                ui.add_space(4.0);

                // Stop Button
                let stop_btn = egui::Button::new(
                    egui::RichText::new("⏹ Stop")
                        .color(Color32::from_rgb(226, 232, 240)),
                )
                .fill(Color32::from_rgb(30, 41, 59))
                .rounding(Rounding::same(6.0));
                if ui.add(stop_btn).clicked() {
                    self.stop();
                }

                // Replay Button
                let replay_btn = egui::Button::new(
                    egui::RichText::new("⟲ Start")
                        .color(Color32::from_rgb(226, 232, 240)),
                )
                .fill(Color32::from_rgb(30, 41, 59))
                .rounding(Rounding::same(6.0));
                if ui.add(replay_btn).clicked() {
                    self.seek_to(self.selection.map(|(s, _)| s).unwrap_or(0.0));
                }

                ui.add_space(12.0);

                // Time Display
                let cur_m = (self.current_time_sec / 60.0) as u32;
                let cur_s = self.current_time_sec % 60.0;
                let tot_m = (self.duration_sec / 60.0) as u32;
                let tot_s = self.duration_sec % 60.0;

                ui.label(
                    egui::RichText::new(format!("{:02}:{:05.2} / {:02}:{:05.2}", cur_m, cur_s, tot_m, tot_s))
                        .color(Color32::from_rgb(56, 189, 248))
                        .strong()
                        .monospace(),
                );

                ui.add_space(12.0);

                // Volume slider
                ui.label(egui::RichText::new("🔊").color(Color32::from_rgb(148, 163, 184)));
                let mut vol_pct = self.volume * 100.0;
                let slider = egui::Slider::new(&mut vol_pct, 0.0..=100.0)
                    .show_value(false);
                if ui.add_sized([70.0, 18.0], slider).changed() {
                    self.volume = (vol_pct / 100.0).clamp(0.0, 1.0);
                    if let Some(ref p) = self.player {
                        p.set_volume(self.volume);
                    }
                }

                ui.add_space(16.0);

                // Selection Pill & Actions
                if let Some((start, end)) = self.selection {
                    let sel_dur = end - start;
                    let sel_str = format!(
                        "⚡ Slice: {:02}:{:04.1} - {:02}:{:04.1} ({:.2}s)",
                        (start / 60.0) as u32,
                        start % 60.0,
                        (end / 60.0) as u32,
                        end % 60.0,
                        sel_dur
                    );

                    let sel_badge = egui::Button::new(
                        egui::RichText::new(sel_str)
                            .small()
                            .strong()
                            .color(Color32::from_rgb(167, 243, 208)),
                    )
                    .fill(Color32::from_rgb(6, 78, 59))
                    .stroke(Stroke::NONE)
                    .rounding(Rounding::same(4.0));
                    ui.add(sel_badge);

                    let clear_btn = egui::Button::new(
                        egui::RichText::new("✕ Clear")
                            .small()
                            .color(Color32::from_rgb(148, 163, 184)),
                    )
                    .fill(Color32::from_rgb(30, 41, 59))
                    .rounding(Rounding::same(4.0));
                    if ui.add(clear_btn).clicked() {
                        self.selection = None;
                    }
                } else {
                    ui.label(
                        egui::RichText::new("Drag on waveform to select slice")
                            .small()
                            .color(Color32::from_rgb(148, 163, 184)),
                    );
                }

                // Copy Segment Button
                let copy_label = if self.selection.is_some() {
                    "📋 Copy Slice [Ctrl+C]"
                } else {
                    "📋 Copy Track [Ctrl+C]"
                };

                let copy_btn = egui::Button::new(
                    egui::RichText::new(copy_label)
                        .strong()
                        .color(Color32::WHITE),
                )
                .fill(Color32::from_rgb(99, 102, 241))
                .rounding(Rounding::same(6.0));

                if ui.add(copy_btn).clicked() {
                    self.copy_selected_segment(now);
                }
            });

            ui.add_space(12.0);

            // 3. Main Waveform Canvas Area
            let avail_w = ui.available_width() - 28.0;
            let avail_h = (ui.available_height() - 40.0).max(180.0);

            ui.horizontal(|ui| {
                ui.add_space(14.0);
                self.render_waveform_canvas(ui, ctx, avail_w, avail_h);
            });

            // 4. Footer Help Bar
            ui.with_layout(egui::Layout::bottom_up(egui::Align::Center), |ui| {
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("Hotkeys:")
                            .small()
                            .strong()
                            .color(Color32::from_rgb(148, 163, 184)),
                    );
                    ui.label(
                        egui::RichText::new("[Space] Play/Pause • [Ctrl+C] Copy Slice • [Click] Scrub/Seek • [Drag] Select Region • [←/→] ±1s • [Esc] Close")
                            .small()
                            .color(Color32::from_rgb(100, 116, 139)),
                    );
                });
            });
        });
    }

    fn render_waveform_canvas(
        &mut self,
        ui: &mut egui::Ui,
        _ctx: &egui::Context,
        width: f32,
        height: f32,
    ) {
        let (rect, response) = ui.allocate_exact_size(Vec2::new(width, height), egui::Sense::click_and_drag());
        let painter = ui.painter_at(rect);

        // Frame border & background
        painter.rect_filled(
            rect,
            Rounding::same(8.0),
            Color32::from_rgb(15, 20, 32),
        );
        painter.rect_stroke(
            rect,
            Rounding::same(8.0),
            Stroke::new(1.0, Color32::from_rgb(30, 41, 59)),
        );

        if self.is_loading_waveform {
            let elapsed = self.loading_start_time.elapsed().as_secs_f32();
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                format!("⏳ Decoding full audio waveform... ({:.1}s)", elapsed),
                egui::FontId::proportional(15.0),
                Color32::from_rgb(148, 163, 184),
            );
            return;
        }

        let wf = match &self.waveform {
            Some(w) => w,
            None => {
                painter.text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    "⚠️ Unable to render audio waveform",
                    egui::FontId::proportional(15.0),
                    Color32::from_rgb(248, 113, 113),
                );
                return;
            }
        };

        let dur = self.duration_sec.max(0.01);
        let time_to_x = |t: f32| rect.min.x + (t / dur).clamp(0.0, 1.0) * rect.width();
        let x_to_time = |x: f32| ((x - rect.min.x) / rect.width()).clamp(0.0, 1.0) * dur;

        let ruler_h = 24.0;
        let ruler_rect = Rect::from_min_max(Pos2::new(rect.min.x, rect.min.y), Pos2::new(rect.max.x, rect.min.y + ruler_h));

        // Draw Time Ruler Background
        painter.rect_filled(
            ruler_rect,
            Rounding { nw: 8.0, ne: 8.0, sw: 0.0, se: 0.0 },
            Color32::from_rgb(20, 26, 42),
        );
        painter.line_segment(
            [Pos2::new(rect.min.x, rect.min.y + ruler_h), Pos2::new(rect.max.x, rect.min.y + ruler_h)],
            Stroke::new(1.0, Color32::from_rgb(38, 48, 70)),
        );

        // Draw Time Grid Lines & Ruler Markers
        let interval_sec = if dur > 300.0 {
            60.0
        } else if dur > 120.0 {
            30.0
        } else if dur > 30.0 {
            10.0
        } else if dur > 10.0 {
            5.0
        } else {
            1.0
        };

        let mut t = 0.0f32;
        while t <= dur {
            let x = time_to_x(t);
            // Grid line through waveform
            painter.line_segment(
                [Pos2::new(x, rect.min.y + ruler_h), Pos2::new(x, rect.max.y)],
                Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 255, 255, 12)),
            );
            // Tick on ruler
            painter.line_segment(
                [Pos2::new(x, rect.min.y + 16.0), Pos2::new(x, rect.min.y + ruler_h)],
                Stroke::new(1.0, Color32::from_rgb(75, 85, 99)),
            );

            let tm_min = (t / 60.0) as u32;
            let tm_sec = (t % 60.0) as u32;
            painter.text(
                Pos2::new(x + 3.0, rect.min.y + 11.0),
                egui::Align2::LEFT_CENTER,
                format!("{:02}:{:02}", tm_min, tm_sec),
                egui::FontId::monospace(10.0),
                Color32::from_rgb(148, 163, 184),
            );

            t += interval_sec;
        }

        // Draw Center Baseline in Waveform Area
        let wave_center_y = (rect.min.y + ruler_h + rect.max.y) / 2.0;
        painter.line_segment(
            [Pos2::new(rect.min.x, wave_center_y), Pos2::new(rect.max.x, wave_center_y)],
            Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 255, 255, 25)),
        );

        // Draw Waveform Bars
        let is_stereo = wf.channels >= 2 && wf.ch2_peaks.is_some();
        let peaks1 = &wf.ch1_peaks;
        let peaks2 = wf.ch2_peaks.as_ref();
        let num_bars = peaks1.len();

        let bar_w = (rect.width() / num_bars as f32).max(1.0);
        let max_half_h = ((rect.max.y - (rect.min.y + ruler_h)) / 2.0) - 8.0;

        for i in 0..num_bars {
            let bar_x = rect.min.x + i as f32 * bar_w;
            let t_bar = (i as f32 / num_bars as f32) * dur;

            // Highlight bar if within selection
            let is_in_selection = if let Some((s, e)) = self.selection {
                t_bar >= s && t_bar <= e
            } else {
                false
            };

            let p1 = peaks1[i].clamp(0.0, 1.0);
            let h1 = p1 * max_half_h;

            let col1 = if is_in_selection {
                Color32::from_rgb(52, 211, 153) // Vibrant Emerald
            } else {
                Color32::from_rgb(56, 189, 248) // Electric Cyan
            };

            if is_stereo {
                let p2 = peaks2.map(|p| p[i].clamp(0.0, 1.0)).unwrap_or(p1);
                let h2 = p2 * max_half_h;

                let col2 = if is_in_selection {
                    Color32::from_rgb(110, 231, 183)
                } else {
                    Color32::from_rgb(129, 140, 248) // Violet
                };

                // Channel 1: Up from center
                painter.rect_filled(
                    Rect::from_min_size(Pos2::new(bar_x, wave_center_y - h1), Vec2::new(bar_w.max(1.0), h1)),
                    Rounding::same(1.0),
                    col1,
                );
                // Channel 2: Down from center
                painter.rect_filled(
                    Rect::from_min_size(Pos2::new(bar_x, wave_center_y), Vec2::new(bar_w.max(1.0), h2)),
                    Rounding::same(1.0),
                    col2,
                );
            } else {
                // Mono: Symmetric around center
                painter.rect_filled(
                    Rect::from_min_size(Pos2::new(bar_x, wave_center_y - h1), Vec2::new(bar_w.max(1.0), h1 * 2.0)),
                    Rounding::same(1.0),
                    col1,
                );
            }
        }

        // Draw Selected Region Overlay
        if let Some((start, end)) = self.selection {
            let x_start = time_to_x(start);
            let x_end = time_to_x(end);
            let sel_rect = Rect::from_min_max(Pos2::new(x_start, rect.min.y + ruler_h), Pos2::new(x_end, rect.max.y));

            // Semi-transparent glowing fill
            painter.rect_filled(
                sel_rect,
                Rounding::ZERO,
                Color32::from_rgba_unmultiplied(16, 185, 129, 40),
            );

            // Left boundary handle
            painter.line_segment(
                [Pos2::new(x_start, rect.min.y), Pos2::new(x_start, rect.max.y)],
                Stroke::new(2.5, Color32::from_rgb(52, 211, 153)),
            );
            // Right boundary handle
            painter.line_segment(
                [Pos2::new(x_end, rect.min.y), Pos2::new(x_end, rect.max.y)],
                Stroke::new(2.5, Color32::from_rgb(52, 211, 153)),
            );

            // Handle caps on ruler
            painter.circle_filled(Pos2::new(x_start, rect.min.y + ruler_h / 2.0), 5.0, Color32::from_rgb(52, 211, 153));
            painter.circle_filled(Pos2::new(x_end, rect.min.y + ruler_h / 2.0), 5.0, Color32::from_rgb(52, 211, 153));
        }

        // Draw Playhead Cursor & Scrub Diamond
        let playhead_x = time_to_x(self.current_time_sec);
        painter.line_segment(
            [Pos2::new(playhead_x, rect.min.y), Pos2::new(playhead_x, rect.max.y)],
            Stroke::new(2.0, Color32::from_rgb(248, 113, 113)), // Coral Red
        );
        let head_top = Pos2::new(playhead_x, rect.min.y);
        let diamond = [
            Pos2::new(head_top.x - 6.0, head_top.y),
            Pos2::new(head_top.x, head_top.y + 12.0),
            Pos2::new(head_top.x + 6.0, head_top.y),
            Pos2::new(head_top.x, head_top.y - 4.0),
        ];
        painter.add(egui::Shape::convex_polygon(
            diamond.to_vec(),
            Color32::from_rgb(248, 113, 113),
            Stroke::NONE,
        ));

        // If dragging playhead via ruler, show precise scrub time badge
        if matches!(self.active_drag, Some(DragAction::ScrubRuler)) {
            let badge_pos = Pos2::new((playhead_x + 8.0).min(rect.max.x - 60.0), rect.min.y + 28.0);
            let cur_m = (self.current_time_sec / 60.0) as u32;
            let cur_s = self.current_time_sec % 60.0;
            let badge_text = format!("{:02}:{:05.2}", cur_m, cur_s);
            painter.rect_filled(
                Rect::from_min_size(badge_pos, Vec2::new(60.0, 18.0)),
                Rounding::same(4.0),
                Color32::from_rgba_unmultiplied(15, 23, 42, 220),
            );
            painter.text(
                Pos2::new(badge_pos.x + 4.0, badge_pos.y + 9.0),
                egui::Align2::LEFT_CENTER,
                badge_text,
                egui::FontId::monospace(10.0),
                Color32::from_rgb(248, 113, 113),
            );
        }

        // Interactive Mouse Handling (Top Ruler Scrub vs Waveform Region Selection)
        if response.drag_started() {
            if let Some(mouse_pos) = response.interact_pointer_pos() {
                let mouse_t = x_to_time(mouse_pos.x);
                let is_in_ruler = mouse_pos.y <= rect.min.y + ruler_h;

                if is_in_ruler {
                    // 1. Dragging top ruler scrubs the playhead
                    self.active_drag = Some(DragAction::ScrubRuler);
                    self.seek_to(mouse_t);
                } else {
                    // 2. In waveform area: check if grabbing existing selection boundaries
                    let handle_grab_radius_px = 6.0;
                    let mut captured = false;
                    if let Some((s, e)) = self.selection {
                        let xs = time_to_x(s);
                        let xe = time_to_x(e);
                        if (mouse_pos.x - xs).abs() <= handle_grab_radius_px {
                            self.active_drag = Some(DragAction::DragLeftHandle { other_anchor: e });
                            captured = true;
                        } else if (mouse_pos.x - xe).abs() <= handle_grab_radius_px {
                            self.active_drag = Some(DragAction::DragRightHandle { other_anchor: s });
                            captured = true;
                        }
                    }

                    // 3. New region drag selection in waveform area
                    if !captured {
                        self.active_drag = Some(DragAction::CreateSelection {
                            start_time: mouse_t,
                            start_pos_x: mouse_pos.x,
                            has_moved: false,
                        });
                        self.seek_to(mouse_t);
                    }
                }
            }
        } else if response.dragged() {
            if let Some(mut drag) = self.active_drag {
                if let Some(mouse_pos) = response.interact_pointer_pos() {
                    let cur_t = x_to_time(mouse_pos.x);
                    match drag {
                        DragAction::ScrubRuler => {
                            self.seek_to(cur_t);
                        }
                        DragAction::DragLeftHandle { other_anchor } => {
                            let t1 = cur_t.min(other_anchor);
                            let t2 = cur_t.max(other_anchor);
                            self.selection = Some((t1, t2));
                            self.seek_to(cur_t);
                        }
                        DragAction::DragRightHandle { other_anchor } => {
                            let t1 = cur_t.min(other_anchor);
                            let t2 = cur_t.max(other_anchor);
                            self.selection = Some((t1, t2));
                            self.seek_to(cur_t);
                        }
                        DragAction::CreateSelection {
                            start_time,
                            start_pos_x,
                            ref mut has_moved,
                        } => {
                            let dx = (mouse_pos.x - start_pos_x).abs();
                            // Distinguish single click from drag in pixel space:
                            // Once mouse has moved at least 2 pixels, start forming the slice.
                            // Once started (*has_moved is true), allow any size without minimum duration clamp!
                            if *has_moved || dx >= 2.0 {
                                *has_moved = true;
                                let t1 = start_time.min(cur_t);
                                let t2 = start_time.max(cur_t);
                                self.selection = Some((t1, t2));
                                self.seek_to(cur_t);
                            }
                            self.active_drag = Some(drag);
                        }
                    }
                }
            }
        } else if response.drag_stopped() {
            if let Some(DragAction::CreateSelection { has_moved, .. }) = self.active_drag {
                if !has_moved {
                    // Clicked without moving -> clear selection
                    self.selection = None;
                }
            }
            self.active_drag = None;
        } else if response.clicked() {
            if let Some(pos) = response.interact_pointer_pos() {
                let clicked_t = x_to_time(pos.x);
                let is_in_ruler = pos.y <= rect.min.y + ruler_h;
                if !is_in_ruler {
                    // Single click on waveform clears region selection and sets playhead
                    self.selection = None;
                }
                self.seek_to(clicked_t);
            }
        } else if response.double_clicked() {
            // Double click toggles selection of entire track
            if self.selection.is_some() {
                self.selection = None;
            } else {
                self.selection = Some((0.0, self.duration_sec));
            }
        }

        // Dynamic Cursor Feedback
        if let Some(hover_pos) = response.hover_pos() {
            let is_in_ruler = hover_pos.y <= rect.min.y + ruler_h;
            if is_in_ruler {
                ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::ResizeHorizontal);
            } else if let Some((s, e)) = self.selection {
                let xs = time_to_x(s);
                let xe = time_to_x(e);
                if (hover_pos.x - xs).abs() <= 6.0 || (hover_pos.x - xe).abs() <= 6.0 {
                    ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::ResizeHorizontal);
                } else {
                    ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::Crosshair);
                }
            } else {
                ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::Crosshair);
            }
        }
    }
}

fn sel_duration_label(d: f32) -> f32 {
    d
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_audio_preview_state_creation_and_seek() {
        let p = PathBuf::from("/home/vega/Documents/type_testing/TAPE_NOISE_04.wav");
        if p.is_file() {
            let mut state = AudioPreviewState::new(p.clone(), "TAPE_NOISE_04.wav".to_string());
            assert!(state.duration_sec > 4.0);
            assert!(state.metadata.is_some());

            // Test seek
            state.seek_to(2.0);
            assert_eq!(state.current_time_sec, 2.0);

            // Test selection
            state.selection = Some((1.0, 3.0));
            assert_eq!(state.selection, Some((1.0, 3.0)));

            // Test segment copy extraction
            state.copy_selected_segment(1.0);
            assert!(state.status_toast.is_some());
            let (toast, _) = state.status_toast.unwrap();
            assert!(toast.contains("Copied slice"));
        }
    }

    #[test]
    fn test_audio_preview_playback_and_seek_mp3() {
        let p = PathBuf::from("/home/vega/Documents/type_testing/Computer Error Song.mp3");
        if p.is_file() {
            let mut state = AudioPreviewState::new(p.clone(), "Computer Error Song.mp3".to_string());
            assert!(state.duration_sec > 300.0);

            state.play();
            // Test seeking forward and backward during play
            state.seek_to(15.0);
            assert_eq!(state.current_time_sec, 15.0);
            state.seek_to(5.0);
            assert_eq!(state.current_time_sec, 5.0);
            state.pause();
            assert!(!state.is_playing);
        }
    }

    #[test]
    fn test_copy_event_syntax() {
        let ev = egui::Event::Copy;
        assert!(matches!(ev, egui::Event::Copy));
        let ctx = egui::Context::default();
        let _ = ctx.input(|i| i.raw.events.iter().any(|e| matches!(e, egui::Event::Copy)));
        ctx.memory_mut(|m| {
            if let Some(id) = m.focused() {
                m.surrender_focus(id);
            }
        });
    }

    #[test]
    fn test_small_sub_50ms_slice_selection() {
        let p = PathBuf::from("/home/vega/Documents/type_testing/TAPE_NOISE_04.wav");
        if p.is_file() {
            let mut state = AudioPreviewState::new(p.clone(), "TAPE_NOISE_04.wav".to_string());
            // Test small 20ms slice (1.000s to 1.020s)
            state.selection = Some((1.000, 1.020));
            state.copy_selected_segment(1.0);
            assert!(state.status_toast.is_some());
            let (toast, _) = state.status_toast.unwrap();
            assert!(toast.contains("Copied slice"));
            assert!(toast.contains("0.020s"));
        }
    }
}
