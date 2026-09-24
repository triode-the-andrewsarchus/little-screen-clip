//! ANSI escape sequence parser and styled egui text layout converter.
//!
//! Converts terminal color codes, bold, italic, and underline markers into styled [`egui::text::LayoutJob`]s.

use eframe::egui::{self, Color32, FontId, Stroke};
use little_clip_core::config::WorkbenchAppearance;
use little_clip_core::palette::AppPalette;

use crate::ui_ext::contrast_text_c32_on;

/// Converts raw ANSI terminal escape sequences into a styled [`egui::text::LayoutJob`].
pub fn ansi_to_layout_job(ansi_text: &str, font_size: f32) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    job.wrap = egui::text::TextWrapping::no_max_width();

    let palette = AppPalette::for_appearance(WorkbenchAppearance::Dark);
    let console = crate::hex_to_c32(&palette.colors.console_surface);
    let append_chunk = |job: &mut egui::text::LayoutJob,
                        chunk: &str,
                        foreground: Color32,
                        background: Color32,
                        dim: bool,
                        underlined: bool| {
        if chunk.is_empty() {
            return;
        }
        let preferred = if dim {
            Color32::from_rgb(
                (foreground.r() as f32 * 0.8) as u8,
                (foreground.g() as f32 * 0.8) as u8,
                (foreground.b() as f32 * 0.8) as u8,
            )
        } else {
            foreground
        };
        let color = contrast_text_c32_on(background, console, Some(preferred), &palette);
        job.append(
            chunk,
            0.0,
            egui::TextFormat {
                font_id: FontId::monospace(font_size),
                color,
                background,
                underline: if underlined {
                    Stroke::new(1.0_f32, color)
                } else {
                    Stroke::NONE
                },
                ..Default::default()
            },
        );
    };

    let def_fg = crate::hex_to_c32(&palette.colors.text_secondary);
    let mut cur_fg = def_fg;
    let mut cur_bg = Color32::TRANSPARENT;
    let mut _is_bold = false;
    let mut is_dim = false;
    let mut is_underlined = false;

    let bytes = ansi_text.as_bytes();
    let mut i = 0;
    let mut text_start = 0;

    while i < bytes.len() {
        if bytes[i] == 0x1b && i + 1 < bytes.len() && bytes[i + 1] == b'[' {
            // Flush current text segment
            if i > text_start {
                append_chunk(
                    &mut job,
                    &ansi_text[text_start..i],
                    cur_fg,
                    cur_bg,
                    is_dim,
                    is_underlined,
                );
            }

            // Parse escape code
            i += 2;
            let code_start = i;
            while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b';') {
                i += 1;
            }

            if i < bytes.len() && bytes[i] == b'm' {
                let code_str = &ansi_text[code_start..i];
                i += 1; // skip 'm'
                text_start = i;

                if code_str.is_empty() || code_str == "0" {
                    // Reset all
                    cur_fg = def_fg;
                    cur_bg = Color32::TRANSPARENT;
                    _is_bold = false;
                    is_dim = false;
                    is_underlined = false;
                } else {
                    let parts: Vec<&str> = code_str.split(';').collect();
                    let mut idx = 0;
                    while idx < parts.len() {
                        let val: u32 = parts[idx].parse().unwrap_or(0);
                        match val {
                            0 => {
                                cur_fg = def_fg;
                                cur_bg = Color32::TRANSPARENT;
                                _is_bold = false;
                                is_dim = false;
                                is_underlined = false;
                            }
                            1 => _is_bold = true,
                            2 => is_dim = true,
                            4 => is_underlined = true,
                            22 => {
                                _is_bold = false;
                                is_dim = false;
                            }
                            24 => is_underlined = false,

                            30 => cur_fg = Color32::from_rgb(148, 163, 184), // Readable dark ANSI
                            31 => cur_fg = Color32::from_rgb(239, 68, 68),   // Red
                            32 => cur_fg = Color32::from_rgb(34, 197, 94),   // Green
                            33 => cur_fg = Color32::from_rgb(234, 179, 8),   // Yellow
                            34 => cur_fg = Color32::from_rgb(59, 130, 246),  // Blue
                            35 => cur_fg = Color32::from_rgb(168, 85, 247),  // Purple
                            36 => cur_fg = Color32::from_rgb(6, 182, 212),   // Cyan
                            37 => cur_fg = Color32::from_rgb(241, 245, 249), // White
                            38 => {
                                // 256-color or 24-bit TrueColor RGB
                                if idx + 4 < parts.len() && parts[idx + 1] == "2" {
                                    let r: u8 = parts[idx + 2].parse().unwrap_or(255);
                                    let g: u8 = parts[idx + 3].parse().unwrap_or(255);
                                    let b: u8 = parts[idx + 4].parse().unwrap_or(255);
                                    cur_fg = Color32::from_rgb(r, g, b);
                                    idx += 4;
                                } else if idx + 2 < parts.len() && parts[idx + 1] == "5" {
                                    let col_idx: u8 = parts[idx + 2].parse().unwrap_or(7);
                                    cur_fg = ansi_256_to_color32(col_idx);
                                    idx += 2;
                                }
                            }
                            39 => cur_fg = def_fg,
                            40 => cur_bg = Color32::TRANSPARENT,
                            41 => cur_bg = Color32::from_rgba_unmultiplied(153, 27, 27, 100),
                            42 => cur_bg = Color32::from_rgba_unmultiplied(20, 83, 45, 100),
                            43 => cur_bg = Color32::from_rgba_unmultiplied(113, 63, 18, 100),
                            44 => cur_bg = Color32::from_rgba_unmultiplied(30, 58, 138, 100),
                            45 => cur_bg = Color32::from_rgba_unmultiplied(107, 33, 168, 100),
                            46 => cur_bg = Color32::from_rgba_unmultiplied(12, 74, 110, 100),
                            47 => cur_bg = Color32::from_rgba_unmultiplied(203, 213, 225, 40),
                            48 => {
                                // 256-color or 24-bit TrueColor RGB
                                if idx + 4 < parts.len() && parts[idx + 1] == "2" {
                                    let r: u8 = parts[idx + 2].parse().unwrap_or(0);
                                    let g: u8 = parts[idx + 3].parse().unwrap_or(0);
                                    let b: u8 = parts[idx + 4].parse().unwrap_or(0);
                                    cur_bg = Color32::from_rgba_unmultiplied(r, g, b, 120);
                                    idx += 4;
                                } else if idx + 2 < parts.len() && parts[idx + 1] == "5" {
                                    let col_idx: u8 = parts[idx + 2].parse().unwrap_or(0);
                                    cur_bg = ansi_256_to_color32(col_idx);
                                    idx += 2;
                                }
                            }
                            49 => cur_bg = Color32::TRANSPARENT,

                            90 => cur_fg = Color32::from_rgb(148, 163, 184), // Bright Black (Slate)
                            91 => cur_fg = Color32::from_rgb(248, 113, 113), // Bright Red
                            92 => cur_fg = Color32::from_rgb(74, 222, 128),  // Bright Green
                            93 => cur_fg = Color32::from_rgb(250, 204, 21),  // Bright Yellow
                            94 => cur_fg = Color32::from_rgb(96, 165, 250),  // Bright Blue
                            95 => cur_fg = Color32::from_rgb(232, 121, 249), // Bright Magenta
                            96 => cur_fg = Color32::from_rgb(56, 189, 248),  // Bright Cyan
                            97 => cur_fg = Color32::from_rgb(255, 255, 255), // Bright White
                            100 => cur_bg = Color32::from_rgba_unmultiplied(51, 65, 85, 90),
                            101 => cur_bg = Color32::from_rgba_unmultiplied(185, 28, 28, 110), // Bright red bg
                            102 => cur_bg = Color32::from_rgba_unmultiplied(21, 128, 61, 110), // Bright green bg
                            _ => {}
                        }
                        idx += 1;
                    }
                }
            } else {
                // Not an SGR code, skip to next character
                i += 1;
                text_start = i;
            }
        } else {
            i += 1;
        }
    }

    // Flush remaining text
    if text_start < ansi_text.len() {
        append_chunk(
            &mut job,
            &ansi_text[text_start..],
            cur_fg,
            cur_bg,
            is_dim,
            is_underlined,
        );
    }

    job
}

