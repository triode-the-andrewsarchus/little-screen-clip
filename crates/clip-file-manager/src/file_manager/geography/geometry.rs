//! Geographic vector batches prepared once on a worker, projected by the GPU.
use little_clip_core::geography::basemap::{BasemapAssets, tiles::TileId, wrap_lon_delta};
use std::{
    collections::BTreeMap,
    sync::atomic::{AtomicBool, Ordering},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Layer {
    Grid30,
    Grid10,
    Grid2,
    Grid1,
    Lake,
    Coast,
    RiverSmall,
    RiverMedium,
    RiverLarge,
    GrwlSmall,
    GrwlLarge,
    RoadMinor,
    RoadMajor,
    State,
    StateDisputed,
    Country,
    CountryDisputed,
}
pub(super) struct Batch {
    pub tile: TileId,
    pub layer: Layer,
    pub data: Vec<f32>,
}
pub(super) struct Scene {
    pub batches: Vec<Batch>,
    pub index: BTreeMap<TileId, Vec<usize>>,
}
#[derive(Clone, Copy)]
pub(super) struct Layers {
    pub countries: bool,
    pub states: bool,
    pub rivers: bool,
    pub roads: bool,
}
impl Layer {
    pub fn appearance(self, zoom: f64, layers: Layers) -> Option<([f32; 4], f32, bool)> {
        let grid = if zoom < 3.0 {
            Self::Grid30
        } else if zoom < 12.0 {
            Self::Grid10
        } else if zoom < 40.0 {
            Self::Grid2
        } else {
            Self::Grid1
        };
        let (color, width, dash) = match self {
            Self::Grid30 | Self::Grid10 | Self::Grid2 | Self::Grid1 if self == grid => {
                ([0.8, 0.85, 0.9, 0.16], 0.8, false)
            }
            Self::Lake if layers.rivers && zoom >= 2.0 => ([0.15, 0.40, 0.67, 0.85], 0.0, false),
            Self::Coast => ([0.70, 0.84, 0.96, 0.5], 1.0, false),
            Self::GrwlLarge if layers.rivers && zoom >= 3.5 => {
                ([0.15, 0.40, 0.67, 0.94], -1.0, false)
            }
            Self::RiverLarge if layers.rivers && zoom >= 2.5 => {
                ([0.25, 0.57, 0.88, 0.65], -1.0, false)
            }
            Self::GrwlSmall if layers.rivers && zoom >= 6.0 => {
                ([0.15, 0.40, 0.67, 0.94], -1.0, false)
            }
            Self::RiverMedium if layers.rivers && zoom >= 4.5 => {
                ([0.25, 0.57, 0.88, 0.65], -1.0, false)
            }
            Self::RiverSmall if layers.rivers && zoom >= 9.0 => {
                ([0.25, 0.57, 0.88, 0.65], -1.0, false)
            }
            Self::RoadMajor if layers.roads && zoom >= 3.0 => {
                ([0.93, 0.71, 0.34, 0.78], 1.5, false)
            }
            Self::RoadMinor if layers.roads && zoom >= 5.5 => {
                ([0.83, 0.80, 0.73, 0.51], 1.0, false)
            }
            Self::State if layers.states && zoom >= 2.0 => (
                [
                    0.82,
                    0.84,
                    0.87,
                    0.37 * ((zoom - 2.0) / 3.0).clamp(0.0, 1.0) as f32,
                ],
                0.9,
                false,
            ),
            Self::StateDisputed if layers.states && zoom >= 2.0 => {
                ([0.82, 0.70, 0.55, 0.37], 0.9, true)
            }
            Self::Country if layers.countries => ([0.92, 0.93, 0.96, 0.65], 1.2, false),
            Self::CountryDisputed if layers.countries => ([0.92, 0.76, 0.51, 0.55], 1.0, true),
            _ => return None,
        };
        Some((color, width, dash))
    }
}
struct Builder {
    batches: BTreeMap<(TileId, Layer), Vec<f32>>,
    floats: usize,
}
impl Builder {
    fn push(&mut self, layer: Layer, position: [f32; 2], data: &[f32]) -> Result<(), String> {
        self.floats += data.len();
        if self.floats * 4 > 192 * 1024 * 1024 {
            return Err("Map vector geometry exceeds the 192 MiB preparation limit".into());
        }
        self.batches
            .entry((TileId::at(4, position[1] as f64, position[0] as f64), layer))
            .or_default()
            .extend_from_slice(data);
        Ok(())
    }
    fn line(
        &mut self,
        layer: Layer,
        points: &[[f32; 2]],
        width: f32,
        cancel: &AtomicBool,
    ) -> Result<(), String> {
        for pair in points.windows(2) {
            if cancel.load(Ordering::Relaxed) {
                return Err("Map preparation cancelled".into());
            }
            let a = pair[0];
            let dx = wrap_lon_delta((pair[1][0] - a[0]) as f64) as f32;
            let dy = pair[1][1] - a[1];
            let count = (dx.abs().max(dy.abs()) / 1.0).ceil().clamp(1.0, 360.0) as usize;
            for i in 0..count {
                let point = |t: f32| [a[0] + dx * t, a[1] + dy * t];
                let p = point(i as f32 / count as f32);
                let q = point((i + 1) as f32 / count as f32);
                self.push(
                    layer,
                    [(p[0] + q[0]) * 0.5, (p[1] + q[1]) * 0.5],
                    &[p[0], p[1], q[0], q[1], width, 0.0],
                )?;
            }
        }
        Ok(())
    }
    fn triangle(&mut self, points: [[f32; 2]; 3]) -> Result<(), String> {
        // Bound projection curvature and keep each batch inside its spatial guard band.
        if !points.iter().flatten().all(|v| v.is_finite()) {
            return Err("Invalid lake coordinate".into());
        }
        let [a, b, c] = points;
        let edges = [(a, b, c), (b, c, a), (c, a, b)];
        let (p, q, r) = edges
            .into_iter()
            .max_by(|(p, q, _), (a, b, _)| length(*p, *q).total_cmp(&length(*a, *b)))
            .unwrap();
        if length(p, q) > 2.0 {
            let mid = [(p[0] + q[0]) * 0.5, (p[1] + q[1]) * 0.5];
            self.triangle([p, mid, r])?;
            self.triangle([mid, q, r])
        } else {
            self.push(
                Layer::Lake,
                [(a[0] + b[0] + c[0]) / 3.0, (a[1] + b[1] + c[1]) / 3.0],
                &[a[0], a[1], b[0], b[1], c[0], c[1]],
            )
        }
    }
}
fn length(a: [f32; 2], b: [f32; 2]) -> f32 {
    (a[0] - b[0]).abs().max((a[1] - b[1]).abs())
}
impl Scene {
    pub fn prepare(assets: &BasemapAssets, cancel: &AtomicBool) -> Result<Self, String> {
        let mut b = Builder {
            batches: BTreeMap::new(),
            floats: 0,
        };
        for (step, layer) in [
            (30, Layer::Grid30),
            (10, Layer::Grid10),
            (2, Layer::Grid2),
            (1, Layer::Grid1),
        ] {
            for lat in (-60..=60).step_by(step) {
                let points: Vec<_> = (-180..=180).map(|lon| [lon as f32, lat as f32]).collect();
                b.line(layer, &points, 1.0, cancel)?;
            }
            for lon in (-180..180).step_by(step) {
                let points: Vec<_> = (-90..=90).map(|lat| [lon as f32, lat as f32]).collect();
                b.line(layer, &points, 1.0, cancel)?;
            }
        }
        for poly in &assets.coastlines {
            b.line(Layer::Coast, &poly.points, 1.0, cancel)?;
        }
        for road in &assets.roads {
            b.line(
                if road.major {
                    Layer::RoadMajor
                } else {
                    Layer::RoadMinor
                },
                &road.points,
                1.0,
                cancel,
            )?;
        }
        for line in &assets.country_borders {
            b.line(
                if line.disputed {
                    Layer::CountryDisputed
                } else {
                    Layer::Country
                },
                &line.points,
                1.0,
                cancel,
            )?;
        }
        for line in &assets.state_borders {
            b.line(
                if line.disputed {
                    Layer::StateDisputed
                } else {
                    Layer::State
                },
                &line.points,
                1.0,
                cancel,
            )?;
        }
        if let Some(grwl) = &assets.grwl_summary {
            for seg in grwl.segments() {
                if seg.width_med_m < 20.0 {
                    continue;
                }
                b.line(
                    if seg.width_med_m >= 90.0 {
                        Layer::GrwlLarge
                    } else {
                        Layer::GrwlSmall
                    },
                    &seg.points,
                    seg.width_med_m,
                    cancel,
                )?;
            }
        }
        if let Some(rivers) = &assets.rivers {
            for seg in rivers.segments() {
                if seg.ord_stra >= 3 {
                    b.line(
                        if seg.ord_stra >= 6 {
                            Layer::RiverLarge
                        } else if seg.ord_stra >= 4 {
                            Layer::RiverMedium
                        } else {
                            Layer::RiverSmall
                        },
                        &seg.points,
                        (7.2 * seg.dis_cms.max(1.0).sqrt()).clamp(50.0, 2400.0),
                        cancel,
                    )?;
                }
            }
        }
        for lake in &assets.lakes {
            if cancel.load(Ordering::Relaxed) {
                return Err("Map preparation cancelled".into());
            }
            for triangle in triangulate(&lake.points, &lake.holes)? {
                b.triangle(triangle)?;
            }
        }
        // Each upload is bounded independently; hidden parts never reach the GPU.
        let batches: Vec<Batch> = b
            .batches
            .into_iter()
            .flat_map(|((tile, layer), data)| {
                data.chunks(4096 * 6)
                    .map(|data| Batch {
                        tile,
                        layer,
                        data: data.to_vec(),
                    })
                    .collect::<Vec<_>>()
            })
            .collect();
        let mut index: BTreeMap<TileId, Vec<usize>> = BTreeMap::new();
        for (i, batch) in batches.iter().enumerate() {
            index.entry(batch.tile).or_default().push(i);
        }
        Ok(Self { batches, index })
    }
}
fn triangulate(outer: &[[f32; 2]], holes: &[Vec<[f32; 2]>]) -> Result<Vec<[[f32; 2]; 3]>, String> {
    use lyon_tessellation::{
        BuffersBuilder, FillOptions, FillRule, FillTessellator, FillVertex, VertexBuffers,
        path::{Path, math::point},
    };
    let mut path = Path::builder();
    let Some(origin) = outer.first() else {
        return Ok(Vec::new());
    };
    for ring in std::iter::once(outer).chain(holes.iter().map(Vec::as_slice)) {
        if ring.len() < 3 {
            continue;
        }
        let p = |v: [f32; 2]| {
            point(
                origin[0] + wrap_lon_delta((v[0] - origin[0]) as f64) as f32,
                v[1],
            )
        };
        path.begin(p(ring[0]));
        for &v in &ring[1..] {
            path.line_to(p(v));
        }
        path.end(true);
    }
    let mut buffers: VertexBuffers<[f32; 2], u32> = VertexBuffers::new();
    FillTessellator::new()
        .tessellate_path(
            &path.build(),
            &FillOptions::default().with_fill_rule(FillRule::EvenOdd),
            &mut BuffersBuilder::new(&mut buffers, |v: FillVertex<'_>| {
                [v.position().x, v.position().y]
            }),
        )
        .map_err(|e| format!("Lake tessellation: {e:?}"))?;
    Ok(buffers
        .indices
        .chunks_exact(3)
        .map(|p| {
            [
                buffers.vertices[p[0] as usize],
                buffers.vertices[p[1] as usize],
                buffers.vertices[p[2] as usize],
            ]
        })
        .collect())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lake_tessellation_preserves_concavity_and_islands() {
        let outer = vec![
            [0., 0.],
            [4., 0.],
            [4., 2.],
            [2., 2.],
            [2., 4.],
            [0., 4.],
            [0., 0.],
        ];
        let hole = vec![[0.5, 0.5], [1.5, 0.5], [1.5, 1.5], [0.5, 1.5], [0.5, 0.5]];
        let triangles = triangulate(&outer, &[hole]).unwrap();
        let area: f32 = triangles
            .into_iter()
            .map(|[a, b, c]| {
                ((b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])).abs() * 0.5
            })
            .sum();
        assert!((area - 11.0).abs() < 0.0001);
    }
}
