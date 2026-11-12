use std::fmt;

use cssparser::{color::PredefinedColorSpace, Delimiter, ParseError, Parser, Token};
use cssparser_color::Color as ParsedColor;

use crate::portable_math::{atan2, cos, hypot, sin, square};
use crate::ColorMode;

mod conversion;

use conversion::{
    a98_rgb_to_xyz_d65, adapt_d50_to_d65, adapt_d65_to_d50, decode_srgb, display_p3_to_xyz_d65,
    encode_srgb, lab_d50_to_srgb, linear_display_p3_to_xyz_d65, prophoto_rgb_to_xyz_d65,
    rec2020_to_xyz_d65, srgb_to_a98_rgb, srgb_to_display_p3, srgb_to_lab_d50, srgb_to_prophoto_rgb,
    srgb_to_rec2020, srgb_to_xyz_d65, xyz_d65_to_linear_srgb,
};

const MAX_COLOR_NESTING_DEPTH: u32 = 128;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct CanonicalColor {
    red: u8,
    green: u8,
    blue: u8,
    alpha: f64,
}

impl CanonicalColor {
    pub(crate) const fn from_rgb8(red: u8, green: u8, blue: u8) -> Self {
        Self {
            red,
            green,
            blue,
            alpha: 1.0,
        }
    }

    const fn from_rgba(red: u8, green: u8, blue: u8, alpha: f64) -> Self {
        Self {
            red,
            green,
            blue,
            alpha,
        }
    }

    fn from_unit(red: f64, green: f64, blue: f64, alpha: f64) -> Self {
        Self::from_rgba(
            unit_channel(red),
            unit_channel(green),
            unit_channel(blue),
            f64::from(unit_channel(alpha)) / 255.0,
        )
    }

    pub(crate) fn for_mode(self, mode: ColorMode) -> Self {
        match mode {
            ColorMode::Light => self,
            ColorMode::Dark => self.lowered(ColorLowering::Floor),
        }
    }

    /// The color after the dark-mode filter is lowered into it the way one
    /// format's authority quantizes it (see [`ColorLowering`]).
    pub(crate) fn lowered(self, lowering: ColorLowering) -> Self {
        let filter = match lowering {
            ColorLowering::None => return self,
            ColorLowering::Floor => theme_filter,
            ColorLowering::Round => theme_filter_rounded,
        };
        let [red, green, blue] = filter(self.rgb8());
        Self {
            red,
            green,
            blue,
            alpha: self.alpha,
        }
    }

    pub(crate) const fn alpha(self) -> f64 {
        self.alpha
    }

    pub(crate) const fn rgb8(self) -> [u8; 3] {
        [self.red, self.green, self.blue]
    }

    #[cfg(any(feature = "png", test))]
    pub(crate) fn rgba8(self) -> [u8; 4] {
        [
            self.red,
            self.green,
            self.blue,
            (self.alpha * 255.0).round() as u8,
        ]
    }

    pub(crate) fn rgb_f64(self) -> [f64; 3] {
        [
            f64::from(self.red) / 255.0,
            f64::from(self.green) / 255.0,
            f64::from(self.blue) / 255.0,
        ]
    }

    pub(crate) fn write_hex(self, output: &mut (impl fmt::Write + ?Sized)) -> fmt::Result {
        write!(
            output,
            "#{:02x}{:02x}{:02x}",
            self.red, self.green, self.blue
        )
    }
}

/// The reference export's dark-mode filter, `THEME_FILTER`
/// (`invert(93%) hue-rotate(180deg)`), on one opaque or unpremultiplied sRGB
/// pixel. The reference's SVG carries it on the root element and its canvas
/// export sets it on the context, so every scene color, frame outline, frame
/// name, placeholder, and nested-SVG image pixel passes through it; the
/// renderer lowers it into the colors instead. Evaluated as the pinned browser
/// evaluates CSS filter functions: in gamma-encoded sRGB, with the Filter
/// Effects hue-rotation matrix, each function's result clamped to the channel
/// range, and the result quantized. Each channel of the `invert(93%)` result
/// is `237.15 - 0.86 * c`, never an integer, so flooring cannot lose a channel
/// to floating-point error the way it could for an exact gray.
pub(crate) fn theme_filter(rgb: [u8; 3]) -> [u8; 3] {
    let [red, green, blue] = hue_rotate_180(invert(rgb.map(f64::from), [0.93, 0.07]));
    [red, green, blue].map(|value| value.clamp(0.0, 255.0).floor() as u8)
}

/// [`theme_filter`] rounded to the nearest 8-bit value, as the pinned browser
/// quantizes the reference SVG's root filter: the rounded exact filter of the
/// composited light image reproduces its dark raster (varmq #84), where
/// flooring is one level low in any channel with a fraction of one half or
/// more. PDF, whose authority is the official SVG, lowers with it.
pub(crate) fn theme_filter_rounded(rgb: [u8; 3]) -> [u8; 3] {
    let [red, green, blue] = hue_rotate_180(invert(rgb.map(f64::from), [0.93, 0.07]));
    [red, green, blue].map(|value| value.clamp(0.0, 255.0).round() as u8)
}

/// How one format lowers the dark-mode filter into the colors it paints. SVG
/// carries the reference's root filter and lowers nothing; PNG follows the
/// canvas, which floors; PDF follows the official SVG, which rounds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ColorLowering {
    None,
    Floor,
    Round,
}

/// The filter the reference's SVG export puts on a raster image's `<use>` in
/// dark mode, `IMAGE_INVERT_FILTER`
/// (`invert(100%) hue-rotate(180deg) saturate(1.25)`), on one opaque or
/// unpremultiplied sRGB pixel. It precedes the root filter, so a raster image
/// comes out roughly as authored with its contrast compressed; nested-SVG
/// images do not receive it and invert with the root filter alone. The result
/// rounds to the 8-bit surface the root filter then reads, so an exact gray,
/// which every function here maps to an exact gray, keeps its channels equal.
#[cfg(any(feature = "pdf", test))]
pub(crate) fn image_invert_filter(rgb: [u8; 3]) -> [u8; 3] {
    let inverted = hue_rotate_180(invert(rgb.map(f64::from), [1.0, 0.0]));
    saturate(inverted, 1.25).map(|value| value.clamp(0.0, 255.0).round() as u8)
}

