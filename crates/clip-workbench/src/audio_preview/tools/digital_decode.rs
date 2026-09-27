//! Digital signal decoding studio tool for amateur radio, telecommunications, and signaling modes.
//!
//! Provides parallel multi-protocol background audio decoding, a logic-analyzer style
//! multi-lane timeline overlay, synchronized playback and waveform highlighting, and dual
//! plaintext and structured event transcripts.

use super::super::*;
use crate::icons::Icon;
use little_clip_ops::digital_decode::{DecodedSymbol, DecoderProtocol, downmix_to_mono};
use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::time::Instant;

/// Active tab within the digital decode transcript inspector panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum TranscriptTab {
    /// Consolidated plaintext readouts per protocol.
    #[default]
    Plaintext,
    /// Detailed chronological event table with timestamps and metrics.
    StructuredEvents,
}

/// Asynchronous progress and results messages emitted during digital signal decoding.
#[derive(Debug)]
pub(crate) enum DigitalDecodeMsg {
    /// A newly decoded batch of symbols from one or more demodulators.
    Batch(Vec<DecodedSymbol>),
    /// Decoding across all enabled demodulators has completed.
    Done(f32),
    /// An error encountered during audio preparation or decoding.
    #[allow(dead_code)]
    Error(String),
}

/// State machine for the Digital Decode studio tool.
pub(crate) struct DigitalDecodeState {
    /// `true` while background demodulation worker threads are active.
    pub(crate) in_progress: bool,
    /// Atomic cancellation flag passed to background demodulation workers.
    pub(crate) cancel_flag: Option<Arc<AtomicBool>>,
    /// Channel receiver for incoming streaming decode results.
    pub(crate) rx: Option<Receiver<DigitalDecodeMsg>>,
    /// Chronological collection of all decoded symbols across all modes.
    pub(crate) symbols: Vec<DecodedSymbol>,
    /// Index of the currently selected symbol, if any.
    pub(crate) selected_symbol: Option<usize>,
    /// Index of the currently hovered symbol on the timeline or table.
    pub(crate) hovered_symbol: Option<usize>,
    /// Set of demodulation protocols currently enabled for scanning.
    pub(crate) enabled_protocols: HashSet<DecoderProtocol>,
    /// Active tab in the bottom transcript panel.
    pub(crate) transcript_tab: TranscriptTab,
    /// Optional search / filter string applied to transcript symbols and payloads.
    pub(crate) filter_text: String,
    /// Status message or summary banner displayed to the user.
    pub(crate) status_msg: Option<String>,
    /// Duration in seconds taken by the most recent scan.
    pub(crate) scan_duration_sec: Option<f32>,
    /// Indicates whether an initial scan has been dispatched for this track.
    pub(crate) has_started_initial_scan: bool,
    /// Start instant of the current in-flight scan.
    pub(crate) scan_start: Option<Instant>,
}

impl Default for DigitalDecodeState {
    fn default() -> Self {
        let mut enabled = HashSet::new();
        for &proto in DecoderProtocol::all() {
            enabled.insert(proto);
        }
        Self {
            in_progress: false,
            cancel_flag: None,
            rx: None,
            symbols: Vec::new(),
            selected_symbol: None,
            hovered_symbol: None,
            enabled_protocols: enabled,
            transcript_tab: TranscriptTab::default(),
            filter_text: String::new(),
            status_msg: None,
            scan_duration_sec: None,
            has_started_initial_scan: false,
            scan_start: None,
        }
    }
}

impl AudioPreviewState {
    /// Launches an asynchronous multi-protocol digital demodulation scan over the full audio buffer.
    pub(crate) fn start_digital_decode(&mut self) {
        if self.analysis.is_none() {
            self.digital_decode.status_msg =
                Some("Waiting for audio track decoding to finish…".to_string());
            return;
        }

        self.cancel_digital_decode();
        let analysis = self.analysis.as_ref().unwrap();

        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_thread = Arc::clone(&cancel);
        let (tx, rx) = channel();

        self.digital_decode.in_progress = true;
        self.digital_decode.cancel_flag = Some(cancel);
        self.digital_decode.rx = Some(rx);
        self.digital_decode.symbols.clear();
        self.digital_decode.selected_symbol = None;
        self.digital_decode.hovered_symbol = None;
        self.digital_decode.status_msg = Some("Demodulating digital signals…".to_string());
        self.digital_decode.scan_start = Some(Instant::now());

        let channels = analysis.channels;
        let sample_rate = analysis.sample_rate;
        let enabled_protocols = self.digital_decode.enabled_protocols.clone();

        // Synthesize interleaved or mono samples from analysis buffers
        let samples: Vec<f32> = if let Some(ch2) = &analysis.ch2_samples {
            let ch1 = &analysis.ch1_samples;
            let len = ch1.len().min(ch2.len());
            let mut interleaved = Vec::with_capacity(len * 2);
            for i in 0..len {
                interleaved.push(ch1[i]);
                interleaved.push(ch2[i]);
            }
            interleaved
        } else {
            analysis.ch1_samples.clone()
        };

        std::thread::spawn(move || {
            let start = Instant::now();
            let mono = downmix_to_mono(&samples, channels);
            if mono.is_empty() {
                let _ = tx.send(DigitalDecodeMsg::Done(0.0));
                return;
            }

            let is_cancelled = || cancel_thread.load(Ordering::Relaxed);

            // 1. DTMF Demodulation
            if enabled_protocols.contains(&DecoderProtocol::Dtmf) && !is_cancelled() {
                let dtmf_mono =
                    little_clip_ops::digital_decode::resample_linear(&mono, sample_rate, 8000);
                let dtmf_symbols = little_clip_ops::digital_decode::decode_dtmf(&dtmf_mono, 8000);
                if !dtmf_symbols.is_empty() && !is_cancelled() {
                    let _ = tx.send(DigitalDecodeMsg::Batch(dtmf_symbols));
                }
            }

            // 2. Morse / CW Demodulation
            if enabled_protocols.contains(&DecoderProtocol::CwMorse) && !is_cancelled() {
                let cw_symbols =
                    little_clip_ops::digital_decode::decode_morse_cw(&mono, sample_rate);
                if !cw_symbols.is_empty() && !is_cancelled() {
                    let _ = tx.send(DigitalDecodeMsg::Batch(cw_symbols));
                }
            }

            // 3. Bell 202 / AX.25 / APRS Demodulation
            if enabled_protocols.contains(&DecoderProtocol::Bell202Aprs) && !is_cancelled() {
                let aprs_symbols =
                    little_clip_ops::digital_decode::decode_bell202_aprs(&mono, sample_rate);
                if !aprs_symbols.is_empty() && !is_cancelled() {
                    let _ = tx.send(DigitalDecodeMsg::Batch(aprs_symbols));
                }
            }

            // 4. FT8 Demodulation (cycles requiring >= 12.64s)
            if enabled_protocols.contains(&DecoderProtocol::Ft8) && !is_cancelled() {
                let ft8_symbols = little_clip_ops::digital_decode::decode_ft8(&mono, sample_rate);
                if !ft8_symbols.is_empty() && !is_cancelled() {
                    let _ = tx.send(DigitalDecodeMsg::Batch(ft8_symbols));
                }
            }

            // 5. WSPR Demodulation (cycles requiring >= 110s)
            if enabled_protocols.contains(&DecoderProtocol::Wspr) && !is_cancelled() {
                let wspr_symbols = little_clip_ops::digital_decode::decode_wspr(&mono, sample_rate);
                if !wspr_symbols.is_empty() && !is_cancelled() {
                    let _ = tx.send(DigitalDecodeMsg::Batch(wspr_symbols));
                }
            }

            let elapsed = start.elapsed().as_secs_f32();
            let _ = tx.send(DigitalDecodeMsg::Done(elapsed));
        });
    }

