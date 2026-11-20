use super::{
    BrushFormat, BrushLibrary, BrushPreset, BrushStrokeParams, BrushTipKind, DabCurve,
    ParametricShape, bounded_label, gimp, mask,
};
use base64::Engine as _;
use std::collections::HashMap;
use std::io::{Cursor, Read};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

const MAX_PRESET_XML_BYTES: usize = 8 * 1024 * 1024;
const MAX_ZIP_ENTRY_BYTES: u64 = 16 * 1024 * 1024;
const MAX_BUNDLE_DECODE_BYTES: u64 = 128 * 1024 * 1024;

pub(super) fn parse_kpp(
    bytes: &[u8],
    fallback_name: &str,
    cancelled: &AtomicBool,
) -> Result<BrushLibrary, String> {
    let preset = parse_kpp_preset(bytes, fallback_name, cancelled)?;
    Ok(BrushLibrary {
        format: BrushFormat::KritaPreset,
        brushes: vec![preset],
        total_count: 1,
    })
}

pub(super) fn parse_kpp_preset(
    bytes: &[u8],
    fallback_name: &str,
    cancelled: &AtomicBool,
) -> Result<BrushPreset, String> {
    kpp_with_resources(bytes, fallback_name, &mut |_| None, cancelled)
}

fn kpp_with_resources(
    bytes: &[u8],
    fallback_name: &str,
    load_resource: &mut impl FnMut(&str) -> Option<ResourceTip>,
    cancelled: &AtomicBool,
) -> Result<BrushPreset, String> {
    let xml = extract_png_preset_xml(bytes)?;
    let doc = roxmltree::Document::parse_with_options(&xml, preset_xml_options())
        .map_err(|error| format!("Invalid Krita preset XML: {error}"))?;
    let root = doc.root_element();
    let name = root
        .attribute("name")
        .filter(|s| !s.trim().is_empty())
        .map(bounded_label)
        .unwrap_or(fallback_name)
        .to_owned();
    let paintop = bounded_label(root.attribute("paintopid").unwrap_or("paintbrush"));

    let param = |name: &str| {
        root.children()
            .find(|node| node.has_tag_name("param") && node.attribute("name") == Some(name))
            .and_then(|node| node.text())
            .map(str::trim)
    };
    let mut params = paintbrush_params(&param);

    let tip = if let Some(def_xml) = param("brush_definition")
        && let Ok(def_doc) = roxmltree::Document::parse_with_options(def_xml, preset_xml_options())
    {
        let definition = def_doc.root_element();
        let tip = parse_brush_definition(definition, root, &mut params, load_resource, cancelled);
        if tip.is_some() {
            params.dynamics.krita_smudge = legacy_smudge(paintop, definition, &param);
        }
        tip
    } else {
        None
    }
    .unwrap_or(BrushTipKind::Parametric {
        shape: ParametricShape::Circle,
        hardness: params.hardness,
        aspect_ratio: 1.0,
        angle_deg: params.angle_deg,
        spikes: 2,
    });

    if cancelled.load(Ordering::Relaxed) {
        return Err("Cancelled".into());
    }
    let detail = format!("{paintop} · Ø {:.0}px", params.size_px);
    Ok(BrushPreset {
        name,
        tip,
        params,
        detail,
    })
}

fn paintbrush_params<'a>(param: &impl Fn(&str) -> Option<&'a str>) -> BrushStrokeParams {
    let number = |name, default| {
        param(name)
            .and_then(|value| value.parse::<f32>().ok())
            .filter(|value| value.is_finite())
            .unwrap_or(default)
            .clamp(0.0, 1.0)
    };
    let mut params = BrushStrokeParams {
        opacity: number("OpacityValue", number("Opacity", 1.0)),
        flow: number("FlowValue", 1.0),
        ..Default::default()
    };
    if param("PressureSize") == Some("true") {
        let strength = number("SizeValue", 1.0);
        params.dynamics.size = sensor_curve(param, "Size", "pressure", |v| v * strength);
    }
    let (opacity, fade) = opacity_sensors(param);
    params.dynamics.opacity = opacity;
    params.dynamics.opacity_fade = fade;
    params.dynamics.flow = sensor_curve(param, "Flow", "pressure", |v| v);
    if param("PressureRotation") == Some("true") {
        let strength = number("RotationValue", 1.0) * 180.0;
        params.dynamics.random_angle =
            sensor_curve(param, "Rotation", "fuzzy", |v| (2.0 * v - 1.0) * strength);
        params.dynamics.angle = sensor_curve(param, "Rotation", "pressure", |v| {
            (1.0 - 2.0 * v) * strength
        });
    }
    params.dynamics.axis_spacing = param("Spacing/Isotropic") != Some("true");
    params
}

