//! Trim canvas sharing Audio Studio's viewport, ruler, color and envelope models.
use super::{ConstellationState, sample_pack::SlotSample};
use crate::audio_preview::{AudioViewport, format_time_label, time_ruler_interval};
use egui::{Color32, Pos2, Rect, Stroke, Vec2};

impl ConstellationState {
    pub(super) fn render_layer_waveform(ui: &mut egui::Ui, sample: &mut SlotSample) {
        let duration = sample.duration.max(f32::EPSILON);
        let frame = sample
            .cached_audio
            .as_ref()
            .map_or(0.000001, |audio| 1.0 / audio.sample_rate as f32);
        let id = ui
            .id()
            .with(("pad_wave_view", &sample.file_path, duration.to_bits()));
        let full = AudioViewport {
            start_sec: 0.0,
            duration_sec: duration,
        };
        let mut view = ui
            .data(|data| data.get_temp::<AudioViewport>(id))
            .unwrap_or(full);
        let height = 164.0;
        let (outer, response) = ui.allocate_exact_size(
            Vec2::new(ui.available_width(), height),
            egui::Sense::click_and_drag(),
        );
        let rect = Rect::from_min_max(
            outer.min + Vec2::new(12.0, 42.0),
            outer.max - Vec2::new(12.0, 12.0),
        );
        if response.hovered() && duration >= 0.01 {
            let (wheel, shift) =
                ui.input(|input| (input.raw_scroll_delta.y, input.modifiers.shift));
            if wheel != 0.0 {
                ui.input_mut(|input| {
                    input.smooth_scroll_delta = Vec2::ZERO;
                });
                if shift {
                    view.pan(-wheel * view.duration_sec / 600.0, duration);
                } else {
                    let fraction = response.hover_pos().map_or(0.5, |pos| {
                        ((pos.x - rect.left()) / rect.width()).clamp(0.0, 1.0)
                    });
                    view.zoom(
                        (-wheel / 250.0).exp(),
                        view.start_sec + fraction * view.duration_sec,
                        duration,
                        frame.recip(),
                    );
                }
            }
        }
        let x_at =
            |time: f32| rect.left() + (time - view.start_sec) / view.duration_sec * rect.width();
        let start = x_at(sample.start_sec);
        let end = x_at(sample.end_sec);
        let painter = ui.painter_at(outer);
        painter.rect_filled(outer, 4.0, ui.visuals().extreme_bg_color);
        paint_ruler(ui, &painter, outer, rect, view);
        let retained = Rect::from_min_max(
            Pos2::new(start.clamp(rect.left(), rect.right()), rect.top()),
            Pos2::new(end.clamp(rect.left(), rect.right()), rect.bottom()),
        );
        painter.rect_filled(
            retained,
            0.0,
            Color32::from_rgba_unmultiplied(
                ui.visuals().text_color().r(),
                ui.visuals().text_color().g(),
                ui.visuals().text_color().b(),
                12,
            ),
        );
        paint_peaks(
            &painter,
            rect,
            sample,
            view,
            ui.ctx().pixels_per_point(),
            ui.visuals().weak_text_color(),
        );
        paint_markers(ui, &painter, rect, sample, view, frame);
        let fade_drag = edit_fades(ui, &painter, rect, sample, view);
        if response.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
        }
        if !fade_drag {
            edit_trim(ui, &response, rect, sample, view, frame);
        }
        ui.data_mut(|data| data.insert_temp(id, view));
    }
}

fn paint_ruler(
    ui: &egui::Ui,
    painter: &egui::Painter,
    outer: Rect,
    rect: Rect,
    view: AudioViewport,
) {
    let x_at = |time| view.time_to_x(time, rect.left(), rect.width());
    let mut interval = time_ruler_interval(view.duration_sec);
    while interval / view.duration_sec * rect.width() < 64.0 {
        interval *= 2.0;
    }
    let mut time = (view.start_sec / interval).ceil() * interval;
    while time <= view.start_sec + view.duration_sec {
        let x = x_at(time);
        painter.line_segment(
            [Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())],
            Stroke::new(1.0_f32, ui.visuals().faint_bg_color),
        );
        painter.text(
            Pos2::new(x, outer.top() + 3.0),
            egui::Align2::CENTER_TOP,
            format_time_label(time, interval),
            egui::FontId::monospace(10.0),
            ui.visuals().weak_text_color(),
        );
        time += interval;
    }
}

