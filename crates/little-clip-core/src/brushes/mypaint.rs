use super::{
    BrushDynamics, BrushFormat, BrushLibrary, BrushPreset, BrushStrokeParams, BrushTipKind,
    DabCurve, ParametricShape, bounded_label,
};
use hokusai_core::{Brush, BrushInput, BrushSetting};

pub(super) fn parse_mypaint(bytes: &[u8], fallback_name: &str) -> Result<BrushLibrary, String> {
    let text = std::str::from_utf8(bytes).map_err(|_| "Invalid UTF-8 in MyPaint brush")?;
    let brush =
        hokusai_brush::from_str(text).map_err(|error| format!("Invalid MyPaint brush: {error}"))?;
    let params = stroke_params(&brush);
    let aspect_ratio = brush
        .get(BrushSetting::EllipticalDabRatio)
        .base_value
        .clamp(1.0, 10.0);
    let name = brush
        .parent_brush_name
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .map(bounded_label)
        .unwrap_or(fallback_name)
        .to_owned();
    let detail = format!(
        "Ø {:.1}px · hardness {:.2}",
        params.size_px, params.hardness
    );

    Ok(BrushLibrary {
        format: BrushFormat::MyPaint,
        brushes: vec![BrushPreset {
            name,
            tip: BrushTipKind::Parametric {
                shape: ParametricShape::Circle,
                hardness: params.hardness,
                aspect_ratio,
                angle_deg: params.angle_deg,
                spikes: 2,
            },
            params,
            detail,
        }],
        total_count: 1,
    })
}

fn stroke_params(brush: &Brush) -> BrushStrokeParams {
    use BrushSetting::*;
    let base = |setting| finite(brush.get(setting).base_value);
    let radius = base(Radius).exp().clamp(0.2, 1000.0);
    let aspect = base(EllipticalDabRatio).clamp(1.0, 10.0);
    let angle = base(EllipticalDabAngle).clamp(-360.0, 360.0);
    let hardness = base(Hardness).clamp(0.0, 1.0);
    let strength = base(OpaqueLinearize).clamp(0.0, 1.0);
    let pressure =
        |setting, p| base(setting) + input_offset(brush, setting, BrushInput::Pressure, p);
    let size = |p| pressure(Radius, p).exp().clamp(0.2, 1000.0) / radius;
    let actual_dabs = |p| pressure(DabsPerActualRadius, p).max(0.0);
    let basic_dabs = |p| pressure(DabsPerBasicRadius, p).max(0.0);

    // Preview the preset's dab mappings without executing its stroke loop.
    BrushStrokeParams {
        size_px: radius * 2.0,
        angle_deg: angle,
        hardness,
        dynamics: BrushDynamics {
            size: Some(DabCurve::sampled(size)),
            flow: Some(DabCurve::sampled(|p| {
                let opaque = (pressure(Opaque, p) * pressure(OpaqueMultiply, p)).clamp(0.0, 1.0);
                let overlap = (2.0 * (actual_dabs(p) + basic_dabs(p))).clamp(1.0, 128.0);
                1.0 - (1.0 - opaque).powf(1.0 / (1.0 + strength * (overlap - 1.0)))
            })),
            spacing: Some(DabCurve::sampled(|p| {
                0.5 / (actual_dabs(p) + basic_dabs(p) * size(p)).max(0.1)
            })),
            angle: Some(DabCurve::sampled(|p| {
                pressure(EllipticalDabAngle, p) - angle
            })),
            random_angle: Some(DabCurve::sampled(|r| {
                input_offset(brush, EllipticalDabAngle, BrushInput::Random, r).clamp(-360.0, 360.0)
            })),
            hardness: Some(DabCurve::sampled(|p| pressure(Hardness, p).clamp(0.0, 1.0))),
            aspect: Some(DabCurve::sampled(|p| {
                pressure(EllipticalDabRatio, p).clamp(1.0, 10.0) / aspect
            })),
            scatter: Some(DabCurve::sampled(|p| {
                pressure(OffsetByRandom, p).clamp(0.0, 4.0)
            })),
            scatter_along_stroke: true,
            size_jitter: base(RadiusByRandom).clamp(0.0, 1.0),
            ..Default::default()
        },
        ..Default::default()
    }
}

fn input_offset(brush: &Brush, setting: BrushSetting, input: BrushInput, value: f32) -> f32 {
    brush
        .get(setting)
        .inputs
        .iter()
        .find(|mapping| mapping.input == input)
        .map_or(0.0, |mapping| finite(mapping.eval(value)))
}

fn finite(value: f32) -> f32 {
    if value.is_finite() {
        value.clamp(-10_000.0, 10_000.0)
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uses_additive_pressure_and_random_dab_mappings() {
        let bytes = br#"{"version":3,"settings":{"radius_logarithmic":{"base_value":2.0,"inputs":{"pressure":[[0,-1],[1,0]]}},"opaque":{"base_value":0.5,"inputs":{}},"opaque_multiply":{"base_value":0,"inputs":{"pressure":[[0,0],[1,1]]}},"elliptical_dab_angle":{"base_value":90,"inputs":{"random":[[0,-180],[1,180]]}}}}"#;
        let library = parse_mypaint(bytes, "Test").unwrap();
        let dynamics = &library.brushes[0].params.dynamics;
        let size = dynamics.size.as_ref().unwrap();
        let flow = dynamics.flow.as_ref().unwrap();
        assert!(size.at(0.0) < size.at(1.0));
        assert_eq!(flow.at(0.0), 0.0);
        assert!(flow.at(1.0) > 0.0);
        assert_eq!(dynamics.random_angle.as_ref().unwrap().at(0.0), -180.0);
    }
}
