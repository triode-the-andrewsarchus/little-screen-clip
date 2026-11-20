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
const MAX_VIEW_LABELS: usize = 72;
const LABEL_COLUMNS: usize = 6;
const LABEL_ROWS: usize = 4;
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
    pub fn has_settlements(&self) -> bool {
        self.labels
            .first()
            .is_some_and(|label| label.class >= Class::Town)
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
    labels.sort_unstable_by(Label::placement_order);
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
    fn placement_order(&self, other: &Self) -> std::cmp::Ordering {
        let priority = |class| match class {
            Class::City => 6,
            Class::Town => 5,
            Class::Neighborhood => 4,
            Class::Rail | Class::WaterRoute => 3,
            Class::Motorway | Class::Trunk | Class::Primary | Class::Secondary => 2,
            _ => 1,
        };
        let len = |label: &Self| (label.a[0] - label.b[0]).hypot(label.a[1] - label.b[1]);
        priority(other.class)
            .cmp(&priority(self.class))
            .then_with(|| len(other).total_cmp(&len(self)))
    }

    fn min_zoom(&self) -> f64 {
        match self.class {
            Class::Neighborhood => 530.0,
            Class::Town
            | Class::City
            | Class::Motorway
            | Class::Trunk
            | Class::Rail
            | Class::WaterRoute => MIN_ZOOM,
            Class::Primary | Class::Secondary | Class::Tertiary => 580.0,
            _ => 1200.0,
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct LabelLayers {
    pub cities: bool,
    pub detailed: bool,
}

impl LabelLayers {
    fn shows(self, class: Class, roads: bool) -> bool {
        match class {
            Class::City | Class::Town => self.cities,
            Class::Neighborhood => self.detailed,
            _ => self.detailed && roads,
        }
    }
}

pub(super) fn paint_labels(
    ui: &egui::Ui,
    rect: egui::Rect,
    camera: GlobeCamera,
    tiles: &[(TileId, Arc<Tile>)],
    layers: LabelLayers,
    roads: bool,
    draped: Option<super::map_camera::Draped<'_>>,
) {
    let painter = ui.painter_at(rect);
    let mut placed = Vec::with_capacity(MAX_VIEW_LABELS);
    let mut names = BTreeSet::new();
    for (settlements, _) in [(true, layers.cities), (false, layers.detailed)]
        .into_iter()
        .filter(|&(_, enabled)| enabled)
    {
        let candidates = tiles
            .iter()
            .flat_map(|(_, tile)| tile.labels.iter().take(256))
            .filter(|label| {
                (label.class >= Class::Town) == settlements
                    && layers.shows(label.class, roads)
                    && camera.zoom >= label.min_zoom()
            });
        let mut cells = label_cells(rect, camera, draped, candidates).map(|mut cell| {
            cell.sort_by(|a, b| a.label.placement_order(b.label));
            cell.into_iter()
        });
        // Each populated part of the viewport gets a turn before a denser
        // part can claim extra labels. Empty cells leave their share available.
        loop {
            let before = placed.len();
            for cell in &mut cells {
                cell.find(|label| label.paint(&painter, rect, &mut placed, &mut names, draped));
                if placed.len() == MAX_VIEW_LABELS {
                    return;
                }
            }
            if before == placed.len() {
                break;
            }
        }
    }
}

struct VisibleLabel<'a> {
    label: &'a Label,
    center: egui::Pos2,
    anchor: Option<super::map_camera::Anchor>,
}

fn label_cells<'a>(
    rect: egui::Rect,
    camera: GlobeCamera,
    draped: Option<super::map_camera::Draped<'_>>,
    labels: impl Iterator<Item = &'a Label>,
) -> [Vec<VisibleLabel<'a>>; LABEL_COLUMNS * LABEL_ROWS] {
    let project = |[lon, lat]: [f32; 2]| {
        camera
            .project(f64::from(lat), f64::from(lon), rect.width(), rect.height())
            .map(|[x, y]| rect.min + egui::vec2(x, y))
    };
    let inner = rect.shrink(12.0);
    let mut cells = std::array::from_fn(|_| Vec::new());
    for label in labels {
        let (center, anchor) = if let Some(draped) = draped {
            let [lon, lat] = label.a;
            let [end_lon, end_lat] = label.b;
            let delta = little_clip_core::geography::basemap::wrap_lon_delta(
                f64::from(end_lon) - f64::from(lon),
            );
            let Some(anchor) = draped.anchor(
                f64::from(lat).midpoint(f64::from(end_lat)),
                f64::from(lon) + delta * 0.5,
            ) else {
                continue;
            };
            (
                rect.min + egui::vec2(anchor.pixel[0], anchor.pixel[1]),
                Some(anchor),
            )
        } else {
            let (Some(a), Some(b)) = (project(label.a), project(label.b)) else {
                continue;
            };
            (a.lerp(b, 0.5), None)
        };
        if inner.contains(center) {
            let x = ((center.x - rect.left()) * LABEL_COLUMNS as f32 / rect.width()) as usize;
            let y = ((center.y - rect.top()) * LABEL_ROWS as f32 / rect.height()) as usize;
            let index = y.min(LABEL_ROWS - 1) * LABEL_COLUMNS + x.min(LABEL_COLUMNS - 1);
            cells[index].push(VisibleLabel {
                label,
                center,
                anchor,
            });
        }
    }
    cells
}

impl<'a> VisibleLabel<'a> {
    fn paint(
        &self,
        painter: &egui::Painter,
        rect: egui::Rect,
        placed: &mut Vec<egui::Rect>,
        names: &mut BTreeSet<&'a str>,
        draped: Option<super::map_camera::Draped<'_>>,
    ) -> bool {
        use egui::{Align2, Color32, FontId, vec2};
        if names.contains(self.label.text.as_str()) {
            return false;
        }
        let place = self.label.class.is_place();
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
        let galley =
            painter.layout_no_wrap(self.label.text.clone(), FontId::proportional(size), color);
        let bounds = align.anchor_size(self.center + offset, galley.size());
        let padded = bounds.expand2(vec2(4.0, 2.0));
        if !rect.contains_rect(padded) || placed.iter().any(|r| r.intersects(padded)) {
            return false;
        }
        // Only spend a terrain ray on labels that can actually be placed.
        if draped
            .zip(self.anchor)
            .is_some_and(|(view, anchor)| !view.unobscured(anchor))
        {
            return false;
        }
        painter.rect_filled(padded, 2.0, Color32::from_black_alpha(background));
        painter.galley(bounds.min, galley, color);
        if place {
            painter.circle_filled(self.center, 2.0, color);
        }
        placed.push(padded);
        names.insert(self.label.text.as_str());
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dense_tiles_leave_room_for_names_in_the_rest_of_the_view() {
        use egui::{Pos2, Rect, vec2};
        use little_clip_core::geography::basemap::ProjectionMode;

        let rect = Rect::from_min_size(Pos2::ZERO, vec2(1440.0, 960.0));
        let camera = GlobeCamera {
            center_lat: 0.0,
            center_lon: 0.0,
            zoom: 700.0,
            mode: ProjectionMode::Map2D,
            orbit: Default::default(),
        };
        let label = |x, y, class, text| {
            let (lat, lon) = camera.unproject(x, y, rect.width(), rect.height()).unwrap();
            Label {
                class,
                text,
                a: [lon as f32, lat as f32],
                b: [lon as f32, lat as f32],
            }
        };
        let west = (0..96)
            .map(|i| {
                label(
                    55.0 + (i % 12) as f32 * 48.0,
                    55.0 + (i / 12) as f32 * 55.0,
                    Class::Neighborhood,
                    format!("N{i}"),
                )
            })
            .collect();
        let east = [
            (950.0, 200.0, Class::Town, "East Town"),
            (1100.0, 455.0, Class::Rail, "East Railway"),
            (1310.0, 705.0, Class::Primary, "East Road"),
        ];
        let tile = |x, labels| {
            (
                TileId {
                    level: LEVEL,
                    x,
                    y: 0,
                },
                Arc::new(Tile {
                    batches: Vec::new(),
                    labels,
                }),
            )
        };
        let tiles = [
            tile(0, west),
            tile(
                1,
                east.iter()
                    .map(|&(x, y, class, name)| label(x, y, class, name.into()))
                    .collect(),
            ),
        ];
        let ctx = egui::Context::default();
        let output = clip_ui::test_support::run_frame(
            &ctx,
            egui::RawInput {
                screen_rect: Some(rect),
                ..Default::default()
            },
            |ctx| {
                ctx.root_ui(|ui| {
                    paint_labels(
                        ui,
                        rect,
                        camera,
                        &tiles,
                        LabelLayers {
                            cities: true,
                            detailed: true,
                        },
                        true,
                        None,
                    );
                });
            },
        );
        let names: BTreeSet<_> = output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::Text(text) => Some(text.galley.text()),
                _ => None,
            })
            .collect();
        assert_eq!(names.len(), MAX_VIEW_LABELS);
        for (_, _, _, name) in east {
            assert!(names.contains(name), "Missing {name}");
        }
    }

    #[test]
    fn city_and_detailed_names_are_independent() {
        for cities in [false, true] {
            for detailed in [false, true] {
                let layers = LabelLayers { cities, detailed };
                assert_eq!(layers.shows(Class::City, true), cities);
                assert_eq!(layers.shows(Class::Town, true), cities);
                assert_eq!(layers.shows(Class::Neighborhood, false), detailed);
                assert_eq!(layers.shows(Class::Primary, true), detailed);
                assert_eq!(layers.shows(Class::Rail, true), detailed);
                assert!(!layers.shows(Class::Primary, false));
            }
        }
    }
}
