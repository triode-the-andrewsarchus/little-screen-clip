//! Small worker-side adapter to the existing normalization, Rough and SVG writers.
use super::{
    geometry,
    resolve::{decoration_fill_color, decoration_stroke_style, StrokeStyle},
    rough,
    svg::push_svg_drawing,
};
use crate::{
    color::{self, CanonicalColor},
    normalization::{self, NormalizedElement, NormalizedElementKind},
    CompatibilityRevision, OutputFormat, SceneDocument, SupplementalAssets,
};
use std::collections::HashMap;

/// Prepared linear styling; geometry regeneration does not shape fonts or decode assets.
#[derive(Clone)]
pub struct LinearPreview {
    source: NormalizedElement,
    background: CanonicalColor,
}
impl LinearPreview {
    /// Prepares editable, unrotated and non-elbowed linear elements on a worker.
    ///
    /// # Errors
    /// Returns an error when normalization cannot complete or the scene exceeds
    /// the interactive budget (2 MiB of source and 2048 elements).
    pub fn prepare(scene: &SceneDocument) -> Result<HashMap<String, Self>, String> {
        if scene.source_bytes() > 2 * 1024 * 1024 || scene.element_occurrence_count() > 2048 {
            return Err("Interactive scene exceeds the source/element budget".into());
        }
        let revision = CompatibilityRevision::REVISION_1;
        let outcome = normalization::normalize_scene_with_assets::<std::convert::Infallible>(
            scene,
            revision,
            crate::catalog::REVISION_1.scene_surface(),
            OutputFormat::Svg,
            &SupplementalAssets::default(),
            false,
            128,
            |_| Ok(()),
        )
        .map_err(|error| format!("Linear preview normalization failed: {error:?}"))?
        .ok_or("Linear preview revision unavailable")?;
        let background =
            color::parse_css(&outcome.view_background_color).ok_or("Invalid drawing background")?;
        Ok(outcome
            .elements
            .into_iter()
            .filter_map(|source| {
                let NormalizedElementKind::Linear(linear) = &source.kind else {
                    return None;
                };
                if source.angle != 0.0 || linear.elbowed || !linear.geometry_available {
                    return None;
                }
                Some((source.source.id().to_owned(), Self { source, background }))
            })
            .collect())
    }

    /// Generates local SVG paths using the exporter's exact seeded geometry and paint.
    ///
    /// # Errors
    /// Returns an error for invalid points or exhausted geometry/work budgets.
    /// Run this synchronous work off the UI thread.
    pub fn svg(&self, points: &[[f64; 2]]) -> Result<String, String> {
        if !(2..=16_384).contains(&points.len())
            || points
                .iter()
                .flatten()
                .any(|v| !v.is_finite() || v.abs() > 1e9)
        {
            return Err("Invalid linear preview points".into());
        }
        let mut source = self.source.clone();
        let NormalizedElementKind::Linear(linear) = &mut source.kind else {
            unreachable!()
        };
        linear.points = points.to_vec();
        let (mut min, mut max) = (points[0], points[0]);
        for point in points {
            for axis in 0..2 {
                min[axis] = min[axis].min(point[axis]);
                max[axis] = max[axis].max(point[axis]);
            }
        }
        source.width = max[0] - min[0];
        source.height = max[1] - min[1];
        let mut budget = rough::GenerationBudget::new(16_384, 2_000_000, 0);
        let drawing =
            geometry::generate_rough(&source, geometry::adjusted_roughness(&source), &mut budget)
                .map_err(|error| format!("Linear preview geometry failed: {error:?}"))?
                .drawing;
        let stroke = color::parse_css(&source.stroke_color).ok_or("Invalid linear stroke")?;
        let fill = (source.background_color != "transparent")
            .then(|| color::parse_css(&source.background_color))
            .flatten();
        let style = match source.stroke_style.as_str() {
            "dashed" => StrokeStyle::Dashed,
            "dotted" => StrokeStyle::Dotted,
            _ => StrokeStyle::Solid,
        };
        let width = source.stroke_width
            + if style == StrokeStyle::Solid {
                0.0
            } else {
                0.5
            };
        let opacity = source.opacity / 100.0;
        let mut svg = String::from("<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"1\" height=\"1\" overflow=\"visible\"><g stroke-linecap=\"round\">");
        push_svg_drawing(
            &mut svg,
            &drawing,
            stroke,
            fill,
            width,
            style,
            source.stroke_width,
            opacity,
        )
        .map_err(|error| format!("Linear preview encoding failed: {error:?}"))?;
        for decoration in &drawing.decorations {
            let fill = decoration_fill_color(decoration, stroke, self.background);
            let (style, dash_width) = decoration_stroke_style(decoration, source.stroke_width);
            push_svg_drawing(
                &mut svg,
                &decoration.drawing,
                stroke,
                Some(fill),
                width,
                style,
                dash_width,
                opacity,
            )
            .map_err(|error| format!("Linear preview encoding failed: {error:?}"))?;
        }
        svg.push_str("</g></svg>");
        Ok(svg)
    }
}

/// Measures unwrapped authored text using the exporter's bundled faces and shaping.
/// Run off the UI thread; no scene normalization, output rendering or I/O occurs.
pub fn measure_text(
    text: &str,
    font_family: i32,
    font_size: f64,
    line_height: f64,
) -> Result<[f64; 2], String> {
    if text.chars().count() > 4096
        || text.contains('\0')
        || !font_size.is_finite()
        || !(1.0..=512.0).contains(&font_size)
        || !line_height.is_finite()
        || !(0.5..=4.0).contains(&line_height)
    {
        return Err("Text measurement exceeds the character/font limits".into());
    }
    super::text::measure_unwrapped(text, font_family, font_size, line_height)
        .map_err(|error| format!("Text cannot be measured with the selected bundled font: {error:?}; use Source for unsupported text"))
}
