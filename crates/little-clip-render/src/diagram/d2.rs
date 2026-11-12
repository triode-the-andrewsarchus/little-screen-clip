//! D2's native compiler/layout and resolved scene, without CLI resource loaders.
use super::{HEIGHT, Rendered, WIDTH};
use d2_rs_scenebuild as scene;
use image::ImageEncoder;
use std::sync::Arc;

#[derive(Debug)]
struct PreviewImages;
impl scene::ImageResolver for PreviewImages {
    fn resolve(
        &self,
        _source: &str,
    ) -> Result<Option<scene::ImageResource>, scene::ImageResolveError> {
        // Never instantiate the CLI's filesystem/HTTP image resolver. Preserve
        // diagram geometry using the engine's explicit unavailable placeholder.
        Err(scene::ImageResolveError {
            unavailable: true,
            message: "Image loading is disabled in diagram previews".into(),
        })
    }
}

pub(super) fn render(source: &str) -> Result<Rendered, String> {
    let (graph, layout, labels, styles, config) =
        d2_rs_wasm::source_layout_native(source, r#"{"max_concurrency":1,"seeds":[1]}"#)?;
    let config = config.unwrap_or_default();
    if config.isometric == Some(true) {
        return Err("Isometric D2 previews are not available yet; use a flat layout".into());
    }
    let sketch = config.sketch.unwrap_or(false);
    let options = d2_rs_render::RenderOptions {
        pad: config.pad.unwrap_or(100) as f64,
        theme_id: config.theme_id,
        theme_overrides: config.theme_overrides,
        center: config.center.unwrap_or(false),
        ..Default::default()
    };
    let svg = String::from_utf8(d2_rs_render::render_d2svg(
        &graph, &layout, &labels, &styles, &options, sketch,
    )?)
    .map_err(|error| error.to_string())?;
    // Reuse SVG structural/dimension budgets and its disabled image resolvers.
    // PNG is painted from D2's native scene: its Markdown/TeX labels cannot be
    // faithfully rasterized by treating SVG foreignObject as ordinary SVG text.
    let (_, mut warnings) = super::parse_svg(&svg, super::default_fonts(), "Noto Sans CJK JP")?;
    warnings.extend(layout.report.warnings.iter().cloned());
    let target =
        d2_rs_render::target_diagram_json(&graph, &layout, &labels, &styles, &options, sketch)?;
    let diagram: d2_rs_target::Diagram =
        serde_json::from_slice(&target).map_err(|error| error.to_string())?;
    let document = scene::build(&diagram, &scene_options(&options, sketch)?)
        .map_err(|error| error.to_string())?;
    let frame = d2_rs_raster::render(
        &document,
        &frame_options(document.logical_width, document.logical_height)?,
    )
    .map_err(|error| error.to_string())?;
    let (width, height) = (frame.width() as u32, frame.height() as u32);
    if frame.stride != width as usize * 4 || frame.pix.len() != width as usize * height as usize * 4
    {
        return Err("Invalid D2 raster stride".into());
    }
    let mut png = Vec::new();
    image::codecs::png::PngEncoder::new(&mut png)
        .write_image(&frame.pix, width, height, image::ExtendedColorType::Rgba8)
        .map_err(|error| error.to_string())?;
    let result = Rendered {
        svg,
        png,
        width,
        height,
        warnings: super::bounded_warnings(warnings),
        math: None,
    };
    result.validate(little_clip_core::diagram::Format::D2)?;
    Ok(result)
}

fn scene_options(
    options: &d2_rs_render::RenderOptions,
    sketch: bool,
) -> Result<scene::Options, String> {
    let fonts = scene::BundledFallbackResolver::new(
        None,
        scene::BundledFallbackLimits {
            max_requested_runes: 64 * 1024,
            max_bundled_bytes: 8 * 1024 * 1024,
            max_resolved_bytes: 16 * 1024 * 1024,
        },
    )?;
    let overrides: std::collections::BTreeMap<_, _> = options
        .theme_overrides
        .iter()
        .map(|(key, value)| (key.to_ascii_lowercase(), value))
        .collect();
    let theme_overrides =
        serde_json::from_value(serde_json::to_value(overrides).map_err(|error| error.to_string())?)
            .map_err(|error| error.to_string())?;
    Ok(scene::Options {
        pad: Some(options.pad as i64),
        theme_id: options.theme_id,
        theme_overrides: Some(theme_overrides),
        sketch,
        max_nodes: 16_384,
        max_path_commands: 256 * 1024,
        link_budget: scene::LinkBudget {
            max_regions: 1024,
            max_string_bytes: 256 * 1024,
        },
        sketch_budget: scene::SketchBudget {
            max_operation_sets: 16_384,
            max_operations: 256 * 1024,
            max_path_commands: 256 * 1024,
        },
        assets: Some(scene::AssetOptions {
            resolver_impl: Some(Arc::new(PreviewImages)),
            // Native TeX labels also pass through this SVG importer. Supplying
            // finite limits admits generated vector labels without granting
            // the resolver access to any external image bytes.
            svg_import_limits: scene::SvgImportLimits {
                max_bytes: super::MAX_SVG as i64,
                max_depth: 64,
                max_elements: 16_384,
                max_attributes: 64 * 1024,
                max_attribute_bytes: 2 * 1024 * 1024,
                max_path_commands: 256 * 1024,
                max_transform_functions: 16_384,
                max_use_depth: 32,
                max_resources: 8192,
            },
            svg_import_budget: scene::SvgImportBudget {
                max_source_bytes: 4 * 1024 * 1024,
                max_elements: 32 * 1024,
                max_attributes: 128 * 1024,
                max_attribute_bytes: 4 * 1024 * 1024,
                max_path_commands: 256 * 1024,
                max_transform_functions: 32 * 1024,
                max_declared_resources: 16_384,
                max_expanded_use_instances: 16_384,
            },
            ..Default::default()
        }),
        fonts: Some(scene::FontFallbackOptions {
            resolver_impl: Some(Arc::new(fonts)),
            max_assets: 32,
            max_bytes: 32 * 1024 * 1024,
            max_runes_per_text: 64 * 1024,
            max_total_runes: 256 * 1024,
            max_coverage_checks: 1024 * 1024,
            max_font_faces_per_text: 16,
            max_shaping_runs: 16_384,
            max_shaped_glyphs: 256 * 1024,
            ..Default::default()
        }),
        ..Default::default()
    })
}

fn frame_options(width: f64, height: f64) -> Result<d2_rs_raster::FrameOptions, String> {
    if !width.is_finite() || !height.is_finite() || width <= 0.0 || height <= 0.0 {
        return Err("D2 scene has invalid dimensions".into());
    }
    Ok(d2_rs_raster::FrameOptions {
        scale: ((WIDTH - 1) as f64 / width)
            .min((HEIGHT - 1) as f64 / height)
            .min(2.0),
        time: 0,
        background: Some(d2_rs_raster::Nrgba::new(255, 255, 255, 255)),
        max_width: WIDTH as i64,
        max_height: HEIGHT as i64,
        max_pixels: (WIDTH * HEIGHT) as i64,
        max_nodes: 16_384,
        max_depth: 64,
        max_path_commands: 256 * 1024,
        max_text_runes_per_run: 64 * 1024,
        max_font_faces_per_text: 16,
        max_text_coverage_checks: 1024 * 1024,
        max_text_shaping_runs: 16_384,
        max_animation_tracks: 16_384,
        max_animation_keyframes: 64 * 1024,
        max_assets: 64,
        max_asset_bytes: 64 * 1024 * 1024,
        max_decoded_asset_bytes: 64 * 1024 * 1024,
        max_import_depth: 64,
        max_offscreen_bytes: 128 * 1024 * 1024,
        max_even_odd_clip_work: 64 * 1024 * 1024,
        max_scanline_work: 512 * 1024 * 1024,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn d2_renders_native_engines_shapes_markdown_math_and_sketch() {
        for source in [
            "direction: right\na -> b: hello",
            "vars: { d2-config: { layout-engine: elk } }\na -> b",
            "vars: { d2-config: { layout-engine: tala } }\na -> b",
            "a: {shape: sql_table; id: int {constraint: primary_key}; name: varchar}\nb: {shape: class; +field: string; method(): bool}\na -> b",
            "x: { shape: text; label: |md\n# Heading\n**Bold** label\n|\n}",
            "equation: |latex x^2 + y^2 = z^2 |",
            "vars: { d2-config: { sketch: true } }\na -> b",
            "shape: sequence_diagram\na -> b: request\nb -> a: response",
            "container: { grid-columns: 2; a; b; c; d }",
            "vars: { d2-config: {theme-id: 300; theme-overrides: {B1: \"#a12929\"}} }\na -> b",
        ] {
            let rendered = render(source).unwrap_or_else(|error| panic!("{source}: {error}"));
            let image = image::load_from_memory(&rendered.png).unwrap().to_rgba8();
            assert!(
                image
                    .pixels()
                    .any(|pixel| pixel[0] < 180 && pixel[1] < 180 && pixel[2] < 180),
                "{source}"
            );
        }
    }

    #[test]
    fn d2_errors_preserve_diagnostics_and_imports_have_no_filesystem_capability() {
        for source in [
            "a: {",
            "...@/etc/passwd",
            "a: @missing.d2",
            "vars: { d2-config: { isometric: true } }\na",
        ] {
            assert!(render(source).is_err(), "{source}");
        }
    }

    #[test]
    fn valid_local_imports_are_denied_and_large_diagrams_fit_the_raster_budget() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("ordinary.d2");
        std::fs::write(&path, "a -> b: readable local fixture").unwrap();
        let error = render(&format!("...@{}", path.display()))
            .err()
            .expect("a valid local import must still be denied");
        assert!(
            error.to_ascii_lowercase().contains("import") || error.contains("opener"),
            "{error}"
        );
        let rendered = render("a: {width: 5000; height: 600}\na -> b").unwrap();
        assert!(rendered.width <= WIDTH && rendered.height <= HEIGHT);
        assert!(rendered.width > 1000);
    }

    #[test]
    fn d2_image_requests_are_inert_and_reported() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("private.png");
        image::RgbaImage::from_pixel(8, 8, image::Rgba([255, 0, 0, 255]))
            .save(&path)
            .unwrap();
        for source in [
            format!("a: {{shape: image; icon: {}}}", path.display()),
            "a: {shape: image; icon: https://example.invalid/secret.png}".into(),
        ] {
            let rendered = render(&source).unwrap();
            let image = image::load_from_memory(&rendered.png).unwrap().to_rgba8();
            assert!(
                !image.pixels().any(|p| p[0] > 240 && p[1] < 10 && p[2] < 10),
                "private red image must not enter the preview"
            );
            assert!(
                rendered
                    .warnings
                    .iter()
                    .any(|warning| warning.contains("images"))
            );
        }
    }
}
