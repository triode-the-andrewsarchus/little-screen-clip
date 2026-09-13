//! Hundred Rabbits Theme parser, Oklab perceptual color space calculations,
//! and full application palette derivation with user overrides.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use serde::{Deserialize, Serialize};

use crate::color::parse_hex_color_rgba;

/// Represents the 9 standard colors defined in a Hundred Rabbits theme.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Theme {
    pub name: String,
    pub background: String,
    pub f_high: String,
    pub f_med: String,
    pub f_low: String,
    pub f_inv: String,
    pub b_high: String,
    pub b_med: String,
    pub b_low: String,
    pub b_inv: String,
}

impl Default for Theme {
    fn default() -> Self {
        Self::apollo()
    }
}

/// Computes the relative luminance (0.0 to 1.0) of an RGB color according to WCAG 2.1.
pub fn relative_luminance(r: u8, g: u8, b: u8) -> f32 {
    fn to_linear(c: u8) -> f32 {
        let s = c as f32 / 255.0;
        if s <= 0.04045 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        }
    }
    0.2126 * to_linear(r) + 0.7152 * to_linear(g) + 0.0722 * to_linear(b)
}

/// Computes the WCAG contrast ratio (between 1.0 and 21.0) between two RGB colors.
pub fn contrast_ratio(rgb1: (u8, u8, u8), rgb2: (u8, u8, u8)) -> f32 {
    let l1 = relative_luminance(rgb1.0, rgb1.1, rgb1.2);
    let l2 = relative_luminance(rgb2.0, rgb2.1, rgb2.2);
    let (lighter, darker) = if l1 > l2 { (l1, l2) } else { (l2, l1) };
    (lighter + 0.05) / (darker + 0.05)
}

impl Theme {
    /// Apollo built-in default theme.
    pub fn apollo() -> Self {
        Self {
            name: "apollo".to_string(),
            background: "#29272b".to_string(),
            f_high: "#ffffff".to_string(),
            f_med: "#e47464".to_string(),
            f_low: "#66606b".to_string(),
            f_inv: "#000000".to_string(),
            b_high: "#000000".to_string(),
            b_med: "#201e21".to_string(),
            b_low: "#322e33".to_string(),
            b_inv: "#e47464".to_string(),
        }
    }

    /// Whether this theme is predominantly dark or light.
    pub fn is_dark(&self) -> bool {
        let (r, g, b, _) = parse_hex_color_rgba(&self.background);
        // Perceived relative luminance
        let lum = 0.2126 * (r as f32) + 0.7152 * (g as f32) + 0.0722 * (b as f32);
        lum < 128.0
    }

    /// Selects the best foreground color to render on `bg_hex`.
    /// 1. Tests `preferred_fg` (or `f_high` by default). If contrast >= 4.5, returns it.
    /// 2. If contrast < 4.5, tries the bg color from other theme fg/bg pairs, or `f_inv` (e.g. `background`, `b_low`, `b_med`, `b_high`, `b_inv`, `f_inv`).
    /// 3. If none achieve contrast >= 4.5, selects between pure black ("#000000") and pure white ("#ffffff"),
    ///    whichever provides higher contrast against `bg_hex`.
    pub fn contrast_pair_fg(&self, bg_hex: &str, preferred_fg: Option<&str>) -> String {
        let (br, bg, bb, _) = parse_hex_color_rgba(bg_hex);
        let bg_rgb = (br, bg, bb);

        let pref = preferred_fg.unwrap_or(&self.f_high);
        let (pr, pg, pb, _) = parse_hex_color_rgba(pref);
        if contrast_ratio(bg_rgb, (pr, pg, pb)) >= 4.5 {
            return pref.to_string();
        }

        // Try the bg color from other fg/bg pairs or f_inv / fg colors
        let candidates = [
            &self.f_inv,
            &self.background,
            &self.b_low,
            &self.b_high,
            &self.b_med,
            &self.b_inv,
            &self.f_high,
            &self.f_med,
            &self.f_low,
        ];

        for cand in candidates {
            let (cr, cg, cb, _) = parse_hex_color_rgba(cand);
            if contrast_ratio(bg_rgb, (cr, cg, cb)) >= 4.5 {
                return cand.clone();
            }
        }

        // Fallback: Pick between pure black and pure white
        let black_ratio = contrast_ratio(bg_rgb, (0, 0, 0));
        let white_ratio = contrast_ratio(bg_rgb, (255, 255, 255));
        if black_ratio >= white_ratio {
            "#000000".to_string()
        } else {
            "#ffffff".to_string()
        }
    }

