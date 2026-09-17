//! Hundred Rabbits Theme parser, Oklab perceptual color space calculations,
//! and full application palette derivation with user overrides.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::color::parse_hex_color_rgba_or_white as parse_hex_color_rgba;

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
        let (br, bg, bb, _) = parse_hex_color_rgba(&self.background);
        let (fr, fg, fb, _) = parse_hex_color_rgba(&self.f_high);
        let bg_lum = relative_luminance(br, bg, bb);
        let fg_lum = relative_luminance(fr, fg, fb);
        // A theme is dark if its background is dark (< 0.20 relative luminance)
        // and its primary text foreground is brighter than the background.
        bg_lum < 0.20 && fg_lum > bg_lum
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

        let candidates = [
            &self.f_high,
            &self.f_inv,
            &self.background,
            &self.b_high,
            &self.b_med,
            &self.b_low,
            &self.b_inv,
            &self.f_med,
            &self.f_low,
        ];

        // Try the theme palette colors to find the one with the highest contrast (>= 4.5)
        let mut best_theme_cand: Option<(&str, f32)> = None;
        for cand in candidates {
            let (cr, cg, cb, _) = parse_hex_color_rgba(cand);
            let ratio = contrast_ratio(bg_rgb, (cr, cg, cb));
            if ratio >= 4.5 {
                if let Some((_, best_r)) = best_theme_cand {
                    if ratio > best_r {
                        best_theme_cand = Some((cand, ratio));
                    }
                } else {
                    best_theme_cand = Some((cand, ratio));
                }
            }
        }

        if let Some((best, _)) = best_theme_cand {
            return best.to_string();
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
            overrides
                .get(key)
                .cloned()
                .unwrap_or_else(|| fallback.to_string())
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

    /// Computes the theme-derived vibrant color for a MIDI track / channel.
    ///
    /// Used for track legend chips, channel headers, and track-level badges.
    pub fn midi_track_color(&self, track: usize, channel: u8, total_tracks: usize) -> (u8, u8, u8) {
        self.midi_note_color(track, channel, total_tracks, 60, 100)
    }

    /// Hex string helper for `midi_track_color`.
    pub fn midi_track_color_hex(&self, track: usize, channel: u8, total_tracks: usize) -> String {
        let (r, g, b) = self.midi_track_color(track, channel, total_tracks);
        format!("#{:02x}{:02x}{:02x}", r, g, b)
    }

    /// Computes a vibrant, theme-harmonized color for an individual MIDI note.
    ///
    /// Ensures note colors:
    /// 1. Genuinely derive from the theme's core accent hue.
    /// 2. Maintain a healthy chroma floor ($C \ge 0.18$) so notes never look muted or washed out,
    ///    even in desaturated themes like Zenburn or Paper.
    /// 3. Provide sufficient contrast against canvas backgrounds in both dark and light modes.
    /// 4. Distinguish GM percussion (channel 9) with a warm attention hue.
    /// 5. Provide chromatic pitch-class wheel separation $(key \% 12) \times 30^\circ$ for single-track files.
    /// 6. Modulate gently by velocity in Oklch space without crushing dark-mode luminance.
    pub fn midi_note_color(
        &self,
        track: usize,
        channel: u8,
        total_tracks: usize,
        pitch_key: u8,
        velocity: u8,
    ) -> (u8, u8, u8) {
        // 1. Determine base hue angle from the theme's base accent
        let (f_r, f_g, f_b, _) = parse_hex_color_rgba(&self.harmonies.base);
        let base_oklch = Oklab::from_rgb(f_r, f_g, f_b).to_oklch();

        let h_0 = if base_oklch.c < 0.04 {
            // Theme has an almost completely neutral/achromatic accent.
            // Check if b_inv has color:
            let (inv_r, inv_g, inv_b, _) = parse_hex_color_rgba(&self.b_inv);
            let inv_oklch = Oklab::from_rgb(inv_r, inv_g, inv_b).to_oklch();
            if inv_oklch.c >= 0.04 {
                inv_oklch.h
            } else {
                240.0 // Default to vibrant sapphire
            }
        } else {
            base_oklch.h
        };

        // 2. Chroma floor: guarantee vibrant notes even for desaturated themes
        let note_c = base_oklch.c.max(0.18).min(0.24);

        // 3. Base lightness depending on dark vs light mode
        let base_l = if self.is_dark { 0.74 } else { 0.38 };

        // 4. Resolve Hue based on track / channel / pitch
        let target_h = if channel == 9 {
            // GM Channel 9 = Drums / Percussion: punchy warm attention hue (amber/orange)
            75.0
        } else if total_tracks <= 1 {
            if channel > 0 {
                // Single-track Type 0 file with multiple channels: color by channel
                const HARMONIC_STEPS: [f32; 12] = [
                    0.0, 180.0, 120.0, 240.0, 60.0, 300.0, 90.0, 270.0, 150.0, 210.0, 30.0, 330.0,
                ];
                let offset = HARMONIC_STEPS[channel as usize % HARMONIC_STEPS.len()];
                (h_0 + offset) % 360.0
            } else {
                // Single-track solo instrument (e.g. piano on ch 0):
                // Chromatic pitch-class wheel (12 semitones around 360 degrees)
                let pitch_class = (pitch_key % 12) as f32;
                (h_0 + pitch_class * 30.0) % 360.0
            }
        } else {
            // Multi-track file: cycle through harmonic wheel
            const HARMONIC_STEPS: [f32; 12] = [
                0.0, 180.0, 120.0, 240.0, 60.0, 300.0, 90.0, 270.0, 150.0, 210.0, 30.0, 330.0,
            ];
            let offset = if track < HARMONIC_STEPS.len() {
                HARMONIC_STEPS[track]
            } else {
                (track as f32 * 137.508) % 360.0
            };
            (h_0 + offset) % 360.0
        };

        // 5. Perceptual velocity dynamics in Oklch space
        let vel_norm = (velocity as f32 / 127.0).clamp(0.1, 1.0);
        let (final_l, final_c) = if channel == 9 {
            let drum_c = note_c.max(0.20);
            if self.is_dark {
                ((0.78 + (vel_norm - 0.7) * 0.08).clamp(0.65, 0.86), drum_c)
            } else {
                ((0.36 - (vel_norm - 0.7) * 0.06).clamp(0.28, 0.42), drum_c)
            }
        } else if self.is_dark {
            // Dark mode: soft notes: ~0.66, loud notes: ~0.78
            let l = (base_l - 0.08 + vel_norm * 0.12).clamp(0.62, 0.85);
            let c = (note_c * (0.85 + 0.20 * vel_norm)).clamp(0.14, 0.26);
            (l, c)
        } else {
            // Light mode: soft notes: ~0.42, loud notes: ~0.34 (richer/deeper, high contrast)
            let l = (base_l + 0.04 - vel_norm * 0.08).clamp(0.28, 0.46);
            let c = (note_c * (0.85 + 0.20 * vel_norm)).clamp(0.14, 0.26);
            (l, c)
        };

        Oklch {
            l: final_l,
            c: final_c,
            h: target_h,
        }
        .to_rgb()
    }

    /// Hex string helper for `midi_note_color`.
    pub fn midi_note_color_hex(
        &self,
        track: usize,
        channel: u8,
        total_tracks: usize,
        pitch_key: u8,
        velocity: u8,
    ) -> String {
        let (r, g, b) = self.midi_note_color(track, channel, total_tracks, pitch_key, velocity);
        format!("#{:02x}{:02x}{:02x}", r, g, b)
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
            assert!(
                themes.len() >= 40,
                "Expected at least 40 themes, found {}",
                themes.len()
            );
            // Verify 'apollo' exists
            let apollo = themes.iter().find(|t| t.name == "apollo");
            assert!(apollo.is_some(), "apollo theme should exist");
        }
    }

    #[test]
    fn test_all_themes_button_contrast() {
        let dir = find_themes_dir();
        if !dir.exists() {
            return;
        }
        let themes = load_themes_from_dir(&dir);
        assert!(!themes.is_empty());

        for theme in themes {
            let palette = AppPalette::from_theme(&theme, &HashMap::new());
            let surfaces = [
                ("harmonies.base", &palette.harmonies.base),
                (
                    "harmonies.warm_attention",
                    &palette.harmonies.warm_attention,
                ),
                ("diff_add", &palette.diff_add),
                ("diff_del", &palette.diff_del),
                ("b_med", &palette.theme.b_med),
                ("b_low", &palette.theme.b_low),
                ("undo_card_bg", &palette.undo_card_bg),
                ("hero_card_bg", &palette.hero_card_bg),
                ("prior_card_bg", &palette.prior_card_bg),
            ];

            for (name, surf) in surfaces {
                let fg = palette.contrast_pair_fg(surf, Some(&palette.theme.f_high));
                let (br, bg, bb, _) = parse_hex_color_rgba(surf);
                let (fr, fg_val, fb, _) = parse_hex_color_rgba(&fg);
                let ratio = contrast_ratio((br, bg, bb), (fr, fg_val, fb));
                assert!(
                    ratio >= 4.5,
                    "Theme '{}' on surface '{}' ({}) with fg '{}' has contrast ratio {:.2} < 4.5",
                    theme.name,
                    name,
                    surf,
                    fg,
                    ratio
                );
            }
        }
    }

    #[test]
    fn test_zenburn_midi_colors_vibrancy() {
        let dir = find_themes_dir();
        let zenburn_file = dir.join("zenburn.svg");
        if !zenburn_file.exists() {
            return;
        }
        let theme = Theme::from_file(&zenburn_file).expect("zenburn load failed");
        let palette = AppPalette::from_theme(&theme, &HashMap::new());

        // Test track colors
        let t0 = palette.midi_track_color(0, 0, 4);
        let t1 = palette.midi_track_color(1, 0, 4);
        let drums = palette.midi_track_color(2, 9, 4);

        assert_ne!(t0, t1, "Track 0 and Track 1 should have distinct colors");
        assert_ne!(t0, drums, "Melodic and drum channels should be distinct");

        // Convert t0 to Oklch and check chroma
        let oklch0 = Oklab::from_rgb(t0.0, t0.1, t0.2).to_oklch();
        assert!(
            oklch0.c >= 0.16,
            "Zenburn track 0 chroma should be vibrant (>= 0.16), got {:.3}",
            oklch0.c
        );

        // Check contrast against b_low (canvas background)
        let (br, bg, bb, _) = parse_hex_color_rgba(&palette.theme.b_low);
        let cr0 = contrast_ratio((br, bg, bb), t0);
        assert!(
            cr0 >= 3.0,
            "Zenburn note should have >= 3.0:1 contrast against b_low, got {:.2}",
            cr0
        );

        // Test soft velocity note is still legible
        let soft_note = palette.midi_note_color(0, 0, 4, 60, 25);
        let cr_soft = contrast_ratio((br, bg, bb), soft_note);
        assert!(
            cr_soft >= 2.5,
            "Soft velocity note should remain legible, got {:.2}",
            cr_soft
        );
    }

    #[test]
    fn test_midi_single_track_pitch_differentiation() {
        let theme = Theme::apollo();
        let palette = AppPalette::from_theme(&theme, &HashMap::new());

        // Single-track piano notes of different pitch classes
        let c4 = palette.midi_note_color(0, 0, 1, 60, 100);
        let e4 = palette.midi_note_color(0, 0, 1, 64, 100);
        let g4 = palette.midi_note_color(0, 0, 1, 67, 100);

        assert_ne!(
            c4, e4,
            "C4 and E4 should have different colors in single-track"
        );
        assert_ne!(
            e4, g4,
            "E4 and G4 should have different colors in single-track"
        );

        // Octaves of same pitch class should share hue
        let c5 = palette.midi_note_color(0, 0, 1, 72, 100);
        assert_eq!(
            c4, c5,
            "Octaves (C4 and C5) should share the same pitch-class color"
        );
    }

    #[test]
    fn test_midi_colors_across_all_themes() {
        let dir = find_themes_dir();
        if !dir.exists() {
            return;
        }
        let themes = load_themes_from_dir(&dir);
        for theme in themes {
            let palette = AppPalette::from_theme(&theme, &HashMap::new());
            let (br, bg, bb, _) = parse_hex_color_rgba(&palette.theme.b_low);

            // Test 4 tracks + drum channel
            for track in 0..4 {
                let col = palette.midi_track_color(track, 0, 4);
                let cr = contrast_ratio((br, bg, bb), col);
                assert!(
                    cr >= 2.5,
                    "Theme '{}' track {} note color {:?} has low contrast {:.2} against b_low",
                    theme.name,
                    track,
                    col,
                    cr
                );
            }

            // Drums
            let drum_col = palette.midi_track_color(0, 9, 4);
            let cr_drum = contrast_ratio((br, bg, bb), drum_col);
            assert!(
                cr_drum >= 2.5,
                "Theme '{}' drum note color {:?} has low contrast {:.2} against b_low",
                theme.name,
                drum_col,
                cr_drum
            );
        }
    }
}
