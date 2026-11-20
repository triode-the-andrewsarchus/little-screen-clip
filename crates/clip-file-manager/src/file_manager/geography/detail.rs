//! Offline Overture transportation tiles and Division labels.
pub(crate) mod import;
mod store;

use super::geometry::{Batch, Layer};
use little_clip_core::geography::basemap::{GlobeCamera, storage::AssetDirectory, tiles::TileId};
use serde::Deserialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    sync::{
        Arc, LazyLock,
        atomic::{AtomicBool, Ordering},
    },
};

pub(crate) const DATABASE: &str = "overture-2026-11-18-v1.sqlite";
pub(crate) const DOWNLOAD_BYTES: u64 = 73_356_433_173;
pub(crate) const STAGING_BYTES: u64 = 2 << 30;
pub(crate) const ESTIMATED_FINAL: u64 = 14 << 30;
pub(crate) const ESTIMATED_PEAK: u64 = ESTIMATED_FINAL + STAGING_BYTES;
pub(super) const LEVEL: u8 = 10;
pub(super) const MIN_ZOOM: f64 = 420.0;
pub(super) const SIDE: f64 = 180.0 / (1u32 << LEVEL) as f64;
const COLS: u32 = 2 << LEVEL;
const MAX_VISIBLE: usize = 64;
const MAX_EDGES: usize = 270_000;
const MAX_LABELS: usize = 1_000;
pub(super) const BRIDGE: u8 = 1;
pub(super) const TUNNEL: u8 = 2;

#[derive(Deserialize)]
pub(crate) struct Manifest {
    pub release: String,
    pub base_url: String,
    pub attribution: String,
    pub sources: Vec<Object>,
}
#[derive(Deserialize)]
pub(crate) struct Object {
    pub kind: Kind,
    pub key: String,
    pub bytes: u64,
}
#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Kind {
    Segment,
    Division,
}

pub(crate) fn manifest() -> &'static Manifest {
    static MANIFEST: LazyLock<Manifest> = LazyLock::new(|| {
        serde_json::from_str(include_str!("detail/overture-2026-11-18.json"))
            .expect("Checked-in Overture manifest")
    });
    &MANIFEST
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub(super) enum Class {
    WaterRoute,
    Unknown,
    Track,
    Service,
    Pedestrian,
    LivingStreet,
    Residential,
    Unclassified,
    Tertiary,
    Secondary,
    Primary,
    Trunk,
    Motorway,
    Rail,
    Neighborhood,
    Town,
    City,
}

impl Class {
    const ALL: [Self; 17] = [
        Self::WaterRoute,
        Self::Unknown,
        Self::Track,
        Self::Service,
        Self::Pedestrian,
        Self::LivingStreet,
        Self::Residential,
        Self::Unclassified,
        Self::Tertiary,
        Self::Secondary,
        Self::Primary,
        Self::Trunk,
        Self::Motorway,
        Self::Rail,
        Self::Neighborhood,
        Self::Town,
        Self::City,
    ];

    fn is_place(self) -> bool {
        self >= Self::Neighborhood
    }

    pub(super) fn is_road(self) -> bool {
        (Self::Unknown..=Self::Motorway).contains(&self)
    }
}

// The existing GPU line pass sorts by level first, then applies all casings
// before fills at that level. Tunnels keep a dashed fill and no solid casing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct Stroke {
    pub stack: i8,
    pub fill: bool,
    pub class: Class,
    pub flags: u8,
}

impl Stroke {
    pub fn appearance(self, zoom: f64) -> ([f32; 4], f32, bool) {
        let (mut rgb, width) = match self.class {
            Class::Rail => ([0.84, 0.69, 0.86], 1.5),
            Class::WaterRoute => ([0.53, 0.84, 0.96], 1.55),
            Class::Motorway | Class::Trunk => ([1.0, 0.75, 0.40], 2.25),
            Class::Primary | Class::Secondary => ([0.99, 0.87, 0.65], 1.7),
            Class::Tertiary => ([0.94, 0.89, 0.77], 1.4),
            _ => ([0.88, 0.87, 0.83], 0.95),
        };
        let scale = (1.0 + 0.27 * (zoom / 850.0).log2().max(0.0)).min(2.1) as f32;
        if !self.fill {
            return ([0.17, 0.20, 0.25, 0.94], width * scale + 2.0, false);
        }
        let tunnel = self.flags & TUNNEL != 0;
        if tunnel {
            rgb = rgb.map(|value| 0.22 + value * 0.54);
        }
        let dashed = tunnel || self.class == Class::Rail || self.class == Class::WaterRoute;
        (
            [rgb[0], rgb[1], rgb[2], if tunnel { 0.74 } else { 0.93 }],
            width * scale,
            dashed,
        )
    }
}

