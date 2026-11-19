use super::{BrushDynamics, BrushFormat, BrushStrokeParams, BrushkitStroke, DabCurve};
use brushkit::preview::procreate::{
    Zip, archive_objects_and_main, parse_brushset_plist, parse_plist_guarded,
};
use std::sync::atomic::{AtomicBool, Ordering};

pub(super) fn settings(
    bytes: &[u8],
    format: BrushFormat,
    indices: &[usize],
    cancelled: &AtomicBool,
) -> Result<(usize, Vec<BrushkitStroke>), String> {
    let mut zip = Zip::open(bytes)?;
    // Use the same index as the tip previews, including repeated pack members.
    let members = if format == BrushFormat::ProcreateBrush {
        vec![String::new()]
    } else if zip.contains("brushset.plist") {
        let bytes = zip
            .read_plist("brushset.plist")
            .map_err(|e| e.to_string())?;
        let (_, members) = parse_brushset_plist(&bytes)?;
        members
    } else {
        zip.members_in_zip_order()
    };

    let mut settings = Vec::with_capacity(indices.len());
    for &index in indices {
        if cancelled.load(Ordering::Relaxed) {
            return Err("Cancelled".into());
        }
        let params = members
            .get(index)
            .and_then(|member| {
                let path = if format == BrushFormat::ProcreateBrush {
                    "Brush.archive".to_owned()
                } else {
                    format!("{member}/Brush.archive")
                };
                let bytes = zip.read_plist(&path).ok()?;
                archive_settings(&bytes)
            })
            .unwrap_or_default();
        settings.push(params);
    }
    Ok((members.len(), settings))
}

fn archive_settings(bytes: &[u8]) -> Option<BrushkitStroke> {
    let archive = parse_plist_guarded(bytes, "Brush.archive").ok()?;
    let (objects, main) = archive_objects_and_main(&archive).ok()?;
    let value = |key| {
        let value = main.get(key)?;
        match value.as_uid() {
            Some(uid) => objects.get(usize::try_from(uid.get()).ok()?),
            None => Some(value),
        }
    };
    let number = |key| {
        let value = value(key)?;
        value
            .as_real()
            .or_else(|| value.as_signed_integer().map(|n| n as f64))
            .map(|n| n as f32)
            .filter(|n| n.is_finite())
    };
    let boolean = |key| value(key).and_then(|v| v.as_boolean()).unwrap_or(false);
    let unit = |key| number(key).unwrap_or(0.0).clamp(0.0, 1.0);
    let pressure = |key| {
        let strength = number(key).unwrap_or(0.0).clamp(-1.0, 1.0);
        match strength {
            strength if strength > 0.0 => Some(DabCurve::linear(1.0 - strength, 1.0)),
            strength if strength < 0.0 => Some(DabCurve::linear(1.0, 1.0 + strength)),
            _ => None,
        }
    };

    let params = BrushStrokeParams {
        spacing: number("plotSpacing").unwrap_or(0.16).clamp(0.01, 5.0),
        angle_deg: number("shapeAngle")
            .unwrap_or(0.0)
            .to_degrees()
            .clamp(-360.0, 360.0),
        scatter: unit("plotJitter") * 2.0,
        dynamics: BrushDynamics {
            size: pressure("dynamicsPressureSize"),
            opacity: pressure("dynamicsPressureOpacity"),
            size_jitter: unit("dynamicsJitterSize"),
            opacity_jitter: unit("dynamicsJitterOpacity"),
            angle_jitter_deg: number("shapeScatter")
                .unwrap_or(0.0)
                .to_degrees()
                .clamp(0.0, 180.0),
            follow_direction: number("shapeRotation").unwrap_or(0.0).clamp(-1.0, 1.0),
            random_start_angle: boolean("shapeRandomise"),
            scatter_along_stroke: true,
            taper_start: unit("taperStartLength"),
            taper_end: unit("taperEndLength"),
            taper_size: unit("taperSize"),
            taper_opacity: unit("taperOpacity"),
            ..Default::default()
        },
        ..Default::default()
    };
    Some(BrushkitStroke {
        params,
        invert_mask: boolean("shapeInverted"),
    })
}