    /// Parses a Hundred Rabbits SVG theme content.
    pub fn from_svg(name: &str, svg_content: &str) -> Result<Self, String> {
        let mut map = HashMap::new();

        for line in svg_content.lines() {
            let line = line.trim();
            if !line.contains("<rect") && !line.contains("<circle") {
                continue;
            }

            let id = extract_attr(line, "id");
            let fill = extract_attr(line, "fill");

            if let (Some(id_val), Some(fill_val)) = (id, fill) {
                map.insert(id_val, fill_val);
            }
        }

        let get_color = |id: &str| -> Result<String, String> {
            map.get(id)
                .cloned()
                .ok_or_else(|| format!("Theme SVG '{}' missing id '{}'", name, id))
        };

        Ok(Self {
            name: name.to_string(),
            background: get_color("background")?,
            f_high: get_color("f_high")?,
            f_med: get_color("f_med")?,
            f_low: get_color("f_low")?,
            f_inv: get_color("f_inv")?,
            b_high: get_color("b_high")?,
            b_med: get_color("b_med")?,
            b_low: get_color("b_low")?,
            b_inv: get_color("b_inv")?,
        })
    }

    /// Loads a theme from an SVG file path.
    pub fn from_file<P: AsRef<Path>>(path: P) -> Result<Self, String> {
        let path = path.as_ref();
        let name = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "theme".to_string());
        let content = fs::read_to_string(path).map_err(|e| e.to_string())?;
        Self::from_svg(&name, &content)
    }
}

fn extract_attr(line: &str, attr: &str) -> Option<String> {
    for quote in ['"', '\''] {
        let search = format!("{attr}={quote}");
        if let Some(pos) = line.find(&search) {
            let rest = &line[pos + search.len()..];
            if let Some(end) = rest.find(quote) {
                return Some(rest[..end].to_string());
            }
        }
    }
    None
}

/// Discovers all SVG themes in a directory.
pub fn load_themes_from_dir<P: AsRef<Path>>(dir: P) -> Vec<Theme> {
    let mut themes = Vec::new();
    let dir = dir.as_ref();
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() && path.extension().and_then(|s| s.to_str()) == Some("svg") {
                if let Ok(theme) = Theme::from_file(&path) {
                    themes.push(theme);
                }
            }
        }
    }
    themes.sort_by(|a, b| a.name.cmp(&b.name));
    themes
}

/// Helper to locate the `themes` directory from current working dir or workspace ancestors.
pub fn find_themes_dir() -> PathBuf {
    let candidates = [
        PathBuf::from("themes"),
        PathBuf::from("../themes"),
        PathBuf::from("../../themes"),
    ];
    for c in &candidates {
        if c.is_dir() {
            return c.clone();
        }
    }
    // Fallback: search relative to current executable if possible
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            let rel = parent.join("themes");
            if rel.is_dir() {
                return rel;
            }
            if let Some(grandparent) = parent.parent() {
                let rel = grandparent.join("themes");
                if rel.is_dir() {
                    return rel;
                }
            }
        }
    }
    PathBuf::from("themes")
}

// -----------------------------------------------------------------------------
// Perceptual Color Space Math (Oklab & Oklch)
// -----------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Oklab {
    pub l: f32,
    pub a: f32,
    pub b: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Oklch {
    pub l: f32,
    pub c: f32,
    pub h: f32, // Degrees: 0.0 .. 360.0
}

