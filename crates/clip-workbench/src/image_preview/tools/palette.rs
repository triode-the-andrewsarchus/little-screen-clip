use super::super::*;

pub fn extract_palette(img: &RgbaImage, target_count: usize) -> (Vec<PaletteColor>, usize) {
    let (w, h) = (img.width(), img.height());
    if w == 0 || h == 0 {
        return (Vec::new(), 0);
    }

    let total_pixels = (w as usize) * (h as usize);
    let sample_stride = ((total_pixels / 5000).max(1)).max(1);

    use std::collections::HashMap;
    let mut bucket_counts: HashMap<u16, (usize, u64, u64, u64)> = HashMap::with_capacity(4096);
    let mut sampled_valid_pixels = 0usize;

    let mut i = 0;
    for pixel in img.pixels() {
        if i % sample_stride == 0 {
            if pixel[3] >= 64 {
                sampled_valid_pixels += 1;
                let r = pixel[0] as u64;
                let g = pixel[1] as u64;
                let b = pixel[2] as u64;
                let key = (((r >> 3) << 10) | ((g >> 3) << 5) | (b >> 3)) as u16;
                let entry = bucket_counts.entry(key).or_insert((0, 0, 0, 0));
                entry.0 += 1;
                entry.1 += r;
                entry.2 += g;
                entry.3 += b;
            }
        }
        i += 1;
    }

    if sampled_valid_pixels == 0 {
        return (Vec::new(), 0);
    }

    let mut candidates: Vec<(usize, u8, u8, u8)> = bucket_counts
        .into_iter()
        .map(|(_, (count, r_sum, g_sum, b_sum))| {
            let r_avg = (r_sum / count as u64).clamp(0, 255) as u8;
            let g_avg = (g_sum / count as u64).clamp(0, 255) as u8;
            let b_avg = (b_sum / count as u64).clamp(0, 255) as u8;
            (count, r_avg, g_avg, b_avg)
        })
        .collect();

    candidates.sort_by(|a, b| b.0.cmp(&a.0));

    fn color_dist(r1: f32, g1: f32, b1: f32, r2: f32, g2: f32, b2: f32) -> f32 {
        let r_mean = (r1 + r2) / 2.0;
        let dr = r1 - r2;
        let dg = g1 - g2;
        let db = b1 - b2;
        let weight_r = 2.0 + (r_mean / 256.0);
        let weight_g = 4.0;
        let weight_b = 2.0 + ((255.0 - r_mean) / 256.0);
        ((weight_r * dr * dr) + (weight_g * dg * dg) + (weight_b * db * db)).sqrt()
    }

    let min_dist = 42.0;
    let mut selected: Vec<(Color32, usize)> = Vec::new();

    for (count, r, g, b) in &candidates {
        let r_f = *r as f32;
        let g_f = *g as f32;
        let b_f = *b as f32;

        let mut closest_idx = None;
        let mut min_d = f32::MAX;
        for (idx, (sel_col, _)) in selected.iter().enumerate() {
            let d = color_dist(
                r_f,
                g_f,
                b_f,
                sel_col.r() as f32,
                sel_col.g() as f32,
                sel_col.b() as f32,
            );
            if d < min_d {
                min_d = d;
                closest_idx = Some(idx);
            }
        }

        if min_d < min_dist {
            if let Some(idx) = closest_idx {
                selected[idx].1 += *count;
            }
        } else if selected.len() < target_count {
            selected.push((Color32::from_rgb(*r, *g, *b), *count));
        } else if let Some(idx) = closest_idx {
            selected[idx].1 += *count;
        }
    }

    selected.sort_by(|a, b| b.1.cmp(&a.1));

    let total_counted: usize = selected.iter().map(|(_, c)| *c).sum();
    let norm_factor = if total_counted > 0 {
        total_counted as f32
    } else {
        sampled_valid_pixels as f32
    };

    let result: Vec<PaletteColor> = selected
        .into_iter()
        .map(|(color, count)| {
            let hex = format!("#{:02X}{:02X}{:02X}", color.r(), color.g(), color.b());
            let percentage = (count as f32 / norm_factor) * 100.0;
            PaletteColor {
                color,
                hex,
                percentage,
                count,
            }
        })
        .collect();

    (result, sampled_valid_pixels)
}

#[derive(Debug, Clone, PartialEq)]
pub struct PaletteColor {
    pub color: Color32,
    pub hex: String,
    pub percentage: f32,
    pub count: usize,
}

#[derive(Debug, Clone)]
pub struct PaletteState {
    pub extracted_colors: Vec<PaletteColor>,
    pub num_colors: usize,
    pub is_extracted: bool,
    pub total_sampled: usize,
}

impl Default for PaletteState {
    fn default() -> Self {
        Self {
            extracted_colors: Vec::new(),
            num_colors: 8,
            is_extracted: false,
            total_sampled: 0,
        }
    }
}

