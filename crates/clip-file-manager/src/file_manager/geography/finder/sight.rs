use super::{CONSTELLATIONS, Direction, Hit, NAMED_STARS, Target};
use crate::file_manager::geography::{gpu, map_camera, sky, solar::world::Backdrop};
use egui::{Align2, Color32, FontId, Pos2, Rect, Stroke, Vec2};
use std::sync::Arc;

pub(in crate::file_manager::geography) struct Sight {
    pub title: Arc<str>,
    target: Direction,
    constellation: Option<usize>,
    longitude: f32,
    latitude: f32,
    aperture: f32,
}

impl Sight {
    pub fn from_hit(hit: &Hit) -> Option<Self> {
        let (direction, constellation) = match hit.target {
            Target::Star(direction) => (direction, None),
            Target::Constellation(index) => (CONSTELLATIONS.get(index)?.2, Some(index)),
            _ => return None,
        };
        let [x, y, z] = direction;
        Some(Self {
            title: hit.label.clone(),
            target: direction,
            constellation,
            longitude: y.atan2(x),
            latitude: z.clamp(-1.0, 1.0).asin(),
            aperture: if constellation.is_some() { 0.70 } else { 0.24 },
        })
    }

    pub fn navigate(&mut self, response: &egui::Response) -> bool {
        let rect = response.rect;
        if !rect.is_positive() {
            return false;
        }
        if response.dragged_by(egui::PointerButton::Primary) {
            let delta = response.drag_delta() * (2.0 * self.aperture / rect.size().min_elem());
            self.longitude -= delta.x / self.latitude.cos().abs().max(0.18);
            self.latitude = (self.latitude + delta.y).clamp(-1.5707, 1.5707);
        }
        let factor = map_camera::scroll_zoom(response);
        let exit = map_camera::past_zoom_limit(
            f64::from(1.0 / self.aperture),
            f64::from(1.0_f32 / 1.45),
            factor,
        );
        self.aperture = (self.aperture / factor as f32).clamp(0.035, 1.45);
        exit
    }

    pub fn paint(
        &self,
        ui: &egui::Ui,
        response: &egui::Response,
        assets: Option<Arc<sky::Assets>>,
    ) {
        let rect = response.rect;
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0, Color32::from_rgb(7, 10, 17));
        let (sin_lat, cos_lat) = self.latitude.sin_cos();
        let (sin_lon, cos_lon) = self.longitude.sin_cos();
        let projection = Projection {
            right: [-sin_lon, cos_lon, 0.0],
            up: [-cos_lon * sin_lat, -sin_lon * sin_lat, cos_lat],
            direction: [cos_lon * cos_lat, sin_lon * cos_lat, sin_lat],
            focal: rect.size().min_elem() * 0.5 / self.aperture.tan(),
            rect,
        };
        if assets.is_some() {
            let to_ecliptic = |[x, y, z]: Direction| {
                [x, 0.917482 * y + 0.397777 * z, -0.397777 * y + 0.917482 * z]
            };
            let _ = gpu::paint_space(
                ui,
                rect,
                Backdrop {
                    axes: [
                        projection.right,
                        projection.up,
                        projection.direction.map(|v| -v),
                    ]
                    .map(to_ecliptic),
                    fov: [rect.width(), rect.height()].map(|side| side / (2.0 * projection.focal)),
                    sun: ([0.0, 0.0, 1.0], 0.0),
                    clock: [0.0; 2],
                    brightness: 0.64,
                    assets: assets.clone(),
                },
            );
        }
        self.paint_figures(&painter, projection);
        let mut named_hover = None;
        let pointer = response.hover_pos();
        let mut paint_star = |name: &str, magnitude: f32, direction: Direction| {
            if let Some(pos) = projection
                .project(direction)
                .filter(|&pos| rect.contains(pos))
            {
                let brightness = (3.0_f32 - magnitude).clamp(-3.0, 4.0);
                let light = (160.0 + brightness * 23.0) as u8;
                painter.circle_filled(
                    pos,
                    (1.4 + brightness * 0.22_f32).clamp(0.6, 2.8),
                    Color32::from_rgb(light, light, light.saturating_add(14)),
                );
                if pointer.is_some_and(|pointer| pointer.distance_sq(pos) < 25.0) {
                    let distance = pointer.map_or(f32::INFINITY, |p| p.distance_sq(pos));
                    if named_hover
                        .as_ref()
                        .is_none_or(|(best, _)| distance < *best)
                    {
                        named_hover = Some((distance, name.to_owned()));
                    }
                }
            }
        };
        if let Some(catalog) = assets.as_deref().and_then(sky::Assets::star_catalog) {
            for star in catalog.stars() {
                paint_star(star.name.as_ref(), star.magnitude, star.direction);
            }
        } else {
            for (name, _, magnitude, direction) in NAMED_STARS.iter() {
                paint_star(name.as_str(), *magnitude, *direction);
            }
        }
        let target_color = Color32::from_rgb(120, 196, 253);
        if let Some(pos) = projection
            .project(self.target)
            .filter(|&pos| rect.contains(pos))
        {
            if self.constellation.is_none() {
                painter.circle_stroke(pos, 8.0, Stroke::new(1.2, target_color));
            }
            painter.text(
                pos + Vec2::new(9.0, -8.0),
                Align2::LEFT_BOTTOM,
                self.title.as_ref(),
                FontId::proportional(14.0),
                target_color,
            );
        }
        if let Some((_, name)) = named_hover {
            egui::Tooltip::for_widget(response).at_pointer().show(|ui| {
                ui.label(name);
            });
        }
    }

    fn paint_figures(&self, painter: &egui::Painter, projection: Projection) {
        for (index, (_, _, _, lines)) in CONSTELLATIONS.iter().enumerate() {
            let selected = self.constellation == Some(index);
            let color = if selected {
                Color32::from_rgb(124, 174, 210)
            } else {
                Color32::from_rgb(40, 55, 73)
            };
            for [a, b] in lines {
                if let Some([a, b]) = projection
                    .project(*a)
                    .zip(projection.project(*b))
                    .map(|(a, b)| [a, b])
                {
                    if projection.rect.intersects(Rect::from_two_pos(a, b)) {
                        painter.line_segment([a, b], Stroke::new(1.0, color));
                    }
                }
            }
        }
    }
}

#[derive(Clone, Copy)]
struct Projection {
    right: Direction,
    up: Direction,
    direction: Direction,
    focal: f32,
    rect: Rect,
}

impl Projection {
    fn project(self, point: Direction) -> Option<Pos2> {
        let dot = |axis: Direction| axis.iter().zip(point).map(|(a, b)| a * b).sum::<f32>();
        let depth = dot(self.direction);
        (depth > 0.02).then(|| {
            self.rect.center() + Vec2::new(dot(self.right), -dot(self.up)) * (self.focal / depth)
        })
    }
}
