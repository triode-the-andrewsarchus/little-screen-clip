//! Coverage comparison in common physical artwork coordinates.

mod geometry;
mod intersections;

use super::{
    RasterRegion, allocate_canvas, check_cancelled, gerber::GerberDocument, pixmap_to_thumbnail,
    read_snapshot,
};
use crate::{ImageThumbnail, file_source::FileSource};
use geometry::{Budget, Coverage, area_mm2, compose, mm};
use i_overlay::core::overlay_rule::OverlayRule;
use lib_gerber_edit::layer::LayerType;
use std::{path::Path, sync::atomic::AtomicBool};
use tiny_skia::{Color, FillRule, Paint, PathBuilder, Pixmap, Transform};

/// One tenth of a micron between neighboring integer coordinates.
const GRID_MM: f64 = 0.0001;
/// Target maximum curve chord deviation of half a micron.
const CHORD_MM: f64 = 0.0005;

pub struct ArtworkComparison {
    pub bounds_mm: [f64; 4],
    pub added_mm2: f64,
    pub removed_mm2: f64,
    before: Coverage,
    after: Coverage,
    added: Coverage,
    removed: Coverage,
}

pub struct ArtworkRaster {
    pub pitch_mm: f64,
    pub before: ImageThumbnail,
    pub after: ImageThumbnail,
    pub changes: ImageThumbnail,
}

/// Read the same guarded artwork source as the preview, with finer aperture chords.
pub fn read_artwork(
    path: &Path,
    cancel: &AtomicBool,
) -> Result<(GerberDocument, FileSource), String> {
    read_snapshot(path, cancel, |bytes, _| parse_artwork(path, bytes, cancel))
}

fn parse_artwork(path: &Path, bytes: &[u8], cancel: &AtomicBool) -> Result<GerberDocument, String> {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("gbr" | "ger") if !crate::brushes::is_gimp_gbr_header(bytes) => {
            GerberDocument::parse_gerber_with_tolerance(path, bytes, CHORD_MM, cancel)
        }
        Some("drl" | "xln") => {
            GerberDocument::parse_excellon_with_tolerance(path, bytes, CHORD_MM, cancel)
        }
        _ => Err("Expected a Gerber or Excellon artwork layer".into()),
    }
}

/// Compare the filled coverage once for the requested physical tolerance.
pub fn compare_artwork(
    pair: [&GerberDocument; 2],
    tolerance_mm: f64,
    cancel: &AtomicBool,
) -> Result<ArtworkComparison, String> {
    let bounds_mm = comparison_bounds(pair)?;
    if !tolerance_mm.is_finite() || !(0.0..=1.0).contains(&tolerance_mm) {
        return Err("Invalid geometric edge tolerance".into());
    }
    let mut budget = Budget::new(cancel);
    let before = compose(pair[0], &mut budget).map_err(|e| format!("A · {e}"))?;
    let after = compose(pair[1], &mut budget).map_err(|e| format!("B · {e}"))?;
    let mut difference = |a: &Coverage, b: &Coverage| {
        if tolerance_mm > 0.0 && !b.is_empty() && !a.is_empty() {
            let expanded = budget.dilate(b, tolerance_mm)?;
            budget.overlay(a, &expanded, OverlayRule::Difference)
        } else {
            budget.overlay(a, b, OverlayRule::Difference)
        }
    };
    let added = difference(&after, &before)?;
    let removed = difference(&before, &after)?;
    Ok(ArtworkComparison {
        bounds_mm,
        added_mm2: area_mm2(&added),
        removed_mm2: area_mm2(&removed),
        before,
        after,
        added,
        removed,
    })
}

impl ArtworkComparison {
    /// Draw a fitted overview of the existing geometry and its difference.
    pub fn render_overview(&self, cancel: &AtomicBool) -> Result<ArtworkRaster, String> {
        let (changes, transform, _) =
            allocate_canvas(self.bounds_mm, [1400, 1400], Color::TRANSPARENT)
                .ok_or("Invalid fabrication comparison bounds")?;
        self.render(changes, transform, cancel)
    }

    /// Draw the same physical result inside a bounded detail viewport.
    pub fn render_region(
        &self,
        region: RasterRegion,
        cancel: &AtomicBool,
    ) -> Result<ArtworkRaster, String> {
        let (changes, transform, _) = region.canvas(Color::TRANSPARENT)?;
        self.render(changes, transform, cancel)
    }