/// A raster image pixel as the reference's dark-mode SVG export shows it: the
/// image filter, then the root filter, rounded as the browser rounds it.
/// Official SVG is the visual authority for PDF (spec section 8.6), so PDF
/// lowers this into its image pixels; the reference's canvas export resets the
/// filter for raster images and PNG leaves their pixels alone.
#[cfg(any(feature = "pdf", test))]
pub(crate) fn dark_mode_raster_image_pixel(rgb: [u8; 3]) -> [u8; 3] {
    theme_filter_rounded(image_invert_filter(rgb))
}

/// `invert(amount)`: `feComponentTransfer` with the table `amount, 1 - amount`
/// on each channel, which weights the inverted channel by the first value and
/// the channel itself by the second and stays inside the channel range. The
/// table is spelled literally so `0.07` is the constant, not `1.0 - 0.93`.
fn invert(rgb: [f64; 3], [inverted, kept]: [f64; 2]) -> [f64; 3] {
    rgb.map(|channel| inverted * (255.0 - channel) + kept * channel)
}

/// `hue-rotate(180deg)`: the Filter Effects `hueRotate` matrix at 180 degrees,
/// clamped to the channel range as the browser clamps each function's result.
fn hue_rotate_180([red, green, blue]: [f64; 3]) -> [f64; 3] {
    [
        -0.574 * red + 1.430 * green + 0.144 * blue,
        0.426 * red + 0.430 * green + 0.144 * blue,
        0.426 * red + 1.430 * green - 0.856 * blue,
    ]
    .map(|value| value.clamp(0.0, 255.0))
}

/// `saturate(amount)`: the Filter Effects `saturate` matrix, clamped like the
/// hue rotation.
#[cfg(any(feature = "pdf", test))]
fn saturate([red, green, blue]: [f64; 3], amount: f64) -> [f64; 3] {
    [
        (0.213 + 0.787 * amount) * red
            + (0.715 - 0.715 * amount) * green
            + (0.072 - 0.072 * amount) * blue,
        (0.213 - 0.213 * amount) * red
            + (0.715 + 0.285 * amount) * green
            + (0.072 - 0.072 * amount) * blue,
        (0.213 - 0.213 * amount) * red
            + (0.715 - 0.715 * amount) * green
            + (0.072 + 0.928 * amount) * blue,
    ]
    .map(|value| value.clamp(0.0, 255.0))
}

pub(crate) fn parse_css(value: &str) -> Option<CanonicalColor> {
    parse_css_at_depth(value, 0)
}

fn parse_css_at_depth(value: &str, depth: u32) -> Option<CanonicalColor> {
    if depth > MAX_COLOR_NESTING_DEPTH {
        return None;
    }
    let value = value.trim();
    if let Some(color) = system_color(value) {
        return Some(color);
    }
    if let Some((name, _)) = value.split_once('(') {
        if name.eq_ignore_ascii_case("color-mix") {
            return parse_color_mix(value, depth);
        }
    }

    let mut parser = Parser::new(value);
    let parsed = ParsedColor::parse(&mut parser).ok()?;
    parser.expect_exhausted().ok()?;
    if let ParsedColor::CurrentColor = parsed {
        return Some(CanonicalColor::from_rgb8(0, 0, 0));
    }
    if let ParsedColor::Lab(color) = parsed {
        return Some(canonical_lab_color(
            f64::from(color.lightness.unwrap_or(0.0)),
            f64::from(color.a.unwrap_or(0.0)),
            f64::from(color.b.unwrap_or(0.0)),
            f64::from(color.alpha.unwrap_or(0.0)),
        ));
    }
    if let ParsedColor::Lch(color) = parsed {
        let hue = f64::from(color.hue.unwrap_or(0.0)).to_radians();
        let chroma = f64::from(color.chroma.unwrap_or(0.0));
        return Some(canonical_lab_color(
            f64::from(color.lightness.unwrap_or(0.0)),
            chroma * cos(hue),
            chroma * sin(hue),
            f64::from(color.alpha.unwrap_or(0.0)),
        ));
    }
    if let ParsedColor::ColorFunction(color) = parsed {
        return color_function(
            color.color_space,
            color.c1.map_or(0.0, f64::from),
            color.c2.map_or(0.0, f64::from),
            color.c3.map_or(0.0, f64::from),
            color.alpha.map_or(0.0, f64::from),
        );
    }
    let color = csscolorparser::parse(value).ok()?;
    Some(canonical_css_color(color))
}

fn color_function(
    space: PredefinedColorSpace,
    first: f64,
    second: f64,
    third: f64,
    alpha: f64,
) -> Option<CanonicalColor> {
    let xyz_d65 = match space {
        PredefinedColorSpace::Srgb => {
            return Some(CanonicalColor::from_unit(first, second, third, alpha));
        }
        PredefinedColorSpace::SrgbLinear => {
            return Some(CanonicalColor::from_unit(
                encode_srgb(first),
                encode_srgb(second),
                encode_srgb(third),
                alpha,
            ));
        }
        PredefinedColorSpace::DisplayP3 => display_p3_to_xyz_d65([first, second, third]),
        PredefinedColorSpace::DisplayP3Linear => {
            linear_display_p3_to_xyz_d65([first, second, third])
        }
        PredefinedColorSpace::A98Rgb => a98_rgb_to_xyz_d65([first, second, third]),
        PredefinedColorSpace::ProphotoRgb => prophoto_rgb_to_xyz_d65([first, second, third]),
        PredefinedColorSpace::Rec2020 => rec2020_to_xyz_d65([first, second, third]),
        PredefinedColorSpace::XyzD65 => [first, second, third],
        PredefinedColorSpace::XyzD50 => adapt_d50_to_d65([first, second, third]),
    };
    let srgb_linear = xyz_d65_to_linear_srgb(xyz_d65);
    Some(CanonicalColor::from_unit(
        encode_srgb(srgb_linear[0]),
        encode_srgb(srgb_linear[1]),
        encode_srgb(srgb_linear[2]),
        alpha,
    ))
}

