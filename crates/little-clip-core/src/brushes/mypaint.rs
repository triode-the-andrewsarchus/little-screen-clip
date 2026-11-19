use super::{
    BrushFormat, BrushLibrary, BrushPreset, BrushStrokeParams, BrushTipKind, ParametricShape,
    bounded_label,
};
use hokusai_core::BrushSetting;

pub(super) fn parse_mypaint(bytes: &[u8], fallback_name: &str) -> Result<BrushLibrary, String> {
    let text = std::str::from_utf8(bytes).map_err(|_| "Invalid UTF-8 in MyPaint brush")?;
    let brush =
        hokusai_brush::from_str(text).map_err(|error| format!("Invalid MyPaint brush: {error}"))?;

    let radius_log = brush.get(BrushSetting::Radius).base_value;
    let radius = radius_log.exp().clamp(0.5, 400.0);
    let hardness = brush
        .get(BrushSetting::Hardness)
        .base_value
        .clamp(0.02, 1.0);
    let opaque = brush.get(BrushSetting::Opaque).base_value.clamp(0.05, 1.0);
    let dabs_per_radius = brush
        .get(BrushSetting::DabsPerActualRadius)
        .base_value
        .max(0.2);
    let spacing = (1.0_f32 / (dabs_per_radius * 2.0)).clamp(0.05, 1.5);
    let aspect_ratio = brush
        .get(BrushSetting::EllipticalDabRatio)
        .base_value
        .clamp(1.0, 10.0);
    let angle_deg = brush.get(BrushSetting::EllipticalDabAngle).base_value;
    let scatter = brush
        .get(BrushSetting::OffsetByRandom)
        .base_value
        .clamp(0.0, 2.0)
        * 0.35;

    let name = brush
        .parent_brush_name
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .map(bounded_label)
        .unwrap_or(fallback_name)
        .to_owned();
    let detail = format!("Ø {:.1}px · hardness {:.2}", radius * 2.0, hardness);

    Ok(BrushLibrary {
        format: BrushFormat::MyPaint,
        brushes: vec![BrushPreset {
            name,
            tip: BrushTipKind::Parametric {
                shape: ParametricShape::Circle,
                hardness,
                aspect_ratio,
                angle_deg,
                spikes: 2,
            },
            params: BrushStrokeParams {
                size_px: radius * 2.0,
                spacing,
                opacity: opaque,
                angle_deg,
                scatter,
                hardness,
                flow: opaque,
            },
            detail,
        }],
        total_count: 1,
    })
}
