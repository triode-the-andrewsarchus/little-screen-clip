//! Bounded, whole-document summaries for passive horizontal file previews.
mod analysis;
mod raster;
mod stream;
#[cfg(test)]
mod tests;
pub(super) use analysis::{is_typescript_source, known_text, load};
pub(super) use raster::{palette_colors, render};

use syntect::highlighting::Color;

const COLUMNS: usize = 96;
const BINS: usize = 1024;
const EXCERPT_LINES: usize = 6;
const SHORT_FILE_LINES: u64 = 20;
const EXCERPT_COLUMNS: usize = 200;
const COLORS: [Color; 9] = [
    Color {
        r: 211,
        g: 208,
        b: 200,
        a: 255,
    },
    Color {
        r: 116,
        g: 115,
        b: 105,
        a: 255,
    },
    Color {
        r: 242,
        g: 119,
        b: 122,
        a: 255,
    },
    Color {
        r: 249,
        g: 145,
        b: 87,
        a: 255,
    },
    Color {
        r: 255,
        g: 204,
        b: 102,
        a: 255,
    },
    Color {
        r: 153,
        g: 204,
        b: 153,
        a: 255,
    },
    Color {
        r: 102,
        g: 204,
        b: 204,
        a: 255,
    },
    Color {
        r: 102,
        g: 153,
        b: 204,
        a: 255,
    },
    Color {
        r: 204,
        g: 153,
        b: 204,
        a: 255,
    },
];

pub(in crate::file_manager) struct Document {
    pub lines: u64,
    pub language: String,
    pub badge: String,
    pub note: String,
    columns: usize,
    bins: Vec<Bin>,
}
struct Bin {
    start: u64,
    end: u64,
    cells: Vec<[u32; COLORS.len()]>,
    excerpt: Vec<ExcerptLine>,
}
struct ExcerptLine {
    number: u64,
    runs: Vec<(String, usize)>,
}

impl Document {
    fn display_range(&self, fraction: f32, width: usize) -> std::ops::Range<usize> {
        let columns = width.max(1).min(self.bins.len());
        let x = ((fraction.clamp(0.0, 1.0) * columns as f32) as usize).min(columns - 1);
        x * self.bins.len() / columns..((x + 1) * self.bins.len() / columns).max(1)
    }
    pub(super) fn excerpt_ui(
        &self,
        ui: &mut egui::Ui,
        fraction: f32,
        width: usize,
        palette: &little_clip_core::palette::AppPalette,
    ) {
        let short_file = self.lines <= SHORT_FILE_LINES;
        let range = if short_file {
            0..self.bins.len()
        } else {
            self.display_range(fraction, width)
        };
        let start = range.start;
        let first = self.bins[start].start + 1;
        let last = self.bins[range.end - 1].end;
        if self.lines == 0 {
            ui.label("Empty text file");
            return;
        }
        ui.label(format!(
            "{} · {} lines · lines {first}–{last}",
            self.language, self.lines
        ));
        if !self.note.is_empty() {
            ui.label(&self.note);
        }
        let width = (ui.ctx().screen_rect().width() - 40.0).clamp(160.0, 760.0);
        ui.set_width(width);
        let font_id = egui::FontId::monospace(14.0);
        let mut remaining = if short_file {
            let screen = ui.ctx().screen_rect();
            let pointer_y = ui.ctx().pointer_hover_pos().map_or(screen.top(), |p| p.y);
            let content_top = ui.cursor().top().max(pointer_y + 16.0);
            // Reserve frame padding and the heading inside the one-third-window
            // budget, and shrink further when hovering near the bottom edge.
            let height = (screen.height() / 3.0 - 40.0)
                .min(screen.bottom() - content_top - 12.0)
                .max(0.0);
            let row_height =
                ui.fonts(|fonts| fonts.row_height(&font_id)) + ui.spacing().item_spacing.y;
            (height / row_height).floor() as usize
        } else {
            EXCERPT_LINES
        };
        for line in self.bins[start..].iter().flat_map(|bin| &bin.excerpt) {
            if remaining == 0 {
                break;
            }
            remaining -= 1;
            let mut job = egui::text::LayoutJob::default();
            job.append(
                &format!("{:>6}  ", line.number),
                0.0,
                egui::TextFormat {
                    font_id: font_id.clone(),
                    color: ui.visuals().weak_text_color(),
                    ..Default::default()
                },
            );
            for (text, category) in &line.runs {
                job.append(
                    text,
                    0.0,
                    egui::TextFormat {
                        font_id: font_id.clone(),
                        color: crate::text_preview::map_syntect_color_to_theme(
                            COLORS[*category],
                            palette,
                        ),
                        ..Default::default()
                    },
                );
            }
            job.wrap.max_width = width;
            job.wrap.max_rows = 1;
            job.wrap.break_anywhere = true;
            ui.add(egui::Label::new(job).selectable(false));
        }
    }
}
