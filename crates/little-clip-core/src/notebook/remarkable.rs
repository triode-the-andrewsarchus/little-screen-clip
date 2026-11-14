//! Read-only reMarkable page decoding through the native `remarkable-rm` crate.
//!
//! This page backend is not yet a multi-page archive reader. Call only inside
//! the notebook's disposable, resource-limited worker: upstream scene assembly
//! and rasterization are synchronous and cannot poll our cancellation token.
use std::sync::atomic::{AtomicBool, Ordering};

pub mod archive;
mod legacy;

const MAX_PAGE_BYTES: usize = 16 * 1024 * 1024;
const MAX_BLOCKS: usize = 65_536;
const MAX_POINTS: usize = 1_000_000;

/// Qualifies the shared raw-page prefix independently of the `.rm` suffix.
/// RealMedia files also use that suffix and must retain their video dispatch.
pub fn has_signature(bytes: &[u8]) -> bool {
    bytes.starts_with(b"reMarkable .lines file, version=")
}

/// Parses a legacy or v6 page with byte, block and geometry budgets.
///
/// # Errors
/// Rejects cancellation, unsupported versions, malformed data and preview limits.
/// The caller must retain and validate the guarded source identity.
pub fn parse(bytes: &[u8], cancel: &AtomicBool) -> Result<remarkable_rm::Page, String> {
    if cancel.load(Ordering::Relaxed) {
        return Err("Notebook preview cancelled".into());
    }
    if bytes.len() > MAX_PAGE_BYTES {
        return Err("reMarkable page exceeds the 16 MiB preview limit".into());
    }
    let version = std::str::from_utf8(bytes.get(..43).ok_or("Truncated reMarkable header")?)
        .map_err(|_| "Invalid reMarkable header")?
        .strip_prefix("reMarkable .lines file, version=")
        .ok_or("Unrecognized reMarkable page header")?
        .trim()
        .parse::<u32>()
        .map_err(|_| "Invalid reMarkable page version")?;
    let page = match version {
        3..=5 => legacy::parse(bytes)?,
        6 => parse_scene(bytes, cancel)?,
        _ => return Err(format!("Unsupported reMarkable page version {version}")),
    };
    validate_geometry(&page)?;
    Ok(page)
}

fn parse_scene(bytes: &[u8], cancel: &AtomicBool) -> Result<remarkable_rm::Page, String> {
    let mut scene = ScenePreflight::default();
    for (index, block) in remarkable_rm::blocks::iter_blocks(bytes)
        .map_err(|error| error.to_string())?
        .enumerate()
    {
        if index >= MAX_BLOCKS {
            return Err("reMarkable page exceeds the block preview limit".into());
        }
        let block = block.map_err(|error| error.to_string())?;
        scene.observe(block)?;
        if cancel.load(Ordering::Relaxed) {
            return Err("Notebook preview cancelled".into());
        }
    }
    scene.validate()?;
    remarkable_rm::parse_page(bytes).map_err(|error| error.to_string())
}

/// Saved tombstones normally refer to records already absent from the file.
/// The library ignores these envelopes. Admit that case, but reject a deletion
/// overlapping a live record rather than accidentally showing erased content.
#[derive(Default)]
struct ScenePreflight {
    live: std::collections::BTreeSet<(remarkable_rm::CrdtId, remarkable_rm::CrdtId)>,
    deleted: Vec<(remarkable_rm::CrdtId, remarkable_rm::CrdtId, u32)>,
}

impl ScenePreflight {
    fn observe(&mut self, block: remarkable_rm::blocks::Block<'_>) -> Result<(), String> {
        let kind = block.header.block_type;
        if !matches!(kind, 3..=6 | 8) {
            return Ok(());
        }
        let mut body = block.body;
        let mut envelope = || {
            let parent = body.read_id(1)?;
            let id = body.read_id(2)?;
            body.read_id(3)?;
            body.read_id(4)?;
            let length = body.read_int(5)?;
            Ok::<_, remarkable_rm::ParseError>((parent, id, length))
        };
        let (parent, id, length) = envelope().map_err(|error| error.to_string())?;
        if matches!(kind, 6 | 8) && !body.is_eof() {
            return Err("Unsupported reMarkable scene text or tombstone payload".into());
        }
        if kind == 8 || length > 0 {
            self.deleted.push((parent, id, length));
        } else if kind != 6 {
            self.live.insert((parent, id));
        }
        Ok(())
    }