#[derive(Clone, Copy, Debug)]
pub(super) enum SmudgeMode {
    Smear,
    Dull,
}

#[derive(Clone, Debug)]
pub(super) struct LegacySmudge {
    pub(super) mode: SmudgeMode,
    pub(super) length: SmudgeControl,
    pub(super) color: SmudgeControl,
    pub(super) radius: SmudgeControl,
    pub(super) maximum_length: f32,
    pub(super) smear_alpha: bool,
}

#[derive(Clone, Debug)]
pub(super) struct SmudgeControl {
    pub(super) strength: f32,
    pressure: Option<DabCurve>,
}

impl SmudgeControl {
    pub(super) fn at(&self, pressure: f32) -> f32 {
        self.strength
            * self
                .pressure
                .as_ref()
                .map_or(1.0, |curve| curve.at(pressure))
    }
}

fn legacy_smudge<'a>(
    paintop: &str,
    definition: roxmltree::Node<'_, '_>,
    param: &impl Fn(&str) -> Option<&'a str>,
) -> Option<LegacySmudge> {
    let enabled = |name: &str| matches!(param(name), Some("true" | "1"));
    if paintop != "colorsmudge"
        || enabled("SmudgeRateUseNewEngine")
        || enabled("MergedPaint")
        || !matches!(param("CompositeOp"), None | Some("normal"))
        || !matches!(definition.attribute("brushApplication"), None | Some("0"))
        || !matches!(param("Texture/Pattern/Enabled"), None | Some("false" | "0"))
    {
        return None;
    }
    let mode = match param("SmudgeRateMode") {
        None | Some("0") => SmudgeMode::Smear,
        Some("1") => SmudgeMode::Dull,
        _ => return None,
    };
    let control = |property: &str, strength, inactive| {
        if !enabled(&format!("Pressure{property}")) {
            return SmudgeControl {
                strength: inactive,
                pressure: None,
            };
        }
        let pressure = sensor_curve(param, property, "pressure", |value| value).or_else(|| {
            (param(&format!("{property}UseCurve")) != Some("false")
                && param(&format!("{property}Sensor")).is_none())
            .then(|| DabCurve::linear(0.0, 1.0))
        });
        SmudgeControl { strength, pressure }
    };
    let number = |name, default| {
        param(name)
            .and_then(|value| value.parse::<f32>().ok())
            .filter(|value| value.is_finite())
            .unwrap_or(default)
    };
    let maximum_length = number("SmudgeRateValue", 1.0).clamp(0.0, 1.0);
    let color = number("ColorRateValue", 1.0).clamp(0.0, 1.0);
    let radius_scale = if number("SmudgeRadiusVersion", 1.0) < 2.0 {
        0.01
    } else {
        1.0
    };
    let radius = (number("SmudgeRadiusValue", 3.0) * radius_scale).clamp(0.0, 3.0);
    Some(LegacySmudge {
        mode,
        length: control("SmudgeRate", maximum_length, 1.0),
        color: control("ColorRate", color, 0.0),
        radius: control("SmudgeRadius", radius, 0.0),
        maximum_length,
        smear_alpha: !matches!(param("SmudgeRateSmearAlpha"), Some("false" | "0")),
    })
}