impl Oklab {
    pub fn from_rgb(r: u8, g: u8, b: u8) -> Self {
        let lr = srgb_to_linear(r as f32 / 255.0);
        let lg = srgb_to_linear(g as f32 / 255.0);
        let lb = srgb_to_linear(b as f32 / 255.0);

        let l = 0.4122214708 * lr + 0.5363325363 * lg + 0.0514459929 * lb;
        let m = 0.2119034982 * lr + 0.6806995451 * lg + 0.1073969566 * lb;
        let s = 0.0883024619 * lr + 0.2817188376 * lg + 0.6299787005 * lb;

        let l_ = l.cbrt();
        let m_ = m.cbrt();
        let s_ = s.cbrt();

        Self {
            l: 0.2104542553 * l_ + 0.7936177850 * m_ - 0.0040720468 * s_,
            a: 1.9779984951 * l_ - 2.4285922050 * m_ + 0.4505937099 * s_,
            b: 0.0259040371 * l_ + 0.7827717662 * m_ - 0.8086757660 * s_,
        }
    }

    pub fn to_rgb(&self) -> (u8, u8, u8) {
        let l_ = self.l + 0.3963377774 * self.a + 0.2158037573 * self.b;
        let m_ = self.l - 0.1055613458 * self.a - 0.0638541728 * self.b;
        let s_ = self.l - 0.0894841775 * self.a - 1.2914855480 * self.b;

        let l = l_ * l_ * l_;
        let m = m_ * m_ * m_;
        let s = s_ * s_ * s_;

        let r = 4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s;
        let g = -1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s;
        let b = -0.0041960863 * l - 0.7034186147 * m + 1.7076147010 * s;

        let r_byte = (linear_to_srgb(r) * 255.0).round().clamp(0.0, 255.0) as u8;
        let g_byte = (linear_to_srgb(g) * 255.0).round().clamp(0.0, 255.0) as u8;
        let b_byte = (linear_to_srgb(b) * 255.0).round().clamp(0.0, 255.0) as u8;

        (r_byte, g_byte, b_byte)
    }

    pub fn to_hex(&self) -> String {
        let (r, g, b) = self.to_rgb();
        format!("#{:02x}{:02x}{:02x}", r, g, b)
    }

    pub fn to_oklch(&self) -> Oklch {
        let c = (self.a * self.a + self.b * self.b).sqrt();
        let mut h = self.b.atan2(self.a).to_degrees();
        if h < 0.0 {
            h += 360.0;
        }
        Oklch { l: self.l, c, h }
    }
}

impl Oklch {
    pub fn to_oklab(&self) -> Oklab {
        let h_rad = self.h.to_radians();
        Oklab {
            l: self.l,
            a: self.c * h_rad.cos(),
            b: self.c * h_rad.sin(),
        }
    }

    pub fn to_rgb(&self) -> (u8, u8, u8) {
        self.to_oklab().to_rgb()
    }

    pub fn to_hex(&self) -> String {
        self.to_oklab().to_hex()
    }