impl ImagePreviewState {
    pub fn run_palette_extraction(&mut self) {
        let disp_img = self.current_display_image();
        let (colors, total_sampled) = extract_palette(&disp_img, self.palette_state.num_colors);
        self.palette_state.extracted_colors = colors;
        self.palette_state.total_sampled = total_sampled;
        self.palette_state.is_extracted = true;
        self.show_toast(format!(
            "🎨 Extracted {} dominant colors ({} sampled px)",
            self.palette_state.extracted_colors.len(),
            total_sampled
        ));
    }

    pub(crate) fn render_palette_panel(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        palette: &AppPalette,
    ) {
        let accent = hex_to_c32(&palette.harmonies.base);
        let secondary = hex_to_c32(&palette.harmonies.triadic_1);
        let surface = hex_to_c32(&palette.colors.surface_panel);
        let text = hex_to_c32(&palette.colors.text_secondary);
        let muted = hex_to_c32(&palette.colors.text_muted);

        ui.label(
            egui::RichText::new("Palette")
                .size(16.0)
                .strong()
                .color(accent),
        );
        ui.label(
            egui::RichText::new("Dominant colors sampled from the image")
                .size(crate::ui_ext::BODY_FONT_SIZE)
                .color(muted),
        );
        ui.add_space(10.0);

        let action_width = ((ui.available_width() - 6.0) * 0.5).max(72.0);
        ui.horizontal(|ui| {
            let refresh = crate::secondary_button("Refresh", surface, palette);
            if ui
                .add_sized([action_width, crate::ui_ext::CONTROL_HEIGHT], refresh)
                .on_hover_text("Re-extract dominant colors from the current image")
                .clicked()
            {
                self.run_palette_extraction();
            }

            let copy = crate::secondary_button("Copy CSV", surface, palette);
            if ui
                .add_enabled_ui(!self.palette_state.extracted_colors.is_empty(), |ui| {
                    ui.add_sized([action_width, crate::ui_ext::CONTROL_HEIGHT], copy)
                })
                .inner
                .on_hover_text("Copy all palette hex codes")
                .clicked()
            {
                let csv = self
                    .palette_state
                    .extracted_colors
                    .iter()
                    .map(|color| color.hex.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                ctx.output_mut(|output| output.copied_text = csv.clone());
                let csv_for_command = csv.clone();
                std::thread::spawn(move || {
                    if let Err(error) =
                        little_clip_core::copy_text_to_system_clipboard(&csv_for_command)
                    {
                        tracing::debug!(%error, "failed to copy palette to system clipboard");
                    }
                });
                self.show_toast("Copied palette hex codes to clipboard");
            }
        });

        ui.add_space(10.0);
        ui.separator();
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("DOMINANT COLORS")
                    .size(crate::ui_ext::BODY_FONT_SIZE)
                    .strong()
                    .color(secondary),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    egui::RichText::new(self.palette_state.extracted_colors.len().to_string())
                        .size(crate::ui_ext::BODY_FONT_SIZE)
                        .color(muted),
                );
            });
        });
        ui.add_space(4.0);

        if self.palette_state.extracted_colors.is_empty() {
            ui.add_space(8.0);
            ui.label(
                egui::RichText::new("No palette extracted yet.")
                    .size(crate::ui_ext::BODY_FONT_SIZE)
                    .italics()
                    .color(muted),
            );
            let extract = crate::action_button("Extract palette", accent, palette);
            if ui
                .add_sized(
                    [ui.available_width(), crate::ui_ext::CONTROL_HEIGHT],
                    extract,
                )
                .clicked()
            {
                self.run_palette_extraction();
            }
            return;
        }

        for color in self.palette_state.extracted_colors.clone() {
            ui.add_space(3.0);
            ui.horizontal(|ui| {
                let (swatch, response) =
                    ui.allocate_exact_size(Vec2::splat(34.0), egui::Sense::click());
                ui.painter()
                    .rect_filled(swatch, Rounding::same(4.0), color.color);
                ui.painter().rect_stroke(
                    swatch,
                    Rounding::same(4.0),
                    Stroke::new(1.0_f32, surface),
                );

                ui.vertical(|ui| {
                    ui.label(
                        egui::RichText::new(&color.hex)
                            .size(crate::ui_ext::BODY_FONT_SIZE)
                            .strong()
                            .monospace()
                            .color(text),
                    );
                    ui.label(
                        egui::RichText::new(format!("{:.1}% of sampled pixels", color.percentage))
                            .size(crate::ui_ext::BODY_FONT_SIZE)
                            .color(muted),
                    );
                });

                if response
                    .on_hover_text("Add this color to the picker")
                    .clicked()
                {
                    self.record_picked_color(color.color);
                }
            });
        }
    }
}