fn sensor_curve<'a>(
    param: &impl Fn(&str) -> Option<&'a str>,
    property: &str,
    sensor_id: &str,
    scale: impl Fn(f32) -> f32,
) -> Option<DabCurve> {
    let doc = sensor_document(param, property)?;
    let sensor = doc.root_element();
    (sensor.attribute("id") == Some(sensor_id))
        .then(|| curve_for_sensor(sensor, common_curve(param, property), scale))
        .flatten()
}

fn sensor_document<'a>(
    param: &impl Fn(&str) -> Option<&'a str>,
    property: &str,
) -> Option<roxmltree::Document<'a>> {
    if param(&format!("{property}UseCurve")) == Some("false") {
        return None;
    }
    let xml = param(&format!("{property}Sensor"))?;
    let xml = xml
        .strip_prefix("<!DOCTYPE params>")
        .unwrap_or(xml)
        .trim_start();
    let document = roxmltree::Document::parse_with_options(xml, preset_xml_options()).ok()?;
    document
        .root_element()
        .has_tag_name("params")
        .then_some(document)
}

fn common_curve<'a>(param: &impl Fn(&str) -> Option<&'a str>, property: &str) -> Option<&'a str> {
    (param(&format!("{property}UseSameCurve")) != Some("false"))
        .then(|| param(&format!("{property}commonCurve")))
        .flatten()
}

fn curve_for_sensor(
    sensor: roxmltree::Node<'_, '_>,
    common: Option<&str>,
    scale: impl Fn(f32) -> f32,
) -> Option<DabCurve> {
    let curve = common.or_else(|| {
        sensor
            .children()
            .find(|node| node.has_tag_name("curve"))
            .and_then(|node| node.text())
    });
    match curve {
        Some(curve) => parse_curve(curve, scale),
        None => Some(DabCurve::linear(scale(0.0), scale(1.0))),
    }
}

fn opacity_sensors<'a>(
    param: &impl Fn(&str) -> Option<&'a str>,
) -> (Option<DabCurve>, Option<OpacityFade>) {
    let Some(doc) = sensor_document(param, "Opacity") else {
        return (None, None);
    };
    let root = doc.root_element();
    let common = common_curve(param, "Opacity");
    match root.attribute("id") {
        Some("pressure") => (curve_for_sensor(root, common, |v| v), None),
        Some("fade") => (None, OpacityFade::read(root, common, None)),
        Some("sensorslist") => pressure_with_fade(root, common, param("OpacitycurveMode")),
        _ => (None, None),
    }
}

fn pressure_with_fade(
    root: roxmltree::Node<'_, '_>,
    common: Option<&str>,
    mode: Option<&str>,
) -> (Option<DabCurve>, Option<OpacityFade>) {
    let mut sensors = root
        .children()
        .filter(|node| node.has_tag_name("ChildSensor"));
    let (Some(first), Some(second), None) = (sensors.next(), sensors.next(), sensors.next()) else {
        return (None, None);
    };
    let (pressure, fade) = match (first.attribute("id"), second.attribute("id")) {
        (Some("pressure"), Some("fade")) => (first, second),
        (Some("fade"), Some("pressure")) => (second, first),
        _ => return (None, None),
    };
    let combination = match mode {
        Some("1") => FadeMode::Add,
        Some("2") => FadeMode::Maximum,
        Some("3") => FadeMode::Minimum,
        Some("4") => FadeMode::Difference,
        _ => FadeMode::Multiply,
    };
    let pressure = curve_for_sensor(pressure, common, |v| v);
    let fade = OpacityFade::read(fade, common, Some(combination));
    match (pressure, fade) {
        (Some(pressure), Some(fade)) => (Some(pressure), Some(fade)),
        _ => (None, None),
    }
}

#[derive(Clone, Copy, Debug)]
enum FadeMode {
    Multiply,
    Add,
    Maximum,
    Minimum,
    Difference,
}

#[derive(Clone, Debug)]
pub(super) struct OpacityFade {
    length: usize,
    periodic: bool,
    curve: DabCurve,
    mode: Option<FadeMode>,
}

