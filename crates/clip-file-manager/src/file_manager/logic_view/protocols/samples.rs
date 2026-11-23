use super::{Event, Kind, active_event, event_label};
use crate::file_manager::scope_view::plot as timing;
use egui::{Align2, FontId, Painter, Rect, Sense, Stroke, Ui, vec2};
use little_clip_core::engineering::logic_decode::I2sChannel;
use std::{fmt::Write, ops::Range};

pub(super) fn plot(
    ui: &mut Ui,
    span: &Range<f64>,
    cursor: Option<f64>,
    events: &[Event],
) -> Option<usize> {
    let (outer, response) =
        ui.allocate_exact_size(vec2(ui.available_width(), 72.0), Sense::click());
    let rect = Rect::from_min_max(outer.min + vec2(78.0, 0.0), outer.max - vec2(9.0, 2.0));
    let painter = ui.painter_at(outer);
    let clipped = painter.with_clip_rect(rect);
    clipped.rect_filled(rect, 2.0, ui.visuals().extreme_bg_color);
    if let Some(index) = active_event(events, cursor) {
        timing::highlight(
            &clipped,
            rect,
            span,
            events[index].time.clone(),
            ui.visuals().selection.bg_fill.gamma_multiply(0.24),
        );
    }
    for (index, channel) in [I2sChannel::Left, I2sChannel::Right]
        .into_iter()
        .enumerate()
    {
        let row = timing::lane(rect, index, 2);
        painter.text(
            row.left_center() - vec2(5.0, 0.0),
            Align2::RIGHT_CENTER,
            channel.label(),
            FontId::monospace(10.0),
            ui.visuals().text_color(),
        );
        let ink = ui.visuals().weak_text_color().gamma_multiply(0.33);
        clipped.hline(row.x_range(), row.center().y, Stroke::new(0.75, ink));
        samples(&clipped, row.shrink2(vec2(0.0, 2.0)), span, events, channel);
    }
    let point = response.hover_pos().filter(|point| rect.contains(*point))?;
    let at = timing::at(rect, span, point.x);
    let index = active_event(events, Some(at))?;
    let event = &events[index];
    let shown_channel = if point.y < rect.center().y {
        I2sChannel::Left
    } else {
        I2sChannel::Right
    };
    if Sample::from_event(event.kind)?.channel != shown_channel {
        return None;
    }
    let clicked = response.clicked();
    response.on_hover_text(event_label(event.kind));
    clicked.then_some(index)
}

fn samples(painter: &Painter, row: Rect, span: &Range<f64>, events: &[Event], channel: I2sChannel) {
    let color = timing::color(usize::from(channel == I2sChannel::Right));
    let dimmed = color.gamma_multiply(0.40);
    for event in events {
        let Some(sample) = Sample::from_event(event.kind) else {
            continue;
        };
        if sample.channel != channel {
            continue;
        }
        let left = timing::x_at(row, span, event.time.start);
        let right = timing::x_at(row, span, event.time.end).max(left + 1.0);
        if let Some(value) = sample.value {
            let full_scale = f64::from(1_u32 << (sample.bits - 1));
            let ratio = (f64::from(value) / full_scale) as f32;
            let y = row.center().y - row.height() * 0.45 * ratio;
            painter.hline(left..=right, y, Stroke::new(2.0, color));
        } else {
            painter.hline(left..=right, row.center().y, Stroke::new(3.0, dimmed));
        }
    }
}

struct Sample {
    channel: I2sChannel,
    value: Option<i32>,
    bits: u8,
    received: u8,
}

impl Sample {
    fn from_event(kind: Kind) -> Option<Self> {
        match kind {
            Kind::I2sSample {
                channel,
                sample,
                bits,
            } => Some(Self {
                channel,
                value: sample,
                bits,
                received: bits,
            }),
            Kind::I2sShortWord {
                channel,
                received,
                bits,
            } => Some(Self {
                channel,
                value: None,
                bits,
                received,
            }),
            _ => None,
        }
    }
}

pub(super) fn tsv(events: &[Event]) -> String {
    let mut text = String::from("start_s\tend_s\tchannel\tsample\tword_bits\treceived_bits\n");
    for event in events {
        if let Some(sample) = Sample::from_event(event.kind) {
            let value = sample
                .value
                .map_or_else(String::new, |value| value.to_string());
            let _ = writeln!(
                text,
                "{}\t{}\t{}\t{value}\t{}\t{}",
                event.time.start,
                event.time.end,
                sample.channel.label(),
                sample.bits,
                sample.received,
            );
        }
    }
    text
}
