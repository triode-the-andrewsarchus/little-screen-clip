//! Prepared document paths and their projection onto the interactive map.
use super::{
    Hit, interpolate_instant,
    radiation::{DoseScale, dose_rate},
};
use crate::ui_appearance::WorkbenchColors;
use chrono::{DateTime, Utc};
use egui::{Painter, Pos2, Rect, Shape, Stroke, pos2};
use little_clip_core::geography::{
    Document, Kind, Point, Sample,
    basemap::{GlobeCamera, wrap_lon_delta},
};

const ARC_STEP_M: f64 = 220_000.0;
const MAX_ARC_INSERTIONS: f64 = 65_536.0;
const OUTLINE: f32 = 0.85;

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
    source_point: bool,
    time: Option<DateTime<Utc>>,
    dose_rate: Option<f64>,
}

pub(super) struct Overlay(Vec<Path>);

struct Path {
    feature: usize,
    index: usize,
    connected: bool,
    interpolate_time: bool,
    vertices: Vec<Vertex>,
    dose_scale: Option<DoseScale>,
}

impl Overlay {
    pub(super) fn prepare(document: &Document, interpolate_time: bool) -> Self {
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
                    interpolate_time,
                    vertices: vertices(path, source.kind == Kind::Line, step, interpolate_time),
                    dose_scale: source.dose_rate.map(DoseScale),
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
    ) -> Option<Pick> {
        let mut hit = HitTest {
            pointer,
            best: None,
        };
        for path in &self.0 {
            let mut stroke = if selected == Some(path.feature) {
                Stroke::new(3.0_f32, colors.accent)
            } else {
                Stroke::new(1.75_f32, colors.text_primary.gamma_multiply(0.85))
            };
            if path.dose_scale.is_some() {
                stroke.width += 1.5;
            }
            path.paint(painter, projection, stroke, &mut hit);
        }
        hit.best
    }
}