#[derive(Clone, Copy)]
enum ColorMixSpace {
    Srgb,
    SrgbLinear,
    DisplayP3,
    A98Rgb,
    ProphotoRgb,
    Rec2020,
    Lab,
    Oklab,
    XyzD50,
    XyzD65,
    Hsl,
    Hwb,
    Lch,
    Oklch,
}

impl ColorMixSpace {
    const fn is_polar(self) -> bool {
        matches!(self, Self::Hsl | Self::Hwb | Self::Lch | Self::Oklch)
    }

    const fn hue_index(self) -> usize {
        match self {
            Self::Hsl | Self::Hwb => 0,
            Self::Lch | Self::Oklch => 2,
            _ => 0,
        }
    }
}

#[derive(Clone, Copy, Default)]
enum HueInterpolation {
    #[default]
    Shorter,
    Longer,
    Increasing,
    Decreasing,
}

fn parse_color_mix(value: &str, depth: u32) -> Option<CanonicalColor> {
    let [interpolation, first, second] = color_mix_parts(value)?;
    mix_colors(interpolation, first, second, depth)
}

fn color_mix_parts(value: &str) -> Option<[&str; 3]> {
    if value.contains("/*") || !crate::css_syntax::fits_inline_block_stack(value) {
        // Comments were visible to the old delimiter grammar. Preserve that
        // unusual policy instead of letting opaque comment tokens widen it.
        let body = value.get("color-mix(".len()..)?.strip_suffix(')')?;
        let ([Some(first), Some(second)], _) = compatibility_color_boundaries(body)? else {
            return None;
        };
        return Some([
            &body[..first],
            &body[first + 1..second],
            &body[second + 1..],
        ]);
    }
    let mut parser = Parser::new(value);
    parser.expect_function_matching("color-mix").ok()?;
    let ([interpolation, first, second], end) = parser
        .parse_nested_block(|body| {
            // The grammar has exactly three parts: never allocate a comma list.
            let mut parts = [""; 3];
            for (index, part) in parts.iter_mut().enumerate() {
                *part = body.parse_until_before(Delimiter::Comma, |part| {
                    let start = part.position();
                    color_tokens(part, 0)?;
                    Ok(part.slice_from(start))
                })?;
                if index != 2 {
                    body.expect_comma()?;
                }
            }
            Ok((parts, body.position()))
        })
        .ok()?;
    // cssparser recovers an EOF-closed function; this contract requires ')'.
    if parser.slice(end..parser.position()) != ")" {
        return None;
    }
    parser.expect_exhausted().ok()?;
    Some([interpolation, first, second])
}

fn mix_colors(
    interpolation: &str,
    first: &str,
    second: &str,
    depth: u32,
) -> Option<CanonicalColor> {
    let mut interpolation = interpolation.split_ascii_whitespace();
    if !interpolation.next()?.eq_ignore_ascii_case("in") {
        return None;
    }
    let space = match interpolation.next()? {
        value if value.eq_ignore_ascii_case("srgb") => ColorMixSpace::Srgb,
        value if value.eq_ignore_ascii_case("srgb-linear") => ColorMixSpace::SrgbLinear,
        value if value.eq_ignore_ascii_case("display-p3") => ColorMixSpace::DisplayP3,
        value if value.eq_ignore_ascii_case("a98-rgb") => ColorMixSpace::A98Rgb,
        value if value.eq_ignore_ascii_case("prophoto-rgb") => ColorMixSpace::ProphotoRgb,
        value if value.eq_ignore_ascii_case("rec2020") => ColorMixSpace::Rec2020,
        value if value.eq_ignore_ascii_case("lab") => ColorMixSpace::Lab,
        value if value.eq_ignore_ascii_case("oklab") => ColorMixSpace::Oklab,
        value if value.eq_ignore_ascii_case("xyz") || value.eq_ignore_ascii_case("xyz-d65") => {
            ColorMixSpace::XyzD65
        }
        value if value.eq_ignore_ascii_case("xyz-d50") => ColorMixSpace::XyzD50,
        value if value.eq_ignore_ascii_case("hsl") => ColorMixSpace::Hsl,
        value if value.eq_ignore_ascii_case("hwb") => ColorMixSpace::Hwb,
        value if value.eq_ignore_ascii_case("lch") => ColorMixSpace::Lch,
        value if value.eq_ignore_ascii_case("oklch") => ColorMixSpace::Oklch,
        _ => return None,
    };
    let hue_interpolation = if space.is_polar() {
        match interpolation.next() {
            None => HueInterpolation::Shorter,
            Some(value) => {
                let method = match value {
                    value if value.eq_ignore_ascii_case("shorter") => HueInterpolation::Shorter,
                    value if value.eq_ignore_ascii_case("longer") => HueInterpolation::Longer,
                    value if value.eq_ignore_ascii_case("increasing") => {
                        HueInterpolation::Increasing
                    }
                    value if value.eq_ignore_ascii_case("decreasing") => {
                        HueInterpolation::Decreasing
                    }
                    _ => return None,
                };
                if !interpolation.next()?.eq_ignore_ascii_case("hue") {
                    return None;
                }
                method
            }
        }
    } else {
        HueInterpolation::Shorter
    };
    if interpolation.next().is_some() {
        return None;
    }
    let (first, first_weight) = color_mix_stop(first, depth)?;
    let (second, second_weight) = color_mix_stop(second, depth)?;
    let (first_weight, second_weight) = match (first_weight, second_weight) {
        (None, None) => (0.5, 0.5),
        (Some(first), None) => (first, 1.0 - first),
        (None, Some(second)) => (1.0 - second, second),
        (Some(first), Some(second)) => (first, second),
    };
    if first_weight < 0.0 || second_weight < 0.0 {
        return None;
    }
    let total = first_weight + second_weight;
    if total == 0.0 {
        return None;
    }
    let alpha_multiplier = total.min(1.0);
    let first_weight = first_weight / total;
    let second_weight = second_weight / total;
    let first_alpha = first.alpha * first_weight;
    let second_alpha = second.alpha * second_weight;
    let alpha = first_alpha + second_alpha;
    let mix = |first: f64, second: f64| {
        if alpha == 0.0 {
            0.0
        } else {
            (first * first_alpha + second * second_alpha) / alpha
        }
    };
    let mixed_alpha = alpha * alpha_multiplier;
    let mut first = color_mix_components(space, first);
    let mut second = color_mix_components(space, second);
    if space.is_polar() {
        let hue = space.hue_index();
        [first[hue], second[hue]] = adjusted_hues(first[hue], second[hue], hue_interpolation);
    }
    color_from_mix_components(
        space,
        [
            mix(first[0], second[0]),
            mix(first[1], second[1]),
            mix(first[2], second[2]),
        ],
        mixed_alpha,
    )
}