    fn validate(&self) -> Result<(), String> {
        for &(parent, id, length) in &self.deleted {
            let end = remarkable_rm::CrdtId {
                seq: id
                    .seq
                    .checked_add(u64::from(length))
                    .ok_or("Invalid reMarkable deleted interval")?,
                ..id
            };
            if self
                .live
                .range((parent, id)..(parent, end))
                .next()
                .is_some()
            {
                return Err("Unsupported reMarkable deletion overlapping retained ink".into());
            }
        }
        Ok(())
    }
}

fn validate_geometry(page: &remarkable_rm::Page) -> Result<(), String> {
    let mut points = 0;
    for layer in &page.layers {
        for line in &layer.lines {
            points += line.points.len();
            if points > MAX_POINTS {
                return Err("reMarkable page exceeds the point preview limit".into());
            }
            validate_line(line)?;
        }
        for glyph in &layer.glyphs {
            validate_highlights(glyph)?;
        }
    }
    validate_text(page)?;
    let size = remarkable_rm::RenderOptions::for_page(page);
    if size.width == 0 || size.height == 0 || size.width > 65_536 || size.height > 65_536 {
        return Err("reMarkable page dimensions exceed the preview limit".into());
    }
    Ok(())
}

fn validate_highlights(glyph: &remarkable_rm::scene::glyph::GlyphRange) -> Result<(), String> {
    for rect in &glyph.rectangles {
        if ![rect.x, rect.y, rect.width, rect.height]
            .into_iter()
            .all(coordinate)
            || rect.width < 0.0
            || rect.height < 0.0
        {
            return Err("Invalid reMarkable highlight geometry".into());
        }
    }
    Ok(())
}

fn validate_line(line: &remarkable_rm::Line) -> Result<(), String> {
    if !coordinate(line.thickness_scale)
        || line.thickness_scale < 0.0
        || !coordinate(f64::from(line.starting_length))
        || line
            .points
            .iter()
            .any(|point| !coordinate(f64::from(point.x)) || !coordinate(f64::from(point.y)))
    {
        return Err("Invalid reMarkable stroke geometry".into());
    }
    Ok(())
}

fn validate_text(page: &remarkable_rm::Page) -> Result<(), String> {
    use remarkable_rm::scene::text::TextContent;
    let Some(text) = &page.text else {
        return Ok(());
    };
    if !coordinate(text.pos_x)
        || !coordinate(text.pos_y)
        || !coordinate(f64::from(text.width))
        || text.width <= 0.0
    {
        return Err("Invalid reMarkable text geometry".into());
    }
    let bytes: usize = text
        .items
        .iter()
        .map(|item| match &item.content {
            Some(TextContent::String(value)) => value.len(),
            _ => 0,
        })
        .sum();
    if bytes > 1024 * 1024 {
        return Err("reMarkable page exceeds the text preview limit".into());
    }
    Ok(())
}

fn coordinate(value: f64) -> bool {
    value.is_finite() && value.abs() <= 1_000_000.0
}

