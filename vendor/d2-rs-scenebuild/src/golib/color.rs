// SPDX-License-Identifier: MPL-2.0
// Port of the parts of D2 v0.9.0 lib/color (color.go, gradient.go) used by
// d2svg and d2themes.

/// color.IsThemeColor: `^(N[1-7]|B[1-6]|AA[245]|AB[45])$`.
pub(crate) fn is_theme_color(value: &str) -> bool {
    let b = value.as_bytes();
    match b {
        [b'N', d] => (b'1'..=b'7').contains(d),
        [b'B', d] => (b'1'..=b'6').contains(d),
        [b'A', b'A', d] => matches!(d, b'2' | b'4' | b'5'),
        [b'A', b'B', d] => matches!(d, b'4' | b'5'),
        _ => false,
    }
}

/// color.IsGradient: `^(linear|radial)-gradient\((.+)\)$` (`.` excludes '\n').
pub fn is_gradient(value: &str) -> bool {
    let rest = value
        .strip_prefix("linear-gradient(")
        .or_else(|| value.strip_prefix("radial-gradient("));
    match rest.and_then(|rest| rest.strip_suffix(')')) {
        Some(inner) => !inner.is_empty() && !inner.contains('\n'),
        None => false,
    }
}

/// color.IsURLGradientID: `^url\('#grad-[a-f0-9]{40}'\)$`.
pub(crate) fn is_url_gradient_id(value: &str) -> bool {
    value
        .strip_prefix("url('#grad-")
        .and_then(|rest| rest.strip_suffix("')"))
        .is_some_and(|hash| {
            hash.len() == 40
                && hash
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        })
}

/// color.UniqueGradientID: "grad-" + hex(sha1(css)).
pub(crate) fn unique_gradient_id(css_gradient: &str) -> String {
    let digest = sha1(css_gradient.as_bytes());
    let mut out = String::from("grad-");
    for byte in digest {
        out += &format!("{byte:02x}");
    }
    out
}

/// color.Luminance. The pinned Go ARM64 build fuses the weighted sum into
/// FMADDs: ((0.299*R) + 0.587*G) + 0.114*B, each addition fused.
pub(crate) fn luminance(value: &str) -> Result<f64, String> {
    let [r, g, b, _] =
        d2_rs_math::svg_color::parse(value).ok_or_else(|| format!("failed to parse {value:?}"))?;
    Ok(0.114f64.mul_add(b, 0.587f64.mul_add(g, 0.299 * r)))
}

/// color.LuminanceCategory.
pub(crate) fn luminance_category(value: &str) -> Result<&'static str, String> {
    if is_url_gradient_id(value) {
        return Ok("normal");
    }
    let l = luminance(value)?;
    Ok(if l >= 0.88 {
        "bright"
    } else if l >= 0.55 {
        "normal"
    } else if l >= 0.30 {
        "dark"
    } else {
        "darker"
    })
}

/// color.Darken.
pub(crate) fn darken(value: &str) -> Result<String, String> {
    if is_theme_color(value) {
        let b = value.as_bytes();
        let out = match (b[0], b[1], b.get(2)) {
            (b'B', b'1' | b'2', _) => "B1",
            (b'B', b'3', _) => "B2",
            (b'B', b'4', _) => "B3",
            (b'B', b'5', _) => "B4",
            (b'B', b'6', _) => "B5",
            (b'A', b'A', Some(b'2' | b'4')) => "AA2",
            (b'A', b'A', Some(b'5')) => "AA4",
            (b'A', b'B', Some(b'4')) => "AB4",
            (b'A', b'B', Some(b'5')) => "AB5",
            (b'N', b'1' | b'2', _) => "N1",
            (b'N', b'3', _) => "N2",
            (b'N', b'4', _) => "N3",
            (b'N', b'5', _) => "N4",
            (b'N', b'6', _) => "N5",
            (b'N', b'7', _) => "N6",
            _ => return Err(format!("invalid color \"{value}\"")),
        };
        return Ok(out.to_owned());
    }
    darken_css(value)
}

