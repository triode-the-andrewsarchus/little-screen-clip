//! Prepared document paths and their projection onto the interactive map.
use super::Hit;
use crate::ui_appearance::WorkbenchColors;
use egui::{Painter, Pos2, Rect, Shape, Stroke, pos2};
use little_clip_core::geography::{
    Document, Kind, Point, Sample,
    basemap::{GlobeCamera, wrap_lon_delta},
};

const ARC_STEP_M: f64 = 220_000.0;
const MAX_ARC_INSERTIONS: f64 = 65_536.0;

#[derive(Clone, Copy)]
struct Position {
    lat: f64,
    lon: f64,
}

impl From<&Point> for Position {
    fn from(point: &Point) -> Self {
        Self {
            lat: point.lat,
            lon: point.lon,
        }
    }
}

#[derive(Clone, Copy)]
struct Vertex {
    position: Position,
    sample: usize,
}

pub(super) struct Overlay(Vec<Path>);

struct Path {
    feature: usize,
    index: usize,
    connected: bool,
    vertices: Vec<Vertex>,
}

impl Overlay {
    pub(super) fn prepare(document: &Document) -> Self {
        let distance: f64 = document
            .features
            .iter()
            .filter(|feature| feature.kind == Kind::Line)
            .map(|feature| feature.distance_m)
            .sum();
        // Spread the tessellation budget across the document while retaining
        // all source samples and the breaks between paths.
        let step = ARC_STEP_M.max(distance / MAX_ARC_INSERTIONS);
        let mut paths = Vec::new();
        for (feature, source) in document.features.iter().enumerate() {
            for (index, path) in source.paths.iter().enumerate() {
                paths.push(Path {
                    feature,
                    index,
                    connected: path.len() > 1 && matches!(source.kind, Kind::Line | Kind::Polygon),
                    vertices: vertices(path, source.kind == Kind::Line, step),
                });
            }
        }
        Self(paths)
    }

    pub(super) fn paint_copy(
        &self,
        painter: &Painter,
        projection: Projection<'_>,
        selected: Option<usize>,
        colors: WorkbenchColors,
        pointer: Option<Pos2>,
    ) -> Option<(Hit, f32)> {
        let mut hit = HitTest {
            pointer,
            best: None,
        };
        for path in &self.0 {
            let stroke = if selected == Some(path.feature) {
                Stroke::new(3.0_f32, colors.accent)
            } else {
                Stroke::new(1.75_f32, colors.text_primary.gamma_multiply(0.85))
            };
            path.paint(painter, projection, stroke, &mut hit);
        }
        hit.best
    }
}

fn vertices(path: &[Sample], arc: bool, step: f64) -> Vec<Vertex> {
    let mut vertices = Vec::with_capacity(path.len());
    for (index, sample) in path.iter().enumerate() {
        let position = Position::from(&sample.point);
        if arc && let Some(previous) = index.checked_sub(1).map(|index| &path[index]) {
            let steps = ((sample.distance_m - previous.distance_m) / step).ceil() as usize;
            if steps > 1 {
                let start = Position::from(&previous.point);
                for i in 1..steps {
                    let t = i as f64 / steps as f64;
                    vertices.push(Vertex {
                        position: great_circle(start, position, t),
                        sample: if t <= 0.5 { index - 1 } else { index },
                    });
                }
            }
        }
        vertices.push(Vertex {
            position,
            sample: index,
        });
    }
    vertices
}

fn great_circle(start: Position, end: Position, t: f64) -> Position {
    let (sin_start, cos_start) = start.lat.to_radians().sin_cos();
    let (sin_end, cos_end) = end.lat.to_radians().sin_cos();
    let (sin_delta, cos_delta) = wrap_lon_delta(end.lon - start.lon).to_radians().sin_cos();
    let east = cos_end * sin_delta;
    let north = cos_start * sin_end - sin_start * cos_end * cos_delta;
    let tangent = east.hypot(north);
    let dot = sin_start * sin_end + cos_start * cos_end * cos_delta;
    // Antipodal endpoints admit multiple shortest arcs. Choose an eastward
    // tangent when the endpoints don't determine a numerically stable plane.
    let (east, north) = if tangent > 1e-12 {
        (east / tangent, north / tangent)
    } else {
        (1.0, 0.0)
    };
    let (sin_arc, cos_arc) = (t * tangent.atan2(dot)).sin_cos();
    let x = cos_start * cos_arc - sin_start * north * sin_arc;
    let y = east * sin_arc;
    let z = sin_start * cos_arc + cos_start * north * sin_arc;
    Position {
        lat: z.atan2(x.hypot(y)).to_degrees(),
        lon: start.lon + y.atan2(x).to_degrees(),
    }
}