impl OpacityFade {
    fn read(
        node: roxmltree::Node<'_, '_>,
        common: Option<&str>,
        mode: Option<FadeMode>,
    ) -> Option<Self> {
        let length = node.attribute("length")?.parse::<u32>().ok()?;
        Some(Self {
            length: usize::try_from(length.max(1)).ok()?,
            periodic: matches!(node.attribute("periodic"), Some("1" | "true")),
            curve: curve_for_sensor(node, common, |v| v)?,
            mode,
        })
    }

    pub(super) fn at(&self, dab: usize, pressure_opacity: f32) -> f32 {
        let progress = if self.periodic {
            dab % self.length
        } else {
            dab.min(self.length)
        };
        let fade = self.curve.at(progress as f32 / self.length as f32);
        match self.mode {
            None => fade,
            Some(FadeMode::Multiply) => pressure_opacity * fade,
            Some(FadeMode::Add) => pressure_opacity + fade,
            Some(FadeMode::Maximum) => pressure_opacity.max(fade),
            Some(FadeMode::Minimum) => pressure_opacity.min(fade),
            Some(FadeMode::Difference) => (pressure_opacity - fade).abs(),
        }
        .clamp(0.0, 1.0)
    }
}

fn parse_curve(curve: &str, scale: impl Fn(f32) -> f32) -> Option<DabCurve> {
    let mut points = curve
        .trim()
        .split_terminator(';')
        .take(65)
        .map(|point| {
            let mut fields = point.split(',').map(str::trim);
            let x = fields.next()?.parse::<f64>().ok()?;
            let y = fields.next()?.parse::<f64>().ok()?;
            let corner = fields.next() == Some("is_corner");
            ((0.0..=1.0).contains(&x) && (0.0..=1.0).contains(&y)).then_some(CurvePoint {
                x,
                y,
                corner,
            })
        })
        .collect::<Option<Vec<_>>>()?;
    points.sort_by(|a, b| a.x.total_cmp(&b.x));
    if !(2..=64).contains(&points.len())
        || points.windows(2).any(|span| span[1].x - span[0].x < 1.0e-7)
    {
        return None;
    }
    let second = spline_second_derivatives(&points);
    Some(DabCurve::lookup(256, |input| {
        let x = f64::from(input).clamp(points[0].x, points[points.len() - 1].x);
        let upper = points.partition_point(|point| point.x < x).max(1);
        let (left, right) = (points[upper - 1], points[upper]);
        let width = right.x - left.x;
        let b = (x - left.x) / width;
        let a = 1.0 - b;
        let linear = a * left.y + b * right.y;
        let curvature = (a * a * a - a) * second[upper - 1] + (b * b * b - b) * second[upper];
        scale((linear + width * width * curvature / 6.0).clamp(0.0, 1.0) as f32)
    }))
}

#[derive(Clone, Copy)]
struct CurvePoint {
    x: f64,
    y: f64,
    corner: bool,
}

fn spline_second_derivatives(points: &[CurvePoint]) -> Vec<f64> {
    // Endpoints and corners have zero second derivative. Between them the
    // natural cubic spline has continuous first and second derivatives.
    let count = points.len();
    let mut upper = vec![0.0; count];
    let mut second = vec![0.0; count];
    for (i, span) in points.windows(3).enumerate() {
        let index = i + 1;
        if span[1].corner {
            continue;
        }
        let left = span[1].x - span[0].x;
        let right = span[2].x - span[1].x;
        let slope_change = (span[2].y - span[1].y) / right - (span[1].y - span[0].y) / left;
        let diagonal = 2.0 * (left + right) - left * upper[index - 1];
        upper[index] = right / diagonal;
        second[index] = (6.0 * slope_change - left * second[index - 1]) / diagonal;
    }
    for index in (1..count - 1).rev() {
        second[index] -= upper[index] * second[index + 1];
    }
    second
}

fn preset_xml_options() -> roxmltree::ParsingOptions {
    roxmltree::ParsingOptions {
        nodes_limit: 100_000,
        ..Default::default()
    }
}

