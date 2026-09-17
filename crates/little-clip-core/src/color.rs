//! Color parsing and format conversion utilities.
//!
//! Provides consolidated `#RGB`, `#RRGGBB`, and `#RRGGBBAA` hex color parsing.

/// Parses a hex color string (e.g. `#FFF`, `#1E293B`, `#1E293BFF`) into `(r, g, b, formatted_hex)`.
///
/// The returned hex value is uppercase and omits an input alpha component. Returns `None` when
/// the input does not begin with `#` or has an unsupported length or invalid digit.
///
/// ```
/// use little_clip_core::parse_hex_color;
///
/// assert_eq!(parse_hex_color("#1e293b"), Some((30, 41, 59, "#1E293B".into())));
/// assert_eq!(parse_hex_color("not a color"), None);
/// ```
pub fn parse_hex_color(s: &str) -> Option<(u8, u8, u8, String)> {
    let s = s.trim();
    if !s.starts_with('#') {
        return None;
    }
    let hex = &s[1..];
    if hex.len() == 6 {
        let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
        let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
        let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
        return Some((r, g, b, format!("#{}", hex.to_uppercase())));
    }
    if hex.len() == 3 {
        let r = u8::from_str_radix(&hex[0..1].repeat(2), 16).ok()?;
        let g = u8::from_str_radix(&hex[1..2].repeat(2), 16).ok()?;
        let b = u8::from_str_radix(&hex[2..3].repeat(2), 16).ok()?;
        return Some((r, g, b, format!("#{}", hex.to_uppercase())));
    }
    if hex.len() == 8 {
        let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
        let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
        let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
        return Some((r, g, b, format!("#{}", hex[0..6].to_uppercase())));
    }
    None
}

/// Strictly parses `#RGB`, `#RRGGBB`, or `#RRGGBBAA` into `(r, g, b, a)`.
///
/// Unlike [`parse_hex_color_rgba_or_white`], this function reports malformed input as `None`.
pub fn parse_hex_color_rgba_strict(s: &str) -> Option<(u8, u8, u8, u8)> {
    let s = s.trim();
    let s = s.strip_prefix('#').unwrap_or(s);
    if s.len() == 6 {
        Some((
            u8::from_str_radix(&s[0..2], 16).ok()?,
            u8::from_str_radix(&s[2..4], 16).ok()?,
            u8::from_str_radix(&s[4..6], 16).ok()?,
            255,
        ))
    } else if s.len() == 8 {
        Some((
            u8::from_str_radix(&s[0..2], 16).ok()?,
            u8::from_str_radix(&s[2..4], 16).ok()?,
            u8::from_str_radix(&s[4..6], 16).ok()?,
            u8::from_str_radix(&s[6..8], 16).ok()?,
        ))
    } else if s.len() == 3 {
        Some((
            u8::from_str_radix(&s[0..1].repeat(2), 16).ok()?,
            u8::from_str_radix(&s[1..2].repeat(2), 16).ok()?,
            u8::from_str_radix(&s[2..3].repeat(2), 16).ok()?,
            255,
        ))
    } else {
        None
    }
}

/// Parses a hex color using opaque white when parsing fails.
///
/// Use this only at display-only fallback boundaries. Validation and configuration input should
/// prefer [`parse_hex_color_rgba_strict`] so invalid values remain observable.
pub fn parse_hex_color_rgba_or_white(s: &str) -> (u8, u8, u8, u8) {
    parse_hex_color_rgba_strict(s).unwrap_or((255, 255, 255, 255))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_hex_color() {
        assert_eq!(
            parse_hex_color("#ff0000"),
            Some((255, 0, 0, "#FF0000".to_string()))
        );
        assert_eq!(
            parse_hex_color("#0f0"),
            Some((0, 255, 0, "#0F0".to_string()))
        );
        assert_eq!(parse_hex_color("invalid"), None);
        assert_eq!(parse_hex_color("#12345"), None);
    }

    #[test]
    fn test_parse_hex_color_rgba() {
        assert_eq!(
            parse_hex_color_rgba_strict("#FF0000"),
            Some((255, 0, 0, 255))
        );
        assert_eq!(parse_hex_color_rgba_strict("invalid"), None);
        assert_eq!(
            parse_hex_color_rgba_or_white("invalid"),
            (255, 255, 255, 255)
        );
    }
}