    /// Rotate hue by degrees (positive or negative).
    pub fn rotate_hue(&self, degrees: f32) -> Self {
        let mut new_h = (self.h + degrees) % 360.0;
        if new_h < 0.0 {
            new_h += 360.0;
        }
        Self {
            l: self.l,
            c: self.c,
            h: new_h,
        }
    }
}

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(c: f32) -> f32 {
    let c = c.clamp(0.0, 1.0);
    if c <= 0.0031308 {
        12.92 * c
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

// -----------------------------------------------------------------------------
// Harmonious Palette Derivation
// -----------------------------------------------------------------------------

/// Set of derived harmonic colors around a base accent color in Oklab space.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DerivedHarmonies {
    /// Base accent ($h_0$)
    pub base: String,
    /// Complementary ($h_0 + 180^\circ$)
    pub complementary: String,
    /// Triadic 1 ($h_0 + 120^\circ$)
    pub triadic_1: String,
    /// Triadic 2 ($h_0 + 240^\circ$)
    pub triadic_2: String,
    /// Tetradic 1 ($h_0 + 90^\circ$)
    pub tetradic_1: String,
    /// Tetradic 2 ($h_0 + 270^\circ$)
    pub tetradic_2: String,
    /// Split Complementary 1 ($h_0 + 150^\circ$)
    pub split_comp_1: String,
    /// Split Complementary 2 ($h_0 + 210^\circ$)
    pub split_comp_2: String,
    /// Warm attention/amber hue (for warnings/undo)
    pub warm_attention: String,
}

impl DerivedHarmonies {
    pub fn from_base_hex(hex: &str, is_dark: bool) -> Self {
        let (r, g, b, _) = parse_hex_color_rgba(hex);
        let mut oklch = Oklab::from_rgb(r, g, b).to_oklch();

        // If chroma is nearly zero (monochromatic accent), give it enough chroma to display harmonies
        if oklch.c < 0.04 {
            oklch.c = 0.13;
            oklch.h = 240.0; // Default to vibrant indigo/blue hue
        }

        // Adjust lightness slightly to maintain good contrast on dark or light theme
        if is_dark {
            oklch.l = oklch.l.clamp(0.60, 0.85);
        } else {
            oklch.l = oklch.l.clamp(0.35, 0.65);
        }

        let make_hex = |deg: f32| oklch.rotate_hue(deg).to_hex();

        // Warm attention hue in Oklch is approximately 75-80 degrees (amber/gold)
        let warm_attention = Oklch {
            l: if is_dark { 0.78 } else { 0.55 },
            c: oklch.c.max(0.14),
            h: 75.0,
        }
        .to_hex();

        Self {
            base: hex.to_string(),
            complementary: make_hex(180.0),
            triadic_1: make_hex(120.0),
            triadic_2: make_hex(240.0),
            tetradic_1: make_hex(90.0),
            tetradic_2: make_hex(270.0),
            split_comp_1: make_hex(150.0),
            split_comp_2: make_hex(210.0),
            warm_attention,
        }
    }
}

// -----------------------------------------------------------------------------
// Unified App Palette (Render + Workbench)
// -----------------------------------------------------------------------------

/// Fully resolved color palette used across all rendering and UI widgets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppPalette {
    pub theme_name: String,
    pub is_dark: bool,
    pub theme: Theme,
    pub harmonies: DerivedHarmonies,

    // Core Canvas & Cards
    pub background: String,
    pub hero_card_bg: String,
    pub prior_card_bg: String,
    pub undo_card_bg: String,

    // Borders & Accents
    pub undo_border: String,
    pub left_hand_border: String,
    pub left_prior1_border: String,
    pub left_prior2_border: String,
    pub left_prior3_border: String,
    pub right_hand_border: String,
    pub right_prior1_border: String,
    pub right_prior2_border: String,
    pub cursor_border: String,
    pub selected_border: String,

    // Text Typography
    pub title_color: String,
    pub subtitle_color: String,
    pub body_color: String,
    pub url_color: String,

    // UI Badges & Overlays
    pub debug_badge_bg: String,
    pub debug_badge_text: String,
    pub tag_pill_bg: String,
    pub tag_pill_text: String,

    // Functional Accents (Diffs, Status)
    pub diff_add: String,
    pub diff_del: String,
    pub diff_mod: String,

    // Raw Theme colors
    pub f_high: String,
    pub f_med: String,
    pub f_low: String,
    pub f_inv: String,
    pub b_high: String,
    pub b_med: String,
    pub b_low: String,
    pub b_inv: String,
}

impl Default for AppPalette {
    fn default() -> Self {
        Self::from_theme(&Theme::apollo(), &HashMap::new())
    }
}