#[derive(Clone, Debug)]
pub(super) struct Label {
    pub class: Class,
    pub text: String,
    pub a: [f32; 2],
    pub b: [f32; 2],
}

pub(super) struct Tile {
    pub batches: Vec<Batch>,
    pub labels: Vec<Label>,
}
impl Tile {
    pub fn has_places(&self) -> bool {
        self.labels
            .first()
            .is_some_and(|label| label.class.is_place())
    }

    pub fn bytes(&self) -> usize {
        self.batches
            .iter()
            .map(|batch| batch.data.len() * 4)
            .sum::<usize>()
            + self
                .labels
                .iter()
                .map(|label| std::mem::size_of::<Label>() + label.text.len())
                .sum::<usize>()
            + std::mem::size_of::<Self>()
    }
}

pub(crate) fn database_bytes(
    dir: &little_clip_core::geography::basemap::storage::AssetDirectory,
) -> u64 {
    store::database_metadata(&dir.path().join(DATABASE)).map_or(0, |metadata| metadata.len())
}

#[derive(Default)]
pub(crate) struct Status {
    pub source: Option<Source>,
    pub bytes: u64,
}

impl Status {
    pub(crate) fn load(root: &Path) -> Self {
        let Ok(directory) = AssetDirectory::open(root) else {
            return Self::default();
        };
        let bytes = database_bytes(&directory);
        let source = (bytes > 0).then(|| Source::open(directory)).flatten();
        Self { source, bytes }
    }
}

pub(crate) fn free_space_for_import(index_bytes: u64) -> u64 {
    ESTIMATED_PEAK
        .saturating_sub(index_bytes)
        .max(STAGING_BYTES)
}

#[derive(Clone)]
pub(crate) struct Source {
    directory: Arc<AssetDirectory>,
}

impl Source {
    fn open(directory: AssetDirectory) -> Option<Self> {
        let db = store::read_only(&directory).ok()?;
        store::is_complete(&db).ok()?.then(|| Self {
            directory: Arc::new(directory),
        })
    }

    pub(super) fn prepare(&self, id: TileId, cancel: &AtomicBool) -> Result<Tile, String> {
        let db = store::read_only(&self.directory)?;
        let mut tile = Tile {
            batches: Vec::new(),
            labels: Vec::new(),
        };
        let mut batches = BTreeMap::<Stroke, Vec<f32>>::new();
        let mut edges = 0usize;
        store::read(&db, id, |block| {
            if cancel.load(Ordering::Relaxed) {
                return Err("Transportation preparation cancelled".into());
            }
            for feature in store::records(block, id) {
                let feature = feature?;
                if let Some(label) = feature.label() {
                    if tile.labels.len() < MAX_LABELS * 2 {
                        tile.labels.push(label);
                    } else {
                        // Keep memory bounded even for dense cells.
                        retain_labels(&mut tile.labels);
                    }
                }
                edges += feature.points.len().saturating_sub(1);
                if edges > MAX_EDGES {
                    return Err("Transportation tile exceeds the preparation budget".into());
                }
                feature.append(&mut batches);
            }
            Ok(())
        })?;
        retain_labels(&mut tile.labels);
        tile.batches = batches
            .into_iter()
            .map(|(stroke, data)| Batch {
                tile: id,
                layer: Layer::Detailed(stroke),
                data,
            })
            .collect();
        Ok(tile)
    }
}

fn retain_labels(labels: &mut Vec<Label>) {
    let priority = |class| match class {
        Class::City => 6,
        Class::Town => 5,
        Class::Neighborhood => 4,
        Class::Rail | Class::WaterRoute => 3,
        Class::Motorway | Class::Trunk | Class::Primary | Class::Secondary => 2,
        _ => 1,
    };
    labels.sort_unstable_by(|a, b| {
        let len = |l: &Label| (l.a[0] - l.b[0]).hypot(l.a[1] - l.b[1]);
        priority(b.class)
            .cmp(&priority(a.class))
            .then_with(|| len(b).total_cmp(&len(a)))
    });
    labels.truncate(MAX_LABELS);
}