fn vertices(path: &[Sample], arc: bool, step: f64, interpolate_time: bool) -> Vec<Vertex> {
    let mut vertices = Vec::with_capacity(path.len());
    let mut previous_time: Option<DateTime<Utc>> = None;
    for (index, sample) in path.iter().enumerate() {
        let position = Position::from(&sample.point);
        let rate = dose_rate(&sample.point);
        let time = if interpolate_time {
            sample.point.instant()
        } else {
            None
        };
        if arc && let Some(previous) = index.checked_sub(1).map(|index| &path[index]) {
            let steps = ((sample.distance_m - previous.distance_m) / step).ceil() as usize;
            if steps > 1 {
                let start = Position::from(&previous.point);
                for i in 1..steps {
                    let t = i as f64 / steps as f64;
                    let rate = dose_rate(&previous.point)
                        .zip(rate)
                        .map(|(a, b)| a + (b - a) * t);
                    vertices.push(Vertex {
                        position: great_circle(start, position, t),
                        sample: if t <= 0.5 { index - 1 } else { index },
                        source_point: false,
                        time: previous_time
                            .zip(time)
                            .map(|(from, to)| interpolate_instant(from, to, t)),
                        dose_rate: rate,
                    });
                }
            }
        }
        vertices.push(Vertex {
            position,
            sample: index,
            source_point: true,
            time,
            dose_rate: rate,
        });
        previous_time = time;
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
    pub draped: Option<super::map_camera::Draped<'a>>,
}

impl Projection<'_> {
    fn point(self, position: Position) -> Option<Pos2> {
        let [x, y] = match self.draped {
            Some(draped) => draped.project(position.lat, position.lon + self.camera.center_lon),
            None => self.camera.project_relative(
                position.lat,
                position.lon,
                self.rect.width(),
                self.rect.height(),
            ),
        }?;
        Some(pos2(
            self.rect.left() + x + self.offset,
            self.rect.top() + y,
        ))
    }

    fn line(self, a: Position, b: Position, mut draw: impl FnMut([Pos2; 2], [f64; 2])) {
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
        if let Some(draped) = self.draped {
            self.draped_line(a, b, draped.mesh_step(), draw);
            return;
        }
        if let Some((edge, fractions)) = self.clipped_line([a, b]) {
            draw(edge, fractions);
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
        if let Some((edge, fractions)) = self.clipped_line(shifted) {
            draw(edge, fractions);
        }
    }

    fn draped_line(
        self,
        a: Position,
        b: Position,
        mesh_step: f64,
        mut draw: impl FnMut([Pos2; 2], [f64; 2]),
    ) {
        let mut prior = (a, self.point(a), 0.0);
        for t in terrain_edges(a, b, mesh_step) {
            let next = a.lerp(b, t);
            let endpoint = self.point(next);
            let (start, screen, from) = prior;
            if let Some((points, fractions)) =
                self.visible_segment([start, next], [screen, endpoint])
            {
                draw(
                    points,
                    fractions.map(|fraction| from + (t - from) * fraction),
                );
            }
            prior = (next, endpoint, t);
        }
    }

    fn clipped_line(self, [a, b]: [Position; 2]) -> Option<([Pos2; 2], [f64; 2])> {
        let [(a, start), (b, tail)] = [a.clip_longitude(b), b.clip_longitude(a)];
        let end = 1.0 - tail;
        self.visible_segment([a, b], [self.point(a), self.point(b)])
            .map(|(points, fractions)| (points, fractions.map(|t| start + t * (end - start))))
    }

    fn visible_segment(
        self,
        [a, b]: [Position; 2],
        [pa, pb]: [Option<Pos2>; 2],
    ) -> Option<([Pos2; 2], [f64; 2])> {
        match (pa, pb) {
            (Some(a), Some(b)) => Some(([a, b], [0.0, 1.0])),
            (Some(visible), None) => {
                let (limb, t) = self.limb(a, b, visible);
                Some(([visible, limb], [0.0, t]))
            }
            (None, Some(visible)) => {
                let (limb, t) = self.limb(b, a, visible);
                Some(([limb, visible], [1.0 - t, 1.0]))
            }
            (None, None) => None,
        }
    }

    fn limb(self, front: Position, back: Position, mut projected: Pos2) -> (Pos2, f64) {
        let (mut low, mut high) = (0.0_f64, 1.0_f64);
        for _ in 0..10 {
            let t = low.midpoint(high);
            let middle = front.lerp(back, t);
            if let Some(position) = self.point(middle) {
                low = t;
                projected = position;
            } else {
                high = t;
            }
        }
        (projected, low)
    }
}

fn terrain_edges(a: Position, b: Position, mesh_step: f64) -> impl Iterator<Item = f64> {
    // The vertical, horizontal and diagonal edges of a terrain triangle all
    // lie on this dyadic lattice. Half-cells add samples for ridge visibility.
    let start = [a.lon, -a.lat, a.lon - a.lat];
    let end = [b.lon, -b.lat, b.lon - b.lat];
    let extent: f64 = (0..3).map(|i| (end[i] - start[i]).abs()).sum();
    let scale = (extent / (mesh_step * 512.0)).log2().ceil().max(0.0).exp2();
    let step = mesh_step * 0.5 * scale;
    let directions: [f64; 3] = std::array::from_fn(|i| (end[i] - start[i]) / step);
    let spacing = directions.map(|d| 1.0 / d.abs().max(1e-12));
    let mut crossing: [f64; 3] = std::array::from_fn(|i| {
        let origin = start[i] / step;
        let distance = if directions[i] < 0.0 {
            origin - (origin - 1e-8).ceil() + 1.0
        } else {
            (origin + 1e-8).floor() + 1.0 - origin
        };
        distance * spacing[i]
    });
    std::iter::from_fn(move || {
        let next = crossing.iter().copied().fold(1.0, f64::min);
        for (t, step) in crossing.iter_mut().zip(spacing) {
            if *t <= next + 1e-10 {
                *t += step;
            }
        }
        (next < 1.0 - 1e-10).then_some(next)
    })
    .chain([1.0])
}

impl Position {
    fn lerp(self, other: Self, t: f64) -> Self {
        Self {
            lat: self.lat + (other.lat - self.lat) * t,
            lon: self.lon + (other.lon - self.lon) * t,
        }
    }

    fn clip_longitude(self, other: Self) -> (Self, f64) {
        let lon = self.lon.clamp(-180.0, 180.0);
        if lon == self.lon {
            return (self, 0.0);
        }
        let t = (lon - self.lon) / (other.lon - self.lon);
        (
            Self {
                lat: self.lat + (other.lat - self.lat) * t,
                lon,
            },
            t,
        )
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
        let mut path = PathPainter::new(painter, self.dose_scale, stroke);
        if !self.connected {
            for &vertex in &self.vertices {
                let mut position = vertex.position;
                position.lon = wrap_lon_delta(position.lon - projection.camera.center_lon);
                if let Some(point) = projection.point(position)
                    && projection.rect.contains(point)
                {
                    path.point(point, 4.5, vertex.dose_rate);
                    hit.point(sample_hit(vertex), vertex.time, point, false);
                }
            }
            return;
        }
        let clip = projection.rect.expand(stroke.width + OUTLINE);
        for pair in self.vertices.windows(2) {
            let [a, b] = [pair[0], pair[1]];
            projection.line(a.position, b.position, |edge, fractions| {
                if Rect::from_two_pos(edge[0], edge[1]).intersects(clip) {
                    let mut doses = [a.dose_rate, b.dose_rate];
                    if let (Some(from), Some(to), Some(_)) = (doses[0], doses[1], projection.draped)
                    {
                        doses = fractions.map(|t| Some(from + (to - from) * t));
                    }
                    path.line(edge, doses);
                    let samples = [sample_hit(a), sample_hit(b)];
                    if self.interpolate_time {
                        for (i, vertex) in [a, b].into_iter().enumerate() {
                            if vertex.source_point && fractions[i] == i as f64 {
                                hit.point(samples[i], vertex.time, edge[i], true);
                            }
                        }
                    }
                    hit.line(samples, [a.time, b.time], edge, fractions);
                }
            });
        }
        path.finish();
    }
}

/// Draws a continuous black casing beneath the colored strokes of each path.
struct PathPainter<'a> {
    painter: &'a Painter,
    dose_scale: Option<DoseScale>,
    fallback: Stroke,
    run_stroke: Stroke,
    run: Vec<Pos2>,
    outline: Vec<Pos2>,
    foreground: Vec<Shape>,
}

