//! Terminal gestures and explicit, source-addressed link removal controls.
use super::{canvas::endpoint_name, *};

impl WireEditor {
    pub(super) fn terminal_input(
        &mut self,
        ui: &egui::Ui,
        response: &egui::Response,
        hover: Option<Endpoint>,
    ) -> Option<egui::Pos2> {
        let (pressed, released, cancel, pointer) = ui.input(|input| {
            (
                input.pointer.button_pressed(egui::PointerButton::Primary),
                input.pointer.button_released(egui::PointerButton::Primary),
                input.key_pressed(egui::Key::Escape)
                    || input.pointer.button_pressed(egui::PointerButton::Middle),
                input.pointer.interact_pos(),
            )
        });
        if cancel {
            self.drag = None;
        }
        if pressed
            && !cancel
            && let Some(from) = hover
            && let (Some(source), Some(model)) = (self.source.clone(), self.model.clone())
        {
            self.select(Some(from));
            self.drag = Some(TerminalDrag {
                from,
                source,
                model,
            });
            response.request_focus();
        }
        if let Some(drag) = &self.drag {
            let current = self.source.as_deref() == Some(drag.source.as_ref())
                && self
                    .model
                    .as_ref()
                    .is_some_and(|model| Arc::ptr_eq(model, &drag.model));
            if !current {
                self.drag = None;
            }
        }
        if released
            && let Some(drag) = self.drag.take()
            && let Some(to) = hover
            && drag.from != to
            && compatible(drag.from, to)
        {
            self.queue(Edit::Connect {
                from: drag.from,
                to,
            });
        }
        pointer
    }

    pub(super) fn paint_drag(
        &self,
        painter: &egui::Painter,
        rect: Rect,
        hits: &[(Endpoint, Rect)],
        pointer: Option<egui::Pos2>,
        accent: Color32,
    ) {
        if let Some(drag) = &self.drag
            && let Some(pointer) = pointer
            && let Some((_, bounds)) = hits.iter().find(|(target, _)| *target == drag.from)
        {
            let from = self.camera.screen_rect(*bounds, rect).center();
            painter.line_segment([from, pointer], Stroke::new(2.0_f32, accent));
        }
    }

    pub(super) fn direct_connections(
        ui: &mut egui::Ui,
        palette: &AppPalette,
        harness: &Harness,
        target: Endpoint,
    ) -> Option<Edit> {
        let mut edit = None;
        ui.strong("Direct connections");
        let mut count = 0;
        for (index, link) in harness.links.iter().enumerate() {
            let [from, to] = link.endpoints;
            let other = if from == target {
                to
            } else if to == target {
                from
            } else {
                continue;
            };
            count += 1;
            if count > 64 {
                continue;
            }
            ui.push_id(("connection", index), |ui| {
                ui.label(endpoint_name(harness, other));
                if matches!(link.kind, LinkKind::Cable | LinkKind::Loop) {
                    if ui
                        .add(ButtonRole::Secondary.button("Disconnect", palette))
                        .clicked()
                    {
                        edit = Some(Edit::Disconnect { link: index });
                    }
                } else {
                    ui.small("Mating annotation · edit in source");
                }
            });
        }
        if count == 0 {
            ui.small("No direct electrical connections.");
        }
        if count > 64 {
            ui.small(format!("{} more connections; edit in source", count - 64));
        }
        ui.separator();
        edit
    }
}

fn compatible(from: Endpoint, to: Endpoint) -> bool {
    matches!(
        (from, to),
        (Endpoint::Pin { .. }, Endpoint::Cable { .. })
            | (Endpoint::Cable { .. }, Endpoint::Pin { .. })
    )
}
