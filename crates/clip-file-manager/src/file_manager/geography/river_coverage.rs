//! Local matching between river products that do not share feature identifiers.
use little_clip_core::geography::basemap::{
    GrwlMaskStore, LakePolygon, tiles::TileId, wrap_lon, wrap_lon_delta,
};
use std::collections::BTreeMap;

const METERS: f64 = 111_320.0;
const CELL: f64 = 2_000.0;

struct Segment {
    a: [f64; 2],
    b: [f64; 2],
    radius: f64,
}

/// A bounded local corridor index. Matching directions prevents a crossing
/// tributary from being mistaken for the main channel at their intersection.
pub(super) struct Coverage<'a> {
    origin: [f64; 2],
    cos_lat: f64,
    extent: [f64; 2],
    segments: Vec<Segment>,
    cells: BTreeMap<(i32, i32), Vec<usize>>,
    memberships: usize,
    lakes: Vec<&'a LakePolygon>,
}
impl<'a> Coverage<'a> {
    pub fn new(tile: TileId, lakes: &'a [LakePolygon]) -> Self {
        let [west, north, side] = tile.bounds();
        let lat = north - side * 0.5;
        let cos_lat = lat.to_radians().cos().abs().max(0.15);
        let lakes = lakes
            .iter()
            .filter(|lake| {
                let [w, s, e, n] = lake.bounds.map(f64::from);
                w <= west + side && e >= west && s <= north && n >= north - side
            })
            .collect();
        Self {
            origin: [west + side * 0.5, lat],
            cos_lat,
            extent: [
                side * 0.5 * METERS * cos_lat + 1500.,
                side * 0.5 * METERS + 1500.,
            ],
            segments: Vec::new(),
            cells: BTreeMap::new(),
            memberships: 0,
            lakes,
        }
    }
    fn project(&self, point: [f32; 2]) -> [f64; 2] {
        [
            wrap_lon_delta(point[0] as f64 - self.origin[0]) * METERS * self.cos_lat,
            (point[1] as f64 - self.origin[1]) * METERS,
        ]
    }
    pub fn intersects(&self, points: &[[f32; 2]]) -> bool {
        points.windows(2).any(|pair| {
            let a = self.project(pair[0]);
            let b = self.project(pair[1]);
            (0..2).all(|axis| {
                a[axis].min(b[axis]) <= self.extent[axis]
                    && a[axis].max(b[axis]) >= -self.extent[axis]
            })
        })
    }
    pub fn insert(&mut self, points: &[[f32; 2]], width: f32) -> Result<(), String> {
        for pair in points.windows(2) {
            let a = self.project(pair[0]);
            let b = self.project(pair[1]);
            // Allow for the 500 m HydroRIVERS grid and chords in the simplified
            // GRWL summary. Never use an entire mask rectangle as river coverage.
            let radius = (width as f64 * 0.5 + 250. + distance(a, b) * 0.2).clamp(250., 1200.);
            let bounds = [0, 1].map(|axis| {
                let low = (a[axis].min(b[axis]) - radius).max(-self.extent[axis]);
                let high = (a[axis].max(b[axis]) + radius).min(self.extent[axis]);
                ((low / CELL).floor() as i32, (high / CELL).floor() as i32)
            });
            let id = self.segments.len();
            for x in bounds[0].0..=bounds[0].1 {
                for y in bounds[1].0..=bounds[1].1 {
                    self.memberships += 1;
                    if self.memberships > 262_144 {
                        return Err("River matching exceeds its spatial index budget".into());
                    }
                    self.cells.entry((x, y)).or_default().push(id);
                }
            }
            self.segments.push(Segment { a, b, radius });
        }
        Ok(())
    }
    pub fn covers(&self, a: [f32; 2], b: [f32; 2]) -> bool {
        let a = self.project(a);
        let b = self.project(b);
        let mid = [(a[0] + b[0]) * 0.5, (a[1] + b[1]) * 0.5];
        let lon = wrap_lon(self.origin[0] + mid[0] / (METERS * self.cos_lat)) as f32;
        let lat = (self.origin[1] + mid[1] / METERS) as f32;
        self.lakes
            .iter()
            .any(|lake| lake_contains(lake, [lon, lat]))
            || self.corridor_covers(a, b, mid)
    }

