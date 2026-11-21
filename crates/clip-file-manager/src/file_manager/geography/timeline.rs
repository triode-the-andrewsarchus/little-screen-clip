//! Scrub the shared astronomy clock without leaving the current view.
mod scale;
mod scrub;

use super::{map_camera, solar};
use chrono::{DateTime, Months, Utc};
use egui::{Align2, Color32, FontId, Mesh, Painter, Rect, Sense, Stroke, pos2, vec2};
use little_clip_core::geography::Document;
pub(super) use scrub::Preview;

pub(super) const HEIGHT: f32 = 63.0;
const MIN_SCALE: f64 = 0.5;
const DEFAULT_SCALE: f64 = 200.0;
const MAX_SCALE: f64 = scale::EARTH_YEAR;
const FILE_RANGE: Color32 = Color32::from_rgb(108, 222, 183);
const PLAYHEAD: Color32 = Color32::from_rgb(255, 225, 166);

pub(super) struct Timeline {
    fixed: Option<DateTime<Utc>>,
    preview: Option<DateTime<Utc>>,
    recorded: Option<[DateTime<Utc>; 2]>,
    seconds_per_point: f64,
    drag_time: Option<f64>,
    scrub: Option<scrub::Scrub>,
}

impl Timeline {
    pub fn new(document: &Document) -> Self {
        Self::with_range(document.recorded_range())
    }

    pub fn with_range(recorded: Option<[DateTime<Utc>; 2]>) -> Self {
        let seconds_per_point = recorded.map_or(DEFAULT_SCALE, |[first, last]| {
            let span = (last - first).num_seconds() as f64;
            if span > 0.0 {
                (span / 300.0).clamp(MIN_SCALE, MAX_SCALE)
            } else {
                DEFAULT_SCALE
            }
        });
        Self {
            fixed: None,
            preview: None,
            recorded,
            seconds_per_point,
            drag_time: None,
            scrub: None,
        }
    }

    pub fn preview(&mut self, time: Option<DateTime<Utc>>) {
        self.preview = time;
    }

    pub fn fixed(&self) -> Option<DateTime<Utc>> {
        self.fixed
    }

    pub fn contextual_time(&self) -> Option<DateTime<Utc>> {
        self.preview
            .or_else(|| self.recorded.map(|[first, _]| first))
    }

    pub fn time(&self) -> Option<DateTime<Utc>> {
        self.fixed.or_else(|| self.contextual_time())
    }

