//! Geographic source tiles, independent of camera position, palette and graphics API.
//!
//! Decode these on bounded workers. Camera changes reuse the same source pixels;
//! projection, filtering, lighting and contours belong to the desktop GPU renderer.
use super::{BasemapAssets, GlobeCamera};
use std::{
    collections::BTreeSet,
    sync::atomic::{AtomicBool, Ordering},
};

pub const EDGE: usize = 256;
pub const PADDED_EDGE: usize = EDGE + 2;
pub const MAX_LEVEL: u8 = 11;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TileId {
    pub level: u8,
    pub x: u32,
    pub y: u32,
}

impl TileId {
    pub fn at(level: u8, lat: f64, lon: f64) -> Self {
        let rows = 1u32 << level.min(MAX_LEVEL);
        Self {
            level: level.min(MAX_LEVEL),
            x: ((((lon + 180.0) / 360.0).rem_euclid(1.0) * (rows * 2) as f64).floor() as u32)
                .min(rows * 2 - 1),
            y: (((90.0 - lat).clamp(0.0, 180.0) / 180.0 * rows as f64).floor() as u32)
                .min(rows - 1),
        }
    }

    /// West, north and angular side length in degrees.
    pub fn bounds(self) -> [f64; 3] {
        let side = 180.0 / (1u32 << self.level) as f64;
        [
            -180.0 + self.x as f64 * side,
            90.0 - self.y as f64 * side,
            side,
        ]
    }

    pub fn parent(self) -> Option<Self> {
        self.level.checked_sub(1).map(|level| Self {
            level,
            x: self.x / 2,
            y: self.y / 2,
        })
    }
}

pub struct SourceTile {
    pub id: TileId,
    pub color: Vec<u8>,
    /// Interleaved meters and validity. Empty when no elevation sources are loaded.
    pub elevation: Vec<f32>,
    /// 0 = no coverage, 128 = observed land, 255 = observed water.
    pub water: Vec<u8>,
    pub elevation_range: Option<[f32; 2]>,
}

impl SourceTile {
    pub fn bytes(&self) -> usize {
        self.color.len() + self.elevation.len() * 4 + self.water.len()
    }

    pub fn prepare(assets: &BasemapAssets, id: TileId, cancel: &AtomicBool) -> Option<Self> {
        let [west, north, side] = id.bounds();
        let target = (EDGE as u32 * (2u32 << id.level)) as f64;
        let mut elevation = assets
            .has_elevation()
            .then(|| assets.elevation_sampler(target));
        let count = PADDED_EDGE * PADDED_EDGE;
        let mut tile = Self {
            id,
            color: Vec::with_capacity(count * 4),
            elevation: Vec::with_capacity(if elevation.is_some() { count * 2 } else { 0 }),
            water: Vec::with_capacity(count),
            elevation_range: None,
        };
        for y in 0..PADDED_EDGE {
            if cancel.load(Ordering::Relaxed) {
                return None;
            }
            let lat = (north - (y as f64 - 0.5) * side / EDGE as f64).clamp(-90.0, 90.0);
            for x in 0..PADDED_EDGE {
                let lon = super::wrap_lon(west + (x as f64 - 0.5) * side / EDGE as f64);
                let rgb = assets.sample_ne2_at(lat, lon, target);
                let color = rgb.or_else(|| {
                    assets
                        .land_cover
                        .as_ref()
                        .map(|lc| lc.sample(lat, lon).rgb())
                });
                tile.color.extend_from_slice(&color.unwrap_or([60, 78, 90]));
                tile.color.push(if color.is_some() { 255 } else { 0 });
                if let Some(elevation) = &mut elevation {
                    tile.push_height(elevation.sample(lat, lon));
                }
                tile.water.push(
                    match (id.level >= 8)
                        .then_some(assets)
                        .and_then(|assets| assets.grwl_masks.as_ref())
                        .as_ref()
                        .and_then(|m| m.sample_water(lat, lon))
                    {
                        Some(true) => 255,
                        Some(false) => 128,
                        None => 0,
                    },
                );
            }
        }
        Some(tile)
    }

    fn push_height(&mut self, height: Option<f32>) {
        let height = height.filter(|h| h.is_finite() && *h > -32000.0);
        self.elevation.extend_from_slice(&[
            height.unwrap_or(0.0),
            if height.is_some() { 1.0 } else { 0.0 },
        ]);
        if let Some(height) = height.filter(|h| *h > 0.0) {
            let range = self.elevation_range.get_or_insert([height, height]);
            range[0] = range[0].min(height);
            range[1] = range[1].max(height);
        }
    }
}

/// Samples the visible surface, with a one-tile guard band and coarse ancestors.
/// Coarse parents cover the limb and newly exposed surface while fine tiles load.
pub fn visible_tiles(
    camera: &GlobeCamera,
    width: f32,
    height: f32,
    pixels_per_point: f32,
) -> Vec<TileId> {
    let mut level = display_level(camera, width, height, pixels_per_point);
    loop {
        let mut ids = surface_tiles(camera, width, height, level);
        let finest: Vec<_> = ids.iter().copied().collect();
        for id in finest {
            let mut parent = id.parent();
            while let Some(id) = parent {
                ids.insert(id);
                parent = id.parent();
            }
        }
        ids.insert(TileId {
            level: 0,
            x: 0,
            y: 0,
        });
        ids.insert(TileId {
            level: 0,
            x: 1,
            y: 0,
        });
        if ids.len() <= 96 || level == 0 {
            return ids.into_iter().collect();
        }
        level -= 1;
    }
}