    fn render(
        &self,
        mut changes: Pixmap,
        transform: Transform,
        cancel: &AtomicBool,
    ) -> Result<ArtworkRaster, String> {
        let allocate = || {
            Pixmap::new(changes.width(), changes.height())
                .ok_or("Could not allocate artwork comparison")
        };
        let mut before = allocate()?;
        let mut after = allocate()?;
        draw(
            &mut before,
            &self.before,
            [255, 255, 255],
            transform,
            cancel,
        )?;
        draw(&mut after, &self.after, [255, 255, 255], transform, cancel)?;
        draw(&mut changes, &self.added, [92, 220, 247], transform, cancel)?;
        draw(
            &mut changes,
            &self.removed,
            [248, 140, 78],
            transform,
            cancel,
        )?;
        Ok(ArtworkRaster {
            pitch_mm: f64::from(transform.sx).recip(),
            before: pixmap_to_thumbnail(before),
            after: pixmap_to_thumbnail(after),
            changes: pixmap_to_thumbnail(changes),
        })
    }
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

fn draw(
    pixmap: &mut Pixmap,
    coverage: &Coverage,
    [r, g, b]: [u8; 3],
    transform: Transform,
    cancel: &AtomicBool,
) -> Result<(), String> {
    let mut paint = Paint::default();
    paint.set_color_rgba8(r, g, b, 255);
    paint.anti_alias = true;
    let [w, h] = [f64::from(pixmap.width()), f64::from(pixmap.height())];
    let project = |point| {
        let [x, y] = mm(point);
        [
            x * f64::from(transform.sx) + f64::from(transform.tx),
            y * f64::from(transform.sy) + f64::from(transform.ty),
        ]
    };
    for (index, polygon) in coverage.iter().enumerate() {
        if index.is_multiple_of(64) {
            check_cancelled(cancel, "Artwork drawing cancelled")?;
        }
        let Some(outer) = polygon.first() else {
            continue;
        };
        let [mut x0, mut y0, mut x1, mut y1] = [
            f64::INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NEG_INFINITY,
        ];
        for &point in outer {
            let [x, y] = project(point);
            [x0, y0, x1, y1] = [x0.min(x), y0.min(y), x1.max(x), y1.max(y)];
        }
        if x1 < 0.0 || y1 < 0.0 || x0 > w || y0 > h {
            continue;
        }
        let mut path = PathBuilder::new();
        for contour in polygon {
            let mut points = contour.iter().copied().map(project);
            if let Some([x, y]) = points.next() {
                path.move_to(x as f32, y as f32);
                for [x, y] in points {
                    path.line_to(x as f32, y as f32);
                }
                path.close();
            }
        }
        if let Some(path) = path.finish() {
            pixmap.fill_path(
                &path,
                &paint,
                FillRule::Winding,
                Transform::identity(),
                None,
            );
        }
    }
    check_cancelled(cancel, "Artwork drawing cancelled")
}

#[cfg(test)]
mod tests {
    use super::*;

    static CANCEL: AtomicBool = AtomicBool::new(false);

    fn gerber(apertures: &str, body: &str) -> GerberDocument {
        let text = format!("%FSLAX46Y46*%\n%MOMM*%\n{apertures}\n{body}\nM02*\n");
        parse_artwork(Path::new("art.gbr"), text.as_bytes(), &CANCEL).unwrap()
    }

    fn area(a: &GerberDocument, b: &GerberDocument, tolerance_mm: f64) -> [f64; 2] {
        let result = compare_artwork([a, b], tolerance_mm, &CANCEL).unwrap();
        [result.added_mm2, result.removed_mm2]
    }

    #[test]
    fn artwork_regions_flashes_and_split_strokes_can_cover_the_same_area() {
        let a = gerber("%ADD10R,2X2*%", "D10*\nX0Y0D02*\nX6000000Y0D01*");
        let b = gerber(
            "",
            "G36*\nX-1000000Y-1000000D02*\nX-1000000Y1000000D01*\nX7000000Y1000000D01*\nX7000000Y-1000000D01*\nX-1000000Y-1000000D01*\nG37*",
        );
        let c = gerber(
            "%ADD10R,2X2*%",
            "D10*\nX0Y0D03*\nX2000000Y0D03*\nX4000000Y0D03*\nX6000000Y0D03*",
        );
        assert_eq!(area(&a, &b, 0.0), [0.0, 0.0]);
        assert_eq!(area(&a, &c, 0.0), [0.0, 0.0]);
    }

    #[test]
    fn inches_and_millimeters_register_on_the_same_board_grid() {
        let metric = gerber(
            "%ADD10C,5.08*%\n%ADD11C,2.54*%",
            "D10*\nX25400000Y0D03*\n%LPC*%\nD11*\nX25400000Y0D03*",
        );
        let imperial = parse_artwork(Path::new("imperial.gbr"),
            b"%FSLAX46Y46*%\n%MOIN*%\n%ADD10C,0.2*%\n%ADD11C,0.1*%\nD10*\nX1000000Y0D03*\n%LPC*%\nD11*\nX1000000Y0D03*\nM02*\n",
            &CANCEL).unwrap();
        assert_eq!(area(&metric, &imperial, 0.0), [0.0, 0.0]);
        let cancelled = AtomicBool::new(true);
        let error = compare_artwork([&metric, &imperial], 0.0, &cancelled)
            .err()
            .unwrap();
        assert!(error.contains("cancelled"), "{error}");
    }

    #[test]
    fn circular_flash_and_region_use_the_same_approximation() {
        let flash = gerber("%ADD10C,6*%", "D10*\nX0Y0D03*");
        let region = gerber(
            "",
            "G75*\nG36*\nX0Y3000000D02*\nG02X0Y-3000000I0J-3000000D01*\nX0Y3000000I0J3000000D01*\nG37*",
        );
        assert_eq!(area(&flash, &region, 0.0), [0.0, 0.0]);
    }

