//! Physically registered comparison of composited fabrication artwork.

mod coverage;
use coverage::CoverageIndex;

use super::{
    RasterRegion, allocate_canvas, check_cancelled, gerber::GerberDocument, pixmap_to_thumbnail,
};
use crate::ImageThumbnail;
use lib_gerber_edit::layer::LayerType;
use std::sync::atomic::AtomicBool;
use tiny_skia::{Color, Pixmap, PremultipliedColorU8, Transform};

pub struct ArtworkComparison {
    pub bounds_mm: [f64; 4],
    pub raster: ArtworkRaster,
}

pub struct ArtworkRaster {
    pub pitch_mm: f64,
    pub before: ImageThumbnail,
    pub after: ImageThumbnail,
    pub changes: ImageThumbnail,
    pub added_pixels: usize,
    pub removed_pixels: usize,
}

/// The full-pair overview and counts at its fixed raster pitch.
pub fn compare_artwork(
    pair: [&GerberDocument; 2],
    tolerance_mm: f64,
    cancel: &AtomicBool,
) -> Result<ArtworkComparison, String> {
    let bounds_mm = comparison_bounds(pair)?;
    let (canvas, transform, scale) = allocate_canvas(bounds_mm, [1400, 1400], Color::TRANSPARENT)
        .ok_or("Invalid fabrication comparison bounds")?;
    let grid = Grid {
        size: [canvas.width(), canvas.height()],
        transform,
        halo: 0,
    };
    drop(canvas);
    let raster = render_pair(pair, grid, f64::from(scale), tolerance_mm, cancel)?;
    Ok(ArtworkComparison { bounds_mm, raster })
}

/// Re-rasterizes both sources around one physical viewport. The comparison
/// searches a halo outside the returned image, so nearby ink across its
/// edges can still satisfy the same physical tolerance.
pub fn compare_artwork_region(
    pair: [&GerberDocument; 2],
    region: RasterRegion,
    tolerance_mm: f64,
    cancel: &AtomicBool,
) -> Result<ArtworkRaster, String> {
    comparison_bounds(pair)?;
    let mut transform = region.transform()?;
    let halo = RasterRegion::halo_pixels(tolerance_mm, region.pixels_per_mm)
        .ok_or("Invalid fabrication tolerance")?;
    if region
        .size
        .iter()
        .any(|&side| side + halo * 2 > RasterRegion::MAX_SIDE)
    {
        return Err("Fabrication detail and tolerance halo exceed the raster budget".into());
    }
    // Translate in whole pixels. The crop will use exactly the requested grid.
    transform.tx += halo as f32;
    transform.ty += halo as f32;
    let grid = Grid {
        size: region.size,
        transform,
        halo,
    };
    render_pair(pair, grid, region.pixels_per_mm, tolerance_mm, cancel)
}

fn comparison_bounds([before, after]: [&GerberDocument; 2]) -> Result<[f64; 4], String> {
    use LayerType::UndefinedGerber;
    if before.is_excellon != after.is_excellon
        || (before.layer_role != after.layer_role
            && ![before.layer_role, after.layer_role].contains(&UndefinedGerber))
    {
        return Err("Choose two files for the same fabrication layer.".into());
    }
    let a = before.bounds_mm;
    let b = after.bounds_mm;
    Ok([
        a[0].min(b[0]),
        a[1].min(b[1]),
        a[2].max(b[2]),
        a[3].max(b[3]),
    ])
}

#[derive(Clone, Copy)]
struct Grid {
    size: [u32; 2],
    transform: Transform,
    halo: u32,
}

