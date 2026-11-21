//! Raster coverage for lakes in the terrain material, including island holes.
use super::{EDGE, PADDED_EDGE, TileId};
use crate::geography::basemap::{LakePolygon, wrap_lon_delta};
use std::sync::atomic::{AtomicBool, Ordering};

pub(super) fn rasterize(
    lakes: &[LakePolygon],
    tile: TileId,
    water: &mut [u8],
    cancel: &AtomicBool,
) -> Option<()> {
    let [west, north, side] = tile.bounds();
    let pixel = side / EDGE as f64;
    let left = west - pixel;
    let right = west + side + pixel;
    let mut crossings = Vec::new();
    for lake in lakes {
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        let [w, south, e, top] = lake.bounds.map(f64::from);
        if top < north - side - pixel || south > north + pixel {
            continue;
        }
        let row_range = texels((north - top) / pixel, (north - south) / pixel);
        let Some(first) = lake.points.first() else {
            continue;
        };
        let anchor = f64::from(first[0]);
        for shift in [-360.0, 0.0, 360.0] {
            // A seam-crossing outline may have raw bounds almost 360° wide.
            let (w, e) = if e - w > 180.0 {
                (anchor - 180.0, anchor + 180.0)
            } else {
                (w, e)
            };
            if w + shift > right || e + shift < left {
                continue;
            }
            for row in row_range.clone() {
                let lat = north - (row as f64 - 0.5) * pixel;
                crossings.clear();
                for ring in std::iter::once(&lake.points).chain(&lake.holes) {
                    for (&a, &b) in ring.iter().zip(ring.iter().cycle().skip(1)) {
                        let [ay, by] = [f64::from(a[1]), f64::from(b[1])];
                        if (ay > lat) != (by > lat) {
                            let ax = wrap_lon_delta(f64::from(a[0]) - anchor);
                            let bx = wrap_lon_delta(f64::from(b[0]) - anchor);
                            let lon = anchor + shift + ax + (lat - ay) * (bx - ax) / (by - ay);
                            crossings.push(lon);
                        }
                    }
                }
                crossings.sort_unstable_by(f64::total_cmp);
                for pair in crossings.chunks_exact(2) {
                    let cols = texels((pair[0] - west) / pixel, (pair[1] - west) / pixel);
                    for col in cols {
                        water[(row * PADDED_EDGE + col) * 2 + 1] = 255;
                    }
                }
            }
        }
    }
    Some(())
}

// The padded texture's centers are at i - 0.5, the same locations used to
// sample terrain and satellite imagery. Half-open spans share edge ownership.
fn texels(start: f64, end: f64) -> std::ops::Range<usize> {
    let index = |value: f64| ((value + 0.5).ceil() as usize).min(PADDED_EDGE);
    index(start)..index(end)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geography::basemap::BasemapAssets;

    #[test]
    fn lake_material_covers_dateline_neighbors_and_keeps_islands_dry() {
        let ring = [[179.0, 1.0], [-179.0, 1.0], [-179.0, 2.0], [179.0, 2.0]];
        let island = [[179.4, 1.3], [179.6, 1.3], [179.6, 1.7], [179.4, 1.7]];
        let lake = LakePolygon {
            bounds: [-179.0, 1.0, 179.0, 2.0],
            points: ring.to_vec(),
            holes: vec![island.to_vec()],
        };
        let assets = BasemapAssets {
            lakes: vec![lake],
            ..BasemapAssets::default()
        };
        let sample = |lat, lon| {
            let tile = TileId::at(6, lat, lon);
            let data =
                super::super::SourceTile::prepare(&assets, tile, &AtomicBool::new(false)).unwrap();
            let [west, north, side] = tile.bounds();
            let pixel = side / EDGE as f64;
            let col = ((lon - west) / pixel + 0.5).round() as usize;
            let row = ((north - lat) / pixel + 0.5).round() as usize;
            data.water[(row * PADDED_EDGE + col) * 2 + 1]
        };
        assert_eq!(sample(1.5, 179.9), 255);
        assert_eq!(sample(1.5, -179.9), 255);
        assert_eq!(sample(1.5, 179.5), 0);
        assert_eq!(sample(0.7, 179.9), 0);
    }
}