fn parse_brush_definition(
    brush: roxmltree::Node<'_, '_>,
    preset: roxmltree::Node<'_, '_>,
    params: &mut BrushStrokeParams,
    load_resource: &mut impl FnMut(&str) -> Option<ResourceTip>,
    cancelled: &AtomicBool,
) -> Option<BrushTipKind> {
    if let Some(spacing) = attribute_number(brush, "spacing") {
        params.spacing = spacing.clamp(0.03, 2.0);
    }
    if matches!(brush.attribute("useAutoSpacing"), Some("1" | "true")) {
        params.dynamics.auto_spacing = Some(
            attribute_number(brush, "autoSpacingCoeff")
                .unwrap_or(1.0)
                .clamp(0.0, 10.0),
        );
    }
    if let Some(angle) = attribute_number(brush, "angle") {
        params.angle_deg = angle.to_degrees();
    }
    match brush.attribute("type").unwrap_or("auto_brush") {
        "auto_brush" => generated_tip(brush, params),
        "gbr_brush" | "png_brush" => {
            predefined_tip(brush, preset, params, load_resource, cancelled)
        }
        _ => None,
    }
}

fn generated_tip(
    brush: roxmltree::Node<'_, '_>,
    params: &mut BrushStrokeParams,
) -> Option<BrushTipKind> {
    let mask = brush
        .children()
        .find(|node| node.has_tag_name("MaskGenerator"))?;
    // Krita's legacy "radius" field also stores a diameter.
    params.size_px = attribute_number(mask, "radius")
        .or_else(|| attribute_number(mask, "diameter"))
        .unwrap_or(24.0)
        .clamp(1.0, 4000.0);
    let ratio = attribute_number(mask, "ratio")
        .unwrap_or(1.0)
        .clamp(0.05, 1.0);
    let fade = attribute_number(mask, "fade").map_or(0.0, |value| 1.0 - value * 0.5);
    let horizontal = attribute_number(mask, "hfade")
        .unwrap_or(fade)
        .clamp(0.0, 1.0);
    let vertical = attribute_number(mask, "vfade")
        .unwrap_or(fade)
        .clamp(0.0, 1.0);
    let shape = match mask.attribute("type") {
        Some("rct" | "rect") => ParametricShape::Square,
        _ => ParametricShape::Circle,
    };
    let spikes = mask
        .attribute("spikes")
        .and_then(|value| value.parse::<u8>().ok())
        .unwrap_or(2)
        .clamp(2, 20);
    params.hardness = (horizontal + vertical) * 0.5;

    let softness = match mask.attribute("id").unwrap_or("default") {
        "default" => None,
        "soft" => parse_curve(mask.attribute("softness_curve").unwrap_or("0,0;1,1"), |y| y),
        _ => {
            return Some(BrushTipKind::Parametric {
                shape,
                hardness: params.hardness,
                aspect_ratio: ratio.recip(),
                angle_deg: params.angle_deg,
                spikes,
            });
        }
    };
    let aa = matches!(mask.attribute("antialiasEdges"), Some("1" | "true"));
    let pixel_x = 2.0 / params.size_px;
    let pixel_y = pixel_x / ratio;
    let fade_x = if horizontal > 0.0 {
        horizontal
    } else {
        pixel_x
    };
    let fade_y = if vertical > 0.0 { vertical } else { pixel_y };
    let mask_ratio = if spikes > 2 { 1.0 } else { ratio };
    Some(mask::generated(mask_ratio, |x, y| {
        let (x, y) = mask::fold_spikes(x, y * mask_ratio, spikes);
        let (x, y) = (x.abs(), y.abs() / ratio);
        if let Some(softness) = &softness {
            soft_coverage(shape, x, y, softness)
        } else {
            let aa_x = pixel_x * f32::from(aa);
            let aa_y = pixel_y * f32::from(aa);
            default_coverage(shape, (x, y), (x + aa_x, y + aa_y), (fade_x, fade_y))
        }
    }))
}