/// A compact preview needs only visible raster tiles and limits work per document.
pub fn preview_tiles(
    camera: &GlobeCamera,
    width: f32,
    height: f32,
    pixels_per_point: f32,
) -> BTreeSet<TileId> {
    for level in (0..=display_level(camera, width, height, pixels_per_point)).rev() {
        let tiles = sample_tiles(camera, width, height, level, 0);
        if tiles.len() <= 6 || level == 0 {
            return tiles;
        }
    }
    unreachable!()
}

fn display_level(camera: &GlobeCamera, width: f32, height: f32, pixels_per_point: f32) -> u8 {
    let target = camera.scale_px(width, height) * std::f64::consts::TAU * pixels_per_point as f64;
    ((target / (EDGE * 2) as f64).log2().ceil().max(0.0) as u8).min(MAX_LEVEL)
}

pub fn surface_tiles(camera: &GlobeCamera, width: f32, height: f32, level: u8) -> BTreeSet<TileId> {
    sample_tiles(camera, width, height, level, 1)
}

fn sample_tiles(
    camera: &GlobeCamera,
    width: f32,
    height: f32,
    level: u8,
    guard: i32,
) -> BTreeSet<TileId> {
    let rows = 1i32 << level.min(MAX_LEVEL);
    let mut ids = BTreeSet::new();
    for y in 0..=12 {
        for x in 0..=16 {
            if let Some((lat, lon)) = camera.unproject(
                width * x as f32 / 16.0,
                height * y as f32 / 12.0,
                width,
                height,
            ) {
                let id = TileId::at(level, lat, lon);
                for dy in -guard..=guard {
                    for dx in -guard..=guard {
                        let ty = id.y as i32 + dy;
                        if (0..rows).contains(&ty) {
                            ids.insert(TileId {
                                level: id.level,
                                x: (id.x as i32 + dx).rem_euclid(rows * 2) as u32,
                                y: ty as u32,
                            });
                        }
                    }
                }
            }
        }
    }
    ids
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn natural_earth_detail_is_not_replaced_by_coarse_land_cover() {
        use crate::geography::basemap::{EquirectTexture, GlobalLandCoverGrid, LandCoverClass};
        let mut assets = BasemapAssets::default();
        assets.ne2 = Some(EquirectTexture {
            width: 1,
            height: 1,
            rgb: std::sync::Arc::new(vec![45, 95, 155]),
        });
        assets.land_cover = Some(GlobalLandCoverGrid {
            width: 1,
            height: 1,
            classes: vec![LandCoverClass::Forest as u8],
        });
        let cancel = AtomicBool::new(false);
        let id = TileId::at(0, 0., 0.);
        let tile = SourceTile::prepare(&assets, id, &cancel).unwrap();
        assert!(
            tile.color
                .chunks_exact(4)
                .all(|pixel| pixel == [45, 95, 155, 255])
        );
        assets.ne2 = None;
        let tile = SourceTile::prepare(&assets, id, &cancel).unwrap();
        let [r, g, b] = LandCoverClass::Forest.rgb();
        assert!(
            tile.color
                .chunks_exact(4)
                .all(|pixel| pixel == [r, g, b, 255])
        );
    }
    #[test]
    fn wrapped_tiles_have_bounded_coordinates_and_parents() {
        assert_eq!(TileId::at(4, 0.0, -180.0), TileId::at(4, 0.0, 180.0));
        assert_eq!(TileId::at(4, -90.0, 179.0).y, 15);
        let child = TileId::at(8, 35.0, -105.0);
        assert_eq!(child.parent(), Some(TileId::at(7, 35.0, -105.0)));
    }
    #[test]
    fn visible_working_set_fits_the_source_memory_budget() {
        for zoom in [0.75, 1.0, 3.15, 6.0, 64.0, 512.0] {
            for lat in [-85.0, 0.0, 85.0] {
                let camera = GlobeCamera {
                    zoom,
                    center_lat: lat,
                    ..GlobeCamera::default()
                };
                let ids = visible_tiles(&camera, 7680.0, 1080.0, 2.0);
                assert!(ids.len() <= 96);
                assert!(ids.len() * PADDED_EDGE * PADDED_EDGE * 13 < 96 * 1024 * 1024);
            }
        }
    }
    #[test]
    fn camera_motion_within_a_tile_reuses_source_keys() {
        assert_eq!(
            TileId::at(8, 35.0, -105.0),
            TileId::at(8, 35.00001, -105.00001)
        );
        let ids = visible_tiles(&GlobeCamera::default(), 960.0, 720.0, 1.0);
        assert!(ids.len() < 128);
        assert!(ids.iter().any(|id| id.level == 0));
    }
}
