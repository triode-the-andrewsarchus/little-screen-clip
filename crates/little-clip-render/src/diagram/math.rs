//! Equation layout stays in the bounded helper; only em metrics cross to hosts.
use super::MathLayout;

pub(super) fn svg(
    source: &str,
    display: bool,
    foreground: [u8; 3],
) -> Result<(String, MathLayout), String> {
    static FALLBACKS: std::sync::LazyLock<Vec<mathra::font::FallbackFont>> =
        std::sync::LazyLock::new(|| {
            [
                little_clip_core::font_assets::CJK_FONT_BYTES,
                little_clip_core::font_assets::EMOJI_FONT_BYTES,
            ]
            .into_iter()
            .filter_map(|bytes| mathra::font::FallbackFont::load(bytes.to_vec()).ok())
            .collect()
        });
    let options = mathra::Options {
        display,
        strict: true,
        fallback_fonts: FALLBACKS.clone(),
        ..Default::default()
    };
    let output = mathra::convert(source, &options).map_err(|error| error.to_string())?;
    if output.svg.len() > super::MAX_SVG {
        return Err("Equation SVG exceeds the preview budget".into());
    }
    let document = roxmltree::Document::parse(&output.svg).map_err(|error| error.to_string())?;
    let viewbox = document
        .root_element()
        .attribute("viewBox")
        .ok_or("Equation has no viewBox")?;
    let values: Vec<f32> = viewbox
        .split_whitespace()
        .map(str::parse)
        .collect::<Result<_, _>>()
        .map_err(|_| "Invalid equation viewBox")?;
    let [_, top, width, height] = values.as_slice() else {
        return Err("Invalid equation viewBox".into());
    };
    let em = options.font.get().units_per_em();
    let layout = MathLayout {
        width_em: width / em,
        height_em: height / em,
        baseline_em: -top / em,
    };
    if !layout.valid() {
        return Err("Equation dimensions exceed the preview budget".into());
    }
    // The engine's viewBox uses font units, not pixels. A physical outer viewport
    // supplies an em scale without relaxing the shared SVG dimension limits.
    // Inline color overrides the engine's standalone-root CSS; explicit colored
    // groups retain their own fill. Empty/phantom equations still get a valid raster.
    let width = (layout.width_em * 32.0).max(1.0);
    let height = (layout.height_em * 32.0).max(1.0);
    let [red, green, blue] = foreground;
    let svg = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" viewBox="0 0 {width} {height}" font-size="32" style="color:#{red:02x}{green:02x}{blue:02x}">{}</svg>"##,
        output.svg
    );
    Ok((svg, layout))
}

#[cfg(test)]
mod tests {
    use super::*;
    use little_clip_core::diagram::Format;

    #[test]
    fn equations_retain_baselines_transparency_theme_and_explicit_colors() {
        let rendered = super::super::render_inline(
            Format::MathInline,
            r"x+\color{red}{y}+\frac{1}{2}",
            Some([30, 210, 90]),
        )
        .unwrap();
        let layout = rendered.math.unwrap();
        assert!(layout.valid());
        assert!(layout.baseline_em > 0.5 && layout.baseline_em < layout.height_em);
        let image = image::load_from_memory(&rendered.png).unwrap().to_rgba8();
        assert!(image.pixels().any(|p| p[3] == 0));
        assert!(
            image
                .pixels()
                .any(|p| p[3] > 200 && p[1] > 180 && p[0] < 60)
        );
        assert!(
            image
                .pixels()
                .any(|p| p[3] > 200 && p[0] > 200 && p[1] < 60)
        );
    }

    #[test]
    fn worker_layout_validation_rejects_missing_nonfinite_and_out_of_bounds_metrics() {
        let mut rendered = super::super::render_inline(Format::Math, "x", None).unwrap();
        rendered.validate(Format::Math).unwrap();
        assert!(rendered.validate(Format::Pikchr).is_err());
        let valid = rendered.math.unwrap();
        for layout in [
            MathLayout {
                width_em: f32::NAN,
                ..valid
            },
            MathLayout {
                baseline_em: valid.height_em + 1.0,
                ..valid
            },
            MathLayout {
                height_em: 513.0,
                ..valid
            },
            MathLayout {
                width_em: -1.0,
                ..valid
            },
        ] {
            rendered.math = Some(layout);
            assert!(rendered.validate(Format::Math).is_err());
        }
        rendered.math = None;
        assert!(rendered.validate(Format::Math).is_err());
    }

    #[test]
    fn matrices_limits_and_spacing_have_finite_layouts() {
        for source in [
            r"\begin{pmatrix}a&b\\c&d\end{pmatrix}",
            r"\sum_{n=1}^{\infty}\frac{1}{n^2}",
            r"\quad",
            r"\phantom{x}",
            "",
        ] {
            let (svg, layout) = svg(source, true, [17; 3]).unwrap();
            assert!(layout.valid(), "{source}");
            super::super::rasterize_svg(svg, false).unwrap();
        }
    }

    #[test]
    fn unsupported_commands_and_malformed_equations_report_errors() {
        for source in [
            r"\frac{1}",
            r"\input{/etc/passwd}",
            r"\write18{touch sentinel}",
            r"\unknowncommand{x}",
        ] {
            assert!(svg(source, true, [17; 3]).is_err(), "{source}");
        }
    }
}