pub(super) fn visible(camera: &GlobeCamera, [w, h]: [f32; 2], enabled: bool) -> BTreeSet<TileId> {
    if !enabled || camera.zoom < MIN_ZOOM {
        return BTreeSet::new();
    }
    let mut tiles = little_clip_core::geography::basemap::tiles::surface_tiles(camera, w, h, LEVEL);
    if tiles.len() > MAX_VISIBLE {
        let mut sorted: Vec<_> = tiles.into_iter().collect();
        let distance = |tile: &TileId| {
            let [lon, lat, side] = tile.bounds();
            let dx = little_clip_core::geography::basemap::wrap_lon_delta(
                lon + side * 0.5 - camera.center_lon,
            ) * camera.center_lat.to_radians().cos().abs().max(0.25);
            dx.hypot(lat - side * 0.5 - camera.center_lat)
        };
        sorted.sort_unstable_by(|a, b| distance(a).total_cmp(&distance(b)));
        tiles = sorted.into_iter().take(MAX_VISIBLE).collect();
    }
    tiles
}

impl Label {
    fn show(&self, zoom: f64, place_pass: bool) -> bool {
        let min_zoom = match self.class {
            Class::Neighborhood => 530.0,
            Class::Town
            | Class::City
            | Class::Motorway
            | Class::Trunk
            | Class::Rail
            | Class::WaterRoute => MIN_ZOOM,
            Class::Primary | Class::Secondary | Class::Tertiary => 580.0,
            _ => 1200.0,
        };
        self.class.is_place() == place_pass && zoom >= min_zoom
    }
}

pub(super) fn paint_labels(
    ui: &egui::Ui,
    rect: egui::Rect,
    camera: GlobeCamera,
    tiles: &[(TileId, Arc<Tile>)],
    roads: bool,
    places: bool,
) {
    use egui::{Align2, Color32, FontId, Rect, vec2};
    let painter = ui.painter_at(rect);
    let mut placed = Vec::<Rect>::new();
    let mut names = BTreeSet::new();
    let project = |[lon, lat]: [f32; 2]| {
        camera
            .project(f64::from(lat), f64::from(lon), rect.width(), rect.height())
            .map(|[x, y]| rect.min + vec2(x, y))
    };
    for (place, _) in [(true, places), (false, roads)]
        .into_iter()
        .filter(|(_, enabled)| *enabled)
    {
        let candidates = tiles
            .iter()
            .flat_map(|(_, tile)| tile.labels.iter().take(256))
            .filter(|label| label.show(camera.zoom, place));
        let (color, size, align, offset, background) = if place {
            (
                Color32::from_rgb(238, 236, 224),
                12.5,
                Align2::LEFT_CENTER,
                vec2(5.0, 0.0),
                157,
            )
        } else {
            (
                Color32::from_rgb(238, 230, 210),
                10.5,
                Align2::CENTER_CENTER,
                vec2(0.0, -4.0),
                130,
            )
        };
        for label in candidates {
            if names.contains(label.text.as_str()) {
                continue;
            }
            let (Some(a), Some(b)) = (project(label.a), project(label.b)) else {
                continue;
            };
            let center = a.lerp(b, 0.5);
            if !rect.shrink(12.0).contains(center) {
                continue;
            }
            let galley =
                painter.layout_no_wrap(label.text.clone(), FontId::proportional(size), color);
            let bounds = align.anchor_size(center + offset, galley.size());
            let padded = bounds.expand2(vec2(4.0, 2.0));
            if !rect.contains_rect(padded) || placed.iter().any(|r| r.intersects(padded)) {
                continue;
            }
            painter.rect_filled(padded, 2.0, Color32::from_black_alpha(background));
            painter.galley(bounds.min, galley, color);
            if place {
                painter.circle_filled(center, 2.0, color);
            }
            placed.push(padded);
            names.insert(label.text.as_str());
            if placed.len() >= 72 {
                return;
            }
        }
    }
}
