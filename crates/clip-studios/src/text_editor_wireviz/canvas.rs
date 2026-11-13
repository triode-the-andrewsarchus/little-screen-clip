use super::*;
use crate::diagram_preview::NativeFrame;
use clip_ui::vectors::svg::{Changes, Geometry};

impl WireEditor {
    pub(super) fn canvas(
        &mut self,
        ui: &mut egui::Ui,
        rect: Rect,
        response: &egui::Response,
        frame: Option<&NativeFrame>,
        ready: bool,
        palette: &AppPalette,
    ) {
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 4.0, Color32::WHITE);
        let Some(frame) = frame else {
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "Harness preview",
                egui::FontId::proportional(16.0),
                Color32::GRAY,
            );
            return;
        };
        if !self.camera.fitted {
            self.camera.fit(frame.layout, rect);
        }
        self.camera.input(ui, response, rect);
        if response.double_clicked_by(egui::PointerButton::Middle) {
            self.camera.fit(frame.layout, rect);
        }
        let image = self.camera.image_rect(frame.layout, rect);
        self.canvas_notice = frame.paint(ui, rect, image, Changes::default(), Color32::WHITE);
        if !frame.fresh {
            painter.rect_filled(rect, 4.0, Color32::from_white_alpha(135));
            painter.text(
                rect.center_top() + egui::vec2(0.0, 12.0),
                egui::Align2::CENTER_TOP,
                "Updating harness…",
                egui::FontId::proportional(14.0),
                Color32::DARK_GRAY,
            );
        }
        let Some(geometry) = &frame.geometry else {
            return;
        };
        let hits = geometry
            .groups
            .iter()
            .filter_map(|(id, bounds)| {
                endpoint(id).map(|target| (target, terminal_rect(geometry, target, *bounds)))
            })
            .collect::<Vec<_>>();
        if !ready {
            return;
        }
        let hover = response.hover_pos().and_then(|position| {
            let point = self.camera.world(position, rect);
            let point = egui::pos2(point[0] as f32, point[1] as f32);
            hits.iter()
                .filter(|(_, bounds)| bounds.contains(point))
                .min_by(|(_, a), (_, b)| {
                    a.center()
                        .distance_sq(point)
                        .total_cmp(&b.center().distance_sq(point))
                })
                .map(|(target, _)| *target)
        });
        if response.clicked() {
            response.request_focus();
            self.select(hover);
        }
        if let Some(target) = hover
            && let Some(model) = &self.model
        {
            response
                .clone()
                .on_hover_text(endpoint_name(model.editable.harness(), target));
        }
        let accent = crate::hex_to_c32(&palette.harmonies.base);
        for (target, bounds) in hits {
            if self.connected.contains(&target) || hover == Some(target) {
                let a = self
                    .camera
                    .screen([f64::from(bounds.left()), f64::from(bounds.top())], rect);
                let b = self.camera.screen(
                    [f64::from(bounds.right()), f64::from(bounds.bottom())],
                    rect,
                );
                let bounds = Rect::from_two_pos(a, b);
                let selected = self.selected == Some(target);
                painter.rect_filled(
                    bounds,
                    2.0,
                    accent.gamma_multiply(if selected { 0.2 } else { 0.08 }),
                );
                painter.rect_stroke(
                    bounds,
                    2.0,
                    Stroke::new(if selected { 2.0_f32 } else { 1.0_f32 }, accent),
                );
            }
        }
    }

    pub(super) fn inspector(&mut self, ui: &mut egui::Ui, palette: &AppPalette) -> Option<Edit> {
        let Some(model) = self.model.clone() else {
            ui.label("Preparing terminal map…");
            return None;
        };
        let harness = model.editable.harness();
        let Some(target) = self.selected else {
            ui.heading("Explore the harness");
            ui.label("Select a pin or conductor to highlight its electrical path and inspect its mapping.");
            ui.small(format!(
                "{} connectors · {} cables",
                harness.connectors.iter().filter(|c| c.used).count(),
                harness.cables.iter().filter(|c| c.used).count()
            ));
            return None;
        };
        ui.strong(endpoint_name(harness, target));
        ui.small(format!("{} linked terminals", self.connected.len()));
        ui.separator();
        let edit = match target {
            Endpoint::Pin { connector, pin } => {
                ui.label("Pin label");
                let input = ui.add(
                    egui::TextEdit::singleline(&mut self.label)
                        .id_source("harness_pin_label")
                        .desired_width(ui.available_width()),
                );
                let apply = input.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                (ui.add(ButtonRole::Secondary.button("Apply label", palette))
                    .clicked()
                    || apply)
                    .then(|| Edit::PinLabel {
                        connector,
                        pin,
                        label: self.label.clone(),
                    })
            }
            Endpoint::Cable {
                cable,
                conductor: Conductor::Wire(wire),
            } => self.color_editor(ui, palette, cable, wire),
            Endpoint::Cable {
                conductor: Conductor::Shield,
                ..
            } => {
                ui.label("Cable shield");
                ui.small("The highlighted path shows its actual terminations.");
                None
            }
            Endpoint::Connector(_) => None,
        };
        ui.separator();
        ui.strong("Electrical path");
        let mut connected: Vec<_> = self.connected.iter().copied().collect();
        connected.sort_by_key(|endpoint| match *endpoint {
            Endpoint::Pin { connector, pin } => [0, connector, pin],
            Endpoint::Cable {
                cable,
                conductor: Conductor::Wire(wire),
            } => [1, cable, wire],
            Endpoint::Cable {
                cable,
                conductor: Conductor::Shield,
            } => [1, cable, usize::MAX],
            Endpoint::Connector(connector) => [2, connector, 0],
        });
        for endpoint in connected.iter().take(12) {
            ui.label(endpoint_name(harness, *endpoint));
        }
        if connected.len() > 12 {
            ui.small(format!("…and {} more", connected.len() - 12));
        }
        edit
    }

    fn color_editor(
        &mut self,
        ui: &mut egui::Ui,
        palette: &AppPalette,
        cable: usize,
        wire: usize,
    ) -> Option<Edit> {
        ui.label("Wire color");
        let input = ui.add(
            egui::TextEdit::singleline(&mut self.color)
                .id_source("harness_wire_color")
                .desired_width(ui.available_width()),
        );
        let enter = input.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        ui.small("Color code, bands (GNYE), or #RRGGBB");
        let mut picked = None;
        ui.horizontal_wrapped(|ui| {
            for code in ["BK", "WH", "RD", "BU", "GN", "YE", "GNYE"] {
                let [r, g, b] = wireviz::colors::bands(code).expect("built-in color")[0];
                let fill = Color32::from_rgb(r, g, b);
                if ui
                    .add(clip_ui::ui_ext::themed_button(code, fill, None, palette))
                    .clicked()
                {
                    picked = Some(code.to_owned());
                }
            }
        });
        let apply = ui
            .add(ButtonRole::Secondary.button("Apply color", palette))
            .clicked()
            || enter;
        if let Some(code) = &picked {
            self.color.clone_from(code);
        }
        if ui
            .add(ButtonRole::Secondary.button("Clear color", palette))
            .clicked()
        {
            self.color.clear();
            return Some(Edit::WireColor {
                cable,
                wire,
                color: None,
            });
        }
        (apply || picked.is_some()).then(|| Edit::WireColor {
            cable,
            wire,
            color: (!self.color.is_empty()).then(|| self.color.clone()),
        })
    }
}