fn color_mix_stop(value: &str, depth: u32) -> Option<(CanonicalColor, Option<f64>)> {
    let value = value.trim();
    let split = if value.contains("/*") || !crate::css_syntax::fits_inline_block_stack(value) {
        compatibility_color_boundaries(value)?.1
    } else {
        color_tokens(&mut Parser::new(value), 0).ok()?
    };
    if let Some(split) = split {
        if let Some(weight) = value[split..].trim().strip_suffix('%') {
            // Keep the original number token, not cssparser's f32 percentage.
            // Rust also historically admits `25.%`; retain that spelling here.
            let weight = weight.parse::<f64>().ok()? / 100.0;
            if !(0.0..=1.0).contains(&weight) {
                return None;
            }
            return Some((
                parse_css_at_depth(value[..split].trim(), depth.checked_add(1)?)?,
                Some(weight),
            ));
        }
    }
    Some((parse_css_at_depth(value, depth.checked_add(1)?)?, None))
}

/// Consume bounded component values, retaining the last top-level whitespace
/// position for the historical weight grammar. Work is linear per invocation;
/// mix evaluation makes at most MAX_COLOR_NESTING_DEPTH + 1 such passes.
fn color_tokens<'i>(parser: &mut Parser<'i>, depth: u32) -> Result<Option<usize>, ParseError<()>> {
    // A deepest admitted stop may itself be rgb()/color(), hence the extra level.
    if depth > MAX_COLOR_NESTING_DEPTH + 1 {
        return Err(ParseError::custom(()));
    }
    let start = parser.position();
    let mut whitespace = None;
    loop {
        let Ok(token) = parser.next_including_whitespace_and_comments() else {
            return Ok(whitespace);
        };
        match token {
            Token::Function(_) | Token::ParenthesisBlock => {
                let body_start = parser.position();
                let end = parser.parse_nested_block(|body| {
                    color_tokens(body, depth + 1)?;
                    Ok(body.position())
                })?;
                if parser.slice(end..parser.position()) != ")" {
                    return Err(ParseError::custom(()));
                }
                debug_assert!(parser.position() > body_start);
            }
            Token::WhiteSpace(_) => {
                whitespace = Some(parser.slice(start..parser.position()).len() - 1);
            }
            Token::SquareBracketBlock | Token::CurlyBracketBlock | Token::QuotedString(_) => {
                return Err(ParseError::custom(()));
            }
            token if token.is_parse_error() => return Err(ParseError::custom(())),
            _ => {}
        }
    }
}

/// Residual scan for comments and values outside the inline-tokenizer guard.
/// In particular,
/// `currentcolor/*(,)*/` was admitted but `currentcolor/*,*/` was not. No CSS
/// tokenizer exposes delimiters inside comments as grammar tokens. Two comma
/// slots and a checked depth retain the original allocation-free work bound.
fn compatibility_color_boundaries(value: &str) -> Option<([Option<usize>; 2], Option<usize>)> {
    let mut commas = [None; 2];
    let mut comma_count = 0;
    let mut whitespace = None;
    let mut depth = 0_u32;
    for (index, character) in value.char_indices() {
        match character {
            '(' => depth = depth.checked_add(1)?,
            ')' => depth = depth.checked_sub(1)?,
            ',' if depth == 0 => {
                *commas.get_mut(comma_count)? = Some(index);
                comma_count += 1;
            }
            c if c.is_ascii_whitespace() && depth == 0 => whitespace = Some(index),
            _ => {}
        }
    }
    (depth == 0).then_some((commas, whitespace))
}

fn css_color(color: CanonicalColor) -> csscolorparser::Color {
    csscolorparser::Color::from_rgba8(
        color.red,
        color.green,
        color.blue,
        (color.alpha * 255.0).round() as u8,
    )
}

fn color_mix_components(space: ColorMixSpace, color: CanonicalColor) -> [f64; 3] {
    let css = css_color(color);
    let rgb = color.rgb_f64();
    match space {
        ColorMixSpace::Srgb => rgb,
        ColorMixSpace::SrgbLinear => rgb.map(decode_srgb),
        ColorMixSpace::DisplayP3 => srgb_to_display_p3(rgb),
        ColorMixSpace::A98Rgb => srgb_to_a98_rgb(rgb),
        ColorMixSpace::ProphotoRgb => srgb_to_prophoto_rgb(rgb),
        ColorMixSpace::Rec2020 => srgb_to_rec2020(rgb),
        ColorMixSpace::Lab => srgb_to_lab_d50(rgb),
        ColorMixSpace::Oklab => {
            let value = css.to_oklaba();
            [
                f64::from(value[0]),
                f64::from(value[1]),
                f64::from(value[2]),
            ]
        }
        ColorMixSpace::XyzD50 => adapt_d65_to_d50(srgb_to_xyz_d65(rgb)),
        ColorMixSpace::XyzD65 => srgb_to_xyz_d65(rgb),
        ColorMixSpace::Hsl => {
            let value = css.to_hsla();
            [
                f64::from(value[0]),
                f64::from(value[1]),
                f64::from(value[2]),
            ]
        }
        ColorMixSpace::Hwb => {
            let value = css.to_hwba();
            [
                f64::from(value[0]),
                f64::from(value[1]),
                f64::from(value[2]),
            ]
        }
        ColorMixSpace::Lch => {
            let [lightness, a, b] = srgb_to_lab_d50(rgb);
            [lightness, hypot(a, b), atan2(b, a).to_degrees()]
        }
        ColorMixSpace::Oklch => {
            let value = css.to_oklcha();
            [
                f64::from(value[0]),
                f64::from(value[1]),
                f64::from(value[2]).to_degrees(),
            ]
        }
    }
}