impl AppPalette {
    /// Builds a fully resolved AppPalette from a Theme and optional user overrides.
    pub fn from_theme(theme: &Theme, overrides: &HashMap<String, String>) -> Self {
        let is_dark = theme.is_dark();

        // Base accent: prefer f_med, fall back to b_inv if f_med is grayscale
        let (f_r, f_g, f_b, _) = parse_hex_color_rgba(&theme.f_med);
        let f_oklch = Oklab::from_rgb(f_r, f_g, f_b).to_oklch();
        let base_accent_hex = if f_oklch.c > 0.03 {
            &theme.f_med
        } else {
            &theme.b_inv
        };

        let harmonies = DerivedHarmonies::from_base_hex(base_accent_hex, is_dark);

        // Diff colors derived in Oklch
        let diff_add = Oklch {
            l: if is_dark { 0.72 } else { 0.45 },
            c: 0.14,
            h: 142.0, // Emerald green
        }
        .to_hex();

        let diff_del = Oklch {
            l: if is_dark { 0.70 } else { 0.45 },
            c: 0.16,
            h: 25.0, // Crimson / rose
        }
        .to_hex();

        let diff_mod = Oklch {
            l: if is_dark { 0.72 } else { 0.45 },
            c: 0.13,
            h: 230.0, // Cyan / sky blue
        }
        .to_hex();

        // Helper to pick override or fallback
        let ov = |key: &str, fallback: &str| -> String {
            overrides.get(key).cloned().unwrap_or_else(|| fallback.to_string())
        };

        Self {
            theme_name: theme.name.clone(),
            is_dark,
            theme: theme.clone(),
            harmonies: harmonies.clone(),

            background: ov("background", &theme.background),
            hero_card_bg: ov("hero_card_bg", &theme.b_high),
            prior_card_bg: ov("prior_card_bg", &theme.b_med),
            undo_card_bg: ov("undo_card_bg", &theme.b_low),

            undo_border: ov("undo_border", &harmonies.warm_attention),
            left_hand_border: ov("left_hand_border", &harmonies.base),
            right_hand_border: ov("right_hand_border", &harmonies.complementary),

            left_prior1_border: ov("left_prior1_border", &harmonies.split_comp_1),
            left_prior2_border: ov("left_prior2_border", &harmonies.triadic_1),
            left_prior3_border: ov("left_prior3_border", &harmonies.split_comp_2),

            right_prior1_border: ov("right_prior1_border", &harmonies.triadic_2),
            right_prior2_border: ov("right_prior2_border", &harmonies.tetradic_1),

            cursor_border: ov("cursor_border", &harmonies.tetradic_2),
            selected_border: ov("selected_border", &harmonies.triadic_1),

            title_color: ov("title_color", &theme.f_high),
            subtitle_color: ov("subtitle_color", &theme.f_med),
            body_color: ov("body_color", &theme.f_low),
            url_color: ov("url_color", &harmonies.split_comp_1),

            debug_badge_bg: ov("debug_badge_bg", &theme.b_inv),
            debug_badge_text: ov("debug_badge_text", &theme.f_inv),
            tag_pill_bg: ov("tag_pill_bg", &theme.b_low),
            tag_pill_text: ov("tag_pill_text", &theme.f_med),

            diff_add,
            diff_del,
            diff_mod,

            f_high: theme.f_high.clone(),
            f_med: theme.f_med.clone(),
            f_low: theme.f_low.clone(),
            f_inv: theme.f_inv.clone(),
            b_high: theme.b_high.clone(),
            b_med: theme.b_med.clone(),
            b_low: theme.b_low.clone(),
            b_inv: theme.b_inv.clone(),
        }
    }

    /// Helper returning `(r, g, b, a)` for any hex string in the palette.
    pub fn rgba(&self, hex: &str) -> (u8, u8, u8, u8) {
        parse_hex_color_rgba(hex)
    }