fn ansi_256_to_color32(idx: u8) -> Color32 {
    match idx {
        0 => Color32::from_rgb(0, 0, 0),
        1 => Color32::from_rgb(239, 68, 68),
        2 => Color32::from_rgb(34, 197, 94),
        3 => Color32::from_rgb(234, 179, 8),
        4 => Color32::from_rgb(59, 130, 246),
        5 => Color32::from_rgb(168, 85, 247),
        6 => Color32::from_rgb(6, 182, 212),
        7 => Color32::from_rgb(226, 232, 240),
        8 => Color32::from_rgb(100, 116, 139),
        9 => Color32::from_rgb(248, 113, 113),
        10 => Color32::from_rgb(74, 222, 128),
        11 => Color32::from_rgb(250, 204, 21),
        12 => Color32::from_rgb(96, 165, 250),
        13 => Color32::from_rgb(232, 121, 249),
        14 => Color32::from_rgb(56, 189, 248),
        15 => Color32::from_rgb(255, 255, 255),
        16..=231 => {
            let n = idx - 16;
            let b = (n % 6) * 51;
            let g = ((n / 6) % 6) * 51;
            let r = (n / 36) * 51;
            Color32::from_rgb(r, g, b)
        }
        232..=255 => {
            let gray = (idx - 232) * 10 + 8;
            Color32::from_rgb(gray, gray, gray)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use little_clip_core::config::WorkbenchAppearance;
    use little_clip_core::palette::{BaseColors, contrast_ratio};

    #[test]
    fn standard_ansi_text_is_readable_on_console_surface_in_both_modes() {
        let job = ansi_to_layout_job(
            "plain \u{1b}[30mblack\u{1b}[0m \u{1b}[31mred\u{1b}[0m \u{1b}[32mgreen\u{1b}[0m",
            crate::ui_ext::MONO_FONT_SIZE,
        );
        for appearance in [WorkbenchAppearance::Dark, WorkbenchAppearance::Light] {
            let colors = BaseColors::for_appearance(appearance);
            let background = crate::hex_to_c32(&colors.console_surface);
            let background = (background.r(), background.g(), background.b());
            for section in &job.sections {
                let color = section.format.color;
                assert!(
                    contrast_ratio(background, (color.r(), color.g(), color.b())) >= 4.5,
                    "{appearance:?}: ANSI segment was unreadable on console surface"
                );
                assert_eq!(section.format.font_id.size, crate::ui_ext::MONO_FONT_SIZE);
            }
        }
    }

    #[test]
    fn test_ansi_parsing_plain() {
        let job = ansi_to_layout_job("Hello world", 12.0);
        assert_eq!(job.text, "Hello world");
    }

    #[test]
    fn test_ansi_parsing_colors() {
        let text = "\x1b[91mremoved\x1b[0m and \x1b[92;1madded\x1b[0m";
        let job = ansi_to_layout_job(text, 12.0);
        assert_eq!(job.text, "removed and added");
        assert_eq!(job.sections.len(), 3);
        assert_eq!(
            job.sections[0].format.color,
            Color32::from_rgb(248, 113, 113)
        );
        assert_eq!(
            job.sections[2].format.color,
            Color32::from_rgb(74, 222, 128)
        );
    }

    #[test]
    fn test_ansi_parsing_truecolor() {
        let text = "\x1b[38;2;123;45;67mcustom colored\x1b[0m";
        let job = ansi_to_layout_job(text, 12.0);
        assert_eq!(job.text, "custom colored");
        assert_eq!(job.sections.len(), 1);
        let console = crate::hex_to_c32(
            &AppPalette::for_appearance(WorkbenchAppearance::Dark)
                .colors
                .console_surface,
        );
        let text = job.sections[0].format.color;
        assert!(
            little_clip_core::palette::contrast_ratio(
                (console.r(), console.g(), console.b()),
                (text.r(), text.g(), text.b())
            ) >= 4.5
        );
    }

    #[test]
    fn unreadable_256_color_is_replaced_and_trailing_underline_is_preserved() {
        let job = ansi_to_layout_job("\x1b[4;38;5;0mblack", 15.0);
        let console = crate::hex_to_c32(
            &AppPalette::for_appearance(WorkbenchAppearance::Dark)
                .colors
                .console_surface,
        );
        let format = &job.sections[0].format;
        assert!(
            little_clip_core::palette::contrast_ratio(
                (console.r(), console.g(), console.b()),
                (format.color.r(), format.color.g(), format.color.b())
            ) >= 4.5
        );
        assert_ne!(format.underline, Stroke::NONE);
    }
}
