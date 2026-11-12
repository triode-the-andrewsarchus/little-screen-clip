//! Direct signal-cell editing over a worker-prepared, lossless source snapshot.
use clip_ui::buttons::ButtonRole;
use egui::{Color32, Pos2, Rect, Sense, Stroke, Vec2};
use little_clip_core::{
    BackgroundJobs, diagram::wavejson::timing::Document, jobs::task::Task, palette::AppPalette,
};
use std::time::{Duration, Instant};

#[derive(Default)]
pub(super) struct WaveEditor {
    source: String,
    pending: Option<Task<Result<Document, String>>>,
    due: Option<Instant>,
    document: Option<Document>,
    error: Option<String>,
    painter: SignalPainter,
}
struct SignalPainter {
    brush: char,
    zoom: f32,
    selected: usize,
    name: String,
    gesture: Option<Gesture>,
}
struct Gesture {
    lane: usize,
    cells: Vec<char>,
    original: Vec<char>,
    painted: std::ops::RangeInclusive<usize>,
}
impl WaveEditor {
    pub(super) fn suspend(&mut self) {
        if self.pending.take().is_some() {
            self.due = Some(Instant::now());
        }
        self.painter.gesture = None;
    }
    fn prepare(&mut self, ui: &egui::Ui, source: &str) {
        if self.source != source {
            self.source = source.into();
            self.pending = None;
            self.document = None;
            self.painter.gesture = None;
            self.error = None;
            self.due = Some(Instant::now() + Duration::from_millis(120));
        }
        if let Some(result) = self.pending.as_ref().and_then(Task::poll) {
            self.pending = None;
            match result {
                Ok(Ok(document)) => {
                    self.painter.selected = self
                        .painter
                        .selected
                        .min(document.lanes.len().saturating_sub(1));
                    self.painter.name = document
                        .lanes
                        .get(self.painter.selected)
                        .map_or(String::new(), |lane| lane.name.clone());
                    self.document = Some(document);
                }
                Err(little_clip_core::JobError::QueueFull) => {
                    self.due = Some(Instant::now() + Duration::from_millis(250))
                }
                Ok(Err(error)) => self.error = Some(error),
                Err(error) => self.error = Some(error.to_string()),
            }
        }
        if self.pending.is_none() && self.due.is_some_and(|due| Instant::now() >= due) {
            self.due = None;
            let source = source.to_owned();
            self.pending = Some(Task::start(
                BackgroundJobs::global(),
                Duration::from_secs(5),
                move |_| Document::parse(&source),
            ));
        }
        if self.pending.is_some() || self.due.is_some() {
            ui.ctx().request_repaint_after(Duration::from_millis(30));
        }
    }
    pub(super) fn show(
        &mut self,
        ui: &mut egui::Ui,
        source: &str,
        palette: &AppPalette,
    ) -> Option<String> {
        self.prepare(ui, source);
        ui.heading("Signal painter");
        ui.small("Paint source cells · drag across a lane · timing and labels render below");
        let Some(document) = &self.document else {
            if let Some(error) = &self.error {
                ui.small(error);
            } else {
                ui.spinner();
            }
            return None;
        };
        let changed = self
            .painter
            .show(ui, document, palette)
            .map(|(lane, name, text)| document.replace(source, lane, name, &text));
        if let Some(error) = &self.error {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }

        match changed {
            Some(Ok(text)) if text != source => Some(text),
            Some(Err(error)) => {
                self.error = Some(error);
                None
            }
            _ => None,
        }
    }
}

