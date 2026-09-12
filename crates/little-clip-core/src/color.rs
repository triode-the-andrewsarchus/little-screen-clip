//! Color parsing and format conversion utilities.
//!
//! Provides consolidated `#RGB`, `#RRGGBB`, and `#RRGGBBAA` hex color parsing.

/// Parses a hex color string (e.g. `#FFF`, `#1E293B`, `#1E293BFF`) into `(r, g, b, formatted_hex)` tuple.
/// Returns `None` if the input is not a valid hex color string.
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

/// Parses a hex color string like `#RGB`, `#RRGGBB`, or `#RRGGBBAA` into `(r, g, b, a)`.
/// Defaults to opaque white `(255, 255, 255, 255)` on parse failure.
pub fn parse_hex_color_rgba(s: &str) -> (u8, u8, u8, u8) {
    let s = s.trim();
    let s = s.strip_prefix('#').unwrap_or(s);
    if s.len() == 6 {
        let r = u8::from_str_radix(&s[0..2], 16).unwrap_or(255);
        let g = u8::from_str_radix(&s[2..4], 16).unwrap_or(255);
        let b = u8::from_str_radix(&s[4..6], 16).unwrap_or(255);
        (r, g, b, 255)
    } else if s.len() == 8 {
        let r = u8::from_str_radix(&s[0..2], 16).unwrap_or(255);
        let g = u8::from_str_radix(&s[2..4], 16).unwrap_or(255);
        let b = u8::from_str_radix(&s[4..6], 16).unwrap_or(255);
        let a = u8::from_str_radix(&s[6..8], 16).unwrap_or(255);
        (r, g, b, a)
    } else if s.len() == 3 {
        let r = u8::from_str_radix(&s[0..1].repeat(2), 16).unwrap_or(255);
        let g = u8::from_str_radix(&s[1..2].repeat(2), 16).unwrap_or(255);
        let b = u8::from_str_radix(&s[2..3].repeat(2), 16).unwrap_or(255);
        (r, g, b, 255)
    } else {
        (255, 255, 255, 255)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_hex_color() {
        assert_eq!(parse_hex_color("#ff0000"), Some((255, 0, 0, "#FF0000".to_string())));
        assert_eq!(parse_hex_color("#0f0"), Some((0, 255, 0, "#0F0".to_string())));
        assert_eq!(parse_hex_color("invalid"), None);
        assert_eq!(parse_hex_color("#12345"), None);
    }

    #[test]
    fn test_parse_hex_color_rgba() {
        assert_eq!(parse_hex_color_rgba("#FF0000"), (255, 0, 0, 255));
        assert_eq!(parse_hex_color_rgba("#00FF0080"), (0, 255, 0, 128));
        assert_eq!(parse_hex_color_rgba("#F00"), (255, 0, 0, 255));
        assert_eq!(parse_hex_color_rgba("invalid"), (255, 255, 255, 255));
    }
}
