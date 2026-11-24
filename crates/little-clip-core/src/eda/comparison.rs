//! Physically registered comparison of composited fabrication artwork.

use super::{allocate_canvas, check_cancelled, gerber::GerberDocument, pixmap_to_thumbnail};
use crate::ImageThumbnail;
use lib_gerber_edit::layer::LayerType;
use std::sync::atomic::AtomicBool;
use tiny_skia::{Color, Pixmap, PremultipliedColorU8};

pub struct ArtworkComparison {
    pub bounds_mm: [f64; 4],
    pub pitch_mm: f32,
    pub before: ImageThumbnail,
    pub after: ImageThumbnail,
    pub changes: ImageThumbnail,
    pub added_pixels: usize,
    pub removed_pixels: usize,
}

pub fn compare_artwork(
    [before, after]: [&GerberDocument; 2],
    tolerance_pixels: u8,
    cancel: &AtomicBool,
) -> Result<ArtworkComparison, String> {
    use LayerType::UndefinedGerber;
    if before.is_excellon != after.is_excellon
        || (before.layer_role != after.layer_role
            && ![before.layer_role, after.layer_role].contains(&UndefinedGerber))
    {
        return Err("Choose two files for the same fabrication layer.".into());
    }
    let a = before.bounds_mm;
    let b = after.bounds_mm;
    let bounds_mm = [
        a[0].min(b[0]),
        a[1].min(b[1]),
        a[2].max(b[2]),
        a[3].max(b[3]),
    ];
    let (mut changes, transform, scale) =
        allocate_canvas(bounds_mm, [1400, 1400], Color::TRANSPARENT)
            .ok_or("Invalid fabrication comparison bounds")?;
    let size = [changes.width(), changes.height()];
    let mut a = before.render_layer(size, transform, 0.0, cancel)?;
    let mut b = after.render_layer(size, transform, 0.0, cancel)?;
    let [width, height] = size.map(|n| n as usize);
    let radius = usize::from(tolerance_pixels.min(4));
    let mut added = 0;
    let mut removed = 0;
    for y in 0..height {
        if y.is_multiple_of(8) {
            check_cancelled(cancel, "Fabrication comparison cancelled")?;
        }
        for x in 0..width {
            let index = y * width + x;
            let [old, new] = [a.pixels()[index].alpha(), b.pixels()[index].alpha()];
            if new > old && !coverage_nearby(&a, x, y, radius, new) {
                added += 1;
                changes.pixels_mut()[index] = ink([92, 220, 247], new);
            } else if old > new && !coverage_nearby(&b, x, y, radius, old) {
                removed += 1;
                changes.pixels_mut()[index] = ink([248, 140, 78], old);
            }
        }
    }
    // A and B use identical white ink. Overlay opacity is a display control.
    for image in [&mut a, &mut b] {
        for pixel in image.pixels_mut() {
            let alpha = pixel.alpha();
            *pixel = PremultipliedColorU8::from_rgba(alpha, alpha, alpha, alpha).unwrap();
        }
    }
    Ok(ArtworkComparison {
        bounds_mm,
        pitch_mm: scale.recip(),
        before: pixmap_to_thumbnail(a),
        after: pixmap_to_thumbnail(b),
        changes: pixmap_to_thumbnail(changes),
        added_pixels: added,
        removed_pixels: removed,
    })
}

fn coverage_nearby(mask: &Pixmap, x: usize, y: usize, radius: usize, coverage: u8) -> bool {
    if radius == 0 {
        return false;
    }
    let width = mask.width() as usize;
    let height = mask.height() as usize;
    for row in y.saturating_sub(radius)..=(y + radius).min(height - 1) {
        let dy = y.abs_diff(row);
        for column in x.saturating_sub(radius)..=(x + radius).min(width - 1) {
            let dx = x.abs_diff(column);
            if dx * dx + dy * dy <= radius * radius
                && mask.pixels()[row * width + column].alpha() >= coverage
            {
                return true;
            }
        }
    }
    false
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
        let same = compare_artwork([&a, &b], 0, &cancel).unwrap();
        assert_eq!((same.added_pixels, same.removed_pixels), (0, 0));

        let moved = gerber(
            "B-F_Cu.gbr",
            "%MOMM*%\n%ADD10C,5.08*%",
            "D10*\nX35400000Y0D03*",
        );
        let changed = compare_artwork([&a, &moved], 0, &cancel).unwrap();
        assert!(changed.added_pixels > 100 && changed.removed_pixels > 100);
        assert!(changed.bounds_mm[0] < 25.4 && changed.bounds_mm[2] > 35.4);
        assert!(compare_artwork([&a, &moved], 0, &AtomicBool::new(true)).is_err());
    }

    #[test]
    fn raster_tolerance_uses_a_bounded_distance_at_equal_coverage() {
        let mut mask = Pixmap::new(5, 5).unwrap();
        mask.pixels_mut()[2 * 5 + 3] = ink([255; 3], 191);
        assert!(coverage_nearby(&mask, 2, 2, 1, 191));
        assert!(!coverage_nearby(&mask, 2, 2, 0, 191));
        assert!(!coverage_nearby(&mask, 2, 2, 1, 192));
        assert!(!coverage_nearby(&mask, 2, 3, 1, 191));
    }
}