fn color_from_mix_components(
    space: ColorMixSpace,
    components: [f64; 3],
    alpha: f64,
) -> Option<CanonicalColor> {
    match space {
        ColorMixSpace::Srgb => Some(CanonicalColor::from_unit(
            components[0],
            components[1],
            components[2],
            alpha,
        )),
        ColorMixSpace::SrgbLinear => Some(CanonicalColor::from_unit(
            encode_srgb(components[0]),
            encode_srgb(components[1]),
            encode_srgb(components[2]),
            alpha,
        )),
        ColorMixSpace::DisplayP3 => color_function(
            PredefinedColorSpace::DisplayP3,
            components[0],
            components[1],
            components[2],
            alpha,
        ),
        ColorMixSpace::A98Rgb => color_function(
            PredefinedColorSpace::A98Rgb,
            components[0],
            components[1],
            components[2],
            alpha,
        ),
        ColorMixSpace::ProphotoRgb => color_function(
            PredefinedColorSpace::ProphotoRgb,
            components[0],
            components[1],
            components[2],
            alpha,
        ),
        ColorMixSpace::Rec2020 => color_function(
            PredefinedColorSpace::Rec2020,
            components[0],
            components[1],
            components[2],
            alpha,
        ),
        ColorMixSpace::Lab => Some(canonical_lab_color(
            components[0],
            components[1],
            components[2],
            alpha,
        )),
        ColorMixSpace::Oklab => Some(canonical_css_color(csscolorparser::Color::from_oklaba(
            components[0] as f32,
            components[1] as f32,
            components[2] as f32,
            alpha as f32,
        ))),
        ColorMixSpace::XyzD50 => color_function(
            PredefinedColorSpace::XyzD50,
            components[0],
            components[1],
            components[2],
            alpha,
        ),
        ColorMixSpace::XyzD65 => color_function(
            PredefinedColorSpace::XyzD65,
            components[0],
            components[1],
            components[2],
            alpha,
        ),
        ColorMixSpace::Hsl => Some(canonical_css_color(csscolorparser::Color::from_hsla(
            components[0] as f32,
            components[1] as f32,
            components[2] as f32,
            alpha as f32,
        ))),
        ColorMixSpace::Hwb => Some(canonical_css_color(csscolorparser::Color::from_hwba(
            components[0] as f32,
            components[1] as f32,
            components[2] as f32,
            alpha as f32,
        ))),
        ColorMixSpace::Lch => {
            let hue = components[2].to_radians();
            Some(canonical_lab_color(
                components[0],
                components[1] * cos(hue),
                components[1] * sin(hue),
                alpha,
            ))
        }
        ColorMixSpace::Oklch => Some(canonical_css_color(csscolorparser::Color::from_oklcha(
            components[0] as f32,
            components[1] as f32,
            components[2].to_radians() as f32,
            alpha as f32,
        ))),
    }
}

fn adjusted_hues(first: f64, second: f64, method: HueInterpolation) -> [f64; 2] {
    let first = first.rem_euclid(360.0);
    let mut second = second.rem_euclid(360.0);
    let difference = second - first;
    match method {
        HueInterpolation::Shorter if difference > 180.0 => second -= 360.0,
        HueInterpolation::Shorter if difference < -180.0 => second += 360.0,
        HueInterpolation::Longer if (0.0..180.0).contains(&difference) => second -= 360.0,
        HueInterpolation::Longer if (-180.0..=0.0).contains(&difference) => second += 360.0,
        HueInterpolation::Increasing if second < first => second += 360.0,
        HueInterpolation::Decreasing if first < second => second -= 360.0,
        HueInterpolation::Shorter
        | HueInterpolation::Longer
        | HueInterpolation::Increasing
        | HueInterpolation::Decreasing => {}
    }
    [first, second]
}

fn canonical_lab_color(lightness: f64, a: f64, b: f64, alpha: f64) -> CanonicalColor {
    let [red, green, blue] = lab_d50_to_srgb([lightness, a, b]);
    canonical_css_color(csscolorparser::Color::new(
        red as f32,
        green as f32,
        blue as f32,
        alpha as f32,
    ))
}

fn canonical_css_color(color: csscolorparser::Color) -> CanonicalColor {
    let color = map_srgb_gamut(color);
    CanonicalColor::from_unit(
        f64::from(color.r),
        f64::from(color.g),
        f64::from(color.b),
        f64::from(color.a),
    )
}

