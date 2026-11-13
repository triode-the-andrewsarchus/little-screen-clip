// SPDX-License-Identifier: MPL-2.0
//! color.go (builder.paint) and d2renderers/internal/scenepaint/gradient.go,
//! over lib/color ParseGradient (master, with canonical stop positions) and
//! csscolorparser v0.1.8 (d2-rs-math svg_color).

use d2_rs_scene::{
    GradientStop, LinearGradient, Matrix, Nrgba, Paint, PaintUnits, Point, RadialGradient,
    SpreadMethod,
};

use crate::golib::color::is_gradient;
use crate::golib::geo::{go_fmadd, gomath};
use crate::util::*;

type SResult<T> = std::result::Result<T, String>;
use crate::Builder;

/// csscolorparser.Parse error text for an unparsable value.
fn css_parse_error(input: &str) -> String {
    let s = input.trim().to_lowercase();
    if s.starts_with('#') {
        return format!("Invalid hex color, {input}");
    }
    if let Some(open) = s.find('(').filter(|_| s.ends_with(')')) {
        let name = s[..open].trim();
        return match name {
            "rgb" | "rgba" | "hsl" | "hsla" => format!("Wrong {name}() components, {input}"),
            "hwb" | "hwba" => format!("Wrong hwb() components, {input}"),
            "hsv" | "hsva" => format!("Wrong hsv() components, {input}"),
            "oklab" => format!("Wrong oklab() components, {input}"),
            "oklch" => format!("Wrong oklch() components, {input}"),
            "lab" => "Invalid lab()".into(),
            "lch" => "Invalid lch()".into(),
            _ => "Invalid format".into(),
        };
    }
    format!("Invalid color format, {input}")
}

/// csscolorparser.Parse followed by Color.RGBA255 (`uint8(c*255 + 0.5)`,
/// fused on ARM64 and converted through a signed 32-bit register).
pub(crate) fn parse_css_color(value: &str) -> SResult<Nrgba> {
    let color = d2_rs_math::svg_color::parse(value).ok_or_else(|| css_parse_error(value))?;
    let byte = |v: f64| go_fmadd(v, 255.0, 0.5) as i32 as u8;
    Ok(Nrgba::new(
        byte(color[0]),
        byte(color[1]),
        byte(color[2]),
        byte(color[3]),
    ))
}

impl Builder<'_> {
    /// paint: none/empty is no paint; gradients become typed paints; theme
    /// codes resolve against the active theme.
    pub(crate) fn paint(&self, raw: &str, description: &str) -> Result<Option<Paint>> {
        if raw.is_empty() || raw.eq_ignore_ascii_case("none") {
            return Ok(None);
        }
        if is_gradient(raw) {
            return gradient_paint(raw).map(Some).map_err(|e| {
                Error(format!(
                    "scene: parse {description} gradient {}: {e}",
                    q(raw)
                ))
            });
        }
        let resolved = self.theme.resolve(raw);
        if resolved.is_empty() {
            return err(format!(
                "scene: {description} has unknown theme color {}",
                q(raw)
            ));
        }
        let color = parse_css_color(&resolved).map_err(|e| {
            Error(format!(
                "scene: parse {description} color {}: {e}",
                q(&resolved)
            ))
        })?;
        Ok(Some(Paint::solid(color)))
    }
}

struct ColorStop {
    color: String,
    position: String,
}

/// lib/color.ParseGradient (type, direction and canonical stops).
fn parse_gradient(css: &str) -> SResult<(String, String, Vec<ColorStop>)> {
    let css = css.trim();
    let (kind, rest) = if let Some(rest) = css.strip_prefix("linear-gradient(") {
        ("linear", rest)
    } else if let Some(rest) = css.strip_prefix("radial-gradient(") {
        ("radial", rest)
    } else {
        return Err("invalid gradient syntax".into());
    };
    // `^(...)\((.*)\)$`: `.` excludes newlines.
    let Some(params) = rest.strip_suffix(')').filter(|p| !p.contains('\n')) else {
        return Err("invalid gradient syntax".into());
    };
    let list = split_params(params);
    if list.is_empty() {
        return Err("no parameters in gradient".into());
    }
    let first = list[0].trim();
    let directed = (kind == "linear" && (first.ends_with("deg") || first.starts_with("to ")))
        || (kind == "radial" && (first == "circle" || first == "ellipse"));
    let (direction, stops) = if directed {
        if list.len() < 2 {
            return Err("no color stops in gradient".into());
        }
        (first.to_owned(), &list[1..])
    } else {
        (String::new(), &list[..])
    };
    let mut color_stops = Vec::new();
    for (i, p) in stops.iter().enumerate() {
        let p = p.trim();
        let parts: Vec<&str> = p.split_whitespace().collect();
        match parts.as_slice() {
            [c] => color_stops.push(ColorStop {
                color: (*c).to_owned(),
                position: String::new(),
            }),
            [c, position] => {
                let canonical = canonical_gradient_stop_position(position)
                    .map_err(|e| format!("color stop {i} position {}: {e}", q(position)))?;
                color_stops.push(ColorStop {
                    color: (*c).to_owned(),
                    position: canonical,
                });
            }
            _ => return Err(format!("invalid color stop {i} {}", q(p))),
        }
    }
    Ok((kind.to_owned(), direction, color_stops))
}

