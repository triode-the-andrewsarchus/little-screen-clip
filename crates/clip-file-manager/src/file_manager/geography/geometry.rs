//! Geographic vector batches prepared once on a worker, projected by the GPU.
use super::river_coverage::{Coverage, mask_represents, spans};
use little_clip_core::geography::basemap::{
    BasemapAssets, GlobeCamera, GrwlMaskStore,
    tiles::{TileId, surface_tiles},
    wrap_lon, wrap_lon_delta,
};
use std::{
    collections::{BTreeMap, BTreeSet},
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
    Detailed(super::detail::Stroke),
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
impl Batch {
    pub fn appearance(&self, zoom: f64, layers: Layers) -> Option<([f32; 4], f32, bool)> {
        match self.layer {
            Layer::Detailed(stroke) => layers.roads.then(|| stroke.appearance(zoom)),
            layer => layer.appearance(zoom, layers),
        }
    }
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
    pub fn is_detailed_road(self) -> bool {
        matches!(self, Self::Detailed(stroke) if stroke.class.is_road())
    }

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
            Self::Coast if layers.countries => ([0.70, 0.84, 0.96, 0.5], 1.0, false),
            Self::GrwlLarge if layers.rivers && zoom >= 3.5 => {
                ([0.15, 0.40, 0.67, 0.94], -1.0, false)
            }
            Self::RiverLarge if layers.rivers && zoom >= 2.5 => {
                ([0.15, 0.40, 0.67, 0.94], -1.0, false)
            }
            Self::GrwlSmall if layers.rivers && zoom >= 6.0 => {
                ([0.25, 0.57, 0.88, 0.65], -1.0, false)
            }
            Self::RiverMedium if layers.rivers && zoom >= 4.5 => {
                ([0.20, 0.49, 0.78, 0.78], -1.0, false)
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
    target: Option<TileId>,
}
impl Builder {
    fn push(&mut self, layer: Layer, position: [f32; 2], data: &[f32]) -> Result<(), String> {
        let tile = TileId::at(
            self.target.map_or(4, |t| t.level),
            position[1] as f64,
            position[0] as f64,
        );
        if self.target.is_some_and(|target| target != tile) {
            return Ok(());
        }
        self.floats += data.len();
        if self.floats * 4 > 192 * 1024 * 1024 {
            return Err("Map vector geometry exceeds the 192 MiB preparation limit".into());
        }
        self.batches
            .entry((tile, layer))
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

    fn river_line(
        &mut self,
        layer: Layer,
        points: &[[f32; 2]],
        width: f32,
        cancel: &AtomicBool,
        higher: Option<&Coverage<'_>>,
        mask: Option<&GrwlMaskStore>,
    ) -> Result<(), String> {
        if cancel.load(Ordering::Relaxed) {
            return Err("Map preparation cancelled".into());
        }
        if higher
            .is_some_and(|coverage| spans(points).all(|pair| coverage.covers(pair[0], pair[1])))
        {
            return Ok(());
        }
        let masked = mask.is_some_and(|mask| mask_represents(points, width, mask));
        if !masked && higher.is_none() {
            return self.line(layer, points, width, cancel);
        }
        for pair in spans(points) {
            if cancel.load(Ordering::Relaxed) {
                return Err("Map preparation cancelled".into());
            }
            if higher.is_some_and(|coverage| coverage.covers(pair[0], pair[1])) {
                continue;
            }
            let lon = pair[0][0] as f64 + wrap_lon_delta((pair[1][0] - pair[0][0]) as f64) * 0.5;
            let lat = (pair[0][1] as f64 + pair[1][1] as f64) * 0.5;
            if masked && mask.is_some_and(|mask| mask.sample_water(lat, wrap_lon(lon)).is_some()) {
                continue;
            }
            self.line(layer, &pair, width, cancel)?;
        }
        Ok(())
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
            target: None,
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
        for lake in &assets.lakes {
            if cancel.load(Ordering::Relaxed) {
                return Err("Map preparation cancelled".into());
            }
            for triangle in triangulate(&lake.points, &lake.holes)? {
                b.triangle(triangle)?;
            }
        }
        Ok(b.finish())
    }
    pub fn bytes(&self) -> usize {
        self.batches.iter().map(|b| b.data.len() * 4).sum()
    }
}
impl Builder {
    fn finish(self) -> Scene {
        // Each upload is bounded independently; hidden parts never reach the GPU.
        let batches: Vec<Batch> = self
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
        Scene { batches, index }
    }
}
/// A river tile is reusable throughout one detail band and one geographic region.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) struct RiverKey {
    pub tile: TileId,
    detail: u8,
    mask_level: u8,
}
impl RiverKey {
    pub fn visible(
        camera: &GlobeCamera,
        size: [f32; 2],
        enabled: bool,
        mask_level: u8,
    ) -> BTreeSet<Self> {
        if !enabled || camera.zoom < 2.5 {
            return BTreeSet::new();
        }
        let detail = if camera.zoom < 3.5 {
            0
        } else if camera.zoom < 4.5 {
            1
        } else if camera.zoom < 6.0 {
            2
        } else if camera.zoom < 9.0 {
            3
        } else {
            4
        };
        let level = if detail < 2 {
            3
        } else if detail < 4 {
            4
        } else {
            5
        };
        surface_tiles(camera, size[0], size[1], level)
            .into_iter()
            .map(|tile| Self {
                tile,
                detail,
                mask_level,
            })
            .collect()
    }
    pub fn prepare(self, assets: &BasemapAssets, cancel: &AtomicBool) -> Result<Scene, String> {
        let [west, north, side] = self.tile.bounds();
        // Minimum HydroRIVERS stream order and GRWL width for each zoom band.
        const DETAIL: [(u8, f32); 5] = [(6, f32::INFINITY), (6, 90.), (4, 90.), (4, 20.), (3, 20.)];
        let (min_order, min_width) = DETAIL[self.detail as usize];
        let mut b = Builder {
            batches: BTreeMap::new(),
            floats: 0,
            target: Some(self.tile),
        };
        let mask = assets.grwl_masks.as_ref().filter(|_| self.mask_level >= 8);
        // A narrow channel can disappear in a coarsely resampled mask. At
        // fine levels let the observed match decide for either line source.
        let mask_width = 1.5 * 111_320. * 180. / (256. * (1u32 << self.mask_level) as f64);
        let matching_mask =
            |width: f32| mask.filter(|_| self.mask_level >= 10 || f64::from(width) >= mask_width);
        let mut coverage = Coverage::new(self.tile, &assets.lakes);
        if let Some(grwl) = &assets.grwl_summary {
            for seg in grwl.query_bbox(
                west,
                north - side,
                west + side,
                north,
                min_width,
                usize::MAX,
            ) {
                if !coverage.intersects(&seg.points) {
                    continue;
                }
                coverage.insert(&seg.points, seg.width_med_m)?;
                b.river_line(
                    if seg.width_med_m >= 90.0 {
                        Layer::GrwlLarge
                    } else {
                        Layer::GrwlSmall
                    },
                    &seg.points,
                    seg.width_med_m,
                    cancel,
                    None,
                    matching_mask(seg.width_med_m),
                )?;
            }
        }
        if let Some(rivers) = &assets.rivers {
            for seg in rivers.query_bbox(
                west,
                north - side,
                west + side,
                north,
                min_order,
                usize::MAX,
            ) {
                if !coverage.intersects(&seg.points) {
                    continue;
                }
                let width = (7.2 * seg.dis_cms.max(1.0).sqrt()).clamp(50.0, 2400.0);
                b.river_line(
                    if seg.ord_stra >= 6 {
                        Layer::RiverLarge
                    } else if seg.ord_stra >= 4 {
                        Layer::RiverMedium
                    } else {
                        Layer::RiverSmall
                    },
                    &seg.points,
                    width,
                    cancel,
                    Some(&coverage),
                    matching_mask(width),
                )?;
            }
        }
        Ok(b.finish())
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
        .as_chunks::<3>()
        .0
        .iter()
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
    #[ignore = "requires the installed GRWL summary and masks"]
    fn raccoon_river_summary_yields_to_its_fine_water_mask() {
        use little_clip_core::geography::basemap::GrwlSummaryIndex;
        let dir = std::env::var_os("LITTLE_CLIP_WATER_FIXTURE").unwrap();
        let root = std::path::Path::new(&dir);
        let cancel = AtomicBool::new(false);
        let assets = BasemapAssets {
            grwl_summary: Some(GrwlSummaryIndex::load_or_build(root, &cancel).unwrap()),
            grwl_masks: Some(GrwlMaskStore::load_or_index(root).unwrap()),
            ..BasemapAssets::default()
        };
        let contains_chord = |mask_level| {
            RiverKey {
                tile: TileId::at(5, 41.54, -93.75),
                detail: 4,
                mask_level,
            }
            .prepare(&assets, &cancel)
            .unwrap()
            .batches
            .iter()
            .filter(|batch| batch.layer == Layer::GrwlSmall)
            .flat_map(|batch| batch.data.chunks_exact(6))
            .any(|r| {
                let lon = (r[0] + r[2]) * 0.5;
                let lat = (r[1] + r[3]) * 0.5;
                (-93.76..-93.74).contains(&lon) && (41.54..41.55).contains(&lat)
            })
        };
        assert!(
            contains_chord(9),
            "coarse mask lost its centerline fallback"
        );
        assert!(
            !contains_chord(11),
            "duplicate beside the fine mask survived"
        );
    }

    #[test]
    fn observed_masks_replace_both_centerlines_and_retain_uncovered_waterways() {
        use std::{fs, path::Path};
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("grwl_masks")).unwrap();
        let file = fs::File::create(root.path().join("grwl_masks/river.tif")).unwrap();
        let pixels: Vec<u8> = (0..1024).map(|i| u8::from(i / 32 == 16)).collect();
        image::ImageEncoder::write_image(
            image::codecs::tiff::TiffEncoder::new(file),
            &pixels,
            32,
            32,
            image::ExtendedColorType::L8,
        )
        .unwrap();
        let index = serde_json::json!([{
            "file":"river.tif", "epsg":4326, "origin":[0.,0.032], "scale":[0.001,0.001],
            "width":32, "height":32, "min_lon":0., "max_lon":0.032, "min_lat":0., "max_lat":0.032
        }]);
        fs::write(
            root.path().join("grwl_masks_idx_v2.json"),
            serde_json::to_vec(&index).unwrap(),
        )
        .unwrap();
        let mask = GrwlMaskStore::load_or_index(Path::new(root.path())).unwrap();
        let points = [[0.002, 0.0155], [0.010, 0.0155], [0.030, 0.0155]];
        let mut coverage = Coverage::new(TileId::at(5, 0., 0.), &[]);
        coverage.insert(&points, 200.).unwrap();
        let cancel = AtomicBool::new(false);
        let mut b = Builder {
            batches: BTreeMap::new(),
            floats: 0,
            target: None,
        };
        b.river_line(Layer::GrwlLarge, &points, 200., &cancel, None, Some(&mask))
            .unwrap();
        b.river_line(
            Layer::RiverLarge,
            &points,
            200.,
            &cancel,
            Some(&coverage),
            Some(&mask),
        )
        .unwrap();
        assert_eq!(b.floats, 0, "Three providers drew the same river");
        // The mask alone can replace an offset HydroRIVERS line without GRWL.
        let offset = points.map(|p| [p[0], p[1] + 0.0015]);
        b.river_line(Layer::RiverLarge, &offset, 100., &cancel, None, Some(&mask))
            .unwrap();
        assert_eq!(
            b.floats, 0,
            "Mask-only coverage left a duplicate centerline"
        );
        // Land in a mask rectangle does not establish coverage of another river.
        let other = points.map(|p| [p[0], 0.028]);
        b.river_line(Layer::RiverSmall, &other, 50., &cancel, None, Some(&mask))
            .unwrap();
        assert!(
            b.floats > 0,
            "An unmapped tributary inside the mask bounds disappeared"
        );
        let partial = [
            [0.002, 0.0155],
            [0.01, 0.0155],
            [0.02, 0.0155],
            [0.03, 0.0155],
            [0.04, 0.0155],
            [0.05, 0.0155],
        ];
        b.river_line(Layer::GrwlLarge, &partial, 200., &cancel, None, Some(&mask))
            .unwrap();
        let fallback: Vec<_> = b
            .batches
            .iter()
            .filter(|((_, l), _)| *l == Layer::GrwlLarge)
            .flat_map(|(_, v)| v.as_chunks::<6>().0)
            .collect();
        assert!(
            !fallback.is_empty(),
            "Fallback stopped at the mask boundary"
        );
        assert!(fallback.iter().all(|p| p[0] > 0.03));
    }

    #[test]
    fn centerline_fallback_keeps_unmatched_tributaries_and_partial_coverage() {
        let mut coverage = Coverage::new(TileId::at(5, 0., 0.), &[]);
        coverage.insert(&[[0., 0.], [0.02, 0.]], 100.).unwrap();
        let cancel = AtomicBool::new(false);
        let mut b = Builder {
            batches: BTreeMap::new(),
            floats: 0,
            target: None,
        };
        b.river_line(
            Layer::RiverLarge,
            &[[0.005, 0.], [0.01, 0.]],
            100.,
            &cancel,
            Some(&coverage),
            None,
        )
        .unwrap();
        assert_eq!(
            b.floats, 0,
            "Duplicate HydroRIVERS reach survived GRWL coverage"
        );
        b.river_line(
            Layer::RiverSmall,
            &[[0., 0.05], [0.01, 0.05]],
            50.,
            &cancel,
            Some(&coverage),
            None,
        )
        .unwrap();
        assert!(b.floats > 0, "Unmapped tributary was hidden");
        b.river_line(
            Layer::RiverLarge,
            &[[0., 0.], [0.1, 0.]],
            100.,
            &cancel,
            Some(&coverage),
            None,
        )
        .unwrap();
        let remaining: Vec<_> = b
            .batches
            .iter()
            .filter(|((_, layer), _)| *layer == Layer::RiverLarge)
            .flat_map(|(_, data)| data.as_chunks::<6>().0)
            .collect();
        assert!(!remaining.is_empty());
        assert!(remaining.iter().all(|p| p[0] > 0.02));
    }

    #[test]
    fn tributaries_stay_lighter_than_main_channels_regardless_of_provider() {
        let layers = Layers {
            countries: false,
            states: false,
            rivers: true,
            roads: false,
        };
        let appearance = |layer: Layer| layer.appearance(20., layers).unwrap().0;
        assert_eq!(appearance(Layer::GrwlLarge), appearance(Layer::RiverLarge));
        assert_eq!(appearance(Layer::GrwlSmall), appearance(Layer::RiverSmall));
        assert!(appearance(Layer::RiverSmall)[2] > appearance(Layer::RiverLarge)[2]);
        assert!(appearance(Layer::RiverMedium)[2] > appearance(Layer::RiverLarge)[2]);
    }

    #[test]
    fn river_buckets_partition_crossing_lines_and_wrap_the_dateline() {
        let cancel = AtomicBool::new(false);
        for points in [vec![[-15., 0.], [15., 0.]], vec![[178., 1.], [-178., 1.]]] {
            let mut all = Builder {
                batches: BTreeMap::new(),
                floats: 0,
                target: None,
            };
            all.line(Layer::RiverLarge, &points, 100., &cancel).unwrap();
            let records: Vec<_> = all
                .batches
                .values()
                .flat_map(|v| v.as_chunks::<6>().0)
                .collect();
            let targets: BTreeSet<_> = records
                .iter()
                .map(|p| {
                    TileId::at(
                        5,
                        ((p[1] + p[3]) * 0.5) as f64,
                        ((p[0] + p[2]) * 0.5) as f64,
                    )
                })
                .collect();
            let mut actual = Vec::new();
            for target in targets {
                let mut tile = Builder {
                    batches: BTreeMap::new(),
                    floats: 0,
                    target: Some(target),
                };
                tile.line(Layer::RiverLarge, &points, 100., &cancel)
                    .unwrap();
                actual.extend(
                    tile.batches
                        .values()
                        .flat_map(|v| v.as_chunks::<6>().0)
                        .map(|p| p.iter().map(|f| f.to_bits()).collect::<Vec<_>>()),
                );
            }
            let expected: BTreeSet<_> = records
                .iter()
                .map(|p| p.iter().map(|f| f.to_bits()).collect::<Vec<_>>())
                .collect();
            assert_eq!(
                actual.len(),
                records.len(),
                "River segments were duplicated or dropped at tile boundaries"
            );
            assert_eq!(actual.into_iter().collect::<BTreeSet<_>>(), expected);
        }
    }
    #[test]
    fn river_detail_keys_follow_visibility_and_reuse_small_camera_moves() {
        let mut camera = GlobeCamera {
            zoom: 32.,
            center_lat: 12.3,
            center_lon: 21.3,
            ..GlobeCamera::default()
        };
        let before = RiverKey::visible(&camera, [1000., 700.], true, 0);
        camera.center_lon += 0.00001;
        assert_eq!(before, RiverKey::visible(&camera, [1000., 700.], true, 0));
        assert!(RiverKey::visible(&camera, [1000., 700.], false, 0).is_empty());
        for (zoom, detail) in [(3., 0), (4., 1), (5., 2), (7., 3), (10., 4)] {
            camera.zoom = zoom;
            let keys = RiverKey::visible(&camera, [1000., 700.], true, 0);
            assert!(!keys.is_empty());
            assert!(keys.iter().all(|key| key.detail == detail));
        }
    }
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
