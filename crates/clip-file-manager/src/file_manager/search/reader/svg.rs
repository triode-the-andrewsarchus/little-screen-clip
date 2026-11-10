//! Static vector thumbnails with bundled fonts and no external resource loading.
use super::{Engine, Prepared, STANDARD};
use resvg::usvg::{self, TreeParsing, TreeTextToPath};
use std::sync::{
    Arc, LazyLock,
    atomic::{AtomicBool, Ordering},
};

static FONTS: LazyLock<usvg::fontdb::Database> = LazyLock::new(|| {
    let mut fonts = usvg::fontdb::Database::new();
    // Use the same bundled fallback as the application; never scan SVG-selected
    // font paths or host font directories in this content reader.
    fonts.load_font_data(little_clip_render::CJK_FONT_BYTES.to_vec());
    fonts.set_sans_serif_family("Noto Sans CJK JP");
    fonts.set_serif_family("Noto Sans CJK JP");
    fonts.set_monospace_family("Noto Sans CJK JP");
    fonts
});

pub(super) fn prepare(bytes: &[u8]) -> Result<Prepared, String> {
    let omitted = Arc::new(AtomicBool::new(false));
    let observed = omitted.clone();
    let embedded = omitted.clone();
    let raster = usvg::ImageHrefResolver::default_data_resolver();
    let options = usvg::Options {
        font_family: "Noto Sans CJK JP".into(),
        image_href_resolver: usvg::ImageHrefResolver {
            // usvg 0.37 resets the resolver while parsing embedded SVG. Permit
            // only raster MIME types, whose decoders cannot resolve SVG hrefs.
            resolve_data: Box::new(move |mime, data, options| {
                if matches!(mime, "image/jpg" | "image/jpeg" | "image/png" | "image/gif") {
                    raster(mime, data, options)
                } else {
                    embedded.store(true, Ordering::Relaxed);
                    None
                }
            }),
            resolve_string: Box::new(move |_, _| {
                observed.store(true, Ordering::Relaxed);
                None
            }),
        },
        ..Default::default()
    };
    let mut tree = usvg::Tree::from_data(bytes, &options).map_err(|e| e.to_string())?;
    if tree.has_text_nodes() {
        tree.convert_text(&FONTS);
    }
    let scale = (1024.0 / tree.size.width())
        .min(1024.0 / tree.size.height())
        .min(1.0);
    let width = ((tree.size.width() * scale).ceil() as u32).clamp(1, 1024);
    let height = ((tree.size.height() * scale).ceil() as u32).clamp(1, 1024);
    let mut pixels =
        tiny_skia::Pixmap::new(width, height).ok_or("Invalid SVG thumbnail dimensions")?;
    // The model consumes RGB. Give transparent artwork a neutral background
    // so black paths/text do not disappear when its alpha channel is discarded.
    pixels.fill(tiny_skia::Color::WHITE);
    resvg::Tree::from_usvg(&tree).render(
        tiny_skia::Transform::from_scale(scale, scale),
        &mut pixels.as_mut(),
    );
    let png = pixels.encode_png().map_err(|e| e.to_string())?;
    Ok(Prepared {
        data: Some(STANDARD.encode(png)),
        modality: "image".into(),
        location: "static SVG image".into(),
        coverage: if omitted.load(Ordering::Relaxed) {
            "partial: SVG resources omitted"
        } else {
            "complete"
        }
        .into(),
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn embedded_svg_cannot_reset_the_external_resource_resolver() {
        let inner = STANDARD.encode(br#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><image href="/must-not-resolve-outside.svg" width="10" height="10"/></svg>"#);
        let markup = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="10" height="10"><image xlink:href="data:image/svg+xml;base64,{inner}" width="10" height="10"/></svg>"#
        );
        assert!(
            prepare(markup.as_bytes())
                .unwrap()
                .coverage
                .contains("SVG resources omitted")
        );
    }
    #[test]
    fn huge_vector_canvas_produces_only_a_bounded_raster() {
        let result = prepare(br#"<svg xmlns="http://www.w3.org/2000/svg" width="1000000" height="500000"><rect width="1000000" height="500000" fill="red"/></svg>"#).unwrap();
        let image = image::load_from_memory(&STANDARD.decode(result.data.unwrap()).unwrap())
            .unwrap()
            .to_rgba8();
        assert_eq!(image.dimensions(), (1024, 512));
        assert_eq!(image.get_pixel(500, 200).0, [255, 0, 0, 255]);
    }
    #[test]
    fn external_images_are_never_loaded_and_omission_is_disclosed() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("outside.png");
        image::RgbaImage::from_pixel(10, 10, image::Rgba([255u8, 0, 0, 255]))
            .save(&path)
            .unwrap();
        let markup = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="10" height="10"><rect width="10" height="10" fill="lime"/><image xlink:href="{}" width="10" height="10"/><image xlink:href="https://example.invalid/never-fetch.png" width="10" height="10"/></svg>"#,
            path.display()
        );
        let result = prepare(markup.as_bytes()).unwrap();
        assert!(result.coverage.contains("SVG resources omitted"));
        let image = image::load_from_memory(&STANDARD.decode(result.data.unwrap()).unwrap())
            .unwrap()
            .to_rgba8();
        assert_eq!(image.get_pixel(5, 5).0, [0, 255, 0, 255]);
    }
    #[test]
    fn text_uses_bundled_fonts_and_malformed_input_fails() {
        let result = prepare(br#"<svg xmlns="http://www.w3.org/2000/svg" width="160" height="40"><text x="2" y="30" fill="black" font-size="25">Hello</text></svg>"#).unwrap();
        let image = image::load_from_memory(&STANDARD.decode(result.data.unwrap()).unwrap())
            .unwrap()
            .to_rgba8();
        assert!(image.pixels().any(|pixel| pixel.0[..3] == [255, 255, 255]));
        assert!(image.pixels().any(|pixel| pixel.0[0] < 100));
        assert!(prepare(b"not XML").is_err());
    }
}