    fn corridor_covers(&self, a: [f64; 2], b: [f64; 2], mid: [f64; 2]) -> bool {
        let cell = (
            (mid[0] / CELL).floor() as i32,
            (mid[1] / CELL).floor() as i32,
        );
        self.cells.get(&cell).is_some_and(|ids| {
            ids.iter().any(|&id| {
                let s = &self.segments[id];
                let dot = (b[0] - a[0]) * (s.b[0] - s.a[0]) + (b[1] - a[1]) * (s.b[1] - s.a[1]);
                dot.abs() >= 0.25 * distance(a, b) * distance(s.a, s.b)
                    && segment_distance(a, s.a, s.b) <= s.radius
                    && segment_distance(b, s.a, s.b) <= s.radius
            })
        })
    }
}

fn lake_contains(lake: &LakePolygon, [lon, lat]: [f32; 2]) -> bool {
    let [w, s, e, n] = lake.bounds;
    let inside = |ring: &[[f32; 2]]| {
        ring.iter()
            .zip(ring.iter().cycle().skip(1))
            .fold(false, |inside, (a, b)| {
                inside
                    ^ ((a[1] > lat) != (b[1] > lat)
                        && lon < a[0] + (lat - a[1]) * (b[0] - a[0]) / (b[1] - a[1]))
            })
    };
    (w..=e).contains(&lon)
        && (s..=n).contains(&lat)
        && inside(&lake.points)
        && !lake.holes.iter().any(|ring| inside(ring))
}
fn distance(a: [f64; 2], b: [f64; 2]) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}
fn segment_distance(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let dx = b[0] - a[0];
    let dy = b[1] - a[1];
    let t =
        (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / (dx * dx + dy * dy).max(1e-12)).clamp(0., 1.);
    distance(p, [a[0] + dx * t, a[1] + dy * t])
}

/// Affirm actual water along a reach before trusting mask land pixels to replace
/// its simplified line. Empty masks, failed reads and absent tiles cannot hide it.
pub(super) fn mask_represents(points: &[[f32; 2]], width: f32, mask: &GrwlMaskStore) -> bool {
    mask_matches(points, width, |p| {
        mask.sample_water(p[1] as f64, wrap_lon(p[0] as f64))
    })
}
fn mask_matches(
    points: &[[f32; 2]],
    width: f32,
    sample: impl Fn([f32; 2]) -> Option<bool>,
) -> bool {
    let radius = (width * 0.5 + 400.).clamp(400., 900.);
    let mut known = 0;
    let mut water = 0;
    for (i, &p) in points.iter().enumerate() {
        let a = points[i.saturating_sub(1)];
        let b = points[(i + 1).min(points.len() - 1)];
        let cos_lat = (p[1] as f64).to_radians().cos().abs().max(0.15);
        let dx = wrap_lon_delta((b[0] - a[0]) as f64) * cos_lat;
        let dy = (b[1] - a[1]) as f64;
        let length = dx.hypot(dy).max(1e-12);
        let observed = sample(p);
        known += usize::from(observed.is_some());
        let found = observed == Some(true)
            || [1., -1., 2., -2., 3., -3.].into_iter().any(|step| {
                let offset = step * radius as f64 / (3. * METERS);
                sample([
                    p[0] + (-dy / length * offset / cos_lat) as f32,
                    p[1] + (dx / length * offset) as f32,
                ]) == Some(true)
            });
        water += usize::from(found);
    }
    water >= 2 && water * 2 >= known.max(points.len() / 2)
}