    #[test]
    fn overlapping_clear_and_dark_artwork_is_composited_in_file_order() {
        let apertures = "%ADD10R,10X10*%\n%ADD11R,6X6*%\n%ADD12R,2X2*%";
        let blank = gerber(apertures, "D10*\nX0Y0D03*");
        let clear_last = gerber(
            apertures,
            "D10*\nX0Y0D03*\nD12*\nX0Y0D03*\n%LPC*%\nD11*\nX0Y0D03*",
        );
        let dark_last = gerber(
            apertures,
            "D10*\nX0Y0D03*\n%LPC*%\nD11*\nX0Y0D03*\n%LPD*%\nD12*\nX0Y0D03*",
        );
        assert_eq!(area(&blank, &clear_last, 0.0), [0.0, 36.0]);
        assert_eq!(area(&blank, &dark_last, 0.0), [0.0, 32.0]);
        assert_eq!(area(&clear_last, &dark_last, 0.0), [4.0, 0.0]);
    }

    #[test]
    fn macro_holes_keep_material_beneath_the_flash() {
        let a = gerber(
            "%AMRING*1,1,6,0,0*1,0,2,0,0*%\n%ADD10RING*%\n%ADD11R,1X1*%",
            "D11*\nX0Y0D03*\nD10*\nX0Y0D03*",
        );
        let b = gerber("%ADD10C,6X2*%", "D10*\nX0Y0D03*");
        let delta = area(&b, &a, 0.0);
        assert!((delta[0] - 1.0).abs() < 1e-6, "{delta:?}");
        assert_eq!(delta[1], 0.0);
    }

    #[test]
    fn subpixel_clear_flash_is_a_change_before_rendering_the_overview() {
        let header = "%ADD10C,1*%\n%ADD11R,4X4*%\n%ADD12C,0.004*%";
        let artwork = "D10*\nX0Y0D03*\nX100000000Y100000000D03*\nD11*\nX50000000Y50000000D03*";
        let a = gerber(header, artwork);
        let b = gerber(
            header,
            &format!("{artwork}\n%LPC*%\nD12*\nX50000000Y50000000D03*"),
        );
        let comparison = compare_artwork([&a, &b], 0.0, &CANCEL).unwrap();
        assert_eq!(comparison.added_mm2, 0.0);
        assert!(comparison.removed_mm2 > 0.000_01);
        let overview = comparison.render_overview(&CANCEL).unwrap();
        assert!(overview.pitch_mm > 0.004);
        let visible_ink = |image: &ImageThumbnail| {
            image
                .premultiplied_rgba
                .chunks_exact(4)
                .filter(|pixel| pixel[3] != 0)
                .count()
        };
        assert_eq!(visible_ink(&overview.changes), 0);
        let region = RasterRegion {
            origin: [49.9, 50.1],
            size: [160, 160],
            pixels_per_mm: 800.0,
            y_down: false,
        };
        let detail = comparison.render_region(region, &CANCEL).unwrap();
        assert!(visible_ink(&detail.changes) >= 4);
    }

    #[test]
    fn excellon_slot_and_equivalent_routed_footprint_compare_as_coverage() {
        let parse = |body| {
            let text = format!("M48\nMETRIC,TZ\nT1C0.500\n%\nG90\nT1\n{body}\nM30\n");
            parse_artwork(Path::new("art.drl"), text.as_bytes(), &CANCEL).unwrap()
        };
        let slot = parse("X1.0Y2.0G85X5.0Y2.0");
        let route = parse("G00X1.0Y2.0\nM15\nG01X3.0Y2.0\nG01X5.0Y2.0\nM16");
        let longer_slot = parse("X1.0Y2.0G85X5.002Y2.0");
        assert_eq!(area(&slot, &route, 0.0), [0.0, 0.0]);
        let [added, removed] = area(&slot, &longer_slot, 0.0);
        assert!((added - 0.001).abs() < 0.000_01, "{added}");
        assert_eq!(removed, 0.0);
    }

    #[test]
    fn edge_tolerance_is_a_physical_dilation_of_the_opposite_coverage() {
        let a = gerber("%ADD10R,1X1*%", "D10*\nX0Y0D03*");
        let b = gerber("%ADD10R,1X1*%", "D10*\nX30000Y0D03*");
        assert!(area(&a, &b, 0.010).into_iter().all(|mm2| mm2 > 0.0));
        assert_eq!(area(&a, &b, 0.050), [0.0, 0.0]);
    }

    #[test]
    fn artwork_with_unsupported_transform_discloses_incomplete_geometry() {
        let a = gerber("%ADD10R,1X1*%", "D10*\nX0Y0D03*");
        let b = gerber("%ADD10R,1X1*%", "%LR90*%\nD10*\nX0Y0D03*");
        let error = compare_artwork([&a, &b], 0.0, &CANCEL).err().unwrap();
        assert!(error.contains("load rotation"), "{error}");
    }
}