fn endpoint(id: &str) -> Option<Endpoint> {
    let id = id.strip_prefix("graph-0-a_wireviz-")?;
    let (kind, indices) = id.split_once('-')?;
    if kind == "shield" {
        return Some(Endpoint::Cable {
            cable: indices.parse().ok()?,
            conductor: Conductor::Shield,
        });
    }
    let (owner, index) = indices.split_once('-')?;
    let (owner, index) = (owner.parse().ok()?, index.parse().ok()?);
    match kind {
        "pin" => Some(Endpoint::Pin {
            connector: owner,
            pin: index,
        }),
        "wire" => Some(Endpoint::Cable {
            cable: owner,
            conductor: Conductor::Wire(index),
        }),
        _ => None,
    }
}

fn terminal_rect(geometry: &Geometry, endpoint: Endpoint, content: Rect) -> Rect {
    let name = match endpoint {
        Endpoint::Pin { connector, .. } | Endpoint::Connector(connector) => {
            format!("graph-0-wireviz-connector-{connector}")
        }
        Endpoint::Cable { cable, .. } => format!("graph-0-wireviz-cable-{cable}"),
    };
    let width = geometry.groups.get(&name).copied().unwrap_or(content);
    Rect::from_min_max(
        egui::pos2(width.left(), content.top() - 5.0),
        egui::pos2(width.right(), content.bottom() + 5.0),
    )
}

fn endpoint_name(harness: &Harness, endpoint: Endpoint) -> String {
    match endpoint {
        Endpoint::Pin { connector, pin } => harness
            .connectors
            .get(connector)
            .and_then(|c| c.pins.get(pin).map(|p| (c, p)))
            .map_or_else(
                || "Unknown pin".into(),
                |(c, p)| {
                    let id = match &p.id {
                        PinId::Number(n) => n.to_string(),
                        PinId::Text(s) => format!("\"{s}\""),
                    };
                    format!("{} · pin {id}", c.name)
                },
            ),
        Endpoint::Cable { cable, conductor } => harness.cables.get(cable).map_or_else(
            || "Unknown conductor".into(),
            |c| match conductor {
                Conductor::Wire(index) => format!("{} · wire {}", c.name, index + 1),
                Conductor::Shield => format!("{} · shield", c.name),
            },
        ),
        Endpoint::Connector(index) => harness
            .connectors
            .get(index)
            .map_or_else(|| "Unknown connector".into(), |c| c.name.clone()),
    }
}