fn paint_peaks(
    painter: &egui::Painter,
    rect: Rect,
    sample: &SlotSample,
    view: AudioViewport,
    pixels_per_point: f32,
    muted: Color32,
) {
    let duration = sample.duration.max(f32::EPSILON);
    if let Some(audio) = &sample.cached_audio {
        // Aggregate prepared peaks into displayed columns; never scan PCM on the UI thread.
        let count = audio.waveform_peaks.len();
        let columns = (rect.width() * pixels_per_point).ceil().clamp(1.0, 2048.0) as usize;
        for column in 0..columns {
            if count == 0 {
                break;
            }
            let time = view.start_sec + column as f32 / columns as f32 * view.duration_sec;
            let next = time + view.duration_sec / columns as f32;
            let begin = ((time / duration * count as f32) as usize).min(count - 1);
            let finish = ((next / duration * count as f32).ceil() as usize).clamp(begin + 1, count);
            let (low, high) = audio.waveform_peaks[begin..finish]
                .iter()
                .fold((0.0_f32, 0.0_f32), |(low, high), &(a, b)| {
                    (low.min(a), high.max(b))
                });
            let rgb = if audio.waveform_colors.len() >= finish {
                let sum = audio.waveform_colors[begin..finish].iter().fold(
                    [0_u32; 3],
                    |mut sum, color| {
                        for channel in 0..3 {
                            sum[channel] += color[channel] as u32;
                        }
                        sum
                    },
                );
                sum.map(|value| (value / (finish - begin) as u32) as u8)
            } else {
                [160; 3]
            };
            let color = if time >= sample.start_sec && time <= sample.end_sec {
                Color32::from_rgb(rgb[0], rgb[1], rgb[2])
            } else {
                muted.linear_multiply(0.4)
            };
            let x = rect.left() + column as f32 / columns as f32 * rect.width();
            let amplitude = rect.height() * 0.48;
            painter.rect_filled(
                Rect::from_min_max(
                    Pos2::new(x, rect.center().y - high.min(1.0) * amplitude),
                    Pos2::new(
                        x + rect.width() / columns as f32,
                        rect.center().y + (-low).clamp(0.005, 1.0) * amplitude,
                    ),
                ),
                0.0,
                color,
            );
        }
    } else {
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            "Preparing audio…",
            egui::FontId::proportional(12.0),
            muted,
        );
    }
}

fn paint_markers(
    ui: &egui::Ui,
    painter: &egui::Painter,
    rect: Rect,
    sample: &SlotSample,
    view: AudioViewport,
    frame: f32,
) {
    let x_at = |time| view.time_to_x(time, rect.left(), rect.width());
    let start = x_at(sample.start_sec);
    let end = x_at(sample.end_sec);
    if sample.attack_ms > 0.0 || sample.release_ms > 0.0 {
        let points = (0..=64)
            .map(|index| {
                let time = view.start_sec + view.duration_sec * index as f32 / 64.0;
                let gain = super::layer_source::fade_gain(
                    time - sample.start_sec,
                    sample.end_sec - time - frame,
                    sample.attack_ms / 1000.0,
                    sample.release_ms / 1000.0,
                );
                Pos2::new(x_at(time), rect.center().y - gain * rect.height() * 0.48)
            })
            .collect();
        painter.add(egui::Shape::line(
            points,
            Stroke::new(1.5_f32, ui.visuals().text_color()),
        ));
    }
    for (x, label) in [(start, "IN"), (end, "OUT")] {
        if x < rect.left() - 0.1 || x > rect.right() + 0.1 {
            continue;
        }
        let color = ui.visuals().text_color();
        painter.line_segment(
            [Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())],
            Stroke::new(1.5_f32, color),
        );
        painter.rect_filled(
            Rect::from_center_size(Pos2::new(x, rect.center().y), Vec2::new(8.0, 22.0)),
            2.0,
            color,
        );
        painter.text(
            Pos2::new(x, rect.bottom() + 1.0),
            egui::Align2::CENTER_TOP,
            label,
            egui::FontId::monospace(9.0),
            color,
        );
    }
}

fn edit_trim(
    ui: &mut egui::Ui,
    response: &egui::Response,
    rect: Rect,
    sample: &mut SlotSample,
    view: AudioViewport,
    frame: f32,
) {
    let [start, end] = [sample.start_sec, sample.end_sec]
        .map(|time| view.time_to_x(time, rect.left(), rect.width()));
    let duration = sample.duration.max(f32::EPSILON);
    if response.double_clicked() {
        sample.start_sec = 0.0;
        sample.end_sec = duration;
    }
    if response.drag_started()
        && let Some(pointer) = response.interact_pointer_pos()
    {
        ui.data_mut(|data| {
            data.insert_temp(
                response.id.with("trim_start"),
                (pointer.x - start).abs() <= (pointer.x - end).abs(),
            )
        });
    }
    if response.dragged()
        && let Some(pointer) = response.interact_pointer_pos()
    {
        let time = sample.snap_time(
            (view.start_sec + (pointer.x - rect.left()) / rect.width() * view.duration_sec)
                .clamp(0.0, duration),
        );
        if ui
            .data(|data| data.get_temp::<bool>(response.id.with("trim_start")))
            .unwrap_or(true)
        {
            sample.start_sec = time.clamp(0.0, (sample.end_sec - frame).max(0.0));
        } else {
            sample.end_sec = time.clamp((sample.start_sec + frame).min(duration), duration);
        }
    }
}

