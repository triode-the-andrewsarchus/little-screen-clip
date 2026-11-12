//! Opt-in embedded raster admission. No path or nested SVG resolver is installed.
use image::{ImageFormat, ImageReader};
use resvg::usvg::{ImageHrefResolver, ImageKind};
use std::{
    io::Cursor,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Policy {
    Block,
    EmbeddedRaster,
}
#[derive(Default)]
pub(super) struct Budget {
    pixels: AtomicU64,
    pub rejected: AtomicBool,
}
const MAX_BYTES: usize = 2 * 1024 * 1024;
const MAX_PIXELS: u64 = 8 * 1024 * 1024;

pub(super) fn resolver(policy: Policy) -> (ImageHrefResolver, Arc<Budget>) {
    let budget = Arc::new(Budget::default());
    let data_budget = budget.clone();
    let path_budget = budget.clone();
    (
        ImageHrefResolver {
            resolve_data: Box::new(move |mime, data, _| {
                let image = (policy == Policy::EmbeddedRaster)
                    .then(|| decode(mime, &data, &data_budget))
                    .flatten();
                if image.is_none() {
                    data_budget.rejected.store(true, Ordering::Relaxed);
                }
                image
            }),
            resolve_string: Box::new(move |_, _| {
                path_budget.rejected.store(true, Ordering::Relaxed);
                None
            }),
        },
        budget,
    )
}
// usvg may reject a malformed data URL before calling either resolver.
// Retain that omission in our diagnostics instead of silently dropping it.
pub(super) fn invalid_declarations(document: &roxmltree::Document<'_>) -> bool {
    document
        .descendants()
        .filter(|node| node.has_tag_name("image"))
        .any(|node| {
            let href = node
                .attribute("href")
                .or_else(|| node.attribute(("http://www.w3.org/1999/xlink", "href")));
            !href.is_some_and(|href| {
                data_url::DataUrl::process(href).is_ok_and(|url| url.decode_to_vec().is_ok())
            })
        })
}

fn decode(mime: &str, bytes: &[u8], budget: &Budget) -> Option<ImageKind> {
    let format = match mime {
        "image/png" => ImageFormat::Png,
        "image/jpeg" | "image/jpg" => ImageFormat::Jpeg,
        "image/webp" => ImageFormat::WebP,
        _ => return None,
    };
    if bytes.len() > MAX_BYTES {
        return None;
    }
    let (width, height) = ImageReader::with_format(Cursor::new(bytes), format)
        .into_dimensions()
        .ok()?;
    if width == 0 || height == 0 || width > 2048 || height > 2048 {
        return None;
    }
    let pixels = u64::from(width) * u64::from(height);
    // Reserve before decode. Rejected or failed inputs cannot reclaim a budget
    // and repeatedly ask the parser for fresh expensive allocations.
    budget
        .pixels
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |used| {
            used.checked_add(pixels)
                .filter(|total| *total <= MAX_PIXELS)
        })
        .ok()?;
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(2048);
    limits.max_image_height = Some(2048);
    limits.max_alloc = Some(32 * 1024 * 1024);
    reader.limits(limits);
    let image = reader.decode().ok()?.to_rgba8();
    let mut output = Cursor::new(Vec::new());
    image.write_to(&mut output, ImageFormat::Png).ok()?;
    let output = output.into_inner();
    if output.len() > MAX_BYTES {
        return None;
    }
    Some(ImageKind::PNG(Arc::new(output)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;
    fn paper(href: &str) -> String {
        format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="12" height="12"><image xlink:href="{href}" width="12" height="12"/></svg>"#
        )
    }
    fn red() -> Vec<u8> {
        let mut image = tiny_skia::Pixmap::new(12, 12).unwrap();
        image.fill(tiny_skia::Color::from_rgba8(255, 0, 0, 255));
        image.encode_png().unwrap()
    }
    #[test]
    fn opt_in_rasters_survive_outline_composition_and_final_rasterization() {
        let bytes = base64::engine::general_purpose::STANDARD.encode(red());
        let source = paper(&format!("data:image/png;base64,{bytes}"));
        let (svg, warnings) = super::super::compose_svg(
            [source.clone()],
            super::super::default_fonts(),
            "Noto Sans CJK JP",
            Policy::EmbeddedRaster,
            super::super::ViewportPolicy::FitContent,
        )
        .unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        let rendered = super::super::rasterize_svg(svg, true, Policy::EmbeddedRaster).unwrap();
        let image = image::load_from_memory(&rendered.png).unwrap().to_rgba8();
        assert!(
            image
                .pixels()
                .any(|pixel| pixel.0[0] > 240 && pixel.0[1] < 10)
        );
        let blocked = super::super::rasterize_svg(source, true, Policy::Block).unwrap();
        assert_eq!(blocked.warnings.len(), 1);
        assert!(
            image::load_from_memory(&blocked.png)
                .unwrap()
                .to_rgba8()
                .pixels()
                .all(|pixel| pixel.0[..3] == [255, 255, 255])
        );
    }
    #[test]
    fn paths_network_nested_svg_and_expansion_requests_stay_unavailable() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("red.png");
        std::fs::write(&path, red()).unwrap();
        for href in [
            path.to_str().unwrap(),
            "https://example.invalid/picture.png",
            "data:image/png;base64,!",
            "data:image/svg+xml,%3Csvg%20xmlns='http://www.w3.org/2000/svg'%3E%3C/svg%3E",
        ] {
            let rendered =
                super::super::rasterize_svg(paper(href), true, Policy::EmbeddedRaster).unwrap();
            assert_eq!(rendered.warnings.len(), 1, "{href}");
            assert!(
                image::load_from_memory(&rendered.png)
                    .unwrap()
                    .to_rgba8()
                    .pixels()
                    .all(|pixel| pixel.0[..3] == [255, 255, 255])
            );
        }
        let budget = Budget {
            pixels: AtomicU64::new(MAX_PIXELS),
            ..Default::default()
        };
        assert!(decode("image/png", &red(), &budget).is_none());
        assert!(decode("image/png", &vec![0; MAX_BYTES + 1], &Budget::default()).is_none());
    }
}