fn map_srgb_gamut(color: csscolorparser::Color) -> csscolorparser::Color {
    const JND: f32 = 0.02;
    const EPSILON: f32 = 0.000_01;

    if [color.r, color.g, color.b]
        .into_iter()
        .all(|component| (0.0..=1.0).contains(&component))
    {
        return color;
    }
    let [lightness, mut chroma, hue, alpha] = color.to_oklcha();
    if lightness >= 1.0 - EPSILON {
        return csscolorparser::Color::new(1.0, 1.0, 1.0, alpha);
    }
    if lightness < EPSILON {
        return csscolorparser::Color::new(0.0, 0.0, 0.0, alpha);
    }

    let mut minimum = 0.0;
    let mut maximum = chroma;
    while maximum - minimum > EPSILON {
        chroma = (minimum + maximum) / 2.0;
        let current = csscolorparser::Color::from_oklcha(lightness, chroma, hue, alpha);
        if [current.r, current.g, current.b]
            .into_iter()
            .all(|component| (0.0..=1.0).contains(&component))
        {
            minimum = chroma;
            continue;
        }
        let clipped = csscolorparser::Color::new(
            current.r.clamp(0.0, 1.0),
            current.g.clamp(0.0, 1.0),
            current.b.clamp(0.0, 1.0),
            alpha,
        );
        let clipped_oklab = clipped.to_oklaba();
        let current_oklab = current.to_oklaba();
        let delta = (square(clipped_oklab[0] - current_oklab[0])
            + square(clipped_oklab[1] - current_oklab[1])
            + square(clipped_oklab[2] - current_oklab[2]))
        .sqrt();
        if delta < JND {
            return clipped;
        }
        maximum = chroma;
    }
    csscolorparser::Color::from_oklcha(lightness, chroma, hue, alpha)
}

fn unit_channel(value: f64) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0).round() as u8
}

