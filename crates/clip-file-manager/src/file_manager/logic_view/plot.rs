use super::protocols::{EventAction, event_label};
use crate::file_manager::scope_view::plot::{highlight, lane, x_at};
use egui::{Color32, Mesh, Painter, Rect, Response, Stroke, Ui, pos2};
use little_clip_core::engineering::{
    logic::{Envelope, Level, Trace},
    logic_decode::Event,
    logic_preview::{Overview, lane_rgb},
};
use std::ops::Range;

pub(in crate::file_manager) fn highlight_event(
    painter: &Painter,
    rect: Rect,
    span: &Range<f64>,
    time: Range<f64>,
) {
    highlight(
        painter,
        rect,
        span,
        time,
        Color32::from_rgba_unmultiplied(241, 189, 76, 75),
    );
}

pub(super) fn preview_color(index: usize, dark: bool) -> Color32 {
    let [r, g, b] = lane_rgb(index).map(|value| {
        if dark {
            value
        } else {
            (u16::from(value) * 3 / 5) as u8
        }
    });
    Color32::from_rgb(r, g, b)
}

pub(super) fn draw_overview(
    painter: &Painter,
    rect: Rect,
    overview: &Overview,
    dark: bool,
) -> Rect {
    let count = overview.lanes_previewed.len();
    let labels = rect.height() / count.max(1) as f32 >= 9.0 && rect.width() >= 150.0;
    let mut axis = rect;
    if labels {
        axis.min.x += 54.0;
    }
    for (index, envelope) in overview.lanes_previewed.iter().enumerate() {
        let row = lane(rect, index, count);
        let color = preview_color(index, dark);
        let mut trace = row.shrink2(egui::vec2(0.0, 1.0));
        if labels {
            let name = &overview.lanes[index].name;
            trace.min.x = axis.min.x;
            painter
                .with_clip_rect(Rect::from_min_max(
                    row.min,
                    pos2(trace.left() - 3.0, row.bottom()),
                ))
                .text(
                    row.left_center(),
                    egui::Align2::LEFT_CENTER,
                    name.rsplit('.').next().unwrap_or(name),
                    egui::FontId::monospace(9.0),
                    color,
                );
        }
        draw_envelope(painter, trace, &overview.span, envelope, color);
    }
    axis
}

pub(in crate::file_manager) fn draw_envelope(
    painter: &Painter,
    rect: Rect,
    visible: &Range<f64>,
    envelope: &Envelope,
    color: Color32,
) {
    let step = (envelope.time.end - envelope.time.start) / envelope.bins.len().max(1) as f64;
    let mut mesh = Mesh::default();
    for (index, bin) in envelope.bins.iter().enumerate() {
        let start = envelope.time.start + step * index as f64;
        let left = x_at(rect, visible, start).max(rect.left());
        let right = x_at(rect, visible, start + step).min(rect.right());
        if right <= left {
            continue;
        }
        let ink = if bin.exact {
            color
        } else {
            color.gamma_multiply(0.6)
        };
        for (present, level) in [
            (bin.high, Some(Level::High)),
            (bin.low, Some(Level::Low)),
            (bin.unknown || bin.high_impedance, None),
        ] {
            if present {
                let y = y_at(rect, level);
                mesh.add_colored_rect(
                    Rect::from_min_max(pos2(left, y - 0.5), pos2(right, y + 0.5)),
                    ink,
                );
            }
        }
        if bin.high && bin.low {
            mesh.add_colored_rect(
                Rect::from_min_max(
                    pos2(left, y_at(rect, Some(Level::High))),
                    pos2(right, y_at(rect, Some(Level::Low))),
                ),
                ink.gamma_multiply(0.35),
            );
        }
    }
    painter.add(mesh);
}

pub(in crate::file_manager) fn draw_trace(
    painter: &Painter,
    rect: Rect,
    visible: &Range<f64>,
    time: &Range<f64>,
    trace: &Trace,
    color: Color32,
) {
    let mut current = trace.initial;
    let mut points = vec![pos2(x_at(rect, visible, time.start), y_at(rect, current))];
    for edge in &trace.edges {
        let x = x_at(rect, visible, edge.time_seconds);
        points.push(pos2(x, y_at(rect, current)));
        current = Some(edge.level);
        points.push(pos2(x, y_at(rect, current)));
    }
    points.push(pos2(
        x_at(rect, visible, trace.inspected_until.min(time.end)),
        y_at(rect, current),
    ));
    painter.add(egui::Shape::line(points, Stroke::new(1.25, color)));
}

pub(in crate::file_manager) fn y_at(rect: Rect, level: Option<Level>) -> f32 {
    match level {
        Some(Level::High) => rect.top() + rect.height() * 0.14,
        Some(Level::Low) => rect.bottom() - rect.height() * 0.14,
        _ => rect.center().y,
    }
}

pub(in crate::file_manager) fn wire_label(
    ui: &egui::Ui,
    painter: &Painter,
    lane: Rect,
    name: &str,
    index: usize,
) -> bool {
    let half = (lane.height() * 0.5 - 2.0).clamp(3.0, 12.0);
    let rect = Rect::from_min_max(
        pos2(lane.left() - 74.0, lane.center().y - half),
        pos2(lane.left() - 4.0, lane.center().y + half),
    );
    let response = ui.interact(
        rect,
        ui.scope_id().with(("wire-logic-lane", index)),
        egui::Sense::click(),
    );
    let style = ui.style().interact(&response);
    painter.rect(
        rect,
        2.0,
        style.weak_bg_fill,
        style.bg_stroke,
        egui::StrokeKind::Inside,
    );
    painter.with_clip_rect(rect.shrink(2.0)).text(
        rect.right_center() - egui::vec2(4.0, 0.0),
        egui::Align2::RIGHT_CENTER,
        name.rsplit('.').next().unwrap_or(name),
        egui::FontId::monospace(11.0),
        style.text_color(),
    );
    response.on_hover_text(format!("Choose {name}")).clicked()
}