fn split_params(params: &str) -> Vec<String> {
    let (mut parts, mut buf, mut nesting) = (Vec::new(), String::new(), 0usize);
    for r in params.chars() {
        match r {
            ',' if nesting == 0 => {
                parts.push(std::mem::take(&mut buf));
                continue;
            }
            '(' => nesting += 1,
            ')' => nesting = nesting.saturating_sub(1),
            _ => {}
        }
        buf.push(r);
    }
    if !buf.is_empty() {
        parts.push(buf);
    }
    parts
}

/// strconv.ParseFloat accepting Go's syntax for finite values.
fn go_parse_float(value: &str) -> Option<f64> {
    d2_rs_core::parse_d2_float(value).filter(|v| v.is_finite())
}

fn canonical_gradient_stop_position(position: &str) -> SResult<String> {
    let percentage = position.ends_with('%');
    let number = position.strip_suffix('%').unwrap_or(position);
    let value =
        go_parse_float(number).ok_or_else(|| "must be a finite number or percentage".to_owned())?;
    // FormatFloat('g', -1) round-trips; only the parsed value is used below.
    let mut canonical = format!("{value:?}");
    if percentage {
        canonical.push('%');
    }
    Ok(canonical)
}

/// scenepaint.Gradient.
pub fn gradient_paint(raw: &str) -> SResult<Paint> {
    let (kind, direction, color_stops) = parse_gradient(raw)?;
    if color_stops.is_empty() {
        return Err("gradient has no valid color stops".into());
    }
    let mut stops = Vec::with_capacity(color_stops.len());
    let mut previous = 0.0;
    for (i, stop) in color_stops.iter().enumerate() {
        let color = parse_css_color(&stop.color)
            .map_err(|e| format!("stop {i} color {}: {e}", q(&stop.color)))?;
        let offset = gradient_stop_offset(&stop.position, i, color_stops.len())
            .map_err(|e| format!("stop {i} position {}: {e}", q(&stop.position)))?;
        use d2_rs_scene::{go_max, go_min};
        let offset = go_max(previous, go_max(0.0, go_min(1.0, offset)));
        previous = offset;
        stops.push(GradientStop { offset, color });
    }
    match kind.as_str() {
        "linear" => {
            let (x1, y1, x2, y2) = linear_gradient_vector(&direction)?;
            Ok(Paint::LinearGradient(LinearGradient {
                start: Point::new(x1, y1),
                end: Point::new(x2, y2),
                stops,
                units: PaintUnits::OBJECT_BOUNDING_BOX,
                transform: Matrix::identity(),
                spread: SpreadMethod::PAD,
            }))
        }
        "radial" => Ok(Paint::RadialGradient(RadialGradient {
            center: Point::new(0.5, 0.5),
            radius: 0.5,
            focal: Point::new(0.5, 0.5),
            focal_radius: 0.0,
            stops,
            units: PaintUnits::OBJECT_BOUNDING_BOX,
            transform: Matrix::identity(),
            spread: SpreadMethod::PAD,
        })),
        other => Err(format!("unsupported gradient type {}", q(other))),
    }
}

fn gradient_stop_offset(position: &str, index: usize, total: usize) -> SResult<f64> {
    if position.is_empty() {
        if total == 1 {
            return Ok(0.0);
        }
        return Ok(index as f64 / (total - 1) as f64);
    }
    let mut position = position.trim();
    let mut divisor = 1.0;
    if let Some(stripped) = position.strip_suffix('%') {
        position = stripped.trim();
        divisor = 100.0;
    }
    let value = go_parse_float(position)
        .ok_or_else(|| "must be a finite number or percentage".to_owned())?;
    Ok(value / divisor)
}

fn linear_gradient_vector(direction: &str) -> SResult<(f64, f64, f64, f64)> {
    let direction = direction.trim();
    if let Some(rest) = direction.strip_prefix("to ") {
        let (mut x1, mut y1, mut x2, mut y2) = (0.5, 0.5, 0.5, 0.5);
        let (mut x_set, mut y_set) = (false, false);
        for part in rest.split_whitespace() {
            match part {
                "left" => (x1, x2, x_set) = (1.0, 0.0, true),
                "right" => (x1, x2, x_set) = (0.0, 1.0, true),
                "top" => (y1, y2, y_set) = (1.0, 0.0, true),
                "bottom" => (y1, y2, y_set) = (0.0, 1.0, true),
                _ => {
                    return Err(format!(
                        "unsupported linear-gradient direction {}",
                        q(direction)
                    ));
                }
            }
        }
        if !x_set {
            (x1, x2) = (0.5, 0.5);
        }
        if !y_set {
            (y1, y2) = (0.5, 0.5);
        }
        return Ok((x1, y1, x2, y2));
    }
    if let Some(degrees) = direction.strip_suffix("deg") {
        let degrees = go_parse_float(degrees.trim())
            .ok_or_else(|| format!("invalid linear-gradient angle {}", q(direction)))?;
        let radians = (90.0 - degrees) * std::f64::consts::PI / 180.0;
        // ARM64 scenepaint/gradient.go: `.5 + .5*cos` is FMADDD.
        return Ok((
            0.5,
            0.5,
            go_fmadd(0.5, gomath::cos(radians), 0.5),
            go_fmadd(0.5, gomath::sin(radians), 0.5),
        ));
    }
    if !direction.is_empty() {
        return Err(format!(
            "unsupported linear-gradient direction {}",
            q(direction)
        ));
    }
    Ok((0.0, 0.0, 0.0, 1.0))
}
