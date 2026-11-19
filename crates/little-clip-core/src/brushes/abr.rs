use super::{BrushDynamics, BrushStrokeParams, PressureCurve};
use brushkit::abr::{BrushDescriptor, SampledBrush, parse_abr_all_deferred_without_patterns};

pub(super) fn settings(
    bytes: &[u8],
    indices: &[usize],
) -> Result<(usize, Vec<BrushStrokeParams>), String> {
    let deferred = parse_abr_all_deferred_without_patterns(bytes)
        .map_err(|error| format!("Invalid ABR brush: {error}"))?;
    let pack = &deferred.pack;

    // Match brushkit-preview's row order, including unavailable rows and ties.
    let mut rows: Vec<_> = pack
        .sampled_brushes
        .iter()
        .map(|sample| match sample {
            SampledBrush::Readable(i) => {
                let brush = &pack.brushes[*i];
                (
                    brush.preset_index.unwrap_or(usize::MAX),
                    Some(&brush.descriptor),
                )
            }
            SampledBrush::Unavailable(brush) => (brush.preset_index.unwrap_or(usize::MAX), None),
        })
        .collect();
    rows.extend(pack.computed_presets.iter().map(|preset| {
        (
            preset.preset_index.unwrap_or(usize::MAX),
            Some(&preset.descriptor),
        )
    }));
    rows.extend(
        pack.unsupported_tip_presets
            .iter()
            .map(|preset| (preset.preset_index, None)),
    );
    rows.sort_by_key(|(index, _)| *index);

    let settings = indices
        .iter()
        .map(|&index| {
            rows.get(index)
                .and_then(|(_, descriptor)| *descriptor)
                .map_or_else(BrushStrokeParams::default, stroke_params)
        })
        .collect();
    Ok((rows.len(), settings))
}

fn stroke_params(desc: &BrushDescriptor) -> BrushStrokeParams {
    const PEN_PRESSURE: Option<i32> = Some(2);
    const DIRECTION: Option<i32> = Some(6);
    let unit = |value| number(value, 0.0, 100.0, 0.0) / 100.0;

    BrushStrokeParams {
        size_px: number(desc.diameter_px, 1.0, 4000.0, 24.0),
        spacing: number(desc.spacing_pct, 1.0, 500.0, 16.0) / 100.0,
        angle_deg: number(desc.shape_angle_deg, -360.0, 360.0, 0.0),
        scatter: number(desc.scatter_amount_pct, 0.0, 1000.0, 0.0) / 50.0,
        dynamics: BrushDynamics {
            size: (desc.pressure_size_control == PEN_PRESSURE)
                .then(|| PressureCurve::linear(unit(desc.minimum_diameter_pct), 1.0)),
            opacity: (desc.pressure_opacity_control == PEN_PRESSURE)
                .then(|| PressureCurve::linear(0.0, 1.0)),
            flow: (desc.flow_control == PEN_PRESSURE).then(|| PressureCurve::linear(0.0, 1.0)),
            size_jitter: unit(desc.size_jitter_pct),
            angle_jitter_deg: unit(desc.angle_jitter_pct) * 180.0,
            opacity_jitter: unit(desc.opacity_jitter_pct),
            flow_jitter: unit(desc.flow_jitter_pct),
            follow_direction: f32::from(desc.angle_control == DIRECTION),
            scatter_along_stroke: desc.scatter_both_axes.unwrap_or(false),
            scatter_count: number(desc.scatter_count, 1.0, 16.0, 1.0).round() as u8,
            count_jitter: unit(desc.count_jitter_pct),
            ..Default::default()
        },
        ..Default::default()
    }
}

fn number(value: Option<f64>, min: f32, max: f32, default: f32) -> f32 {
    value
        .map(|value| value as f32)
        .filter(|value| value.is_finite())
        .map_or(default, |value| value.clamp(min, max))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptor_enables_pressure_direction_and_scatter() {
        let params = stroke_params(&BrushDescriptor {
            spacing_pct: Some(37.0),
            pressure_size_control: Some(2),
            minimum_diameter_pct: Some(15.0),
            angle_control: Some(6),
            scatter_both_axes: Some(true),
            scatter_count: Some(3.0),
            ..Default::default()
        });
        assert_eq!(params.spacing, 0.37);
        assert_eq!(params.dynamics.size.unwrap().at(0.0), 0.15);
        assert_eq!(params.dynamics.follow_direction, 1.0);
        assert_eq!(params.dynamics.scatter_count, 3);
        assert!(params.dynamics.scatter_along_stroke);
    }
}