pub(in crate::file_manager) fn annotations(
    ui: &Ui,
    painter: &Painter,
    response: Response,
    span: &Range<f64>,
    events: &[Event],
    selected: Option<&Event>,
) -> Option<EventAction> {
    let band = response.rect;
    if !ui.is_rect_visible(band) {
        return None;
    }
    let painter = painter.with_clip_rect(band);
    let pointer = response
        .hover_pos()
        .or_else(|| response.interact_pointer_pos());
    let mut hit = None;
    let mut keyboard_target = None;
    for (event, segment, picked) in annotation_segments(band, span, events, selected) {
        let hovered = pointer.is_some_and(|point| segment.contains(point));
        if hovered {
            hit = Some(event);
        }
        if keyboard_target.is_none() || picked {
            keyboard_target = Some(event);
        }
        paint_annotation(ui, &painter, segment, event, picked, hovered);
    }
    let double_clicked = response.double_clicked();
    let clicked = response.clicked();
    if let Some(event) = hit {
        response
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text(format!(
                "{} · Double-click to show pulses",
                event_label(event.kind)
            ));
    } else if pointer.is_some() {
        return None;
    }
    let event = hit.or(keyboard_target)?;
    if double_clicked {
        Some(EventAction::Inspect(event.clone()))
    } else if clicked {
        Some(EventAction::Select(event.clone()))
    } else {
        None
    }
}

fn annotation_segments<'a>(
    band: Rect,
    span: &'a Range<f64>,
    events: &'a [Event],
    selected: Option<&'a Event>,
) -> impl Iterator<Item = (&'a Event, Rect, bool)> {
    let mut represented_until = band.left();
    let visible = events.iter().filter_map(move |event| {
        let mut segment = annotation_rect(band, span, event)?;
        segment.min.x = segment.left().max(represented_until);
        if segment.width() < 1.0 {
            return None;
        }
        represented_until = segment.right();
        // Keep the packing stable when a represented event is selected.
        (!selected.is_some_and(|selected| std::ptr::eq(selected, event)))
            .then_some((event, segment, false))
    });
    // The selected event stays visible even when the unselected overview
    // skipped its annotation at this zoom. Hit tests follow this paint order.
    visible.chain(selected.into_iter().filter_map(move |event| {
        annotation_rect(band, span, event).map(|segment| (event, segment, true))
    }))
}

fn annotation_rect(band: Rect, span: &Range<f64>, event: &Event) -> Option<Rect> {
    if event.time.start >= span.end
        || (event.time.end <= span.start && event.time.start < span.start)
    {
        return None;
    }
    let left = x_at(band, span, event.time.start).max(band.left());
    let right = x_at(band, span, event.time.end)
        .max(left + 2.0)
        .min(band.right());
    (right > left).then(|| {
        Rect::from_min_max(
            pos2(left, band.top() + 3.0),
            pos2(right, band.bottom() - 2.0),
        )
    })
}

fn paint_annotation(
    ui: &Ui,
    painter: &Painter,
    segment: Rect,
    event: &Event,
    picked: bool,
    hovered: bool,
) {
    let visuals = ui.visuals();
    let fill = visuals.selection.bg_fill;
    if picked || hovered {
        let border = if picked {
            visuals.selection.stroke.color
        } else {
            fill
        };
        painter.rect(
            segment,
            2.0,
            fill.gamma_multiply(0.48),
            Stroke::new(1.0, border),
            egui::StrokeKind::Inside,
        );
    } else {
        painter.rect_filled(segment, 2.0, fill.gamma_multiply(0.26));
    }
    if segment.width() > 65.0 {
        painter.with_clip_rect(segment).text(
            segment.left_center() + egui::vec2(3.0, 0.0),
            egui::Align2::LEFT_CENTER,
            event_label(event.kind),
            egui::FontId::monospace(10.0),
            visuals.text_color(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use little_clip_core::engineering::logic_decode::Kind;

    fn event(time: Range<f64>, byte: u8) -> Event {
        Event {
            time,
            kind: Kind::UartByte { byte },
        }
    }

    #[test]
    fn zoomed_out_annotations_keep_the_drawn_event_and_restore_a_hidden_selection_on_top() {
        let band = Rect::from_min_max(pos2(0.0, 0.0), pos2(100.0, 20.0));
        let span = 1.0..2.0;
        let events = [
            event(0.0..0.5, 0),
            event(0.7..1.1, 1),
            event(1.5..1.5, 2),
            event(1.5..1.8, 3),
            event(1.6..1.7, 4),
            event(1.9..2.1, 5),
        ];
        let represented = annotation_segments(band, &span, &events, None).collect::<Vec<_>>();
        assert_eq!(
            represented
                .iter()
                .map(|(event, _, _)| event.kind)
                .collect::<Vec<_>>(),
            [
                events[1].kind,
                events[2].kind,
                events[3].kind,
                events[5].kind
            ]
        );
        assert_eq!(represented[0].1.left(), band.left());
        assert_eq!(represented[1].1.width(), 2.0);
        assert_eq!(represented[2].1.left(), represented[1].1.right());
        assert_eq!(represented[3].1.right(), band.right());

        let selected = annotation_segments(band, &span, &events, Some(&events[4]))
            .filter(|(_, rect, _)| rect.contains(pos2(65.0, 10.0)))
            .last()
            .unwrap();
        assert_eq!(selected.0.kind, events[4].kind);
        assert!(selected.2);
    }
}
