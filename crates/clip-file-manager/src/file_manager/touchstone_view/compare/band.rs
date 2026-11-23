use super::{Band, plot};
use egui::{Button, DragValue, Id, PointerButton, Rect, Response, Ui};

#[derive(Default)]
pub(super) struct MatchBand {
    pub selected: Option<Band>,
    brush: Option<Brush>,
}

struct Brush {
    plot: Id,
    start: f64,
    end: f64,
}

impl MatchBand {
    pub(super) fn controls(&mut self, ui: &mut Ui, viewport: &mut Band, full: Band) {
        let Some([mut low, mut high]) = self.selected else {
            ui.horizontal_wrapped(|ui| {
                if ui
                    .button("Use visible match band")
                    .on_hover_text("Keep the current frequency interval when you zoom or pan")
                    .clicked()
                {
                    self.selected = Some(*viewport);
                }
                ui.weak("Shift-drag either frequency plot");
            });
            return;
        };
        ui.horizontal_wrapped(|ui| {
            ui.label("Match band");
            low = endpoint(ui, low, [full[0].min(low), high], "Lower frequency");
            ui.label("–");
            high = endpoint(ui, high, [low, full[1].max(high)], "Upper frequency");
            self.selected = Some([low, high]);
            let covered = [low.max(full[0]), high.min(full[1])];
            if ui
                .add_enabled(covered[0] < covered[1], Button::new("Fit band"))
                .clicked()
            {
                *viewport = plot::fit_band(covered, full);
            }
            if ui.button("Clear band").clicked() {
                self.selected = None;
            }
        });
    }

    pub(super) fn visible_selection(&self) -> Option<Band> {
        self.brush.as_ref().map(Brush::interval).or(self.selected)
    }

    pub(super) fn drag(&mut self, ui: &Ui, rect: Rect, hit: &Response, viewport: Band) -> bool {
        let primary = PointerButton::Primary;
        let shift = ui.input(|input| input.modifiers.shift);
        let at = |x| plot::hz_at(rect, viewport, x);
        if shift
            && hit.drag_started_by(primary)
            && let Some(press) = ui.input(|input| input.pointer.press_origin())
        {
            self.brush = Some(Brush {
                plot: hit.id,
                start: at(press.x),
                end: at(press.x),
            });
        }
        let Some(brush) = self.brush.as_mut().filter(|brush| brush.plot == hit.id) else {
            return shift;
        };
        if let Some(pointer) = hit.interact_pointer_pos() {
            let end = at(pointer.x);
            if brush.end != end {
                brush.end = end;
                ui.ctx().request_repaint();
            }
        }
        if hit.drag_stopped_by(primary) {
            let band = brush.interval();
            if band[0] < band[1] {
                self.selected = Some(band);
            }
            self.brush = None;
            ui.ctx().request_repaint();
        }
        true
    }

    pub(super) fn finish_frame(&mut self, ui: &Ui) {
        if self.brush.is_some() && !ui.input(|input| input.pointer.primary_down()) {
            self.brush = None;
            ui.ctx().request_repaint();
        }
    }
}

impl Brush {
    fn interval(&self) -> Band {
        [self.start.min(self.end), self.start.max(self.end)]
    }
}

fn endpoint(ui: &mut Ui, hz: f64, [lo, hi]: Band, label: &str) -> f64 {
    let mut mhz = hz * 1e-6;
    let response = ui
        .add(
            DragValue::new(&mut mhz)
                .range(lo * 1e-6..=hi * 1e-6)
                .min_decimals(3)
                .max_decimals(9)
                .speed(0.1)
                .suffix(" MHz")
                .update_while_editing(false),
        )
        .on_hover_text(label);
    if response.changed() {
        (mhz * 1e6).clamp(lo, hi)
    } else {
        hz
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Context, Event, Modifiers, RawInput, Sense, pos2, vec2};

    #[test]
    fn brush_previews_a_reverse_drag_and_commits_after_shift_is_released() {
        const ORIGINAL: Band = [110.0, 120.0];
        const BRUSHED: Band = [125.0, 175.0];

        let mut band = MatchBand {
            selected: Some(ORIGINAL),
            ..Default::default()
        };
        let context = Context::default();
        let plots = [
            Rect::from_min_size(pos2(12.0, 12.0), vec2(200.0, 70.0)),
            Rect::from_min_size(pos2(12.0, 100.0), vec2(200.0, 70.0)),
        ];
        let from = pos2(162.0, 40.0);
        let to = pos2(62.0, 120.0);
        let shift = Modifiers::SHIFT;
        let press = |pos, pressed, modifiers| Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers,
        };

        for (modifiers, events, selected, visible) in [
            (Modifiers::NONE, vec![], ORIGINAL, ORIGINAL),
            (
                shift,
                vec![Event::PointerMoved(from), press(from, true, shift)],
                ORIGINAL,
                ORIGINAL,
            ),
            (shift, vec![Event::PointerMoved(to)], ORIGINAL, BRUSHED),
            (
                Modifiers::NONE,
                vec![press(to, false, Modifiers::NONE)],
                BRUSHED,
                BRUSHED,
            ),
        ] {
            let mut output = context.run_ui(
                RawInput {
                    screen_rect: Some(Rect::from_min_max(pos2(0.0, 0.0), pos2(240.0, 200.0))),
                    events: [Event::ModifiersChanged(modifiers)]
                        .into_iter()
                        .chain(events)
                        .collect(),
                    ..Default::default()
                },
                |ui| {
                    for (index, rect) in plots.into_iter().enumerate() {
                        let hit =
                            ui.interact(rect, ui.scope_id().with(index), Sense::click_and_drag());
                        band.drag(ui, rect, &hit, [100.0, 200.0]);
                    }
                    band.finish_frame(ui);
                },
            );
            output.textures_delta.clear();
            assert_eq!(band.selected, Some(selected));
            assert_eq!(band.visible_selection(), Some(visible));
        }
    }
}