#[derive(Clone, Copy)]
pub(super) struct Projection<'a> {
    pub camera: &'a GlobeCamera,
    pub rect: Rect,
    pub offset: f32,
}

impl Projection<'_> {
    fn point(self, position: Position) -> Option<Pos2> {
        let [x, y] = self.camera.project_relative(
            position.lat,
            position.lon,
            self.rect.width(),
            self.rect.height(),
        )?;
        Some(pos2(
            self.rect.left() + x + self.offset,
            self.rect.top() + y,
        ))
    }

    fn line(self, a: Position, b: Position, mut draw: impl FnMut([Pos2; 2])) {
        // Unwrap the leg as a pair, then split it at the camera's map seam.
        let delta = wrap_lon_delta(b.lon - a.lon);
        let a = Position {
            lon: wrap_lon_delta(a.lon - self.camera.center_lon),
            ..a
        };
        let b = Position {
            lon: a.lon + delta,
            ..b
        };
        if let Some(edge) = self.clipped_line([a, b]) {
            draw(edge);
        }
        let shift = if b.lon < -180.0 {
            360.0
        } else if b.lon > 180.0 {
            -360.0
        } else {
            return;
        };
        let shifted = [a, b].map(|p| Position {
            lon: p.lon + shift,
            ..p
        });
        if let Some(edge) = self.clipped_line(shifted) {
            draw(edge);
        }
    }

    fn clipped_line(self, [a, b]: [Position; 2]) -> Option<[Pos2; 2]> {
        let [a, b] = [a.clip_longitude(b), b.clip_longitude(a)];
        match (self.point(a), self.point(b)) {
            (Some(a), Some(b)) => Some([a, b]),
            (Some(visible), None) => Some([visible, self.limb(a, b, visible)]),
            (None, Some(visible)) => Some([self.limb(b, a, visible), visible]),
            (None, None) => None,
        }
    }

    fn limb(self, mut front: Position, mut back: Position, mut projected: Pos2) -> Pos2 {
        for _ in 0..10 {
            let middle = Position {
                lat: (front.lat + back.lat) * 0.5,
                lon: (front.lon + back.lon) * 0.5,
            };
            if let Some(position) = self.point(middle) {
                front = middle;
                projected = position;
            } else {
                back = middle;
            }
        }
        projected
    }
}

impl Position {
    fn clip_longitude(self, other: Self) -> Self {
        let lon = self.lon.clamp(-180.0, 180.0);
        if lon == self.lon {
            return self;
        }
        let t = (lon - self.lon) / (other.lon - self.lon);
        Self {
            lat: self.lat + (other.lat - self.lat) * t,
            lon,
        }
    }
}

impl Path {
    fn paint(
        &self,
        painter: &Painter,
        projection: Projection<'_>,
        stroke: Stroke,
        hit: &mut HitTest,
    ) {
        let sample_hit = |vertex: Vertex| Hit {
            feature: self.feature,
            path: self.index,
            point: vertex.sample,
        };
        if !self.connected {
            for &vertex in &self.vertices {
                let mut position = vertex.position;
                position.lon = wrap_lon_delta(position.lon - projection.camera.center_lon);
                if let Some(point) = projection.point(position)
                    && projection.rect.contains(point)
                {
                    painter.circle_filled(point, 4.5, stroke.color);
                    hit.point(sample_hit(vertex), point);
                }
            }
            return;
        }
        let mut run = Vec::new();
        let clip = projection.rect.expand(stroke.width);
        for pair in self.vertices.windows(2) {
            let [a, b] = [pair[0], pair[1]];
            projection.line(a.position, b.position, |[start, end]| {
                if !Rect::from_two_pos(start, end).intersects(clip) {
                    return;
                }
                if run
                    .last()
                    .is_none_or(|last: &Pos2| last.distance_sq(start) > 1e-6)
                {
                    if !run.is_empty() {
                        painter.add(Shape::line(std::mem::take(&mut run), stroke));
                    }
                    run.push(start);
                }
                run.push(end);
                hit.line([sample_hit(a), sample_hit(b)], [start, end]);
            });
        }
        if !run.is_empty() {
            painter.add(Shape::line(run, stroke));
        }
    }
}

struct HitTest {
    pointer: Option<Pos2>,
    best: Option<(Hit, f32)>,
}

impl HitTest {
    fn point(&mut self, sample: Hit, point: Pos2) {
        if let Some(pointer) = self.pointer {
            self.consider(sample, point.distance_sq(pointer));
        }
    }

    fn line(&mut self, samples: [Hit; 2], [a, b]: [Pos2; 2]) {
        if let Some(pointer) = self.pointer {
            let (t, distance) = crate::ui_ext::segment_hit(pointer, a, b);
            self.consider(samples[usize::from(t > 0.5)], distance);
        }
    }