    pub fn rendered_time(&mut self, ctx: &egui::Context) -> Option<Preview> {
        let scrub = self.scrub.as_mut()?;
        let frame = scrub.frame(ctx.input(|input| input.time));
        if scrub.settled() {
            self.scrub = None;
        } else {
            ctx.request_repaint();
        }
        Some(frame)
    }

    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        rect: Rect,
        id: solar::Id,
        now: DateTime<Utc>,
        source: Option<&str>,
    ) {
        if rect.width() < 32.0 || rect.height() < 36.0 {
            return;
        }
        let response = ui
            .interact(
                rect,
                ui.scope_id().with("astronomy-timeline"),
                Sense::click_and_drag(),
            )
            .on_hover_cursor(egui::CursorIcon::Grab)
            .on_hover_ui(|ui| {
                ui.label("Drag left for later or right for earlier. Scroll to change scale.");
                ui.label("Double-click to return to the start of the file or to the live clock.");
                if let Some([first, last]) = self.recorded {
                    ui.label(format!(
                        "Recorded time   {} – {} UTC",
                        first.format("%Y-%m-%d %H:%M"),
                        last.format("%Y-%m-%d %H:%M")
                    ));
                }
                if let Some(source) = source {
                    ui.label(source);
                }
                if id != solar::EARTH {
                    ui.label("Cycle numbers are relative to J2000.");
                }
            });
        let limit = Months::new(6_000);
        let bounds = [now - limit, now + limit].map(scale::seconds);
        let time = self.navigate(&response, now, bounds);
        if self.time().is_none() {
            let millis = 60_000 - now.timestamp_millis().rem_euclid(60_000);
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(millis as u64));
        }
        let track = Track {
            rect: Rect::from_min_max(
                rect.left_top() + vec2(10.0, 28.0),
                rect.right_bottom() - vec2(10.0, 5.0),
            ),
            center: scale::seconds(time),
            scale: self.seconds_per_point,
        };
        let calendar = scale::Calendar::for_body(id);
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 5.0, ui.visuals().panel_fill);
        track.paint(&painter, &calendar, bounds, self.recorded);
        painter.text(
            rect.center_top() + vec2(0.0, 6.0),
            Align2::CENTER_TOP,
            time.format("%Y-%m-%d  %H:%M UTC").to_string(),
            FontId::monospace(13.0),
            ui.visuals().strong_text_color(),
        );
        if rect.width() > 460.0 {
            painter.text(
                rect.left_top() + vec2(12.0, 7.0),
                Align2::LEFT_TOP,
                calendar.caption(track.span()),
                FontId::proportional(11.0),
                ui.visuals().text_color(),
            );
            if self.recorded.is_some() {
                painter.text(
                    rect.right_top() + vec2(-12.0, 7.0),
                    Align2::RIGHT_TOP,
                    "Recorded time",
                    FontId::proportional(11.0),
                    FILE_RANGE,
                );
            }
        }
    }

    fn navigate(
        &mut self,
        response: &egui::Response,
        now: DateTime<Utc>,
        bounds: [f64; 2],
    ) -> DateTime<Utc> {
        let current = self.time().unwrap_or(now);
        let previous_fixed = self.fixed;
        let scroll = map_camera::scroll_zoom(response);
        self.seconds_per_point =
            (self.seconds_per_point / scroll.powf(2.5)).clamp(MIN_SCALE, MAX_SCALE);

        if response.double_clicked() {
            self.fixed = None;
            self.drag_time = None;
            response.ctx.request_repaint();
        } else if response.dragged_by(egui::PointerButton::Primary) {
            let raw = self
                .drag_time
                .get_or_insert_with(|| scale::seconds(current));
            *raw = (*raw - f64::from(response.drag_delta().x) * self.seconds_per_point)
                .clamp(bounds[0], bounds[1]);
            self.fixed = Some(scale::instant(
                ((*raw / 60.0).round() * 60.0).clamp(bounds[0], bounds[1]),
            ));
            response.ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
            response.ctx.request_repaint();
        } else if self.drag_time.take().is_some()
            && let Some(scrub) = &mut self.scrub
        {
            scrub.end_drag();
        }

        let time = self.time().unwrap_or(now);
        if self.fixed != previous_fixed && time != current {
            let animation_time = response.ctx.input(|input| input.time);
            self.scrub
                .get_or_insert_with(|| scrub::Scrub::new(current, animation_time))
                .retarget(time, animation_time, self.drag_time.is_some());
        }
        time
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Context, Event, Modifiers, PointerButton, RawInput};

    #[test]
    fn dragging_and_double_clicking_share_the_file_or_live_clock() {
        for document in [
            Document::geojson(br#"{"type":"Point","coordinates":[0,0]}"#).unwrap(),
            Document::gpx(
                br#"<gpx xmlns="http://www.topografix.com/GPX/1/1">
                <trk><trkseg>
                  <trkpt lat="1" lon="1"><time>2024-04-08T13:17:00-05:00</time></trkpt>
                  <trkpt lat="1" lon="2"><time>2023-10-14T18:00:00Z</time></trkpt>
                </trkseg></trk>
              </gpx>"#,
            )
            .unwrap(),
        ] {
            let mut timeline = Timeline::new(&document);
            let now = "2026-11-21T17:00:00Z".parse().unwrap();
            let original = timeline.time();
            if !document.features[0].paths[0][0].point.time.is_empty() {
                assert_eq!(original.unwrap().to_rfc3339(), "2023-10-14T18:00:00+00:00");
            }
            let ctx = Context::default();
            let screen = Rect::from_min_size(pos2(0.0, 0.0), vec2(720.0, 80.0));
            let track = screen.shrink(7.0);
            let middle = track.center();
            let moved = middle - vec2(90.0, 0.0);
            let button = |pos, pressed| Event::PointerButton {
                pos,
                button: PointerButton::Primary,
                pressed,
                modifiers: Modifiers::NONE,
            };
            let frame = |timeline: &mut Timeline, time, events| {
                let mut output = ctx.run_ui(
                    RawInput {
                        screen_rect: Some(screen),
                        time: Some(time),
                        events,
                        ..RawInput::default()
                    },
                    |ui| timeline.show(ui, track, solar::EARTH, now, None),
                );
                output.textures_delta.clear();
            };
            frame(&mut timeline, 0.0, vec![Event::PointerMoved(middle)]);
            frame(&mut timeline, 0.1, vec![button(middle, true)]);
            frame(&mut timeline, 0.2, vec![Event::PointerMoved(moved)]);
            frame(&mut timeline, 0.3, vec![button(moved, false)]);
            assert!(timeline.time().unwrap() > original.unwrap_or(now));
            for (time, pressed) in [(1.0, true), (1.05, false), (1.12, true), (1.17, false)] {
                frame(&mut timeline, time, vec![button(moved, pressed)]);
            }
            assert_eq!(timeline.time(), original);

            timeline.seconds_per_point = MAX_SCALE;
            let left = pos2(track.left() + 3.0, middle.y);
            let right = pos2(track.right() - 3.0, middle.y);
            frame(&mut timeline, 2.0, vec![Event::PointerMoved(right)]);
            frame(&mut timeline, 2.1, vec![button(right, true)]);
            frame(&mut timeline, 2.2, vec![Event::PointerMoved(left)]);
            frame(&mut timeline, 2.3, vec![button(left, false)]);
            assert_eq!(timeline.time(), Some(now + Months::new(6_000)));

            frame(&mut timeline, 2.8, vec![button(left, true)]);
            frame(&mut timeline, 3.0, vec![Event::PointerMoved(right)]);
            frame(&mut timeline, 3.1, vec![button(right, false)]);
            frame(&mut timeline, 3.2, vec![Event::PointerMoved(left)]);
            frame(&mut timeline, 3.3, vec![button(left, true)]);
            frame(&mut timeline, 3.5, vec![Event::PointerMoved(right)]);
            frame(&mut timeline, 3.6, vec![button(right, false)]);
            assert_eq!(timeline.time(), Some(now - Months::new(6_000)));
        }
    }
}