/// color.darkenCSS: go-colorful v1.4.1 Hsl round trip, lightness - 0.1.
fn darken_css(value: &str) -> Result<String, String> {
    let [r, g, b, _] =
        d2_rs_math::svg_color::parse(value).ok_or_else(|| format!("failed to parse {value:?}"))?;
    let (h, s, l) = colorful_hsl(r, g, b);
    let [r, g, b] = colorful_from_hsl(h, s, l - 0.1).map(|v| v.min(1.0).max(0.0));
    // Hex: uint8(c*255 + 0.5); the pinned ARM64 build fuses the sum.
    let byte = |v: f64| v.mul_add(255.0, 0.5) as u8;
    Ok(format!("#{:02x}{:02x}{:02x}", byte(r), byte(g), byte(b)))
}

/// go-colorful Color.Hsl.
fn colorful_hsl(r: f64, g: f64, b: f64) -> (f64, f64, f64) {
    let min = r.min(g).min(b);
    let max = r.max(g).max(b);
    let l = (max + min) / 2.0;
    if min == max {
        return (0.0, 0.0, l);
    }
    let s = if l < 0.5 {
        (max - min) / (max + min)
    } else {
        (max - min) / (2.0 - max - min)
    };
    let mut h = if max == r {
        (g - b) / (max - min)
    } else if max == g {
        2.0 + (b - r) / (max - min)
    } else {
        4.0 + (r - g) / (max - min)
    };
    h *= 60.0;
    if h < 0.0 {
        h += 360.0;
    }
    (h, s, l)
}

/// go-colorful Hsl (ARM64 FMADD/FMSUB fusions as the pinned build emits).
fn colorful_from_hsl(h: f64, s: f64, l: f64) -> [f64; 3] {
    if s == 0.0 {
        return [l, l, l];
    }
    let t1 = if l < 0.5 {
        l * (1.0 + s)
    } else {
        (-l).mul_add(s, l + s)
    };
    let t2 = 2.0 * l - t1;
    let h = h / 360.0;
    let wrap = |mut t: f64| {
        if t < 0.0 {
            t += 1.0;
        }
        if t > 1.0 {
            t -= 1.0;
        }
        t
    };
    let channel = |t: f64| {
        if 6.0 * t < 1.0 {
            ((t1 - t2) * 6.0).mul_add(t, t2)
        } else if 2.0 * t < 1.0 {
            t1
        } else if 3.0 * t < 2.0 {
            ((t1 - t2) * (2.0 / 3.0 - t)).mul_add(6.0, t2)
        } else {
            t2
        }
    };
    [
        channel(wrap(h + 1.0 / 3.0)),
        channel(wrap(h)),
        channel(wrap(h - 1.0 / 3.0)),
    ]
}

/// color.Gradient.
#[derive(Clone, Debug, Default)]
pub(crate) struct Gradient {
    pub(crate) kind: String,
    pub(crate) direction: String,
    pub(crate) color_stops: Vec<(String, String)>,
    pub(crate) id: String,
}