/// Renders a raw page's real ink and text within a requested longest edge.
///
/// No template or PDF companion is inferred from a raw page. Containers must
/// resolve those assets separately before promising a complete notebook view.
///
/// # Errors
/// Returns parser, geometry, cancellation, pixel-budget or PNG encoding errors.
pub fn render(bytes: &[u8], edge: u32, cancel: &AtomicBool) -> Result<(Vec<u8>, u32, u32), String> {
    if !(1..=4096).contains(&edge) {
        return Err("reMarkable preview edge must be in 1..=4096".into());
    }
    let page = parse(bytes, cancel)?;
    let size = remarkable_rm::RenderOptions::for_page(&page);
    let scale = f64::from(edge) / f64::from(size.width.max(size.height));
    let width = (f64::from(size.width) * scale).round().max(1.0) as u32;
    let height = (f64::from(size.height) * scale).round().max(1.0) as u32;
    let png = remarkable_rm::render_page(&page, remarkable_rm::RenderOptions { width, height })
        .map_err(|error| error.to_string())?;
    if cancel.load(Ordering::Relaxed) {
        return Err("Notebook preview cancelled".into());
    }
    Ok((png, width, height))
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::GenericImageView;
    use std::path::{Path, PathBuf};

    fn corpus() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/examples/notebooks/remarkable")
    }

    #[test]
    fn real_v6_pages_parse_and_render_bounded_visible_ink() {
        let cancel = AtomicBool::new(false);
        for folder in ["rmscene", "rmc", "tablet-thumbnails"] {
            for entry in std::fs::read_dir(corpus().join(folder)).unwrap() {
                let path = entry.unwrap().path();
                if path.extension().is_none_or(|ext| ext != "rm") {
                    continue;
                }
                let bytes = std::fs::read(&path).unwrap();
                assert!(has_signature(&bytes));
                let (png, width, height) = render(&bytes, 320, &cancel)
                    .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
                assert!(width <= 320 && height <= 320);
                assert!(
                    image::load_from_memory(&png)
                        .unwrap()
                        .to_rgb8()
                        .pixels()
                        .any(|pixel| pixel.0.iter().any(|channel| *channel < 200)),
                    "{}",
                    path.display()
                );
            }
        }
    }

    #[test]
    fn real_and_generated_legacy_pages_keep_layers_geometry_and_visible_ink() {
        let cancel = AtomicBool::new(false);
        for file in [
            "legacy-real/test_v5_advent_of_code.rm",
            "legacy-real/test_v5_math.rm",
            "legacy-generated/simple.rm",
            "legacy-generated/multi-layer.rm",
        ] {
            let bytes = std::fs::read(corpus().join(file)).unwrap();
            let page = parse(&bytes, &cancel).unwrap();
            assert!(!page.layers.is_empty());
            if file.ends_with("test_v5_advent_of_code.rm") {
                assert_eq!(page.layers.len(), 1);
                assert_eq!(page.layers[0].lines.len(), 139);
            }
            let generated = file.starts_with("legacy-generated/");
            let (png, width, height) =
                render(&bytes, if generated { 1872 } else { 512 }, &cancel).unwrap();
            let image = image::load_from_memory(&png).unwrap().to_rgb8();
            assert_eq!(image.dimensions(), (width, height));
            assert!(
                image
                    .pixels()
                    .filter(|pixel| pixel.0.iter().any(|channel| *channel < 200))
                    .count()
                    > if generated { 0 } else { 20 },
                "{file}"
            );
            if let Some(dir) = std::env::var_os("REMARKABLE_REVIEW_DIR") {
                std::fs::create_dir_all(&dir).unwrap();
                image
                    .save(Path::new(&dir).join(format!(
                        "{}.png",
                        Path::new(file).file_stem().unwrap().to_string_lossy()
                    )))
                    .unwrap();
            }
        }
        let mut bytes = std::fs::read(corpus().join("legacy-generated/simple.rm")).unwrap();
        let page = parse(&bytes, &cancel).unwrap();
        let point = page.layers[0].lines[0].points[0];
        assert_eq!((point.x, point.y), (-602.0, 200.0));
        assert_eq!((point.speed, point.width, point.pressure), (200, 4, 204));
        assert!(parse(&bytes[..bytes.len() - 1], &cancel).is_err());
        // The older layout omits the second reserved stroke-header word.
        // These are explicit synthetic compatibility probes, not real captures.
        bytes.drain(67..71);
        for version in *b"34" {
            bytes[b"reMarkable .lines file, version=".len()] = version;
            let page = parse(&bytes, &cancel).unwrap();
            assert_eq!(page.layers[0].lines[0].points.len(), 3);
        }
        bytes[b"reMarkable .lines file, version=".len()] = b'2';
        assert!(parse(&bytes, &cancel).unwrap_err().contains("version 2"));
    }

    // Image differences are measured against native tablet thumbnails, never
    // snapshots produced by the parser/renderer being qualified.
    #[test]
    fn pencil_ink_matches_independent_tablet_thumbnails() {
        for (name, maximum_error) in [("quicksheets-p2", 20.0), ("quicksheets-p3", 10.0)] {
            let base = corpus().join("tablet-thumbnails");
            let bytes = std::fs::read(base.join(format!("{name}.rm"))).unwrap();
            let (png, width, height) = render(&bytes, 512, &AtomicBool::new(false)).unwrap();
            let expected = image::open(base.join(format!("{name}.png")))
                .unwrap()
                .to_luma8();
            let actual = image::load_from_memory(&png).unwrap().to_luma8();
            assert_eq!((width, height), expected.dimensions());
            let error: u64 = expected
                .pixels()
                .zip(actual.pixels())
                .map(|(a, b)| u64::from(a.0[0].abs_diff(b.0[0])))
                .sum();
            let mae = error as f64 / f64::from(width * height);
            let recall = ink_match(&expected, &actual);
            let precision = ink_match(&actual, &expected);
            eprintln!("{name}: MAE {mae:.3}, recall {recall:.3}, precision {precision:.3}");
            assert!(mae < maximum_error && recall >= 0.90 && precision >= 0.90);
            let white = image::GrayImage::from_pixel(width, height, image::Luma([255]));
            assert_eq!(ink_match(&expected, &white), 0.0, "Blank output must fail");
            if let Some(dir) = std::env::var_os("REMARKABLE_REVIEW_DIR") {
                std::fs::create_dir_all(&dir).unwrap();
                actual
                    .save(Path::new(&dir).join(format!("{name}.png")))
                    .unwrap();
            }
        }
    }

    fn ink_match(source: &image::GrayImage, other: &image::GrayImage) -> f64 {
        let mut count = 0usize;
        let mut matched = 0usize;
        for (x, y, _) in source.enumerate_pixels().filter(|(_, _, p)| p.0[0] < 240) {
            count += 1;
            let neighborhood = image::imageops::crop_imm(
                other,
                x.saturating_sub(2),
                y.saturating_sub(2),
                (x + 3).min(other.width()) - x.saturating_sub(2),
                (y + 3).min(other.height()) - y.saturating_sub(2),
            );
            matched += usize::from(neighborhood.pixels().any(|(_, _, p)| p.0[0] < 240));
        }
        assert!(count > 0, "Reference must contain visible ink");
        matched as f64 / count as f64
    }

    #[test]
    fn deleted_live_records_and_nonfinite_ink_are_rejected() {
        use remarkable_rm::CrdtId;
        let parent = CrdtId { author: 0, seq: 11 };
        let id = CrdtId { author: 1, seq: 10 };
        let mut scene = ScenePreflight::default();
        scene.live.insert((parent, id));
        scene.deleted.push((parent, id, 1));
        assert!(scene.validate().is_err());
        scene.deleted[0].1.author = 2;
        assert!(
            scene.validate().is_ok(),
            "Another author's IDs remain distinct"
        );
        let bytes = std::fs::read(corpus().join("tablet-thumbnails/quicksheets-p3.rm")).unwrap();
        let mut page = parse(&bytes, &AtomicBool::new(false)).unwrap();
        page.layers[0].lines[0].points[0].x = f32::NAN;
        assert!(validate_geometry(&page).is_err());
        assert!(parse(&bytes[..bytes.len() - 1], &AtomicBool::new(false)).is_err());
        let mut excess = remarkable_rm::blocks::FILE_HEADER.to_vec();
        for _ in 0..=MAX_BLOCKS {
            excess.extend_from_slice(&[0, 0, 0, 0, 0, 1, 1, 0]);
        }
        assert!(
            parse(&excess, &AtomicBool::new(false))
                .unwrap_err()
                .contains("block preview limit")
        );
    }

    #[test]
    fn page_budgets_and_header_collisions_fail_explicitly() {
        let cancel = AtomicBool::new(false);
        assert!(!has_signature(b".RMF\0\0\0\0"));
        assert!(parse(b".RMF\0\0\0\0", &cancel).is_err());
        let bytes = std::fs::read(corpus().join("rmscene/Normal_AB.rm")).unwrap();
        assert!(render(&bytes, 0, &cancel).is_err());
        assert!(render(&bytes, 4097, &cancel).is_err());
        assert!(parse(&bytes, &AtomicBool::new(true)).is_err());
        assert!(parse(&vec![0; MAX_PAGE_BYTES + 1], &cancel).is_err());
        let invalid = remarkable_rm::Page {
            paper_size: Some((u32::MAX, 1)),
            ..Default::default()
        };
        assert!(validate_geometry(&invalid).is_err());
    }
}