struct Track {
    rect: Rect,
    center: f64,
    scale: f64,
}

impl Track {
    fn span(&self) -> f64 {
        f64::from(self.rect.width()) * self.scale
    }

    fn x(&self, time: f64) -> f32 {
        self.rect.center().x + ((time - self.center) / self.scale) as f32
    }

    fn paint(
        &self,
        painter: &Painter,
        calendar: &scale::Calendar,
        bounds: [f64; 2],
        recorded: Option<[DateTime<Utc>; 2]>,
    ) {
        let span = self.span();
        let range = [self.center - span / 2.0, self.center + span / 2.0];
        let painter = painter.with_clip_rect(painter.clip_rect().intersect(self.rect));
        let mut gradient = Mesh::default();
        let steps = (self.rect.width() / 5.0).ceil().clamp(1.0, 320.0) as u32;
        for i in 0..=steps {
            let fraction = f64::from(i) / f64::from(steps);
            let time = range[0] + span * fraction;
            let x = self.x(time);
            let colors = calendar.color(time, self.scale);
            gradient.colored_vertex(pos2(x, self.rect.top()), colors[0]);
            gradient.colored_vertex(pos2(x, self.rect.bottom()), colors[1]);
            if i != 0 {
                let last = i * 2;
                gradient.add_triangle(last - 2, last - 1, last);
                gradient.add_triangle(last - 1, last + 1, last);
            }
        }
        painter.add(egui::Shape::mesh(gradient));
        for [start, end] in [[range[0], bounds[0]], [bounds[1], range[1]]] {
            if start < end {
                painter.rect_filled(
                    Rect::from_min_max(
                        pos2(self.x(start), self.rect.top()),
                        pos2(self.x(end), self.rect.bottom()),
                    ),
                    0.0,
                    Color32::from_black_alpha(145),
                );
            }
        }
        if let Some([first, last]) = recorded {
            self.paint_recorded(&painter, first, last);
        }
        for tick in calendar.ticks(range, self.scale * 94.0) {
            let x = self.x(tick.time);
            let top = self.rect.top();
            painter.line_segment(
                [pos2(x, top), pos2(x, top + 8.0)],
                Stroke::new(1.0, Color32::from_white_alpha(118)),
            );
            painter.text(
                pos2(x + 5.0, top + 10.0),
                Align2::LEFT_TOP,
                tick.label,
                FontId::proportional(11.0),
                Color32::from_rgb(244, 245, 248),
            );
        }
        let x = self.rect.center().x;
        painter.line_segment(
            [pos2(x, self.rect.top()), pos2(x, self.rect.bottom())],
            Stroke::new(1.5, PLAYHEAD),
        );
        painter.add(egui::Shape::convex_polygon(
            vec![
                pos2(x - 4.0, self.rect.top()),
                pos2(x + 4.0, self.rect.top()),
                pos2(x, self.rect.top() + 5.0),
            ],
            PLAYHEAD,
            Stroke::NONE,
        ));
    }

    fn paint_recorded(&self, painter: &Painter, first: DateTime<Utc>, last: DateTime<Utc>) {
        let [left, right] = [first, last].map(|time| self.x(scale::seconds(time)));
        let bottom = self.rect.bottom();
        if right < self.rect.left() || left > self.rect.right() {
            let direction = if right < self.rect.left() { -1.0 } else { 1.0 };
            let edge = self.rect.center().x + direction * (self.rect.width() / 2.0 - 1.0);
            painter.add(egui::Shape::convex_polygon(
                vec![
                    pos2(edge, bottom - 4.0),
                    pos2(edge - direction * 9.0, bottom - 8.0),
                    pos2(edge - direction * 9.0, bottom),
                ],
                FILE_RANGE,
                Stroke::NONE,
            ));
            return;
        }
        let span = Rect::from_min_max(
            pos2(left, self.rect.top()),
            pos2(right.max(left + 4.0), bottom),
        );
        painter.rect_filled(span, 0.0, FILE_RANGE.gamma_multiply(0.14));
        painter.rect_filled(
            Rect::from_min_max(pos2(span.left(), bottom - 4.0), span.max),
            0.0,
            FILE_RANGE,
        );
    }
}