/// Short spans allow fallback to resume at the edge of finer coverage.
pub(super) fn spans(points: &[[f32; 2]]) -> impl Iterator<Item = [[f32; 2]; 2]> + '_ {
    points.windows(2).flat_map(|pair| {
        let a = pair[0];
        let dx = wrap_lon_delta((pair[1][0] - a[0]) as f64);
        let dy = (pair[1][1] - a[1]) as f64;
        let cos_lat = (a[1] as f64).to_radians().cos().abs().max(0.15);
        let count = ((dx * cos_lat).hypot(dy) * METERS / 250.)
            .ceil()
            .clamp(1., 4096.) as usize;
        (0..count).map(move |i| {
            [i, i + 1].map(|j| {
                let t = j as f64 / count as f64;
                [(a[0] as f64 + dx * t) as f32, (a[1] as f64 + dy * t) as f32]
            })
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearby_parallel_reaches_match_but_crossings_and_other_tributaries_survive() {
        let mut coverage = Coverage::new(TileId::at(5, 0., 0.), &[]);
        coverage.insert(&[[0., 0.], [0.05, 0.]], 200.).unwrap();
        assert!(coverage.covers([0.01, 0.002], [0.012, 0.002]));
        assert!(coverage.covers([0.012, 0.002], [0.01, 0.002]));
        assert!(!coverage.covers([0.01, -0.001], [0.01, 0.001]));
        assert!(!coverage.covers([0.01, 0.04], [0.012, 0.04]));
        assert!(!coverage.covers([0.10, 0.], [0.102, 0.]));
    }

    #[test]
    fn fallback_resumes_beyond_the_end_of_better_coverage() {
        let mut coverage = Coverage::new(TileId::at(5, 0., 0.), &[]);
        coverage.insert(&[[0., 0.], [0.02, 0.]], 100.).unwrap();
        let pieces: Vec<_> = spans(&[[0., 0.], [0.1, 0.]])
            .filter(|p| !coverage.covers(p[0], p[1]))
            .collect();
        assert!(!pieces.is_empty());
        assert!(pieces.first().unwrap()[0][0] > 0.02);
        assert_eq!(pieces.last().unwrap()[1][0], 0.1);
    }

    #[test]
    fn lake_water_hides_fallback_routes_at_the_shore_but_keeps_island_channels() {
        let lakes = [LakePolygon {
            bounds: [0.01, 0., 0.05, 0.04],
            points: vec![[0.01, 0.], [0.05, 0.], [0.05, 0.04], [0.01, 0.04]],
            holes: vec![vec![
                [0.025, 0.01],
                [0.035, 0.01],
                [0.035, 0.03],
                [0.025, 0.03],
            ]],
        }];
        let coverage = Coverage::new(TileId::at(5, 0.02, 0.02), &lakes);
        let xs: Vec<_> = spans(&[[0., 0.02], [0.06, 0.02]])
            .filter(|p| !coverage.covers(p[0], p[1]))
            .map(|p| (p[0][0] + p[1][0]) * 0.5)
            .collect();
        assert!(xs.iter().any(|&x| x < 0.01));
        assert!(xs.iter().any(|x| (0.025..0.035).contains(x)));
        assert!(xs.iter().any(|&x| x > 0.05));
        assert!(
            xs.iter()
                .all(|x| !(0.01..0.025).contains(x) && !(0.035..0.05).contains(x))
        );
    }

    #[test]
    fn matching_and_clipping_take_the_short_path_across_the_dateline() {
        let mut coverage = Coverage::new(TileId::at(5, 0., 179.99), &[]);
        coverage
            .insert(&[[179.98, 0.], [-179.98, 0.]], 100.)
            .unwrap();
        assert!(coverage.covers([179.999, 0.], [-179.999, 0.]));
        let parts: Vec<_> = spans(&[[179.99, 0.], [-179.99, 0.]]).collect();
        assert!(parts.len() < 12);
        assert!(
            parts
                .iter()
                .all(|p| wrap_lon_delta((p[1][0] - p[0][0]) as f64).abs() < 0.003)
        );
    }

    #[test]
    fn mask_selection_requires_actual_channel_evidence_and_tolerates_grid_offsets() {
        let points = [[0., 0.], [0.01, 0.], [0.02, 0.]];
        assert!(!mask_matches(&points, 100., |_| None));
        assert!(!mask_matches(&points, 100., |_| Some(false)));
        assert!(mask_matches(&points, 100., |p| Some(
            (p[1] - 0.0027).abs() < 0.0003
        )));
        assert!(!mask_matches(&points, 100., |p| Some(p[0] == 0.01)));
        assert!(!mask_matches(&[], 100., |_| Some(true)));
    }
}