fn default_coverage(
    shape: ParametricShape,
    (x, y): (f32, f32),
    (soft_x, soft_y): (f32, f32),
    (fade_x, fade_y): (f32, f32),
) -> f32 {
    if shape == ParametricShape::Square {
        let axis = |position: f32, softened: f32, hard_edge: f32| {
            let fade = softened / hard_edge;
            if fade <= 1.0 {
                1.0
            } else {
                fade * (1.0 - position) / (fade - position)
            }
        };
        if x >= 1.0 || y >= 1.0 {
            return 0.0;
        }
        return axis(x, soft_x, fade_x).min(axis(y, soft_y, fade_y));
    }

    let distance = x * x + y * y;
    if distance >= 1.0 {
        return 0.0;
    }
    let fade = (soft_x / fade_x).powi(2) + (soft_y / fade_y).powi(2);
    if fade <= 1.0 {
        1.0
    } else {
        fade * (1.0 - distance) / (fade - distance)
    }
}

fn soft_coverage(shape: ParametricShape, x: f32, y: f32, curve: &DabCurve) -> f32 {
    if shape == ParametricShape::Square {
        let axis = |position: f32| curve.at(position) * (1.0 - curve.at(1.0 - position));
        if x >= 1.0 || y >= 1.0 {
            0.0
        } else {
            1.0 - axis(x) * axis(y)
        }
    } else {
        let distance = x * x + y * y;
        if distance >= 1.0 {
            0.0
        } else {
            1.0 - curve.at(distance)
        }
    }
}

fn attribute_number(node: roxmltree::Node<'_, '_>, name: &str) -> Option<f32> {
    node.attribute(name)?
        .parse::<f32>()
        .ok()
        .filter(|value| value.is_finite())
}

fn predefined_tip(
    brush: roxmltree::Node<'_, '_>,
    preset: roxmltree::Node<'_, '_>,
    params: &mut BrushStrokeParams,
    load_resource: &mut impl FnMut(&str) -> Option<ResourceTip>,
    cancelled: &AtomicBool,
) -> Option<BrushTipKind> {
    let filename = brush.attribute("filename");
    let encoded = match filename {
        Some(filename) => find_embedded_brush(preset, filename, brush.attribute("md5sum")),
        None => brush.text(),
    };
    let resource = encoded
        .and_then(|encoded| {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(encoded.trim())
                .ok()?;
            resource_tip(&bytes, filename, cancelled)
        })
        .or_else(|| load_resource(filename?))?;
    params.size_px =
        (resource.size * attribute_number(brush, "scale").unwrap_or(1.0)).clamp(1.0, 4000.0);
    params.dynamics.tip_sequence = resource.sequence;
    Some(resource.tip)
}

#[derive(Clone)]
struct ResourceTip {
    tip: BrushTipKind,
    size: f32,
    sequence: Option<gimp::TipSequence>,
}

fn resource_tip(
    bytes: &[u8],
    filename: Option<&str>,
    cancelled: &AtomicBool,
) -> Option<ResourceTip> {
    if bytes.starts_with(mask::PNG_MAGIC) {
        // Krita's opaque grayscale tip images use white for empty canvas.
        let (tip, width, height) = mask::png(bytes, mask::OpaqueCoverage::Dark)?;
        return Some(ResourceTip {
            tip,
            size: width.max(height) as f32,
            sequence: None,
        });
    }
    let preset = if filename.is_some_and(|name| name.ends_with(".gih")) {
        gimp::parse_gih(bytes, "Krita", 1, cancelled)
            .ok()?
            .brushes
            .into_iter()
            .next()?
    } else {
        gimp::parse_gbr_at(bytes, "Krita").ok()?.0
    };
    Some(ResourceTip {
        tip: preset.tip,
        size: preset.params.size_px,
        sequence: preset.params.dynamics.tip_sequence,
    })
}