/// Small handles above the trim rails control fades; the envelope remains visible.
fn edit_fades(
    ui: &mut egui::Ui,
    painter: &egui::Painter,
    rect: Rect,
    sample: &mut SlotSample,
    view: AudioViewport,
) -> bool {
    let mut editing = false;
    let span = (sample.end_sec - sample.start_sec).max(0.0);
    let [attack_x, release_x] = [
        sample.start_sec + (sample.attack_ms / 1000.0).min(span),
        sample.end_sec - (sample.release_ms / 1000.0).min(span),
    ]
    .map(|time| view.time_to_x(time, rect.left(), rect.width()));
    let crowded = (attack_x - release_x).abs() < 20.0;
    for (attack, x) in [(true, attack_x), (false, release_x)] {
        if x < rect.left() || x > rect.right() {
            continue;
        }
        // Keep both targets accessible when fades meet or the trim is very short.
        let offset = if crowded {
            if attack { 21.0 } else { 8.0 }
        } else {
            12.0
        };
        let handle =
            Rect::from_center_size(Pos2::new(x, rect.top() - offset), Vec2::new(18.0, 12.0));
        let response = ui.interact(
            handle,
            ui.id().with(("fade", attack)),
            egui::Sense::click_and_drag(),
        );
        let color = ui.visuals().hyperlink_color;
        painter.rect_filled(handle, 2.0, ui.visuals().extreme_bg_color);
        painter.rect_stroke(handle, 2.0, Stroke::new(1.0_f32, color));
        painter.text(
            handle.center(),
            egui::Align2::CENTER_CENTER,
            if attack { "A" } else { "R" },
            egui::FontId::monospace(9.0),
            color,
        );
        if response.dragged()
            && let Some(pointer) = response.interact_pointer_pos()
        {
            let time =
                view.start_sec + (pointer.x - rect.left()) / rect.width() * view.duration_sec;
            let ms = if attack {
                time - sample.start_sec
            } else {
                sample.end_sec - time
            } * 1000.0;
            let limit = ((sample.end_sec - sample.start_sec) * 1000.0).min(500.0);
            if attack {
                sample.attack_ms = ms.clamp(0.0, limit);
            } else {
                sample.release_ms = ms.clamp(0.0, limit);
            }
        }
        if response.double_clicked() {
            if attack {
                sample.attack_ms = 0.0;
            } else {
                sample.release_ms = 0.0;
            }
        }
        editing |= response.hovered() || response.dragged();
        response.on_hover_text(if attack {
            "Attack · drag right"
        } else {
            "Release · drag left"
        });
    }
    editing
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fade_flags_edit_the_envelope_without_moving_trim() {
        let ctx = egui::Context::default();
        let mut sample = SlotSample::new("hit.wav".into(), "hit.wav".into(), None, 1.0);
        let mut frame = |events| {
            ctx.run(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(600.0, 240.0))),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        ConstellationState::render_layer_waveform(ui, &mut sample)
                    });
                },
            )
        };
        let output = frame(vec![]);
        let start = output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.text() == "A" => {
                    Some(text.pos + text.galley.size() * 0.5)
                }
                _ => None,
            })
            .unwrap();
        let end = start + Vec2::new(90.0, 0.0);
        frame(vec![
            egui::Event::PointerMoved(start),
            egui::Event::PointerButton {
                pos: start,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
        ]);
        frame(vec![egui::Event::PointerMoved(end)]);
        frame(vec![egui::Event::PointerButton {
            pos: end,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        }]);
        assert!(sample.attack_ms > 100.0);
        assert_eq!((sample.start_sec, sample.end_sec), (0.0, 1.0));
        assert_eq!(sample.release_ms, 0.0);
        sample.attack_ms = 500.0;
        sample.release_ms = 500.0;
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(600.0, 240.0))),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    ConstellationState::render_layer_waveform(ui, &mut sample)
                });
            },
        );
        let label_pos = |label: &str| {
            output
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) if text.galley.text() == label => {
                        Some(text.pos + text.galley.size() * 0.5)
                    }
                    _ => None,
                })
                .unwrap()
        };
        assert!(
            (label_pos("A").y - label_pos("R").y).abs() >= 12.0,
            "coincident fades need independent drag targets"
        );
    }
}