impl<'a> PathPainter<'a> {
    fn new(painter: &'a Painter, dose_scale: Option<DoseScale>, stroke: Stroke) -> Self {
        Self {
            painter,
            dose_scale,
            fallback: stroke,
            run_stroke: stroke,
            run: Vec::new(),
            outline: Vec::new(),
            foreground: Vec::new(),
        }
    }

    fn line(&mut self, [start, end]: [Pos2; 2], [a, b]: [Option<f64>; 2]) {
        let stroke = self
            .dose_scale
            .map_or(self.fallback, |scale| scale.segment(a, b, self.fallback));
        let discontinuous = self
            .outline
            .last()
            .is_none_or(|last| last.distance_sq(start) > 1e-6);
        if discontinuous {
            self.flush_outline();
            self.outline.push(start);
        }
        self.outline.push(end);
        if self.run_stroke != stroke || discontinuous {
            self.flush_color();
            self.run_stroke = stroke;
            self.run.push(start);
        }
        self.run.push(end);
    }

    fn point(&self, position: Pos2, radius: f32, dose: Option<f64>) {
        let color = self
            .dose_scale
            .zip(dose)
            .map_or(self.fallback.color, |(scale, dose)| scale.color(dose));
        self.painter
            .circle_filled(position, radius + OUTLINE, egui::Color32::BLACK);
        self.painter.circle_filled(position, radius, color);
    }

    fn flush_color(&mut self) {
        if !self.run.is_empty() {
            self.foreground
                .push(Shape::line(std::mem::take(&mut self.run), self.run_stroke));
        }
    }

    fn flush_outline(&mut self) {
        if !self.outline.is_empty() {
            self.painter.add(Shape::line(
                std::mem::take(&mut self.outline),
                Stroke::new(self.fallback.width + 2.0 * OUTLINE, egui::Color32::BLACK),
            ));
        }
    }

    fn finish(mut self) {
        self.flush_outline();
        self.flush_color();
        self.painter.extend(self.foreground);
    }
}

struct HitTest {
    pointer: Option<Pos2>,
    best: Option<Pick>,
}

pub(super) const SNAP_RADIUS: f32 = 6.0;

#[derive(Clone, Copy)]
pub(super) struct Pick {
    pub sample: Hit,
    pub time: Option<DateTime<Utc>>,
    distance: f32,
    snapped: bool,
}

impl Pick {
    pub(super) fn precedes(self, other: Option<Self>) -> bool {
        self.distance < 64.0
            && other.is_none_or(|other| {
                self.snapped > other.snapped
                    || (self.snapped == other.snapped && self.distance < other.distance)
            })
    }
}