fn find_embedded_brush<'a>(
    preset: roxmltree::Node<'a, '_>,
    filename: &str,
    hash: Option<&str>,
) -> Option<&'a str> {
    let resources = preset
        .children()
        .find(|node| node.has_tag_name("resources"))?;
    let candidates = resources
        .children()
        .filter(|node| node.has_tag_name("resource") && node.attribute("type") == Some("brushes"));
    let hash = hash.filter(|hash| !hash.is_empty());
    if let Some(hash) = hash
        && let Some(resource) = candidates
            .clone()
            .find(|node| node.attribute("md5sum") == Some(hash))
    {
        return resource.text();
    }
    candidates
        .filter(|node| node.attribute("filename") == Some(filename))
        .find_map(|node| node.text())
}

fn extract_png_preset_xml(bytes: &[u8]) -> Result<String, String> {
    let mut decoder = png::Decoder::new(Cursor::new(bytes));
    decoder.set_limits(png::Limits {
        bytes: 2 * MAX_PRESET_XML_BYTES,
    });
    let mut reader = decoder.read_info().map_err(|error| error.to_string())?;
    if let Some(xml) = preset_text(reader.info()) {
        return xml;
    }
    reader.finish().map_err(|error| error.to_string())?;
    preset_text(reader.info()).ok_or_else(|| "No Krita preset metadata in .kpp".to_owned())?
}

fn preset_text(info: &png::Info<'_>) -> Option<Result<String, String>> {
    if let Some(text) = info
        .uncompressed_latin1_text
        .iter()
        .find(|text| text.keyword == "preset")
    {
        return Some(bounded_xml(&text.text).map(str::to_owned));
    }
    if let Some(text) = info
        .compressed_latin1_text
        .iter()
        .find(|text| text.keyword == "preset")
    {
        let mut text = text.clone();
        return Some(
            text.decompress_text_with_limit(MAX_PRESET_XML_BYTES)
                .and_then(|()| text.get_text())
                .map_err(|error| error.to_string())
                .and_then(|xml| {
                    bounded_xml(&xml)?;
                    Ok(xml)
                }),
        );
    }
    info.utf8_text
        .iter()
        .find(|text| text.keyword == "preset")
        .map(|text| {
            let mut text = text.clone();
            text.decompress_text_with_limit(MAX_PRESET_XML_BYTES)
                .and_then(|()| text.get_text())
                .map_err(|error| error.to_string())
                .and_then(|xml| {
                    bounded_xml(&xml)?;
                    Ok(xml)
                })
        })
}

fn bounded_xml(xml: &str) -> Result<&str, String> {
    (xml.len() <= MAX_PRESET_XML_BYTES)
        .then_some(xml)
        .ok_or_else(|| "Krita preset metadata exceeds its size limit".to_owned())
}

pub(super) fn parse_bundle(
    bytes: &[u8],
    max_brushes: usize,
    cancelled: &AtomicBool,
) -> Result<BrushLibrary, String> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes))
        .map_err(|error| format!("Invalid Krita .bundle archive: {error}"))?;
    // In preset bundles, the brush bitmaps are resources for the presets.
    // Tip-only bundles still expose those tips directly.
    let extensions: &[_] = if archive.file_names().filter_map(Result::ok).any(|name| {
        !name.ends_with(['/', '\\'])
            && Path::new(name.as_ref())
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("kpp"))
    }) {
        &["kpp"]
    } else {
        &["gbr", "vbr"]
    };
    let mut brushes = Vec::new();
    let mut total_count = 0usize;
    let mut remaining_decode_bytes = MAX_BUNDLE_DECODE_BYTES;
    let mut resources = HashMap::new();

    for i in 0..archive.len() {
        if cancelled.load(Ordering::Relaxed) {
            return Err("Cancelled".into());
        }
        let Ok(entry) = archive.by_index_raw(i) else {
            continue;
        };
        let Ok(name) = entry.name().map(std::borrow::Cow::into_owned) else {
            continue;
        };
        let size = entry.size();
        if entry.is_dir() || size > MAX_ZIP_ENTRY_BYTES {
            continue;
        }
        drop(entry);
        let path = Path::new(&name);
        let extension = path
            .extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if !extensions.contains(&extension.as_str()) {
            continue;
        }
        total_count += 1;
        if brushes.len() >= max_brushes || size > remaining_decode_bytes {
            continue;
        }
        let Ok(entry) = archive.by_index(i) else {
            continue;
        };
        let Some(buf) = read_bundle_entry(entry, size, &mut remaining_decode_bytes) else {
            continue;
        };
        let mut load_resource = |name: &str| {
            resources
                .entry(name.to_owned())
                .or_insert_with(|| {
                    bundle_resource(&mut archive, name, &mut remaining_decode_bytes, cancelled)
                })
                .clone()
        };
        if let Ok(preset) = bundle_preset(&buf, path, &extension, &mut load_resource, cancelled) {
            brushes.push(preset);
        }
    }

    if cancelled.load(Ordering::Relaxed) {
        return Err("Cancelled".into());
    }
    if brushes.is_empty() {
        return Err("No readable brushes in Krita .bundle".into());
    }
    Ok(BrushLibrary {
        format: BrushFormat::KritaBundle,
        brushes,
        total_count,
    })
}