impl Default for SignalPainter {
    fn default() -> Self {
        Self {
            brush: '1',
            zoom: 38.0,
            selected: 0,
            name: String::new(),
            gesture: None,
        }
    }
}
impl SignalPainter {
    fn show(
        &mut self,
        ui: &mut egui::Ui,
        document: &Document,
        palette: &AppPalette,
    ) -> Option<(usize, bool, String)> {
        ui.horizontal_wrapped(|ui| {
            for (state, label) in [
                ('0', "Low"),
                ('1', "High"),
                ('.', "Hold"),
                ('x', "Unknown"),
                ('z', "Hi-Z"),
                ('p', "Clock"),
                ('3', "Bus"),
            ] {
                ui.selectable_value(&mut self.brush, state, label);
            }
        });
        ui.horizontal(|ui| {
            ui.label("Cell width");
            clip_ui::sliders::render_track_slider(ui, &mut self.zoom, 22.0..=80.0, 150.0, palette)
                .on_hover_text("Width of one source cell; arrow keys adjust when focused.");
        });
        let mut changed = None;
        egui::ScrollArea::both()
            .id_source("wave_signal_cells")
            .max_height(330.0)
            .show(ui, |ui| {
                for (index, lane) in document.lanes.iter().enumerate() {
                    self.row(ui, index, lane);
                }
            });
        // Release can happen outside the lane or window. A gesture is one source
        // replacement and one undo step, never a stream of half-written drafts.
        if ui.input(|input| !input.pointer.primary_down())
            && let Some(gesture) = self.gesture.take()
        {
            let wave: String = gesture.cells.iter().collect();
            changed = Some((gesture.lane, false, wave));
        }
        self.inspector(ui, document, palette).or(changed)
    }
    fn inspector(
        &mut self,
        ui: &mut egui::Ui,
        document: &Document,
        palette: &AppPalette,
    ) -> Option<(usize, bool, String)> {
        if let Some(lane) = document.lanes.get(self.selected) {
            let rename = ui
                .horizontal(|ui| {
                    ui.label("Signal name");
                    let input = ui.add_enabled(
                        document.can_rename(self.selected),
                        egui::TextEdit::singleline(&mut self.name).desired_width(150.0),
                    );
                    (input.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)))
                        || ui
                            .add_enabled(
                                document.can_rename(self.selected),
                                ButtonRole::Secondary.button("Rename", palette),
                            )
                            .clicked()
                })
                .inner;
            if rename {
                return Some((self.selected, true, self.name.clone()));
            }
            ui.small(format!("{} source cells · period {} · phase {} · groups, labels and markers stay in source",lane.wave.chars().count(),lane.period,lane.phase));
        }
        None
    }
    fn row(
        &mut self,
        ui: &mut egui::Ui,
        index: usize,
        lane: &little_clip_core::diagram::wavejson::timing::Lane,
    ) {
        ui.horizontal(|ui| {
            let label = ui.add_sized(
                [120.0, 42.0],
                egui::SelectableLabel::new(self.selected == index, &lane.name),
            );
            if label.clicked() {
                self.selected = index;
                self.name.clone_from(&lane.name);
            }
            let editable = lane
                .wave
                .chars()
                .all(|ch| "01.xz=23456789pPnNhHlL".contains(ch));
            let cells: Vec<char> = self
                .gesture
                .as_ref()
                .filter(|g| g.lane == index)
                .map_or_else(|| lane.wave.chars().collect(), |g| g.cells.clone());
            let (rect, response) = ui.allocate_exact_size(
                Vec2::new((cells.len() as f32 * self.zoom).max(self.zoom), 42.0),
                Sense::click_and_drag(),
            );
            paint_lane(ui, rect, &cells, self.zoom, self.selected == index);
            if let Some(pos) = response.hover_pos() {
                let cell = ((pos.x - rect.left()) / self.zoom) as usize;
                if let Some(state) = cells.get(cell) {
                    response.clone().on_hover_text(format!(
                        "{} · cell {} · {}\nperiod {} · phase {}",
                        lane.name,
                        cell + 1,
                        state,
                        lane.period,
                        lane.phase
                    ));
                }
            }
            if editable && let Some(pos) = response.interact_pointer_pos() {
                let cell = (((pos.x - rect.left()) / self.zoom).max(0.0) as usize)
                    .min(cells.len().saturating_sub(1));
                if (response.drag_started() || response.clicked()) && !cells.is_empty() {
                    self.selected = index;
                    self.name.clone_from(&lane.name);
                    let origin =
                        ui.input(|input| input.pointer.press_origin())
                            .map_or(cell, |origin| {
                                (((origin.x - rect.left()) / self.zoom).max(0.0) as usize)
                                    .min(cells.len() - 1)
                            });
                    self.gesture = Some(Gesture {
                        lane: index,
                        cells: cells.clone(),
                        original: cells.clone(),
                        painted: origin..=origin,
                    });
                }
                if let Some(gesture) = &mut self.gesture
                    && gesture.lane == index
                {
                    gesture.paint_to(cell, self.brush);
                }
            }
            if !editable {
                response.on_hover_text("Gaps or extended wave states: edit this lane in source.");
            }
        });
    }
}