impl HitTest {
    fn point(&mut self, sample: Hit, time: Option<DateTime<Utc>>, point: Pos2, snap: bool) {
        if let Some(pointer) = self.pointer {
            let distance = point.distance_sq(pointer);
            if !snap || distance <= SNAP_RADIUS * SNAP_RADIUS {
                self.consider(Pick {
                    sample,
                    time,
                    distance,
                    snapped: snap,
                });
            }
        }
    }

    fn line(
        &mut self,
        samples: [Hit; 2],
        times: [Option<DateTime<Utc>>; 2],
        [a, b]: [Pos2; 2],
        [start, end]: [f64; 2],
    ) {
        if let Some(pointer) = self.pointer {
            let (t, distance) = crate::ui_ext::segment_hit(pointer, a, b);
            let t = start + (end - start) * f64::from(t);
            let mut pick = Pick {
                sample: samples[usize::from(t > 0.5)],
                time: None,
                distance,
                snapped: false,
            };
            if pick.precedes(self.best) {
                if let [Some(from), Some(to)] = times {
                    pick.time = Some(interpolate_instant(from, to, t));
                }
                self.best = Some(pick);
            }
        }
    }

    fn consider(&mut self, pick: Pick) {
        if pick.precedes(self.best) {
            self.best = Some(pick);
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
        Overlay::prepare(&document, false)
    }

    fn project_path(projection: Projection<'_>, vertices: &[Vertex]) -> Vec<[Pos2; 2]> {
        let mut lines = Vec::new();
        for pair in vertices.windows(2) {
            projection.line(pair[0].position, pair[1].position, |edge, _| {
                lines.push(edge)
            });
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
                draped: None,
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
                draped: None,
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
            draped: None,
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
            |edge, _| lines.push(edge),
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
            |edge, _| lines.push(edge),
        );
        assert_eq!(lines.len(), 1);
        let limb = rect.center().x + camera.scale_px(rect.width(), rect.height()) as f32;
        assert!((lines[0][1].x - limb).abs() < 1e-3);
    }

    #[test]
    fn a_terrain_ridge_hides_the_track_and_marker_from_painting_and_picking() {
        use crate::file_manager::geography::{gpu::Surface, map_camera::Draped};
        use little_clip_core::geography::basemap::{
            perspective::{EARTH_RADIUS_M, Orbit, Perspective},
            tiles::{EDGE, PADDED_EDGE, SourceTile, TileId},
        };
        use std::sync::Arc;

        let id = TileId {
            level: 8,
            x: 256,
            y: 127,
        };
        let [_, north, side] = id.bounds();
        let tile = Arc::new(SourceTile {
            id,
            color: Vec::new(),
            elevation: (0..PADDED_EDGE)
                .flat_map(|y| {
                    let lat = north - (y as f64 - 0.5) * side / EDGE as f64;
                    let meters = if (0.28..=0.31).contains(&lat) {
                        3500.0
                    } else {
                        180.0
                    };
                    [meters, 1.0].repeat(PADDED_EDGE)
                })
                .collect(),
            water: Vec::new(),
            elevation_range: Some([180.0, 3500.0]),
        });
        let camera = GlobeCamera {
            center_lat: 0.35,
            center_lon: 0.35,
            zoom: 900.0,
            mode: ProjectionMode::Map2D,
            orbit: Orbit {
                tilt: 77.0,
                ..Default::default()
            },
        };
        let size = [900.0, 600.0];
        let rect = Rect::from_min_size(Pos2::ZERO, vec2(size[0], size[1]));
        let surface = Surface::new([tile].into_iter(), camera.zoom);
        let perspective = Perspective::new(&camera, size, EARTH_RADIUS_M);
        let ([x, y], _) = surface.project(perspective, 0.48, 0.35).unwrap();
        let hidden_pointer = pos2(x, y);
        assert!(rect.contains(hidden_pointer));

        let document = Document::gpx(
            br#"<gpx xmlns="http://www.topografix.com/GPX/1/1"><wpt lat="0.48" lon="0.35"/><trk><trkseg>
            <trkpt lat="0.48" lon="0.32"><time>2024-04-08T16:00:00Z</time></trkpt>
            <trkpt lat="0.48" lon="0.38"><time>2024-04-08T16:01:00Z</time></trkpt>
            </trkseg></trk></gpx>"#,
        )
        .unwrap();
        let overlay = Overlay::prepare(&document, true);
        let tilted = Projection {
            camera: &camera,
            rect,
            offset: 0.0,
            draped: Draped::new(&camera, Some(&surface), size),
        };
        assert!(
            tilted.draped.unwrap().project(0.48, 0.35).is_none(),
            "A label anchor behind the ridge must be hidden"
        );
        assert!(
            tilted.draped.unwrap().project(0.30, 0.35).is_some(),
            "The foreground ridge must remain visible"
        );
        assert!(
            project_path(
                tilted,
                &overlay.0.iter().find(|p| p.connected).unwrap().vertices
            )
            .is_empty()
        );

        let flat_camera = GlobeCamera {
            orbit: Orbit::default(),
            center_lat: 0.48,
            ..camera
        };
        let flat = Projection {
            camera: &flat_camera,
            draped: None,
            ..tilted
        };
        let ctx = egui::Context::default();
        clip_ui::test_support::run_frame(
            &ctx,
            egui::RawInput {
                screen_rect: Some(rect),
                ..Default::default()
            },
            |ctx| {
                let painter = ctx.layer_painter(egui::LayerId::background());
                let colors = WorkbenchColors::from_palette(&crate::ui_appearance::palette(
                    little_clip_core::config::WorkbenchAppearance::Dark,
                ));
                assert!(
                    overlay
                        .paint_copy(&painter, tilted, None, colors, Some(hidden_pointer))
                        .is_none()
                );
                let [x, y] = flat_camera.project(0.48, 0.36, size[0], size[1]).unwrap();
                assert!(
                    overlay
                        .paint_copy(&painter, flat, None, colors, Some(pos2(x, y)))
                        .and_then(|pick| pick.time)
                        .is_some()
                );
            },
        );
    }

    #[test]
    fn dated_path_interpolates_at_the_dateline_and_snaps_only_to_real_points() {
        let gpx = br#"<gpx xmlns="http://www.topografix.com/GPX/1/1"><trk><trkseg>
            <trkpt lat="0" lon="176"><time>2024-04-08T16:00:00Z</time></trkpt>
            <trkpt lat="0" lon="-176"><time>2024-04-08T18:00:00Z</time></trkpt>
            <trkpt lat="0" lon="-170"/>
            </trkseg></trk></gpx>"#;
        let document = Document::gpx(gpx).unwrap();
        let overlay = Overlay::prepare(&document, true);
        let rect = Rect::from_min_size(Pos2::ZERO, vec2(900.0, 600.0));
        let camera = GlobeCamera {
            center_lat: 0.0,
            center_lon: 180.0,
            zoom: 10.0,
            mode: ProjectionMode::Map2D,
            ..GlobeCamera::default()
        };
        let projection = Projection {
            camera: &camera,
            rect,
            offset: 0.0,
            draped: None,
        };
        let ctx = egui::Context::default();
        clip_ui::test_support::run_frame(
            &ctx,
            egui::RawInput {
                screen_rect: Some(rect),
                ..Default::default()
            },
            |ctx| {
                let painter = ctx.layer_painter(egui::LayerId::background());
                let colors = WorkbenchColors::from_palette(&crate::ui_appearance::palette(
                    little_clip_core::config::WorkbenchAppearance::Dark,
                ));
                let pick = |lon: f64, dx: f32| {
                    let point = projection
                        .point(Position {
                            lat: 0.0,
                            lon: wrap_lon_delta(lon - camera.center_lon),
                        })
                        .unwrap();
                    overlay
                        .paint_copy(
                            &painter,
                            projection,
                            Some(0),
                            colors,
                            Some(point + vec2(dx, 0.0)),
                        )
                        .unwrap()
                };
                let middle = pick(180.0, 0.0);
                assert!(!middle.snapped);
                assert_eq!(
                    middle.time.unwrap().to_rfc3339(),
                    "2024-04-08T17:00:00+00:00"
                );
                let point = pick(176.0, 4.0);
                assert!(point.snapped && point.sample.point == 0);
                assert_eq!(
                    point.time.unwrap().to_rfc3339(),
                    "2024-04-08T16:00:00+00:00"
                );
                assert!(pick(-174.0, 0.0).time.is_none());
            },
        );
    }
}