    /// Cancels any in-flight background demodulation scan.
    pub(crate) fn cancel_digital_decode(&mut self) {
        if let Some(flag) = self.digital_decode.cancel_flag.take() {
            flag.store(true, Ordering::Relaxed);
        }
        self.digital_decode.in_progress = false;
        self.digital_decode.rx = None;
    }

    /// Polls for incoming streaming decode batches from the background demodulation worker.
    pub(crate) fn poll_digital_decode(&mut self, ctx: &egui::Context) {
        let mut new_batches = Vec::new();
        let mut done_time = None;
        let mut error_msg = None;

        if let Some(rx) = &self.digital_decode.rx {
            ctx.request_repaint();
            while let Ok(msg) = rx.try_recv() {
                match msg {
                    DigitalDecodeMsg::Batch(batch) => {
                        new_batches.push(batch);
                    }
                    DigitalDecodeMsg::Done(elapsed) => {
                        done_time = Some(elapsed);
                    }
                    DigitalDecodeMsg::Error(err) => {
                        error_msg = Some(err);
                    }
                }
            }
        }

        for batch in new_batches {
            self.digital_decode.symbols.extend(batch);
            self.digital_decode.symbols.sort_by(|a, b| {
                a.start_sec
                    .partial_cmp(&b.start_sec)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
        }

        if let Some(err) = error_msg {
            self.digital_decode.in_progress = false;
            self.digital_decode.rx = None;
            self.digital_decode.status_msg = Some(format!("Decoding error: {err}"));
        } else if let Some(elapsed) = done_time {
            self.digital_decode.in_progress = false;
            self.digital_decode.rx = None;
            self.digital_decode.scan_duration_sec = Some(elapsed);
            let count = self.digital_decode.symbols.len();
            self.digital_decode.status_msg = Some(format!(
                "Decoded {count} symbol{} in {:.2}s",
                if count == 1 { "" } else { "s" },
                elapsed
            ));
        }
    }

    /// Renders the complete Digital Decode tool interface.
    pub(crate) fn render_digital_decode_ui(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        palette: &AppPalette,
        _close_requested: &mut bool,
    ) {
        let now = ctx.input(|i| i.time);
        self.poll_audio_analysis(ctx, now);
        self.poll_digital_decode(ctx);

        // Advance playhead if playing
        if self.is_playing {
            ctx.request_repaint();
            if let Some(start_inst) = self.playback_start_instant {
                let elapsed = start_inst.elapsed().as_secs_f32();
                let new_time = self.playback_start_offset_sec + elapsed;
                if new_time >= self.duration_sec {
                    self.pause();
                    self.current_time_sec = self.duration_sec;
                } else {
                    self.current_time_sec = new_time;
                }
            }
        }

        // Automatic initial scan when opening the tool with ready analysis
        if !self.digital_decode.has_started_initial_scan && self.analysis.is_some() {
            self.digital_decode.has_started_initial_scan = true;
            self.start_digital_decode();
        }

        // Spacebar toggles playback in Digital Decode view
        if !ctx.wants_keyboard_input() && ctx.input(|i| i.key_pressed(egui::Key::Space)) {
            if self.is_playing {
                self.pause();
            } else {
                self.play();
            }
        }

        let available_w = ui.available_width();
        let available_h = ui.available_height();

        // Calculate partition: 62% for logic analyzer + waveform + spectrum canvas, 38% for transcript & controls
        let canvas_h = (available_h * 0.58).clamp(240.0, 600.0);

        self.render_digital_decode_canvas(ui, ctx, palette, available_w, canvas_h);
        ui.add_space(crate::ui_ext::ITEM_SPACING_Y);
        self.render_digital_decode_transcript(ui, ctx, palette);
    }

    /// Renders the logic-analyzer timeline lanes, ruler, waveform, and spectrum canvas.
    fn render_digital_decode_canvas(
        &mut self,
        ui: &mut egui::Ui,
        _ctx: &egui::Context,
        palette: &AppPalette,
        width: f32,
        height: f32,
    ) {
        let (rect, response) =
            ui.allocate_exact_size(Vec2::new(width, height), egui::Sense::click_and_drag());
        let painter = ui.painter_at(rect);

        let bg_frame = crate::hex_to_c32(&palette.colors.surface_recessed);
        let border_frame = crate::hex_to_c32(&palette.colors.surface_panel);
        let text_f_med = crate::hex_to_c32(&palette.colors.text_secondary);
        let text_f_low = crate::hex_to_c32(&palette.colors.text_muted);
        let accent_base = crate::hex_to_c32(&palette.harmonies.base);
        let accent_playhead = crate::hex_to_c32(&palette.harmonies.complementary);

        // Frame border & background
        painter.rect_filled(rect, Rounding::same(8.0), bg_frame);
        painter.rect_stroke(
            rect,
            Rounding::same(8.0),
            Stroke::new(1.0_f32, border_frame),
        );

        if self.is_loading_analysis || self.analysis.is_none() {
            let elapsed = self.loading_start_time.elapsed().as_secs_f32();
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                format!("⏳ Decoding audio buffer... ({:.1}s)", elapsed),
                egui::FontId::proportional(15.0),
                text_f_med,
            );
            return;
        }

        let label_w = 90.0;
        let track_min_x = rect.min.x + label_w;
        let track_max_x = rect.max.x - 6.0;
        let track_width = (track_max_x - track_min_x).max(10.0);

        // Determine which protocols have symbols to display on timeline lanes
        let mut active_protocols: Vec<DecoderProtocol> = Vec::new();
        for &proto in DecoderProtocol::all() {
            if self
                .digital_decode
                .symbols
                .iter()
                .any(|s| s.protocol == proto)
            {
                active_protocols.push(proto);
            }
        }
        // If no symbols yet, show at least the primary enabled protocols as placeholder lanes
        if active_protocols.is_empty() {
            for &proto in &[
                DecoderProtocol::CwMorse,
                DecoderProtocol::Dtmf,
                DecoderProtocol::Bell202Aprs,
            ] {
                if self.digital_decode.enabled_protocols.contains(&proto) {
                    active_protocols.push(proto);
                }
            }
        }

        let lane_h = 26.0;
        let lane_spacing = 3.0;
        let lanes_total_h = (active_protocols.len() as f32) * (lane_h + lane_spacing);

        let ruler_h = 22.0;
        let scrollbar_h = 12.0;
        let spacing = 4.0;

        let remaining_h =
            (rect.height() - lanes_total_h - ruler_h - scrollbar_h - spacing * 3.0 - 8.0).max(80.0);
        let wave_h = (remaining_h * 0.52).max(40.0);
        let spec_h = (remaining_h * 0.48).max(35.0);

        // Partition rectangles
        let lanes_rect = Rect::from_min_max(
            Pos2::new(rect.min.x, rect.min.y + 4.0),
            Pos2::new(rect.max.x, rect.min.y + 4.0 + lanes_total_h),
        );
        let ruler_rect = Rect::from_min_max(
            Pos2::new(track_min_x, lanes_rect.max.y + spacing),
            Pos2::new(track_max_x, lanes_rect.max.y + spacing + ruler_h),
        );
        let wave_rect = Rect::from_min_max(
            Pos2::new(track_min_x, ruler_rect.max.y + spacing),
            Pos2::new(track_max_x, ruler_rect.max.y + spacing + wave_h),
        );
        let spec_rect = Rect::from_min_max(
            Pos2::new(track_min_x, wave_rect.max.y + spacing),
            Pos2::new(track_max_x, wave_rect.max.y + spacing + spec_h),
        );
        let scroll_rect = Rect::from_min_max(
            Pos2::new(track_min_x, spec_rect.max.y + spacing),
            Pos2::new(track_max_x, spec_rect.max.y + spacing + scrollbar_h),
        );

        let v_start = self.view_start_sec;
        let v_dur = self.view_duration_sec.max(0.0001);

        let time_to_x =
            move |t: f32| -> f32 { track_min_x + ((t - v_start) / v_dur) * track_width };
        let x_to_time =
            move |x: f32| -> f32 { v_start + ((x - track_min_x) / track_width) * v_dur };

        // Interaction Handling
        let mut clicked_chip_idx = None;
        let mut hovered_chip_idx = None;

        if response.hovered() {
            if let Some(mouse_pos) = response.hover_pos() {
                // Check if mouse is hovering over any decoded symbol chips
                for (lane_idx, &proto) in active_protocols.iter().enumerate() {
                    let y_top = lanes_rect.min.y + (lane_idx as f32) * (lane_h + lane_spacing);
                    let y_bot = y_top + lane_h;
                    if mouse_pos.y >= y_top && mouse_pos.y <= y_bot {
                        for (sym_idx, sym) in self.digital_decode.symbols.iter().enumerate() {
                            if sym.protocol != proto {
                                continue;
                            }
                            let font_id = egui::FontId::monospace(10.0);
                            let text_galley =
                                painter.layout_no_wrap(sym.symbol.clone(), font_id, Color32::WHITE);
                            let needed_w = (text_galley.size().x + 14.0).max(22.0);
                            let x_start = time_to_x(sym.start_sec);
                            let x_end = time_to_x(sym.end_sec).max(x_start + needed_w);
                            if mouse_pos.x >= x_start && mouse_pos.x <= x_end {
                                hovered_chip_idx = Some(sym_idx);
                                break;
                            }
                        }
                    }
                }
            }

            // Zoom and Pan handling
            let raw_scroll = ui.input(|i| i.raw_scroll_delta);
            if ui.input(|i| i.modifiers.shift) && (raw_scroll.x + raw_scroll.y).abs() > 0.1 {
                let shifted_scroll = raw_scroll.x + raw_scroll.y;
                let dt = -(shifted_scroll / 50.0) * self.view_duration_sec * 0.12;
                self.pan(dt);
            } else if raw_scroll.y.abs() > 0.1 {
                let is_fine = ui.input(|i| i.modifiers.alt || i.modifiers.ctrl);
                let step_ratio: f32 = if is_fine { 1.025 } else { 1.055 };
                let steps = (raw_scroll.y / 50.0).clamp(-4.0, 4.0);
                let factor = step_ratio.powf(-steps);

                let anchor = if let Some(pos) = response.hover_pos() {
                    x_to_time(pos.x).clamp(0.0, self.duration_sec)
                } else {
                    self.view_start_sec + self.view_duration_sec * 0.5
                };
                self.zoom(factor, anchor);
            }
        }

        if response.clicked_by(egui::PointerButton::Primary) {
            if let Some(pos) = response.interact_pointer_pos() {
                if scroll_rect.contains(pos) {
                    let ratio = (pos.x - scroll_rect.min.x) / scroll_rect.width();
                    let target_start = ratio * self.duration_sec - self.view_duration_sec * 0.5;
                    let max_start = (self.duration_sec - self.view_duration_sec).max(0.0);
                    self.view_start_sec = target_start.clamp(0.0, max_start);
                } else if hovered_chip_idx.is_some() {
                    clicked_chip_idx = hovered_chip_idx;
                } else {
                    let clicked_t = x_to_time(pos.x).clamp(0.0, self.duration_sec);
                    self.seek_to(clicked_t);
                }
            }
        }

        if let Some(idx) = clicked_chip_idx {
            self.digital_decode.selected_symbol = Some(idx);
            let sym = &self.digital_decode.symbols[idx];
            self.seek_to(sym.start_sec);
        }
        self.digital_decode.hovered_symbol = hovered_chip_idx;

        // 1. LOGIC ANALYZER LANES
        for (lane_idx, &proto) in active_protocols.iter().enumerate() {
            let y_top = lanes_rect.min.y + (lane_idx as f32) * (lane_h + lane_spacing);
            let y_bot = y_top + lane_h;
            let lane_rect =
                Rect::from_min_max(Pos2::new(track_min_x, y_top), Pos2::new(track_max_x, y_bot));

            // Lane track background
            painter.rect_filled(
                lane_rect,
                Rounding::same(3.0),
                crate::hex_to_c32_alpha(&palette.colors.surface_panel, 40),
            );
            painter.rect_stroke(
                lane_rect,
                Rounding::same(3.0),
                Stroke::new(
                    1.0_f32,
                    crate::hex_to_c32_alpha(&palette.colors.surface_raised, 30),
                ),
            );

            // Gutter protocol badge
            let badge_rect = Rect::from_min_max(
                Pos2::new(rect.min.x + 4.0, y_top + 1.0),
                Pos2::new(track_min_x - 6.0, y_bot - 1.0),
            );
            let rgb = proto.chip_color();
            let proto_color = Color32::from_rgb(rgb[0], rgb[1], rgb[2]);
            let badge_fill = Color32::from_rgba_unmultiplied(rgb[0], rgb[1], rgb[2], 40);
            painter.rect_filled(badge_rect, Rounding::same(4.0), badge_fill);
            painter.rect_stroke(
                badge_rect,
                Rounding::same(4.0),
                Stroke::new(1.0_f32, proto_color),
            );
            painter.text(
                badge_rect.center(),
                egui::Align2::CENTER_CENTER,
                proto.label(),
                egui::FontId::monospace(10.0),
                crate::ui_ext::contrast_text_c32_on(
                    badge_fill,
                    bg_frame,
                    Some(proto_color),
                    palette,
                ),
            );

            // Render symbol chips in this lane
            for (sym_idx, sym) in self.digital_decode.symbols.iter().enumerate() {
                if sym.protocol != proto {
                    continue;
                }
                let is_cw = sym.protocol == DecoderProtocol::CwMorse;
                let font_id = egui::FontId::monospace(10.0);
                let text_galley =
                    painter.layout_no_wrap(sym.symbol.clone(), font_id.clone(), Color32::WHITE);
                let needed_w = (text_galley.size().x + 14.0).max(22.0);
                let x_start = time_to_x(sym.start_sec);
                let x_end = time_to_x(sym.end_sec).max(x_start + needed_w);

                if x_end < track_min_x || x_start > track_max_x {
                    continue;
                }

                let chip_rect = Rect::from_min_max(
                    Pos2::new(x_start.clamp(track_min_x, track_max_x), y_top + 2.0),
                    Pos2::new(x_end.clamp(track_min_x, track_max_x), y_bot - 2.0),
                );

                let is_selected = self.digital_decode.selected_symbol == Some(sym_idx);
                let is_hovered = self.digital_decode.hovered_symbol == Some(sym_idx);

                let fill_alpha = if is_selected {
                    220
                } else if is_hovered {
                    180
                } else {
                    120
                };
                let chip_fill = Color32::from_rgba_unmultiplied(rgb[0], rgb[1], rgb[2], fill_alpha);
                painter.rect_filled(chip_rect, Rounding::same(4.0), chip_fill);

                let stroke_color = if is_selected {
                    Color32::WHITE
                } else if is_hovered {
                    Color32::from_rgb(rgb[0], rgb[1], rgb[2])
                } else {
                    Color32::from_rgba_unmultiplied(rgb[0], rgb[1], rgb[2], 160)
                };
                painter.rect_stroke(
                    chip_rect,
                    Rounding::same(4.0),
                    Stroke::new(if is_selected { 1.8_f32 } else { 1.0_f32 }, stroke_color),
                );

                // Chip label text
                if chip_rect.width() >= 14.0 {
                    let preferred_text = if is_selected {
                        Color32::WHITE
                    } else {
                        crate::hex_to_c32(&palette.colors.text_primary)
                    };
                    let text_color = crate::ui_ext::contrast_text_c32_on(
                        chip_fill,
                        bg_frame,
                        Some(preferred_text),
                        palette,
                    );
                    let label = if chip_rect.width() < needed_w - 6.0 && is_cw {
                        sym.symbol.split_whitespace().next().unwrap_or(&sym.symbol)
                    } else {
                        &sym.symbol
                    };
                    painter.text(
                        chip_rect.center(),
                        egui::Align2::CENTER_CENTER,
                        label,
                        font_id,
                        text_color,
                    );
                }
            }
        }

        // 2. TIME RULER
        painter.rect_filled(
            Rect::from_min_max(
                Pos2::new(rect.min.x, ruler_rect.min.y),
                Pos2::new(rect.max.x, ruler_rect.max.y),
            ),
            Rounding::ZERO,
            bg_frame,
        );
        painter.line_segment(
            [
                Pos2::new(rect.min.x, ruler_rect.max.y),
                Pos2::new(rect.max.x, ruler_rect.max.y),
            ],
            Stroke::new(1.0_f32, border_frame),
        );

        let interval_sec = if v_dur > 120.0 {
            30.0
        } else if v_dur > 30.0 {
            10.0
        } else if v_dur > 10.0 {
            2.0
        } else if v_dur > 2.0 {
            0.5
        } else if v_dur > 0.5 {
            0.1
        } else if v_dur > 0.1 {
            0.02
        } else {
            0.005
        };

        let first_marker = (v_start / interval_sec).floor() * interval_sec;
        let mut t = first_marker;
        let max_t = v_start + v_dur;
        while t <= max_t + interval_sec * 0.5 {
            if t >= v_start {
                let x = time_to_x(t);
                if x >= track_min_x && x <= track_max_x {
                    painter.line_segment(
                        [
                            Pos2::new(x, ruler_rect.max.y),
                            Pos2::new(x, spec_rect.max.y),
                        ],
                        Stroke::new(
                            1.0_f32,
                            crate::hex_to_c32_alpha(&palette.colors.text_muted, 20),
                        ),
                    );
                    painter.line_segment(
                        [
                            Pos2::new(x, ruler_rect.min.y + 12.0),
                            Pos2::new(x, ruler_rect.max.y),
                        ],
                        Stroke::new(
                            1.0_f32,
                            crate::hex_to_c32_alpha(&palette.colors.text_secondary, 80),
                        ),
                    );
                    let label = if interval_sec >= 1.0 {
                        let m = (t / 60.0) as u32;
                        let s = (t % 60.0) as u32;
                        format!("{:02}:{:02}", m, s)
                    } else {
                        let s = t % 60.0;
                        format!("{:05.2}s", s)
                    };
                    painter.text(
                        Pos2::new(x + 3.0, ruler_rect.min.y + 8.0),
                        egui::Align2::LEFT_CENTER,
                        label,
                        egui::FontId::monospace(9.0),
                        text_f_med,
                    );
                }
            }
            t += interval_sec;
        }

        // 3. WAVEFORM ENVELOPE
        painter.rect_filled(wave_rect, Rounding::ZERO, bg_frame);
        painter.rect_stroke(
            wave_rect,
            Rounding::ZERO,
            Stroke::new(1.0_f32, border_frame),
        );

        let analysis = self.analysis.as_ref().unwrap();
        let is_stereo = analysis.channels >= 2 && analysis.ch2_samples.is_some();
        let total_frames = analysis.total_frames;
        let sample_rate = analysis.sample_rate as f32;

        let num_bars = (track_width as usize).clamp(100, 1400);
        let bar_w = (track_width / num_bars as f32).max(1.0);
        let ch_count = if is_stereo { 2 } else { 1 };

        for ch_idx in 0..ch_count {
            let (y_base, max_def) = if ch_idx == 0 {
                let half = if is_stereo { wave_h / 2.0 } else { wave_h };
                let base = if is_stereo {
                    wave_rect.min.y + wave_h / 4.0
                } else {
                    wave_rect.center().y
                };
                (base, half * 0.45)
            } else {
                let half = wave_h / 2.0;
                let base = wave_rect.min.y + 3.0 * wave_h / 4.0;
                (base, half * 0.45)
            };

            painter.line_segment(
                [
                    Pos2::new(track_min_x, y_base),
                    Pos2::new(track_max_x, y_base),
                ],
                Stroke::new(1.0_f32, border_frame),
            );

            let mut mesh = egui::Mesh::default();
            for b in 0..num_bars {
                let bar_x = track_min_x + b as f32 * bar_w;
                let t_start = x_to_time(bar_x);
                let t_end = x_to_time(bar_x + bar_w);

                let f_s = ((t_start * sample_rate) as usize).min(total_frames);
                let f_e = (((t_end * sample_rate) as usize) + 1).min(total_frames);

                let (min_s, max_s, block_col) = analysis.query_peaks(ch_idx, f_s, f_e);
                let bar_col = Color32::from_rgb(block_col[0], block_col[1], block_col[2]);

                let y_top = y_base - max_s * max_def;
                let y_bot = y_base - min_s * max_def;
                let h = (y_bot - y_top).max(1.0);

                mesh.add_colored_rect(
                    Rect::from_min_size(Pos2::new(bar_x, y_top), Vec2::new(bar_w.max(1.0), h)),
                    bar_col,
                );
            }
            painter.add(egui::Shape::mesh(mesh));
        }

        // Gutter channel labels
        if is_stereo {
            let half = wave_h / 2.0;
            painter.text(
                Pos2::new(track_min_x - 8.0, wave_rect.min.y + half * 0.5),
                egui::Align2::RIGHT_CENTER,
                "L",
                egui::FontId::proportional(11.0),
                accent_base,
            );
            painter.text(
                Pos2::new(track_min_x - 8.0, wave_rect.min.y + half * 1.5),
                egui::Align2::RIGHT_CENTER,
                "R",
                egui::FontId::proportional(11.0),
                accent_base,
            );
        } else {
            painter.text(
                Pos2::new(track_min_x - 8.0, wave_rect.center().y),
                egui::Align2::RIGHT_CENTER,
                "Mono",
                egui::FontId::proportional(11.0),
                accent_base,
            );
        }

        // 4. SPECTROGRAM HEATMAP
        painter.rect_filled(spec_rect, Rounding::ZERO, bg_frame);
        if let Some(ref tex) = self.spectrogram_texture {
            let total_dur = self.duration_sec.max(0.01);
            let u_start = (v_start / total_dur).clamp(0.0, 1.0);
            let u_end = ((v_start + v_dur) / total_dur).clamp(0.0, 1.0);
            let uv = Rect::from_min_max(Pos2::new(u_start, 0.0), Pos2::new(u_end, 1.0));
            painter.image(tex.id(), spec_rect, uv, Color32::WHITE);
        } else {
            painter.text(
                spec_rect.center(),
                egui::Align2::CENTER_CENTER,
                "⏳ Generating Spectrogram Heatmap...",
                egui::FontId::proportional(12.0),
                text_f_low,
            );
        }
        painter.rect_stroke(
            spec_rect,
            Rounding::ZERO,
            Stroke::new(1.0_f32, border_frame),
        );

        // Spectrogram frequency ticks in gutter
        let nyquist = analysis.sample_rate as f32 * 0.5;
        let freq_labels = [(10_000.0, "10k"), (5_000.0, "5k"), (1_000.0, "1k")];
        for (frequency, lbl) in freq_labels {
            if frequency >= nyquist {
                continue;
            }
            let y = spec_rect.max.y - (frequency / nyquist) * spec_h;
            painter.text(
                Pos2::new(track_min_x - 8.0, y),
                egui::Align2::RIGHT_CENTER,
                lbl,
                egui::FontId::monospace(9.0),
                text_f_med,
            );
            painter.line_segment(
                [Pos2::new(track_min_x, y), Pos2::new(track_max_x, y)],
                Stroke::new(1.0_f32, border_frame),
            );
        }

        // 5. HIGHLIGHT BAND OVER WAVEFORM & SPECTRUM FOR ACTIVE / HOVERED SYMBOL
        let highlight_idx = self
            .digital_decode
            .hovered_symbol
            .or(self.digital_decode.selected_symbol);
        if let Some(idx) = highlight_idx {
            if let Some(sym) = self.digital_decode.symbols.get(idx) {
                let x_start = time_to_x(sym.start_sec).clamp(track_min_x, track_max_x);
                let x_end = time_to_x(sym.end_sec).clamp(track_min_x, track_max_x);
                let rgb = sym.protocol.chip_color();
                let highlight_col = Color32::from_rgba_unmultiplied(rgb[0], rgb[1], rgb[2], 65);
                let border_col = Color32::from_rgb(rgb[0], rgb[1], rgb[2]);

                let highlight_rect = Rect::from_min_max(
                    Pos2::new(x_start, ruler_rect.max.y),
                    Pos2::new(x_end.max(x_start + 2.0), spec_rect.max.y),
                );
                painter.rect_filled(highlight_rect, Rounding::ZERO, highlight_col);
                painter.line_segment(
                    [
                        Pos2::new(x_start, ruler_rect.max.y),
                        Pos2::new(x_start, spec_rect.max.y),
                    ],
                    Stroke::new(1.8_f32, border_col),
                );
                painter.line_segment(
                    [
                        Pos2::new(x_end, ruler_rect.max.y),
                        Pos2::new(x_end, spec_rect.max.y),
                    ],
                    Stroke::new(1.8_f32, border_col),
                );

                // Floating tooltip chip over hovered symbol
                if self.digital_decode.hovered_symbol == Some(idx) {
                    let desc = sym.details.as_deref().unwrap_or(&sym.symbol);
                    egui::show_tooltip(ui.ctx(), response.id.with("sym_tt"), |ui| {
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new(format!("[{}]", sym.protocol.label()))
                                    .strong()
                                    .color(border_col),
                            );
                            ui.label(
                                egui::RichText::new(&sym.symbol)
                                    .strong()
                                    .color(Color32::WHITE),
                            );
                        });
                        ui.label(
                            egui::RichText::new(format!(
                                "Time: {:.3}s - {:.3}s ({:.1} ms) • Confidence: {:.0}%",
                                sym.start_sec,
                                sym.end_sec,
                                (sym.end_sec - sym.start_sec) * 1000.0,
                                sym.confidence * 100.0
                            ))
                            .monospace()
                            .color(crate::hex_to_c32(&palette.colors.text_secondary)),
                        );
                        if !desc.is_empty() {
                            ui.label(
                                egui::RichText::new(desc)
                                    .color(crate::hex_to_c32(&palette.colors.text_primary)),
                            );
                        }
                    });
                }
            }
        }

        // 6. VIEWPORT SCROLLBAR / PAN TRACK
        painter.rect_filled(scroll_rect, Rounding::same(3.0), bg_frame);
        painter.rect_stroke(
            scroll_rect,
            Rounding::same(3.0),
            Stroke::new(1.0_f32, border_frame),
        );

        let total_dur = self.duration_sec.max(0.01);
        let thumb_x = scroll_rect.min.x + (v_start / total_dur) * scroll_rect.width();
        let thumb_w = ((v_dur / total_dur) * scroll_rect.width()).clamp(16.0, scroll_rect.width());
        let thumb_rect = Rect::from_min_size(
            Pos2::new(thumb_x, scroll_rect.min.y),
            Vec2::new(thumb_w, scroll_rect.height()),
        );
        painter.rect_filled(thumb_rect, Rounding::same(3.0), border_frame);
        painter.rect_stroke(
            thumb_rect,
            Rounding::same(3.0),
            Stroke::new(1.0_f32, text_f_med),
        );

        // 7. UNIFIED PLAYHEAD CURSOR
        let playhead_x = time_to_x(self.current_time_sec);
        if playhead_x >= track_min_x && playhead_x <= track_max_x {
            painter.line_segment(
                [
                    Pos2::new(playhead_x, lanes_rect.min.y),
                    Pos2::new(playhead_x, spec_rect.max.y),
                ],
                Stroke::new(2.0_f32, accent_playhead),
            );
            let head_top = Pos2::new(playhead_x, ruler_rect.min.y);
            let diamond = [
                Pos2::new(head_top.x - 5.0, head_top.y),
                Pos2::new(head_top.x, head_top.y + 12.0),
                Pos2::new(head_top.x + 5.0, head_top.y),
                Pos2::new(head_top.x, head_top.y - 4.0),
            ];
            painter.add(egui::Shape::convex_polygon(
                diamond.to_vec(),
                accent_playhead,
                Stroke::NONE,
            ));
        }
    }

    /// Renders the toolbar controls and dual transcript tabs below the timeline canvas.
    fn render_digital_decode_transcript(
        &mut self,
        ui: &mut egui::Ui,
        _ctx: &egui::Context,
        palette: &AppPalette,
    ) {
        let text_f_med = crate::hex_to_c32(&palette.colors.text_secondary);
        let accent_base = crate::hex_to_c32(&palette.harmonies.base);

        // Top Toolbar: Scan button, protocol toggles, playback controls, and summary
        ui.horizontal(|ui| {
            let b_med = crate::hex_to_c32(&palette.colors.surface_panel);
            // Play/Pause button
            let (play_label, play_icon) = if self.is_playing {
                ("Pause", Icon::Pause)
            } else {
                ("Play", Icon::Play)
            };
            if ui
                .add(crate::ui_ext::secondary_button_icon(
                    Some(play_icon),
                    play_label,
                    b_med,
                    palette,
                ))
                .on_hover_text("Play / Pause audio (Space)")
                .clicked()
            {
                if self.is_playing {
                    self.pause();
                } else {
                    self.play();
                }
            }

            ui.label(
                egui::RichText::new(format!(
                    "{:02}:{:05.2} / {:02}:{:05.2}",
                    (self.current_time_sec / 60.0) as u32,
                    self.current_time_sec % 60.0,
                    (self.duration_sec / 60.0) as u32,
                    self.duration_sec % 60.0
                ))
                .monospace()
                .color(text_f_med),
            );

            ui.separator();

            // Re-scan button
            let (scan_label, scan_icon) = if self.digital_decode.in_progress {
                ("Scanning…", None)
            } else {
                ("Re-scan Buffer", Some(Icon::RotateCw))
            };
            if ui
                .add_enabled(
                    !self.digital_decode.in_progress && self.analysis.is_some(),
                    crate::ui_ext::secondary_button_icon(scan_icon, scan_label, b_med, palette),
                )
                .clicked()
            {
                self.start_digital_decode();
            }

            // Protocol enable toggles
            ui.label(egui::RichText::new("Demodulators:").color(text_f_med));
            for &proto in &[
                DecoderProtocol::CwMorse,
                DecoderProtocol::Dtmf,
                DecoderProtocol::Bell202Aprs,
                DecoderProtocol::Ft8,
                DecoderProtocol::Wspr,
            ] {
                let is_enabled = self.digital_decode.enabled_protocols.contains(&proto);
                let mut enabled = is_enabled;
                let rgb = proto.chip_color();
                let col = Color32::from_rgb(rgb[0], rgb[1], rgb[2]);
                let controls_surface = ui.visuals().panel_fill;
                let label_text =
                    egui::RichText::new(proto.label()).color(crate::ui_ext::contrast_text_c32_on(
                        controls_surface,
                        controls_surface,
                        Some(if is_enabled { col } else { text_f_med }),
                        palette,
                    ));
                if ui.checkbox(&mut enabled, label_text).changed() {
                    if enabled {
                        self.digital_decode.enabled_protocols.insert(proto);
                    } else {
                        self.digital_decode.enabled_protocols.remove(&proto);
                    }
                    self.start_digital_decode();
                }
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if let Some(ref msg) = self.digital_decode.status_msg {
                    ui.label(
                        egui::RichText::new(msg)
                            .monospace()
                            .color(crate::hex_to_c32(&palette.diff_add)),
                    );
                }
            });
        });

        ui.add_space(crate::ui_ext::SPACE_XS);

        // Transcript Panel Tabs & Filter Bar
        ui.horizontal_wrapped(|ui| {
            ui.selectable_value(
                &mut self.digital_decode.transcript_tab,
                TranscriptTab::Plaintext,
                egui::RichText::new("Plaintext Transcript").strong(),
            );
            ui.selectable_value(
                &mut self.digital_decode.transcript_tab,
                TranscriptTab::StructuredEvents,
                egui::RichText::new("Structured Events Table").strong(),
            );

            ui.add_space(16.0);
            ui.label(egui::RichText::new("Filter:").color(text_f_med));
            let filter_width = (ui.available_width() - 160.0).max(120.0);
            ui.add_sized(
                egui::vec2(filter_width, crate::ui_ext::CONTROL_HEIGHT),
                egui::TextEdit::singleline(&mut self.digital_decode.filter_text)
                    .font(egui::FontId::proportional(crate::ui_ext::BODY_FONT_SIZE)),
            );

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // Copy transcript to clipboard button
                let b_med = crate::hex_to_c32(&palette.colors.surface_panel);
                if ui
                    .add(crate::ui_ext::secondary_button_icon(
                        Some(Icon::Copy),
                        "Copy Transcript",
                        b_med,
                        palette,
                    ))
                    .on_hover_text("Copy decoded text to clipboard")
                    .clicked()
                {
                    let mut transcript_text = String::new();
                    for &proto in DecoderProtocol::all() {
                        let proto_symbols: Vec<&DecodedSymbol> = self
                            .digital_decode
                            .symbols
                            .iter()
                            .filter(|s| s.protocol == proto)
                            .collect();
                        if !proto_symbols.is_empty() {
                            transcript_text.push_str(&format!("=== {} ===\n", proto.label()));
                            for sym in proto_symbols {
                                if let Some(ref details) = sym.details {
                                    transcript_text.push_str(&format!(
                                        "[{:.3}s - {:.3}s] {}: {}\n",
                                        sym.start_sec, sym.end_sec, sym.symbol, details
                                    ));
                                } else {
                                    transcript_text.push_str(&format!(
                                        "[{:.3}s - {:.3}s] {}\n",
                                        sym.start_sec, sym.end_sec, sym.symbol
                                    ));
                                }
                            }
                            transcript_text.push('\n');
                        }
                    }
                    ui.output_mut(|o| o.copied_text = transcript_text);
                    self.status_toast = Some((
                        "📋 Copied decoded transcript to clipboard".to_string(),
                        ui.input(|i| i.time),
                    ));
                }
            });
        });

        ui.separator();

        let filter_query = self.digital_decode.filter_text.trim().to_lowercase();
        let mut seek_to_time = None;

        match self.digital_decode.transcript_tab {
            TranscriptTab::Plaintext => {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let active_protocols: Vec<DecoderProtocol> = DecoderProtocol::all()
                            .iter()
                            .copied()
                            .filter(|&proto| {
                                self.digital_decode
                                    .symbols
                                    .iter()
                                    .any(|s| s.protocol == proto)
                            })
                            .collect();

                        if active_protocols.is_empty() {
                            ui.label(
                                egui::RichText::new("No digital signals detected in buffer.")
                                    .italics()
                                    .color(text_f_med),
                            );
                            return;
                        }

                        for proto in active_protocols {
                            let symbols: Vec<(usize, &DecodedSymbol)> = self
                                .digital_decode
                                .symbols
                                .iter()
                                .enumerate()
                                .filter(|(_, s)| s.protocol == proto)
                                .filter(|(_, s)| {
                                    filter_query.is_empty()
                                        || s.symbol.to_lowercase().contains(&filter_query)
                                        || s.details
                                            .as_ref()
                                            .map(|d| d.to_lowercase().contains(&filter_query))
                                            .unwrap_or(false)
                                })
                                .collect();

                            if symbols.is_empty() {
                                continue;
                            }

                            let rgb = proto.chip_color();
                            let col = Color32::from_rgb(rgb[0], rgb[1], rgb[2]);

                            ui.group(|ui| {
                                ui.horizontal(|ui| {
                                    ui.label(
                                        egui::RichText::new(format!("● {}", proto.label()))
                                            .strong()
                                            .color(crate::ui_ext::contrast_text_c32_on(
                                                ui.visuals().panel_fill,
                                                ui.visuals().panel_fill,
                                                Some(col),
                                                palette,
                                            )),
                                    );
                                    ui.label(
                                        egui::RichText::new(format!("({} symbols)", symbols.len()))
                                            .color(text_f_med),
                                    );
                                });

                                ui.horizontal_wrapped(|ui| {
                                    for (orig_idx, sym) in symbols {
                                        let is_selected =
                                            self.digital_decode.selected_symbol == Some(orig_idx);
                                        let btn_text = egui::RichText::new(&sym.symbol)
                                            .monospace()
                                            .color(if is_selected { Color32::WHITE } else { col });
                                        let resp = ui.button(btn_text).on_hover_ui(|ui| {
                                            ui.label(format!(
                                                "{:.3}s - {:.3}s (confidence: {:.0}%)",
                                                sym.start_sec,
                                                sym.end_sec,
                                                sym.confidence * 100.0
                                            ));
                                            if let Some(ref d) = sym.details {
                                                ui.label(d);
                                            }
                                        });

                                        if resp.clicked() {
                                            self.digital_decode.selected_symbol = Some(orig_idx);
                                            seek_to_time = Some(sym.start_sec);
                                        }
                                    }
                                });
                            });
                            ui.add_space(crate::ui_ext::SPACE_XS);
                        }
                    });
            }
            TranscriptTab::StructuredEvents => {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let filtered_symbols: Vec<(usize, &DecodedSymbol)> = self
                            .digital_decode
                            .symbols
                            .iter()
                            .enumerate()
                            .filter(|(_, s)| {
                                filter_query.is_empty()
                                    || s.symbol.to_lowercase().contains(&filter_query)
                                    || s.details
                                        .as_ref()
                                        .map(|d| d.to_lowercase().contains(&filter_query))
                                        .unwrap_or(false)
                            })
                            .collect();

                        if filtered_symbols.is_empty() {
                            ui.label(
                                egui::RichText::new("No matching event records.").color(text_f_med),
                            );
                            return;
                        }

                        egui::Grid::new("digital_decode_events_grid")
                            .striped(true)
                            .min_col_width(60.0)
                            .spacing([12.0, 4.0])
                            .show(ui, |ui| {
                                // Table Header
                                ui.label(egui::RichText::new("Time Range").strong());
                                ui.label(egui::RichText::new("Protocol").strong());
                                ui.label(egui::RichText::new("Symbol").strong());
                                ui.label(egui::RichText::new("Payload / Details").strong());
                                ui.label(egui::RichText::new("Confidence").strong());
                                ui.label(egui::RichText::new("Action").strong());
                                ui.end_row();

                                for (orig_idx, sym) in filtered_symbols {
                                    let is_selected =
                                        self.digital_decode.selected_symbol == Some(orig_idx);
                                    let rgb = sym.protocol.chip_color();
                                    let proto_col = Color32::from_rgb(rgb[0], rgb[1], rgb[2]);

                                    // Time
                                    ui.label(
                                        egui::RichText::new(format!(
                                            "{:02}:{:05.2} - {:02}:{:05.2}",
                                            (sym.start_sec / 60.0) as u32,
                                            sym.start_sec % 60.0,
                                            (sym.end_sec / 60.0) as u32,
                                            sym.end_sec % 60.0
                                        ))
                                        .monospace(),
                                    );

                                    // Protocol
                                    ui.label(
                                        egui::RichText::new(sym.protocol.label()).strong().color(
                                            crate::ui_ext::contrast_text_c32_on(
                                                ui.visuals().panel_fill,
                                                ui.visuals().panel_fill,
                                                Some(proto_col),
                                                palette,
                                            ),
                                        ),
                                    );

                                    // Symbol
                                    ui.label(
                                        egui::RichText::new(&sym.symbol)
                                            .monospace()
                                            .strong()
                                            .color(if is_selected {
                                                accent_base
                                            } else {
                                                crate::hex_to_c32(&palette.colors.text_primary)
                                            }),
                                    );

                                    // Details
                                    ui.label(sym.details.as_deref().unwrap_or("—"));

                                    // Confidence
                                    ui.label(format!("{:.0}%", sym.confidence * 100.0));

                                    // Action button: Seek & Play
                                    if ui
                                        .add(crate::ui_ext::icon_button(
                                            Icon::Play,
                                            12.0,
                                            crate::hex_to_c32(&palette.colors.surface_panel),
                                            None,
                                            palette,
                                        ))
                                        .on_hover_text("Seek to timestamp")
                                        .clicked()
                                    {
                                        self.digital_decode.selected_symbol = Some(orig_idx);
                                        seek_to_time = Some(sym.start_sec);
                                    }

                                    ui.end_row();
                                }
                            });
                    });
            }
        }

        if let Some(t) = seek_to_time {
            self.seek_to(t);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn composite(fill: Color32, surface: Color32) -> Color32 {
        let foreground = egui::Rgba::from(fill);
        let background = egui::Rgba::from(surface);
        let remaining = 1.0 - foreground.a();
        Color32::from(egui::Rgba::from_rgb(
            foreground.r() + background.r() * remaining,
            foreground.g() + background.g() * remaining,
            foreground.b() + background.b() * remaining,
        ))
    }

    fn contrast(a: Color32, b: Color32) -> f32 {
        little_clip_core::palette::contrast_ratio((a.r(), a.g(), a.b()), (b.r(), b.g(), b.b()))
    }

    #[test]
    fn digital_decode_state_initializes_with_all_protocols_enabled() {
        let state = DigitalDecodeState::default();
        assert!(!state.in_progress);
        assert!(state.symbols.is_empty());
        assert_eq!(state.transcript_tab, TranscriptTab::Plaintext);
        assert_eq!(state.enabled_protocols.len(), DecoderProtocol::all().len());
    }

    #[test]
    fn transcript_tabs_toggle_correctly() {
        assert_eq!(TranscriptTab::default(), TranscriptTab::Plaintext);
        let tab = TranscriptTab::StructuredEvents;
        assert_ne!(tab, TranscriptTab::Plaintext);
    }

    #[test]
    fn protocol_labels_meet_contrast_on_controls_and_timeline_chips() {
        for appearance in [
            little_clip_core::config::WorkbenchAppearance::Dark,
            little_clip_core::config::WorkbenchAppearance::Light,
        ] {
            let palette = crate::ui_appearance::palette(appearance);
            let colors = crate::ui_appearance::WorkbenchColors::from_palette(&palette);

            for &protocol in DecoderProtocol::all() {
                let rgb = protocol.chip_color();
                let protocol_color = Color32::from_rgb(rgb[0], rgb[1], rgb[2]);
                let control_text = crate::ui_ext::contrast_text_c32_on(
                    colors.panel,
                    colors.panel,
                    Some(protocol_color),
                    &palette,
                );
                assert!(
                    contrast(colors.panel, control_text) >= 4.5,
                    "{appearance:?} {:?} control label lacks contrast",
                    protocol
                );

                let badge_fill = Color32::from_rgba_unmultiplied(rgb[0], rgb[1], rgb[2], 40);
                let badge_text = crate::ui_ext::contrast_text_c32_on(
                    badge_fill,
                    colors.canvas,
                    Some(protocol_color),
                    &palette,
                );
                assert!(
                    contrast(composite(badge_fill, colors.canvas), badge_text) >= 4.5,
                    "{appearance:?} {:?} timeline label lacks contrast",
                    protocol
                );

                for alpha in [120, 180, 220] {
                    let chip_fill = Color32::from_rgba_unmultiplied(rgb[0], rgb[1], rgb[2], alpha);
                    let chip_text = crate::ui_ext::contrast_text_c32_on(
                        chip_fill,
                        colors.canvas,
                        Some(colors.text_primary),
                        &palette,
                    );
                    assert!(
                        contrast(composite(chip_fill, colors.canvas), chip_text) >= 4.5,
                        "{appearance:?} {:?} symbol chip lacks contrast at alpha {alpha}",
                        protocol
                    );
                }
            }
        }
    }
}
