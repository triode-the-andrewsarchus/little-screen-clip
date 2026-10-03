//! Fixed application palettes and Oklab perceptual color space calculations.

use serde::{Deserialize, Serialize};

use crate::color::parse_hex_color_rgba_or_white as parse_hex_color_rgba;
use crate::config::WorkbenchAppearance;

/// Semantic base colors shared by the workbench and little-screen display.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BaseColors {
    pub canvas: String,
    /// High-contrast data viewport shared by both appearances.
    pub console_surface: String,
    pub surface_raised: String,
    pub surface_panel: String,
    pub surface_recessed: String,
    pub surface_inverse: String,
    pub text_primary: String,
    pub text_secondary: String,
    pub text_muted: String,
    pub text_inverse: String,
}

impl BaseColors {
    pub fn for_appearance(appearance: WorkbenchAppearance) -> Self {
        let (canvas, primary, secondary, muted, inverse_text, raised, panel, recessed, inverse) =
            match appearance {
                WorkbenchAppearance::Dark => (
                    "#1f1f1f", "#f2f2f2", "#d5d5d5", "#b5b5b5", "#181818", "#383838", "#2c2c2c",
                    "#242424", "#d8d8d8",
                ),
                WorkbenchAppearance::Light => (
                    "#eeeeee", "#262626", "#484848", "#595959", "#ffffff", "#ffffff", "#e3e3e3",
                    "#e9e9e9", "#484848",
                ),
            };
        Self {
            canvas: canvas.into(),
            console_surface: "#141414".into(),
            surface_raised: raised.into(),
            surface_panel: panel.into(),
            surface_recessed: recessed.into(),
            surface_inverse: inverse.into(),
            text_primary: primary.into(),
            text_secondary: secondary.into(),
            text_muted: muted.into(),
            text_inverse: inverse_text.into(),
        }
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

impl BaseColors {
    /// Selects the best foreground color to render on `bg_hex`.
    /// 1. Tests `preferred_fg` (or primary text by default). If contrast >= 4.5, returns it.
    /// 2. If contrast < 4.5, tries the other semantic surface and text colors.
    /// 3. If none achieve contrast >= 4.5, selects between pure black ("#000000") and pure white ("#ffffff"),
    ///    whichever provides higher contrast against `bg_hex`.
    pub fn contrast_pair_fg(&self, bg_hex: &str, preferred_fg: Option<&str>) -> String {
        let (br, bg, bb, _) = parse_hex_color_rgba(bg_hex);
        let bg_rgb = (br, bg, bb);

        let pref = preferred_fg.unwrap_or(&self.text_primary);
        let (pr, pg, pb, _) = parse_hex_color_rgba(pref);
        if contrast_ratio(bg_rgb, (pr, pg, pb)) >= 4.5 {
            return pref.to_string();
        }

        let candidates = [
            &self.text_primary,
            &self.text_inverse,
            &self.canvas,
            &self.surface_raised,
            &self.surface_panel,
            &self.surface_recessed,
            &self.surface_inverse,
            &self.text_secondary,
            &self.text_muted,
        ];

        // Try the semantic palette colors to find the one with the highest contrast (>= 4.5).
        let mut best_candidate: Option<(&str, f32)> = None;
        for cand in candidates {
            let (cr, cg, cb, _) = parse_hex_color_rgba(cand);
            let ratio = contrast_ratio(bg_rgb, (cr, cg, cb));
            if ratio >= 4.5 {
                if let Some((_, best_r)) = best_candidate {
                    if ratio > best_r {
                        best_candidate = Some((cand, ratio));
                    }
                } else {
                    best_candidate = Some((cand, ratio));
                }
            }
        }

        if let Some((best, _)) = best_candidate {
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
            l: if is_dark { 0.78 } else { 0.50 },
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
    pub is_dark: bool,
    pub colors: BaseColors,
    pub harmonies: DerivedHarmonies,

    // Borders & Accents
    pub undo_border: String,
    pub chrome_border: String,
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
    pub url_color: String,

    // Functional Accents (Diffs, Status)
    pub diff_add: String,
    pub diff_del: String,
    pub diff_mod: String,
}

impl Default for AppPalette {
    fn default() -> Self {
        Self::for_appearance(WorkbenchAppearance::Dark)
    }
}

impl AppPalette {
    /// Fixed application colors shared by the workbench and little-screen renderer.
    pub fn for_appearance(appearance: WorkbenchAppearance) -> Self {
        let colors = BaseColors::for_appearance(appearance);
        let link = match appearance {
            WorkbenchAppearance::Dark => "#42b9f3",
            WorkbenchAppearance::Light => "#215c84",
        };
        Self::from_fixed_colors(appearance, colors, link)
    }

    fn from_fixed_colors(appearance: WorkbenchAppearance, colors: BaseColors, link: &str) -> Self {
        let is_dark = appearance == WorkbenchAppearance::Dark;

        // A fixed sky-blue anchor gives both renderers the same perceptual accent
        // family. Dark uses luminous pastels; Light uses ink-like counterparts.
        let base_accent = Oklch {
            l: if is_dark { 0.80 } else { 0.43 },
            c: 0.13,
            h: 235.0,
        }
        .to_hex();
        let harmonies = DerivedHarmonies::from_base_hex(&base_accent, is_dark);

        // Diff colors derived in Oklch
        let diff_add = Oklch {
            l: if is_dark { 0.72 } else { 0.45 },
            c: 0.14,
            h: 142.0, // Emerald green
        }
        .to_hex();

        let diff_del = Oklch {
            l: if is_dark { 0.73 } else { 0.42 },
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

        Self {
            is_dark,
            harmonies: harmonies.clone(),

            undo_border: harmonies.warm_attention.clone(),
            chrome_border: if is_dark { "#8c8c8c" } else { "#707070" }.to_owned(),
            left_hand_border: harmonies.base.clone(),
            right_hand_border: harmonies.tetradic_1.clone(),

            left_prior1_border: harmonies.split_comp_1.clone(),
            left_prior2_border: harmonies.triadic_1.clone(),
            left_prior3_border: harmonies.split_comp_2.clone(),

            right_prior1_border: harmonies.triadic_2.clone(),
            right_prior2_border: harmonies.tetradic_1.clone(),

            cursor_border: harmonies.warm_attention.clone(),
            selected_border: harmonies.triadic_1.clone(),

            url_color: link.to_owned(),

            diff_add,
            diff_del,
            diff_mod,

            colors,
        }
    }

    /// Helper returning `(r, g, b, a)` for any hex string in the palette.
    pub fn rgba(&self, hex: &str) -> (u8, u8, u8, u8) {
        parse_hex_color_rgba(hex)
    }

    /// Selects the best foreground color to render on `bg_hex` with sufficient contrast (minimum 4.5:1).
    pub fn contrast_pair_fg(&self, bg_hex: &str, preferred_fg: Option<&str>) -> String {
        self.colors.contrast_pair_fg(bg_hex, preferred_fg)
    }

    /// Computes an appearance-aware data color for a MIDI track or channel.
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

    /// Computes a vibrant, appearance-aware data color for an individual MIDI note.
    ///
    /// Ensures note colors:
    /// 1. Keeps the data color wheel independent of workbench chrome accents.
    /// 2. Starts from an Oklch chroma of 0.18 to preserve vibrant track colors.
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
        // Keep the data color wheel independent of the restrained chrome accents.
        // Starting from warm orange also preserves familiar MIDI track colors.
        let h_0 = if self.is_dark { 55.0 } else { 34.0 };
        let note_c = 0.18_f32;

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
    fn fixed_palette_contrast_is_sufficient_in_both_appearances() {
        assert!((contrast_ratio((0, 0, 0), (255, 255, 255)) - 21.0).abs() < 0.1);
        for appearance in [WorkbenchAppearance::Dark, WorkbenchAppearance::Light] {
            let palette = AppPalette::for_appearance(appearance);
            for neutral in [
                &palette.colors.canvas,
                &palette.colors.surface_panel,
                &palette.colors.surface_raised,
                &palette.colors.surface_recessed,
                &palette.colors.surface_inverse,
                &palette.colors.console_surface,
                &palette.colors.text_primary,
                &palette.colors.text_secondary,
                &palette.colors.text_muted,
                &palette.colors.text_inverse,
                &palette.chrome_border,
            ] {
                let (r, g, b, _) = parse_hex_color_rgba(neutral);
                assert_eq!(
                    (r, r),
                    (g, b),
                    "{appearance:?}: {neutral} must be neutral gray"
                );
            }
            let fg = palette
                .contrast_pair_fg(&palette.colors.canvas, Some(&palette.colors.text_primary));
            let (br, bg, bb, _) = parse_hex_color_rgba(&palette.colors.canvas);
            let (fr, fg, fb, _) = parse_hex_color_rgba(&fg);
            assert!(contrast_ratio((br, bg, bb), (fr, fg, fb)) >= 4.5);
        }
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
    fn fixed_palette_button_surfaces_meet_contrast_in_both_appearances() {
        for appearance in [WorkbenchAppearance::Dark, WorkbenchAppearance::Light] {
            let palette = AppPalette::for_appearance(appearance);
            let surfaces = [
                &palette.harmonies.base,
                &palette.harmonies.warm_attention,
                &palette.diff_add,
                &palette.diff_del,
                &palette.colors.surface_panel,
                &palette.colors.surface_recessed,
                &palette.colors.surface_recessed,
                &palette.colors.surface_raised,
                &palette.colors.surface_panel,
            ];

            for surface in surfaces {
                let fg = palette.contrast_pair_fg(surface, Some(&palette.colors.text_primary));
                let (br, bg, bb, _) = parse_hex_color_rgba(surface);
                let (fr, fg, fb, _) = parse_hex_color_rgba(&fg);
                assert!(
                    contrast_ratio((br, bg, bb), (fr, fg, fb)) >= 4.5,
                    "{appearance:?} palette has insufficient text contrast on {surface}"
                );
            }
        }
    }

    #[test]
    fn fixed_palette_midi_pitch_classes_are_distinct_in_both_appearances() {
        for appearance in [WorkbenchAppearance::Dark, WorkbenchAppearance::Light] {
            let palette = AppPalette::for_appearance(appearance);
            let c4 = palette.midi_note_color(0, 0, 1, 60, 100);
            let e4 = palette.midi_note_color(0, 0, 1, 64, 100);
            let g4 = palette.midi_note_color(0, 0, 1, 67, 100);
            assert_ne!(c4, e4, "{appearance:?}: C4 and E4 should differ");
            assert_ne!(e4, g4, "{appearance:?}: E4 and G4 should differ");
            assert_eq!(c4, palette.midi_note_color(0, 0, 1, 72, 100));
        }
    }

    #[test]
    fn fixed_palette_midi_colors_remain_distinct_and_legible_in_both_appearances() {
        for appearance in [WorkbenchAppearance::Dark, WorkbenchAppearance::Light] {
            let palette = AppPalette::for_appearance(appearance);
            let (br, bg, bb, _) = parse_hex_color_rgba(&palette.colors.surface_recessed);
            let tracks: Vec<_> = (0..4)
                .map(|track| palette.midi_track_color(track, 0, 4))
                .collect();
            let drums = palette.midi_track_color(0, 9, 4);

            assert_ne!(tracks[0], tracks[1], "{appearance:?} tracks should differ");
            assert_ne!(tracks[0], drums, "{appearance:?} drums should differ");
            for color in tracks.into_iter().chain([drums]) {
                assert!(
                    contrast_ratio((br, bg, bb), color) >= 2.5,
                    "{appearance:?} MIDI color {color:?} lacks contrast against display surface"
                );
            }
            let soft_note = palette.midi_note_color(0, 0, 4, 60, 25);
            assert!(
                contrast_ratio((br, bg, bb), soft_note) >= 2.5,
                "{appearance:?} soft-velocity MIDI note lacks contrast"
            );
        }
    }
}