    fn consider(&mut self, sample: Hit, distance: f32) {
        if distance < self.best.map_or(64.0, |(_, nearest)| nearest) {
            self.best = Some((sample, distance));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::vec2;
    use little_clip_core::geography::basemap::ProjectionMode;

    fn world_tour() -> Overlay {
        let document = Document::geojson(
            br#"{"type":"LineString","coordinates":[[-160,60],[-90,40],[-30,0],[30,0],[90,40],[160,60]]}"#,
        )
        .unwrap();
        Overlay::prepare(&document)
    }

    fn project_path(projection: Projection<'_>, vertices: &[Vertex]) -> Vec<[Pos2; 2]> {
        let mut lines = Vec::new();
        for pair in vertices.windows(2) {
            projection.line(pair[0].position, pair[1].position, |edge| lines.push(edge));
        }
        lines
    }

    #[test]
    fn world_tour_leg_curves_on_globe_and_retains_waypoints() {
        let overlay = world_tour();
        let vertices = &overlay.0[0].vertices;
        let end = vertices
            .iter()
            .position(|v| v.position.lon == -90.0)
            .unwrap();
        let leg = &vertices[..=end];
        assert_eq!(leg[0].sample, 0);
        assert_eq!(leg[end].sample, 1);
        assert!(leg.len() > 2 && leg.iter().all(|v| v.sample <= 1));

        let camera = GlobeCamera {
            center_lon: -125.0,
            mode: ProjectionMode::Globe3D,
            ..GlobeCamera::default()
        };
        let rect = Rect::from_min_size(Pos2::ZERO, vec2(900.0, 600.0));
        let lines = project_path(
            Projection {
                camera: &camera,
                rect,
                offset: 0.0,
            },
            leg,
        );
        assert_eq!(lines.len(), end);
        let [start, finish] = [lines[0][0], lines[end - 1][1]];
        let mid = lines[end / 2][0];
        let (_, deviation_sq) = crate::ui_ext::segment_hit(mid, start, finish);
        assert!(
            deviation_sq > 25.0,
            "midpoint should visibly leave the chord: {deviation_sq}"
        );
    }

    #[test]
    fn flat_world_tour_keeps_its_path_through_camera_seam_crossings() {
        let overlay = world_tour();
        let rect = Rect::from_min_size(Pos2::ZERO, vec2(1600.0, 600.0));
        for center_lon in [-179.0, -125.0, -55.0, 0.0, 55.0, 125.0, 179.0] {
            let camera = GlobeCamera {
                center_lat: 0.0,
                center_lon,
                mode: ProjectionMode::Map2D,
                ..GlobeCamera::default()
            };
            let projection = Projection {
                camera: &camera,
                rect,
                offset: 0.0,
            };
            let lines = project_path(projection, &overlay.0[0].vertices);
            let period = camera.world_width_px(rect.width(), rect.height()) as f32;
            let signed_distance: f32 = lines.iter().map(|[a, b]| b.x - a.x).sum();
            assert!((signed_distance / period - 320.0 / 360.0).abs() < 1e-5);
            assert!(lines.iter().all(|[a, b]| (b.x - a.x).abs() < period / 40.0));
            for pair in lines.windows(2) {
                let gap = pair[1][0] - pair[0][1];
                assert!(gap.y.abs() < 1e-3);
                assert!(gap.x.abs() < 1e-3 || (gap.x.abs() - period).abs() < 1e-3);
            }
        }
    }

    #[test]
    fn globe_keeps_visible_part_of_a_segment_up_to_the_limb() {
        let camera = GlobeCamera {
            center_lat: 0.0,
            mode: ProjectionMode::Globe3D,
            ..GlobeCamera::default()
        };
        let rect = Rect::from_min_size(Pos2::ZERO, vec2(900.0, 600.0));
        let projection = Projection {
            camera: &camera,
            rect,
            offset: 0.0,
        };
        let mut lines = Vec::new();
        projection.line(
            Position {
                lat: 0.0,
                lon: 89.0,
            },
            Position {
                lat: 0.0,
                lon: 91.0,
            },
            |edge| lines.push(edge),
        );
        projection.line(
            Position {
                lat: 0.0,
                lon: 91.0,
            },
            Position {
                lat: 0.0,
                lon: 93.0,
            },
            |edge| lines.push(edge),
        );
        assert_eq!(lines.len(), 1);
        let limb = rect.center().x + camera.scale_px(rect.width(), rect.height()) as f32;
        assert!((lines[0][1].x - limb).abs() < 1e-3);
    }
}