fn system_color(value: &str) -> Option<CanonicalColor> {
    let color = if value.eq_ignore_ascii_case("ActiveText") {
        CanonicalColor::from_rgb8(255, 0, 0)
    } else if [
        "ButtonFace",
        "ButtonHighlight",
        "ButtonShadow",
        "ThreeDFace",
    ]
    .iter()
    .any(|name| value.eq_ignore_ascii_case(name))
    {
        CanonicalColor::from_rgb8(239, 239, 239)
    } else if [
        "Canvas",
        "Field",
        "HighlightText",
        "MarkText",
        "SelectedItemText",
        "ActiveCaption",
        "AppWorkspace",
        "Background",
        "InactiveCaption",
        "InfoBackground",
        "Menu",
        "Scrollbar",
        "Window",
    ]
    .iter()
    .any(|name| value.eq_ignore_ascii_case(name))
    {
        CanonicalColor::from_rgb8(255, 255, 255)
    } else if ["GrayText", "InactiveCaptionText"]
        .iter()
        .any(|name| value.eq_ignore_ascii_case(name))
    {
        CanonicalColor::from_rgb8(128, 128, 128)
    } else if value.eq_ignore_ascii_case("Highlight") {
        CanonicalColor::from_rgba(0, 65, 198, 0.8)
    } else if value.eq_ignore_ascii_case("LinkText") {
        CanonicalColor::from_rgb8(0, 0, 238)
    } else if value.eq_ignore_ascii_case("Mark") {
        CanonicalColor::from_rgb8(255, 255, 0)
    } else if value.eq_ignore_ascii_case("SelectedItem") {
        CanonicalColor::from_rgb8(25, 103, 210)
    } else if value.eq_ignore_ascii_case("VisitedText") {
        CanonicalColor::from_rgb8(85, 26, 139)
    } else if [
        "ButtonBorder",
        "ButtonText",
        "CanvasText",
        "FieldText",
        "ActiveBorder",
        "CaptionText",
        "InactiveBorder",
        "InfoText",
        "MenuText",
        "ThreeDDarkShadow",
        "ThreeDHighlight",
        "ThreeDLightShadow",
        "ThreeDShadow",
        "WindowFrame",
        "WindowText",
    ]
    .iter()
    .any(|name| value.eq_ignore_ascii_case(name))
    {
        CanonicalColor::from_rgb8(0, 0, 0)
    } else {
        return None;
    };
    Some(color)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rgb(hex: u32) -> [u8; 3] {
        [(hex >> 16) as u8, (hex >> 8) as u8, hex as u8]
    }

    /// `THEME_FILTER`, `invert(93%) hue-rotate(180deg)`, evaluated as the
    /// pinned browser evaluates CSS filter functions: in gamma-encoded sRGB,
    /// hue rotation by the Filter Effects matrix, each function clamped, the
    /// result quantized. The expected values are worked from those definitions
    /// for the export's constants: text `#1e1e1e`, white, the frame outline
    /// `#bbb`, the canvas magic-frame outlines, and the PNG image placeholder.
    #[test]
    fn theme_filter_lowers_the_reference_constants() {
        assert_eq!(theme_filter(rgb(0x1e1e1e)), rgb(0xd3d3d3));
        assert_eq!(theme_filter(rgb(0xffffff)), rgb(0x111111));
        assert_eq!(theme_filter(rgb(0x000000)), rgb(0xededed));
        assert_eq!(theme_filter(rgb(0xbbbbbb)), rgb(0x4c4c4c));
        assert_eq!(theme_filter(rgb(0x7affd7)), rgb(0x004725));
        assert_eq!(theme_filter(rgb(0x1d8264)), rgb(0x4fa68c));
        assert_eq!(theme_filter(rgb(0xe7e7e7)), rgb(0x262626));
        assert_eq!(theme_filter(rgb(0x888888)), rgb(0x787878));
        assert_eq!(theme_filter(rgb(0xa5d8ff)), rgb(0x154162));
    }

    #[test]
    fn a_dark_mode_color_is_the_theme_filter_of_its_light_mode_color() {
        let color = CanonicalColor::from_rgba(0xa5, 0xd8, 0xff, 0.5);
        let dark = color.for_mode(ColorMode::Dark);
        assert_eq!(dark.rgb8(), theme_filter(rgb(0xa5d8ff)));
        assert_eq!(dark.alpha(), 0.5, "the filter leaves alpha alone");
        assert_eq!(color.for_mode(ColorMode::Light), color);
    }

    /// `IMAGE_INVERT_FILTER`, `invert(100%) hue-rotate(180deg) saturate(1.25)`:
    /// grays invert exactly, chromatic colors clamp between the functions, and
    /// the result rounds to the 8-bit surface the root filter then reads.
    #[test]
    fn image_invert_filter_inverts_and_saturates_a_raster_pixel() {
        assert_eq!(image_invert_filter(rgb(0xffffff)), rgb(0x000000));
        assert_eq!(image_invert_filter(rgb(0x000000)), rgb(0xffffff));
        assert_eq!(image_invert_filter(rgb(0x808080)), rgb(0x7f7f7f));
        assert_eq!(image_invert_filter(rgb(0xff0000)), rgb(0xff8d8d));
        assert_eq!(image_invert_filter(rgb(0x00ff00)), rgb(0x009c00));
        assert_eq!(image_invert_filter(rgb(0x0000ff)), rgb(0xdadaff));
        assert_eq!(image_invert_filter(rgb(0xa5d8ff)), rgb(0x00396a));
    }

    /// A raster pixel in a dark-mode SVG export passes through the image
    /// filter and then the root filter, rounded as the browser rounds it: its
    /// contrast lands in `#121212` to `#ededed` and its hue survives.
    #[test]
    fn a_dark_mode_raster_image_pixel_keeps_its_hue_with_compressed_contrast() {
        assert_eq!(dark_mode_raster_image_pixel(rgb(0xffffff)), rgb(0xededed));
        assert_eq!(dark_mode_raster_image_pixel(rgb(0x000000)), rgb(0x121212));
        assert_eq!(dark_mode_raster_image_pixel(rgb(0x808080)), rgb(0x808080));
        assert_eq!(dark_mode_raster_image_pixel(rgb(0xff0000)), rgb(0xac4a4a));
        assert_eq!(dark_mode_raster_image_pixel(rgb(0x00ff00)), rgb(0x2db32d));
        assert_eq!(dark_mode_raster_image_pixel(rgb(0x0000ff)), rgb(0x2d2d4d));
        assert_eq!(dark_mode_raster_image_pixel(rgb(0x1e1e1e)), rgb(0x2c2c2c));
    }

    /// The official SVG's root filter rounds where the canvas floors: the
    /// varmq background `#fffce8` filters to (24.39, 21.81, 4.61), which the
    /// pinned browser shows as `#181605` (#84).
    #[test]
    fn rounded_lowering_matches_the_official_svg_where_floor_is_one_low() {
        assert_eq!(theme_filter_rounded(rgb(0xfffce8)), rgb(0x181605));
        assert_eq!(theme_filter(rgb(0xfffce8)), rgb(0x181504));
        assert_eq!(theme_filter_rounded(rgb(0xa5d8ff)), rgb(0x154163));
        let color = CanonicalColor::from_rgba(0xa5, 0xd8, 0xff, 0.5);
        assert_eq!(color.lowered(ColorLowering::None), color);
        assert_eq!(
            color.lowered(ColorLowering::Floor),
            color.for_mode(ColorMode::Dark)
        );
        assert_eq!(
            color.lowered(ColorLowering::Round).rgb8(),
            [0x15, 0x41, 0x63]
        );
        assert_eq!(color.lowered(ColorLowering::Round).alpha(), 0.5);
    }

    /// `parse_css(value)` written back as the SVG hex the renderer emits, or
    /// `None` when the value is not a color.
    fn hex_of(value: &str) -> Option<String> {
        parse_css(value).map(|color| {
            let mut hex = String::new();
            color.write_hex(&mut hex).expect("writing to a String");
            hex
        })
    }

    /// The CSS Color Level 4 forms the pinned Chromium lowers to these sRGB
    /// hex values: every predefined color space, `color-mix` in each
    /// interpolation space with and without a hue method, and
    /// `currentcolor`. Keywords are matched case-insensitively.
    #[test]
    fn pinned_chromium_color_level_4_forms_lower_to_canonical_srgb() {
        for (color, expected) in [
            ("RGB(30 30 30)", "#1e1e1e"),
            ("hwb(120 0% 0% / .5)", "#00ff00"),
            ("lab(50% 0 0)", "#777777"),
            ("oklab(50% 0 0)", "#636363"),
            ("color(display-p3 1 0 0)", "#ff0000"),
            ("color(srgb 0.25 0 0.75)", "#4000bf"),
            (
                "color(srgb-linear 0.21586050011389926 0.21586050011389926 0.21586050011389926)",
                "#808080",
            ),
            ("color-mix(IN SRGB, red 25%, blue)", "#4000bf"),
            ("color-mix(in oklab, red, blue)", "#8c53a2"),
            ("color-mix(in hsl, red, blue)", "#ff00ff"),
            ("color-mix(in hsl longer hue, red, blue)", "#00ff00"),
            ("color-mix(in hwb, red, blue)", "#ff00ff"),
            ("color-mix(in display-p3, red, blue)", "#800a91"),
            ("color-mix(in xyz, red, blue)", "#bc00bc"),
            ("currentcolor", "#000000"),
            ("rebeccapurple", "#663399"),
            ("  red  ", "#ff0000"),
        ] {
            assert_eq!(hex_of(color).as_deref(), Some(expected), "{color}");
        }
    }

    /// The malformed values the whole-scene tests substitute: a comma mixed
    /// into a space-separated `rgb()`, an unknown `color()` space, a
    /// non-numeric channel, and a bare word that is not a named color.
    #[test]
    fn values_that_are_not_colors_parse_to_none() {
        for value in [
            "rgb(1, 2 3)",
            "color(invalid 1 1 1)",
            "rgb(future)",
            "not-a-color",
            "",
            "color-mix(in srgb, red)",
            "color-mix(in nowhere, red, blue)",
            "color-mix(in srgb, red 150%, blue)",
            "color-mix(in srgb, red 0%, blue 0%)",
            "color-mix(in hsl sideways hue, red, blue)",
        ] {
            assert_eq!(parse_css(value), None, "{value:?}");
        }
    }

    /// `color-mix` nests to the depth the renderer bounds, 128 stops deep,
    /// and one level deeper is refused rather than recursed into, so an
    /// adversarial `viewBackgroundColor` cannot exhaust the stack.
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
    #[cfg_attr(not(target_arch = "wasm32"), test)]
    fn nested_color_mix_is_bounded() {
        let nested = |depth: usize| {
            let mut color = String::from("red");
            for _ in 0..depth {
                color = format!("color-mix(in srgb, {color}, blue)");
            }
            color
        };
        assert_eq!(hex_of(&nested(1)).as_deref(), Some("#800080"));
        assert!(parse_css(&nested(128)).is_some(), "128 levels are admitted");
        assert_eq!(parse_css(&nested(129)), None, "129 levels are refused");
        assert_eq!(parse_css(&nested(130)), None);
    }

    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
    #[cfg_attr(not(target_arch = "wasm32"), test)]
    fn color_mix_preserves_nested_syntax_and_double_precision_weights() {
        for (value, expected) in [
            ("color-mix(in srgb, rgb(255, 0, 0) 25%, blue)", "#4000bf"),
            (
                "color-mix(in srgb, color-mix(in srgb, red, blue), blue)",
                "#4000c0",
            ),
            // Rounding these weights to f32 moves a channel across its half-byte.
            ("color-mix(in srgb, red 49.9999999%, blue)", "#7f0080"),
            ("color-mix(in srgb, red 50.0000001%, blue)", "#80007f"),
            ("color-mix(in srgb, red +2.5e1%, blue)", "#4000bf"),
            // Rust's historical f64 weight spelling, not a CSS percentage token.
            ("color-mix(in srgb, red 25.%, blue)", "#4000bf"),
            ("color-mix(in srgb, currentcolor/*(,)*/, blue)", "#000080"),
            (
                "color-mix(in srgb, currentcolor/*(*/ /*)*/, blue)",
                "#000080",
            ),
        ] {
            assert_eq!(hex_of(value).as_deref(), Some(expected), "{value}");
        }
        for value in [
            "color-mix(in srgb, red, blue",
            "color-mix(in srgb, rgb(255, 0, 0, blue)",
            "color-mix(in srgb, red, rgb(0, 0, 255)",
            "color-mix(in srgb, red, blue))",
            "color-mix(in srgb, red, blue, green)",
            "color-mix(in srgb, 'red, blue', green)",
            "color-mix(in srgb, red 25% 25%, blue)",
            "color-mix(in srgb, red25%, blue)",
            "color-mix(in srgb, red\u{b}25%, blue)",
            "color-mix(in srgb, red calc(25%), blue)",
            "color-mix(in/**/ srgb, red, blue)",
            "color-mix(in srgb, currentcolor/*,*/, blue)",
            "color-mix(in srgb, red NaN%, blue)",
            r"color-mix(in s\72gb, red, blue)",
        ] {
            assert_eq!(parse_css(value), None, "{value}");
        }
    }

    /// Alpha survives parsing in the 8-bit precision the encoders read: the
    /// `#rrggbbaa` byte and the `/ 50%` percentage both land on `128 / 255`,
    /// which the PNG encoder writes as `128` and SVG and PDF as
    /// `0.5019607843137255`.
    #[test]
    fn alpha_is_parsed_at_eight_bit_precision_for_every_projection() {
        const ALPHA: f64 = 0.5019607843137255;
        let stroke = parse_css("#ff000080").expect("eight-digit hex is a color");
        assert_eq!(stroke.alpha(), ALPHA);
        assert_eq!(stroke.rgba8(), [255, 0, 0, 128]);
        assert_eq!(hex_of("#ff000080").as_deref(), Some("#ff0000"));

        let background = parse_css("rgb(10 20 30 / 50%)").expect("percentage alpha is a color");
        assert_eq!(background.alpha(), ALPHA);
        assert_eq!(background.rgba8(), [10, 20, 30, 128]);
        assert_eq!(background.rgb8(), [10, 20, 30]);

        let opaque = parse_css("rebeccapurple").expect("named color");
        assert_eq!(opaque.alpha(), 1.0);
        assert_eq!(opaque.rgba8(), [102, 51, 153, 255]);
        assert_eq!(opaque.rgb_f64(), [0.4, 0.2, 0.6]);
    }

    /// `write_hex` is the six-digit lowercase form with every channel padded,
    /// whatever the alpha.
    #[test]
    fn write_hex_pads_every_channel_to_two_lowercase_digits() {
        let mut hex = String::new();
        CanonicalColor::from_rgba(0, 10, 255, 0.25)
            .write_hex(&mut hex)
            .expect("writing to a String");
        assert_eq!(hex, "#000aff");
    }

    /// Dark mode lowers a parsed scene color through the theme filter: the
    /// default text color `#1e1e1e` becomes `#d3d3d3` and a white canvas
    /// `#111111`, the constants the reference's dark export shows.
    #[test]
    fn dark_mode_lowers_parsed_scene_colors_to_the_reference_constants() {
        let dark = |value: &str| {
            let mut hex = String::new();
            parse_css(value)
                .expect("the reference constant is a color")
                .for_mode(ColorMode::Dark)
                .write_hex(&mut hex)
                .expect("writing to a String");
            hex
        };
        assert_eq!(dark("#1e1e1e"), "#d3d3d3");
        assert_eq!(dark("#ffffff"), "#111111");
        assert_eq!(dark("white"), "#111111");
    }

    /// CSS system colors resolve to the pinned Chromium light-theme values,
    /// case-insensitively; `Highlight` is the one translucent entry.
    #[test]
    fn system_colors_resolve_to_the_pinned_chromium_palette() {
        assert_eq!(hex_of("Canvas").as_deref(), Some("#ffffff"));
        assert_eq!(hex_of("canvastext").as_deref(), Some("#000000"));
        assert_eq!(hex_of("LinkText").as_deref(), Some("#0000ee"));
        assert_eq!(hex_of("VISITEDTEXT").as_deref(), Some("#551a8b"));
        assert_eq!(hex_of("GrayText").as_deref(), Some("#808080"));
        assert_eq!(hex_of("ButtonFace").as_deref(), Some("#efefef"));
        let highlight = parse_css("Highlight").expect("Highlight is a system color");
        assert_eq!(highlight.rgb8(), [0, 65, 198]);
        assert_eq!(highlight.alpha(), 0.8);
    }
}