fn bundle_preset(
    bytes: &[u8],
    path: &Path,
    extension: &str,
    load_resource: &mut impl FnMut(&str) -> Option<ResourceTip>,
    cancelled: &AtomicBool,
) -> Result<BrushPreset, String> {
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .map(bounded_label)
        .unwrap_or("Brush");
    match extension {
        "kpp" => kpp_with_resources(bytes, stem, load_resource, cancelled),
        "gbr" => gimp::parse_gbr_at(bytes, stem).map(|(preset, _)| preset),
        "vbr" => gimp::parse_vbr_preset(bytes, stem),
        _ => Err("Unsupported Krita bundle member".into()),
    }
}

fn bundle_resource(
    archive: &mut zip::ZipArchive<Cursor<&[u8]>>,
    name: &str,
    remaining: &mut u64,
    cancelled: &AtomicBool,
) -> Option<ResourceTip> {
    if Path::new(name).file_name()?.to_str()? != name || name.contains('\\') {
        return None;
    }
    let entry = archive.by_name(&format!("brushes/{name}")).ok()?;
    let size = entry.size();
    let buf = read_bundle_entry(entry, size, remaining)?;
    resource_tip(&buf, Some(name), cancelled)
}

fn read_bundle_entry(entry: impl Read, size: u64, remaining: &mut u64) -> Option<Vec<u8>> {
    if size > MAX_ZIP_ENTRY_BYTES || size > *remaining {
        return None;
    }
    *remaining -= size;
    let mut buf = Vec::with_capacity(size as usize);
    entry.take(size + 1).read_to_end(&mut buf).ok()?;
    (buf.len() as u64 == size).then_some(buf)
}

#[cfg(test)]
mod curve_tests {
    use super::*;

    #[test]
    fn opaque_krita_png_tips_use_dark_pixels_as_ink() {
        let image = image::GrayImage::from_raw(3, 1, vec![255, 128, 0]).unwrap();
        let mut png = Cursor::new(Vec::new());
        image.write_to(&mut png, image::ImageFormat::Png).unwrap();
        let tip =
            resource_tip(&png.into_inner(), Some("tip.png"), &AtomicBool::new(false)).unwrap();
        assert!(
            matches!(tip.tip, BrushTipKind::AlphaMask { alpha, .. } if *alpha == [0, 127, 255])
        );
    }

    #[test]
    fn krita_curves_use_splines_and_keep_marked_corners() {
        let smooth = parse_curve("0,0;0.5,1;1,0;", |y| y).unwrap();
        let corner = parse_curve("0,0;0.5,1,is_corner;1,0;", |y| y).unwrap();
        assert_eq!(smooth.at(0.25), 0.6875);
        assert_eq!(corner.at(0.25), 0.5);
        assert_eq!(smooth.at(0.5), 1.0);
        assert!(parse_curve("0.5,0;0.5,1", |y| y).is_none());
    }
}