fn render_pair(
    [before, after]: [&GerberDocument; 2],
    grid: Grid,
    density: f64,
    tolerance_mm: f64,
    cancel: &AtomicBool,
) -> Result<ArtworkRaster, String> {
    let radius = tolerance_mm * density;
    if !radius.is_finite() || radius < 0.0 {
        return Err("Invalid fabrication tolerance".into());
    }
    let size = grid.size.map(|side| side + grid.halo * 2);
    let a = before.render_layer(size, grid.transform, 0.0, cancel)?;
    let b = after.render_layer(size, grid.transform, 0.0, cancel)?;
    let [width, height] = grid.size;
    let mut changes = Pixmap::new(width, height).ok_or("Could not allocate fabrication changes")?;
    let mut old_ink = Pixmap::new(width, height).ok_or("Could not allocate fabrication preview")?;
    let mut new_ink = Pixmap::new(width, height).ok_or("Could not allocate fabrication preview")?;
    let [added, removed] = changed_pixels(
        [&a, &b],
        [&mut old_ink, &mut new_ink, &mut changes],
        grid.halo,
        radius,
        cancel,
    )?;
    Ok(ArtworkRaster {
        pitch_mm: density.recip(),
        before: pixmap_to_thumbnail(old_ink),
        after: pixmap_to_thumbnail(new_ink),
        changes: pixmap_to_thumbnail(changes),
        added_pixels: added,
        removed_pixels: removed,
    })
}

fn changed_pixels(
    [a, b]: [&Pixmap; 2],
    [old_ink, new_ink, changes]: [&mut Pixmap; 3],
    halo: u32,
    radius: f64,
    cancel: &AtomicBool,
) -> Result<[usize; 2], String> {
    let width = changes.width() as usize;
    let height = changes.height() as usize;
    let halo = halo as usize;
    let stride = a.width() as usize;
    let index = |mask| {
        (radius >= 1.0)
            .then(|| CoverageIndex::new(mask, cancel))
            .transpose()
    };
    let from_a = index(a)?;
    let from_b = index(b)?;
    let mut added = 0;
    let mut removed = 0;
    for y in 0..height {
        if y.is_multiple_of(8) {
            check_cancelled(cancel, "Fabrication comparison cancelled")?;
        }
        for x in 0..width {
            let source = (y + halo) * stride + x + halo;
            let target = y * width + x;
            let [old, new] = [a.pixels()[source].alpha(), b.pixels()[source].alpha()];
            old_ink.pixels_mut()[target] = white(old);
            new_ink.pixels_mut()[target] = white(new);
            if new > old
                && !from_a
                    .as_ref()
                    .is_some_and(|i| i.nearby(x + halo, y + halo, radius, new))
            {
                added += 1;
                changes.pixels_mut()[target] = ink([92, 220, 247], new);
            } else if old > new
                && !from_b
                    .as_ref()
                    .is_some_and(|i| i.nearby(x + halo, y + halo, radius, old))
            {
                removed += 1;
                changes.pixels_mut()[target] = ink([248, 140, 78], old);
            }
        }
    }
    Ok([added, removed])
}

fn white(alpha: u8) -> PremultipliedColorU8 {
    PremultipliedColorU8::from_rgba(alpha, alpha, alpha, alpha).unwrap()
}