impl Gesture {
    fn paint_to(&mut self, cell: usize, brush: char) {
        let lo = cell.min(*self.painted.start());
        let hi = cell.max(*self.painted.end());
        if hi >= self.cells.len() {
            return;
        }
        self.cells.clone_from(&self.original);
        // Repeated explicit digital states mean edges/glitches to WaveDrom.
        // A painted run is one state followed by holds. Materialize the old
        // state after the run so unpainted hold cells keep their old meaning.
        let previous = self.original[..lo]
            .iter()
            .rev()
            .find(|code| **code != '.')
            .copied()
            .unwrap_or('x');
        let effective = if brush == '.' { previous } else { brush };
        if self.original.get(hi + 1) == Some(&'.') {
            let old = self.original[..=hi]
                .iter()
                .rev()
                .find(|code| **code != '.')
                .copied()
                .unwrap_or('x');
            if old != effective {
                self.cells[hi + 1] = old;
            }
        }
        self.cells[lo..=hi].fill('.');
        self.cells[lo] = if lo > 0 && previous == brush {
            '.'
        } else {
            brush
        };
        self.painted = lo..=hi;
    }
}
fn paint_lane(ui: &egui::Ui, rect: Rect, cells: &[char], width: f32, selected: bool) {
    let painter = ui.painter();
    let accent = if selected {
        ui.visuals().widgets.hovered.bg_stroke.color
    } else {
        ui.visuals().text_color()
    };
    let mut previous = 'x';
    for (index, code) in cells.iter().copied().enumerate() {
        let cell = Rect::from_min_size(
            Pos2::new(rect.left() + index as f32 * width, rect.top()),
            Vec2::new(width, rect.height()),
        );
        let effective = if matches!(code, '.' | '|') {
            previous
        } else {
            code
        };
        previous = effective;
        if !ui.is_rect_visible(cell) {
            continue;
        }
        painter.rect_filled(cell.shrink(1.0), 3.0, ui.visuals().faint_bg_color);
        paint_state(
            painter,
            cell.shrink2(Vec2::new(0.0, 9.0)),
            effective,
            accent,
        );
        painter.text(
            cell.center_bottom() - Vec2::new(0.0, 4.0),
            egui::Align2::CENTER_BOTTOM,
            code,
            egui::FontId::monospace(10.0),
            ui.visuals().weak_text_color(),
        );
    }
}
fn paint_state(painter: &egui::Painter, cell: Rect, code: char, color: Color32) {
    let left = cell.left();
    let right = cell.right();
    let high = cell.top();
    let low = cell.bottom();
    let points = match code {
        '0' | 'l' | 'L' => vec![Pos2::new(left, low), Pos2::new(right, low)],
        '1' | 'h' | 'H' => vec![Pos2::new(left, high), Pos2::new(right, high)],
        'p' | 'P' | 'n' | 'N' => {
            let (a, b) = if matches!(code, 'n' | 'N') {
                (high, low)
            } else {
                (low, high)
            };
            vec![
                Pos2::new(left, a),
                Pos2::new(left + 2.0, b),
                Pos2::new(cell.center().x, b),
                Pos2::new(cell.center().x, a),
                Pos2::new(right, a),
            ]
        }
        'z' => vec![
            Pos2::new(left, cell.center().y),
            Pos2::new(right, cell.center().y),
        ],
        _ => {
            painter.rect_filled(cell.shrink(2.0), 3.0, color.gamma_multiply(0.2));
            vec![
                Pos2::new(left, cell.center().y),
                Pos2::new(left + 4.0, high),
                Pos2::new(right - 4.0, high),
                Pos2::new(right, cell.center().y),
                Pos2::new(right - 4.0, low),
                Pos2::new(left + 4.0, low),
                Pos2::new(left, cell.center().y),
            ]
        }
    };
    painter.add(egui::Shape::line(points, Stroke::new(1.5_f32, color)));
}