/// color.ParseGradient (errors yield Go's zero Gradient at the d2svg call).
pub(crate) fn parse_gradient(css: &str) -> Option<Gradient> {
    let css = css.trim_matches(|c: char| c.is_whitespace());
    let (kind, rest) = if let Some(rest) = css.strip_prefix("linear-gradient(") {
        ("linear", rest)
    } else {
        let rest = css.strip_prefix("radial-gradient(")?;
        ("radial", rest)
    };
    let params = rest.strip_suffix(')')?;
    if params.contains('\n') {
        return None;
    }
    let list = split_params(params);
    let first = list.first()?.trim();
    let mut gradient = Gradient {
        kind: kind.into(),
        ..Default::default()
    };
    let stops: &[String] = if (kind == "linear"
        && (first.ends_with("deg") || first.starts_with("to ")))
        || (kind == "radial" && (first == "circle" || first == "ellipse"))
    {
        gradient.direction = first.to_owned();
        if list.len() < 2 {
            return None;
        }
        &list[1..]
    } else {
        &list
    };
    for p in stops {
        let parts: Vec<&str> = p.split_whitespace().collect();
        match parts.as_slice() {
            [c] => gradient.color_stops.push(((*c).to_owned(), String::new())),
            [c, pos] => gradient
                .color_stops
                .push(((*c).to_owned(), (*pos).to_owned())),
            _ => {}
        }
    }
    gradient.id = unique_gradient_id(css);
    Some(gradient)
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

/// color.GradientToSVG.
pub(crate) fn gradient_to_svg(g: &Gradient) -> String {
    let stops = |out: &mut String| {
        let total = g.color_stops.len();
        for (i, (color, position)) in g.color_stops.iter().enumerate() {
            let offset = if !position.is_empty() {
                position.clone()
            } else if total == 1 {
                "0.00%".into()
            } else {
                format!("{:.2}%", i as f64 / (total - 1) as f64 * 100.0)
            };
            out.push_str(&format!(
                "<stop offset=\"{offset}\" stop-color=\"{color}\" />\n"
            ));
        }
    };
    match g.kind.as_str() {
        "linear" => {
            let (x1, y1, x2, y2) = linear_direction(&g.direction);
            let mut out = format!(
                "<linearGradient id=\"{}\" x1=\"{x1}\" y1=\"{y1}\" x2=\"{x2}\" y2=\"{y2}\">\n",
                g.id
            );
            stops(&mut out);
            out + "</linearGradient>"
        }
        "radial" => {
            let mut out = format!("<radialGradient id=\"{}\">\n", g.id);
            stops(&mut out);
            out + "</radialGradient>"
        }
        _ => String::new(),
    }
}

/// color.parseLinearGradientDirection.
fn linear_direction(direction: &str) -> (String, String, String, String) {
    let direction = direction.trim();
    if let Some(dir) = direction.strip_prefix("to ") {
        let (mut xs, mut ys, mut xe, mut ye) = ("50%", "50%", "50%", "50%");
        let (mut x_set, mut y_set) = (false, false);
        for part in dir.split_whitespace() {
            match part {
                "left" => (xs, xe, x_set) = ("100%", "0%", true),
                "right" => (xs, xe, x_set) = ("0%", "100%", true),
                "top" => (ys, ye, y_set) = ("100%", "0%", true),
                "bottom" => (ys, ye, y_set) = ("0%", "100%", true),
                _ => {}
            }
        }
        if !x_set {
            (xs, xe) = ("50%", "50%");
        }
        if !y_set {
            (ys, ye) = ("50%", "50%");
        }
        return (xs.into(), ys.into(), xe.into(), ye.into());
    }
    if let Some(angle) = direction.strip_suffix("deg") {
        if let Some(angle) = d2_rs_core::parse_d2_float(angle.trim()) {
            let svg_angle = (90.0 - angle) * (std::f64::consts::PI / 180.0);
            // Go ARM64 fuses x1f + 50*cos(a) and y1f + 50*sin(a).
            let x2 = 50.0f64.mul_add(super::geo::gomath::cos(svg_angle), 50.0);
            let y2 = 50.0f64.mul_add(super::geo::gomath::sin(svg_angle), 50.0);
            return (
                "50.00%".into(),
                "50.00%".into(),
                format!("{x2:.2}%"),
                format!("{y2:.2}%"),
            );
        }
    }
    ("0%".into(), "0%".into(), "0%".into(), "100%".into())
}

/// Minimal SHA-1 (crypto/sha1) for gradient identifiers.
fn sha1(data: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [
        0x6745_2301,
        0xEFCD_AB89,
        0x98BA_DCFE,
        0x1032_5476,
        0xC3D2_E1F0,
    ];
    let mut message = data.to_vec();
    let bit_len = (data.len() as u64).wrapping_mul(8);
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&bit_len.to_be_bytes());
    for block in message.chunks(64) {
        let mut w = [0u32; 80];
        for (i, word) in block.chunks(4).enumerate() {
            w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let [mut a, mut b, mut c, mut d, mut e] = h;
        for (i, word) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | (!b & d), 0x5A82_7999),
                20..=39 => (b ^ c ^ d, 0x6ED9_EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC),
                _ => (b ^ c ^ d, 0xCA62_C1D6),
            };
            let temp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(*word);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = temp;
        }
        for (slot, value) in h.iter_mut().zip([a, b, c, d, e]) {
            *slot = slot.wrapping_add(value);
        }
    }
    let mut out = [0u8; 20];
    for (i, word) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha1_matches_known_vectors() {
        assert_eq!(
            unique_gradient_id("abc"),
            "grad-a9993e364706816aba3e25717850c26c9cd0d89d"
        );
        assert_eq!(
            unique_gradient_id(""),
            "grad-da39a3ee5e6b4b0d3255bfef95601890afd80709"
        );
    }

    #[test]
    fn theme_colors_and_gradients() {
        assert!(is_theme_color("AA4") && is_theme_color("N7") && !is_theme_color("N8"));
        assert!(!is_theme_color("AB2") && !is_theme_color("b1"));
        assert!(is_gradient("linear-gradient(#000, #fff)"));
        assert!(!is_gradient("linear-gradient()"));
    }
}