fn ink(rgb: [u8; 3], coverage: u8) -> PremultipliedColorU8 {
    let [r, g, b] = rgb.map(|component| ((u16::from(component) * u16::from(coverage)) / 255) as u8);
    PremultipliedColorU8::from_rgba(r, g, b, coverage).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn gerber(path: &str, header: &str, body: &str) -> GerberDocument {
        let text = format!("%FSLAX46Y46*%\n{header}\n{body}\nM02*\n");
        GerberDocument::parse_gerber(Path::new(path), text.as_bytes(), &AtomicBool::new(false))
            .unwrap()
    }

    #[test]
    fn artwork_comparison_registers_units_and_composes_clear_polarity_first() {
        let a = gerber(
            "A-F_Cu.gbr",
            "%MOMM*%\n%ADD10C,5.08*%\n%ADD11C,2.54*%",
            "D10*\nX25400000Y0D03*\n%LPC*%\nD11*\nX25400000Y0D03*",
        );
        let b = gerber(
            "B-F_Cu.gbr",
            "%MOIN*%\n%ADD10C,0.2*%\n%ADD11C,0.1*%",
            "D10*\nX1000000Y0D03*\n%LPC*%\nD11*\nX1000000Y0D03*",
        );
        let cancel = AtomicBool::new(false);
        let same = compare_artwork([&a, &b], 0.0, &cancel).unwrap();
        assert_eq!(
            (same.raster.added_pixels, same.raster.removed_pixels),
            (0, 0)
        );

        let moved = gerber(
            "B-F_Cu.gbr",
            "%MOMM*%\n%ADD10C,5.08*%",
            "D10*\nX35400000Y0D03*",
        );
        let changed = compare_artwork([&a, &moved], 0.0, &cancel).unwrap();
        assert!(changed.raster.added_pixels > 100 && changed.raster.removed_pixels > 100);
        assert!(changed.bounds_mm[0] < 25.4 && changed.bounds_mm[2] > 35.4);
        assert!(compare_artwork([&a, &moved], 0.0, &AtomicBool::new(true)).is_err());
    }

    #[test]
    fn detail_reveals_a_clear_flash_below_overview_resolution() {
        let header = "%MOMM*%\n%ADD10C,1*%\n%ADD11R,4X4*%\n%ADD12C,0.004*%";
        let artwork = "D10*\nX0Y0D03*\nX100000000Y100000000D03*\nD11*\nX50000000Y50000000D03*";
        let a = gerber("before.gbr", header, artwork);
        let b = gerber(
            "after.gbr",
            header,
            &format!("{artwork}\n%LPC*%\nD12*\nX50000000Y50000000D03*"),
        );
        let same = gerber(
            "equivalent.gbr",
            header,
            &format!("{artwork}\nG04 same composed artwork*"),
        );
        let cancel = AtomicBool::new(false);
        let overview = compare_artwork([&a, &b], 0.0, &cancel).unwrap();
        assert_eq!(
            (overview.raster.added_pixels, overview.raster.removed_pixels),
            (0, 0)
        );
        let region = RasterRegion {
            origin: [49.9, 50.1],
            size: [160, 160],
            pixels_per_mm: 800.0,
            y_down: false,
        };
        let detail = compare_artwork_region([&a, &b], region, 0.0, &cancel).unwrap();
        assert_eq!(detail.added_pixels, 0);
        assert!(detail.removed_pixels >= 4);
        let same = compare_artwork_region([&a, &same], region, 0.0, &cancel).unwrap();
        assert_eq!((same.added_pixels, same.removed_pixels), (0, 0));
    }

    #[test]
    fn regional_tolerance_can_match_ink_outside_the_tile_at_two_densities() {
        let header = "%MOMM*%\n%ADD10C,0.02*%";
        let a = gerber("before.gbr", header, "D10*\nX-30000Y0D03*");
        let b = gerber("after.gbr", header, "D10*\nX30000Y0D03*");
        let cancel = AtomicBool::new(false);
        for density in [500.0, 1000.0] {
            let region = RasterRegion {
                origin: [0.0, 0.06],
                size: [(0.08 * density) as u32, (0.12 * density) as u32],
                pixels_per_mm: density,
                y_down: false,
            };
            let exact = compare_artwork_region([&a, &b], region, 0.0, &cancel).unwrap();
            let narrow = compare_artwork_region([&a, &b], region, 0.05, &cancel).unwrap();
            let wide = compare_artwork_region([&a, &b], region, 0.07, &cancel).unwrap();
            assert!(exact.added_pixels > 0 && narrow.added_pixels > 0);
            assert_eq!((wide.added_pixels, wide.removed_pixels), (0, 0));
            assert_eq!(
                wide.after.premultiplied_rgba,
                exact.after.premultiplied_rgba
            );
        }
    }

    #[test]
    fn tolerance_search_uses_euclidean_distance_and_equal_coverage() {
        let mut mask = Pixmap::new(5, 5).unwrap();
        mask.pixels_mut()[2 * 5 + 3] = ink([255; 3], 191);
        let index = CoverageIndex::new(&mask, &AtomicBool::new(false)).unwrap();
        assert!(index.nearby(2, 2, 1.0, 191));
        assert!(!index.nearby(2, 2, 0.0, 191));
        assert!(!index.nearby(2, 2, 1.0, 192));
        assert!(!index.nearby(2, 3, 1.0, 191));
    }
}