#[cfg(test)]
mod tests {
    use super::*;
    fn frame(
        ctx: &egui::Context,
        editor: &mut WaveEditor,
        source: &str,
        time: f64,
        events: Vec<egui::Event>,
    ) -> (egui::FullOutput, Option<String>) {
        let mut edit = None;
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(900.0, 600.0))),
                time: Some(time),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    edit = editor.show(ui, source, &AppPalette::default());
                });
            },
        );
        (output, edit)
    }
    #[test]
    fn pointer_painting_includes_press_origin_and_commits_once_on_release_outside() {
        let ctx = egui::Context::default();
        let source = "{signal:[{name:'request',wave:'0.....'}], /* preserve */ extra:42}";
        let mut editor = WaveEditor {
            source: source.into(),
            document: Some(Document::parse(source).unwrap()),
            ..Default::default()
        };
        let (output, _) = frame(&ctx, &mut editor, source, 0.0, Vec::new());
        let mut labels: Vec<_> = output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::epaint::Shape::Text(text) if text.galley.text() == "0" => {
                    Some(text.pos + text.galley.rect.center().to_vec2())
                }
                _ => None,
            })
            .collect();
        labels.sort_by(|a, b| a.x.total_cmp(&b.x));
        assert_eq!(labels.len(), 1);
        let start = labels[0];
        let end = labels[0] + Vec2::new(editor.painter.zoom * 3.0, 0.0);
        let event = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        assert!(
            frame(
                &ctx,
                &mut editor,
                source,
                0.1,
                vec![egui::Event::PointerMoved(start), event(start, true)]
            )
            .1
            .is_none()
        );
        assert!(
            frame(
                &ctx,
                &mut editor,
                source,
                0.2,
                vec![egui::Event::PointerMoved(end)]
            )
            .1
            .is_none()
        );
        let outside = Pos2::new(end.x, 500.0);
        let (_, edited) = frame(
            &ctx,
            &mut editor,
            source,
            0.3,
            vec![egui::Event::PointerMoved(outside), event(outside, false)],
        );
        let edited = edited.expect("one committed gesture");
        assert_eq!(edited, source.replace("0.....", "1...0."));
        assert!(
            frame(&ctx, &mut editor, &edited, 0.4, Vec::new())
                .1
                .is_none()
        );
        assert!(
            editor.document.is_none(),
            "old source geometry must retire immediately"
        );
        assert!(editor.painter.gesture.is_none());
    }

    #[test]
    fn fast_drags_fill_in_both_directions_and_preserve_surrounding_holds() {
        let original: Vec<char> = "0.....".chars().collect();
        let mut gesture = Gesture {
            lane: 0,
            cells: original.clone(),
            original,
            painted: 1..=1,
        };
        gesture.paint_to(4, '1');
        assert_eq!(gesture.cells.iter().collect::<String>(), "01...0");
        gesture.paint_to(2, '1');
        assert_eq!(gesture.cells.iter().collect::<String>(), "01...0");
        gesture.paint_to(0, '1');
        assert_eq!(gesture.cells.iter().collect::<String>(), "1....0");
        let mut unchanged = Gesture {
            lane: 0,
            cells: "0.....".chars().collect(),
            original: "0.....".chars().collect(),
            painted: 1..=1,
        };
        unchanged.paint_to(4, '0');
        assert_eq!(unchanged.cells.iter().collect::<String>(), "0.....");
    }
}