    /// Selects the best foreground color to render on `bg_hex` with sufficient contrast (minimum 4.5:1).
    pub fn contrast_pair_fg(&self, bg_hex: &str, preferred_fg: Option<&str>) -> String {
        self.theme.contrast_pair_fg(bg_hex, preferred_fg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_oklab_roundtrip() {
        let (r, g, b) = (228, 116, 100); // #e47464
        let oklab = Oklab::from_rgb(r, g, b);
        let (out_r, out_g, out_b) = oklab.to_rgb();
        assert_eq!((r, g, b), (out_r, out_g, out_b));
    }

    #[test]
    fn test_contrast_calculation_and_selection() {
        // Black vs White contrast is 21:1
        let cr = contrast_ratio((0, 0, 0), (255, 255, 255));
        assert!((cr - 21.0).abs() < 0.1);

        let theme = Theme::apollo();
        // On dark background (#28211c), preferred f_high (#ffefc9) has great contrast
        let fg_on_dark = theme.contrast_pair_fg(&theme.background, Some(&theme.f_high));
        assert_eq!(fg_on_dark, theme.f_high);

        // On a light background that matches f_high, contrast_pair_fg should choose a dark color
        let fg_on_light = theme.contrast_pair_fg("#ffffff", Some(&theme.f_high));
        assert_ne!(fg_on_light, "#ffffff");
        let (fg_r, fg_g, fg_b, _) = parse_hex_color_rgba(&fg_on_light);
        let ratio = contrast_ratio((255, 255, 255), (fg_r, fg_g, fg_b));
        assert!(ratio >= 4.5);
    }

    #[test]
    fn test_theme_from_svg() {
        let svg = r##"<svg width="96px" height="64px">
  <rect width="96" height="64" id="background" fill="#28211c"/>
  <circle cx='56' cy='24' r='8' id='f_high' fill='#ffefc9'/>
  <circle cx='72' cy='24' r='8' id='f_med' fill='#9f9fa2'/>
  <circle cx="40" cy="40" r="8" id="f_low" fill="#a3832c"/>
  <circle cx="24" cy="24" r="8" id="f_inv" fill="#666666"/>
  <circle cx='40' cy='24' r='8' id='b_high' fill='#aa0000'/>
  <circle cx="24" cy="40" r="8" id="b_med" fill="#214c05"/>
  <circle cx="56" cy="40" r="8" id="b_low" fill="#48413a"/>
  <circle cx="72" cy="40" r="8" id="b_inv" fill="#4cb1cf"/>
</svg>"##;
        let theme = Theme::from_svg("garden", svg).expect("parse failed");
        assert_eq!(theme.name, "garden");
        assert_eq!(theme.background, "#28211c");
        assert_eq!(theme.f_high, "#ffefc9");
        assert_eq!(theme.b_inv, "#4cb1cf");
    }

    #[test]
    fn test_harmonies_generation() {
        let harmonies = DerivedHarmonies::from_base_hex("#e47464", true);
        assert!(!harmonies.complementary.is_empty());
        assert!(!harmonies.triadic_1.is_empty());
        assert!(!harmonies.tetradic_1.is_empty());
        assert!(!harmonies.warm_attention.is_empty());
    }

    #[test]
    fn test_palette_with_overrides() {
        let theme = Theme::apollo();
        let mut overrides = HashMap::new();
        overrides.insert("left_hand_border".to_string(), "#123456".to_string());

        let palette = AppPalette::from_theme(&theme, &overrides);
        assert_eq!(palette.left_hand_border, "#123456");
        assert_eq!(palette.background, theme.background);
    }

    #[test]
    fn test_load_all_hundredrabbits_themes() {
        let dir = find_themes_dir();
        if dir.exists() {
            let themes = load_themes_from_dir(&dir);
            assert!(themes.len() >= 40, "Expected at least 40 themes, found {}", themes.len());
            // Verify 'apollo' exists
            let apollo = themes.iter().find(|t| t.name == "apollo");
            assert!(apollo.is_some(), "apollo theme should exist");
        }
    }
}
