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
    pub is_extracting: bool,
    pub error: Option<String>,
    pub total_sampled: usize,
}

impl Default for PaletteState {
    fn default() -> Self {
        Self {
            extracted_colors: Vec::new(),
            num_colors: 8,
            is_extracted: false,
            is_extracting: false,
            error: None,
            total_sampled: 0,
        }
    }
}

impl ImagePreviewState {
    fn copy_palette_csv_on(
        &mut self,
        ctx: &egui::Context,
        jobs: &BackgroundJobs,
        csv: String,
        copy: impl FnOnce(&str) + Send + 'static,
    ) {
        ctx.output_mut(|output| output.copied_text = csv.clone());
        super::schedule_system_clipboard_copy_on(jobs, csv, copy);
        self.show_toast("Copied palette hex codes to clipboard");
    }

    fn palette_csv(&self) -> String {
        self.palette_state
            .extracted_colors
            .iter()
            .map(|color| color.hex.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    }

    pub fn run_palette_extraction(&mut self) {
        self.start_palette_extraction(BackgroundJobs::global());
    }

    fn start_palette_extraction(&mut self, jobs: &BackgroundJobs) {
        if let Some(ticket) = self.palette_ticket.take() {
            ticket.cancel();
        }
        self.palette_rx = None;
        let snapshot = self.display_image_snapshot_request();
        let target_count = self.palette_state.num_colors;
        let stamp = snapshot.input_stamp.clone();
        self.invalidate_stale_palette_result_for(&stamp, target_count);
        let (ticket, receiver) = jobs.submit(std::time::Duration::MAX, move || {
            let disp_img = snapshot.compose();
            extract_palette(&disp_img, target_count)
        });
        self.palette_ticket = Some(ticket);
        self.palette_rx = Some(receiver);
        self.palette_request_stamp = Some(stamp);
        self.palette_request_count = Some(target_count);
        self.palette_state.is_extracting = true;
        self.palette_state.error = None;
    }

    pub(crate) fn poll_palette_extraction(&mut self, ctx: &egui::Context) {
        let Some(receiver) = &self.palette_rx else {
            return;
        };
        let request_is_stale = self.palette_request_stamp.as_ref() != Some(&self.input_stamp())
            || self.palette_request_count != Some(self.palette_state.num_colors);
        if request_is_stale {
            if let Some(ticket) = self.palette_ticket.take() {
                ticket.cancel();
            }
            self.palette_rx = None;
            self.palette_request_stamp = None;
            self.palette_request_count = None;
            self.palette_state.is_extracting = false;
            self.palette_state.is_extracted = false;
            self.palette_state.extracted_colors.clear();
            self.palette_state.total_sampled = 0;
            self.palette_results_stamp = None;
            self.palette_results_count = None;
            return;
        }

        match receiver.try_recv() {
            Ok(Ok((colors, total_sampled))) => {
                let count = colors.len();
                self.palette_results_stamp = self.palette_request_stamp.take();
                self.palette_results_count = self.palette_request_count.take();
                self.palette_state.extracted_colors = colors;
                self.palette_state.total_sampled = total_sampled;
                self.palette_state.is_extracted = true;
                self.palette_state.is_extracting = false;
                self.palette_state.error = None;
                self.palette_ticket = None;
                self.palette_rx = None;
                self.show_toast(format!(
                    "🎨 Extracted {count} dominant colors ({total_sampled} sampled px)"
                ));
            }
            Ok(Err(error)) => {
                self.palette_state.is_extracting = false;
                self.palette_state.error = Some(format!("Palette extraction failed: {error}"));
                self.palette_ticket = None;
                self.palette_rx = None;
                self.palette_request_stamp = None;
                self.palette_request_count = None;
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => ctx.request_repaint(),
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.palette_state.is_extracting = false;
                self.palette_state.error = Some("Palette extraction worker disconnected".into());
                self.palette_ticket = None;
                self.palette_rx = None;
                self.palette_request_stamp = None;
                self.palette_request_count = None;
            }
        }
    }

    fn invalidate_stale_palette_result_for(&mut self, stamp: &ImageInputStamp, count: usize) {
        if self.palette_results_stamp.as_ref() != Some(stamp)
            || self.palette_results_count != Some(count)
        {
            self.palette_state.extracted_colors.clear();
            self.palette_state.total_sampled = 0;
            self.palette_state.is_extracted = false;
            self.palette_results_stamp = None;
            self.palette_results_count = None;
        }
    }

    pub(crate) fn invalidate_stale_palette_result(&mut self) {
        let stamp = self.input_stamp();
        self.invalidate_stale_palette_result_for(&stamp, self.palette_state.num_colors);
    }

    pub(crate) fn render_palette_panel(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        palette: &AppPalette,
    ) {
        self.invalidate_stale_palette_result();

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
                ctx.request_repaint();
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
                let csv = self.palette_csv();
                self.copy_palette_csv_on(ctx, BackgroundJobs::global(), csv, |text| {
                    if let Err(error) = little_clip_core::copy_text_to_system_clipboard(text) {
                        tracing::debug!(%error, "failed to copy palette to system clipboard");
                    }
                });
            }
        });

        ui.add_space(10.0);
        if self.palette_state.is_extracting {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Extracting palette…");
            });
        }
        if let Some(error) = &self.palette_state.error {
            ui.colored_label(hex_to_c32(&palette.colors.text_primary), error);
        }
        ui.separator();
        ui.add_space(crate::ui_ext::SPACE_SM);
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
        ui.add_space(crate::ui_ext::SPACE_XS);

        if self.palette_state.extracted_colors.is_empty() {
            ui.add_space(crate::ui_ext::SPACE_SM);
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
                ctx.request_repaint();
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    fn rendered_copy_csv_button_center(output: &egui::FullOutput) -> Pos2 {
        output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.text() == "Copy CSV" => {
                    Some(text.pos + text.galley.size() * 0.5)
                }
                _ => None,
            })
            .expect("palette panel renders the Copy CSV button")
    }

    #[test]
    fn copy_palette_csv_helper_preserves_order_and_egui_fallback() {
        let config = little_clip_core::config::AppConfig::default();
        let image = RgbaImage::from_fn(4, 1, |x, _| match x {
            0..=2 => image::Rgba([0x12, 0x34, 0x56, 255]),
            _ => image::Rgba([0xFE, 0xDC, 0xBA, 255]),
        });
        let mut png_bytes = Vec::new();
        image
            .write_to(
                &mut std::io::Cursor::new(&mut png_bytes),
                image::ImageFormat::Png,
            )
            .unwrap();
        let item = little_clip_core::ClipItem::from_image(png_bytes, 4, 1, "image/png");
        let mut state = ImagePreviewState::from_item(&item, &config).unwrap();
        state.palette_state.num_colors = 2;
        let jobs = BackgroundJobs::new(1, 1);
        state.start_palette_extraction(&jobs);
        wait_for_palette(&mut state);

        let extracted_hex: Vec<_> = state
            .palette_state
            .extracted_colors
            .iter()
            .map(|color| color.hex.as_str())
            .collect();
        assert_eq!(extracted_hex, ["#123456", "#FEDCBA"]);

        let context = egui::Context::default();
        let (copy_tx, copy_rx) = mpsc::channel();
        let csv = state.palette_csv();
        let copied = context.run(Default::default(), |ctx| {
            state.copy_palette_csv_on(ctx, &jobs, csv, move |text| {
                copy_tx.send(text.to_owned()).unwrap();
            });
        });

        assert_eq!(copied.platform_output.copied_text, "#123456, #FEDCBA");
        assert_eq!(
            copy_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            "#123456, #FEDCBA"
        );
        assert_eq!(state.palette_state.extracted_colors.len(), 2);
        assert!(
            state
                .status_toast
                .as_ref()
                .is_some_and(|(toast, _)| toast == "Copied palette hex codes to clipboard")
        );
    }

    #[test]
    fn palette_copy_queue_full_keeps_egui_fallback_without_running_write() {
        let jobs = BackgroundJobs::new(1, 1);
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (blocker_ticket, blocker_rx) = jobs.submit(Duration::MAX, move || {
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        });
        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        let (queued_ticket, queued_rx) = jobs.submit(Duration::MAX, || ());
        let (copy_tx, copy_rx) = mpsc::channel();

        let mut state = palette_state_for_image(RgbaImage::new(2, 2));
        state.palette_state.extracted_colors = vec![
            PaletteColor {
                color: Color32::from_rgb(0x12, 0x34, 0x56),
                hex: "#123456".into(),
                percentage: 60.0,
                count: 6,
            },
            PaletteColor {
                color: Color32::from_rgb(0xfe, 0xdc, 0xba),
                hex: "#FEDCBA".into(),
                percentage: 40.0,
                count: 4,
            },
        ];
        let prior_colors = state.palette_state.extracted_colors.clone();
        let context = egui::Context::default();
        let csv = state.palette_csv();
        let output = context.run(Default::default(), |ctx| {
            state.copy_palette_csv_on(ctx, &jobs, csv, move |text| {
                copy_tx.send(text.to_owned()).unwrap();
            });
        });

        assert_eq!(output.platform_output.copied_text, "#123456, #FEDCBA");
        assert_eq!(state.palette_state.extracted_colors, prior_colors);
        assert_eq!(
            state.status_toast.as_ref().map(|(toast, _)| toast.as_str()),
            Some("Copied palette hex codes to clipboard")
        );
        release_tx.send(()).unwrap();
        blocker_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .unwrap();
        queued_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .unwrap();
        assert!(copy_rx.recv_timeout(Duration::from_millis(50)).is_err());
        blocker_ticket.cancel();
        queued_ticket.cancel();
    }

    fn palette_state_for_image(image: RgbaImage) -> ImagePreviewState {
        let mut bytes = Vec::new();
        image
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .unwrap();
        let item = little_clip_core::ClipItem::from_image(
            bytes,
            image.width(),
            image.height(),
            "image/png",
        );
        ImagePreviewState::from_item(&item, &little_clip_core::config::AppConfig::default())
            .unwrap()
    }

    fn wait_for_palette(state: &mut ImagePreviewState) {
        let context = egui::Context::default();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while state.palette_state.is_extracting && std::time::Instant::now() < deadline {
            state.poll_palette_extraction(&context);
            std::thread::yield_now();
        }
        assert!(
            !state.palette_state.is_extracting,
            "palette worker timed out"
        );
    }

    #[test]
    fn large_palette_extraction_preserves_exact_colors_and_order() {
        let image = RgbaImage::from_fn(2048, 512, |x, y| {
            let band = ((x / 256) + (y / 128)) % 4;
            let rgb = match band {
                0 => [0x12, 0x34, 0x56],
                1 => [0xFE, 0xDC, 0xBA],
                2 => [0x36, 0x9A, 0x20],
                _ => [0x75, 0x28, 0xC0],
            };
            image::Rgba([rgb[0], rgb[1], rgb[2], 255])
        });
        let mut state = palette_state_for_image(image);
        state.palette_state.num_colors = 4;
        let expected = extract_palette(&state.current_display_image(), 4);
        let jobs = BackgroundJobs::new(1, 1);

        state.start_palette_extraction(&jobs);
        assert!(state.palette_state.is_extracting);
        wait_for_palette(&mut state);

        assert_eq!(state.palette_state.extracted_colors, expected.0);
        assert_eq!(state.palette_state.total_sampled, expected.1);
        assert!(state.palette_state.is_extracted);
        assert!(state.palette_state.error.is_none());
    }

    #[test]
    fn palette_queue_saturation_is_visible_and_clears_pending_state() {
        use std::sync::mpsc;
        use std::time::Duration;

        let jobs = BackgroundJobs::new(1, 1);
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let _blocker = jobs.submit(Duration::MAX, move || {
            let _ = started_tx.send(());
            let _ = release_rx.recv();
        });
        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        let _queued = jobs.submit(Duration::MAX, || ());

        let mut state = palette_state_for_image(RgbaImage::from_pixel(
            8,
            8,
            image::Rgba([0x12, 0x34, 0x56, 255]),
        ));
        state.start_palette_extraction(&jobs);
        let context = egui::Context::default();
        state.poll_palette_extraction(&context);

        assert!(!state.palette_state.is_extracting);
        let expected = "Palette extraction failed: background job queue is full";
        assert_eq!(state.palette_state.error.as_deref(), Some(expected));
        let palette = AppPalette::default();
        let output = context.run(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(600.0, 400.0))),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    state.render_palette_panel(ui, ctx, &palette);
                });
            },
        );
        assert!(output.shapes.iter().any(|shape| {
            matches!(&shape.shape, egui::Shape::Text(text) if text.galley.text() == expected)
        }));

        let _ = release_tx.send(());
    }

    #[test]
    fn pending_palette_extraction_is_visible() {
        use std::sync::mpsc;
        use std::time::Duration;

        let jobs = BackgroundJobs::new(1, 1);
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let _blocker = jobs.submit(Duration::MAX, move || {
            let _ = started_tx.send(());
            let _ = release_rx.recv();
        });
        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        let mut state = palette_state_for_image(RgbaImage::from_pixel(
            8,
            8,
            image::Rgba([0x12, 0x34, 0x56, 255]),
        ));
        state.start_palette_extraction(&jobs);

        let context = egui::Context::default();
        let palette = AppPalette::default();
        let output = context.run(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(600.0, 400.0))),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    state.render_palette_panel(ui, ctx, &palette);
                });
            },
        );
        assert!(state.palette_state.is_extracting);
        assert!(output.shapes.iter().any(|shape| {
            matches!(&shape.shape, egui::Shape::Text(text) if text.galley.text() == "Extracting palette…")
        }));

        drop(state);
        let _ = release_tx.send(());
    }

    #[test]
    fn palette_result_is_ignored_after_source_changes() {
        use std::sync::mpsc;
        use std::time::Duration;

        let jobs = BackgroundJobs::new(1, 1);
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let _blocker = jobs.submit(Duration::MAX, move || {
            let _ = started_tx.send(());
            let _ = release_rx.recv();
        });
        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();

        let mut state = palette_state_for_image(RgbaImage::from_pixel(
            8,
            8,
            image::Rgba([0x12, 0x34, 0x56, 255]),
        ));
        state.start_palette_extraction(&jobs);
        state.mark_pixels_changed();
        state.poll_palette_extraction(&egui::Context::default());

        assert!(!state.palette_state.is_extracting);
        assert!(!state.palette_state.is_extracted);
        assert!(state.palette_state.extracted_colors.is_empty());
        assert!(state.palette_ticket.is_none());
        assert!(state.palette_rx.is_none());

        let _ = release_tx.send(());
    }

    #[test]
    fn palette_result_is_ignored_after_target_count_changes() {
        use std::sync::mpsc;
        use std::time::Duration;

        let jobs = BackgroundJobs::new(1, 1);
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let _blocker = jobs.submit(Duration::MAX, move || {
            let _ = started_tx.send(());
            let _ = release_rx.recv();
        });
        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();

        let mut state = palette_state_for_image(RgbaImage::from_pixel(
            8,
            8,
            image::Rgba([0x12, 0x34, 0x56, 255]),
        ));
        state.palette_state.num_colors = 5;
        state.start_palette_extraction(&jobs);
        state.palette_state.num_colors = 2;
        state.poll_palette_extraction(&egui::Context::default());

        assert!(!state.palette_state.is_extracting);
        assert!(!state.palette_state.is_extracted);
        assert!(state.palette_state.extracted_colors.is_empty());
        assert!(state.palette_ticket.is_none());
        assert!(state.palette_rx.is_none());

        let _ = release_tx.send(());
    }

    #[test]
    fn dropping_image_viewer_cancels_queued_palette_extraction() {
        use std::sync::mpsc;
        use std::time::Duration;

        let jobs = BackgroundJobs::new(1, 1);
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let _blocker = jobs.submit(Duration::MAX, move || {
            let _ = started_tx.send(());
            let _ = release_rx.recv();
        });
        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();

        let mut state = palette_state_for_image(RgbaImage::from_pixel(
            8,
            8,
            image::Rgba([0x12, 0x34, 0x56, 255]),
        ));
        state.start_palette_extraction(&jobs);
        let result_rx = state.palette_rx.take().unwrap();
        drop(state);

        let _ = release_tx.send(());
        assert!(matches!(
            result_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            Err(little_clip_core::JobError::Cancelled)
        ));
    }

    #[test]
    fn changed_source_invalidates_previous_palette_before_copy() {
        let image = RgbaImage::from_pixel(4, 4, image::Rgba([0x12, 0x34, 0x56, 255]));
        let mut state = palette_state_for_image(image);
        let (colors, total_sampled) = extract_palette(&state.current_display_image(), 8);
        state.palette_state.extracted_colors = colors;
        state.palette_state.total_sampled = total_sampled;
        state.palette_state.is_extracted = true;
        state.palette_results_stamp = Some(state.input_stamp());
        state.palette_results_count = Some(8);
        state.mark_pixels_changed();

        let palette = AppPalette::default();
        let context = egui::Context::default();
        let render = |state: &mut ImagePreviewState, time, events| {
            context.run(
                egui::RawInput {
                    time: Some(time),
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(600.0, 400.0))),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        state.render_palette_panel(ui, ctx, &palette);
                    });
                },
            )
        };
        let initial = render(&mut state, 0.0, Vec::new());
        assert!(state.palette_state.extracted_colors.is_empty());
        assert!(!state.palette_state.is_extracted);
        let copy_center = rendered_copy_csv_button_center(&initial);
        render(
            &mut state,
            0.1,
            vec![
                egui::Event::PointerMoved(copy_center),
                egui::Event::PointerButton {
                    pos: copy_center,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::default(),
                },
            ],
        );
        let clicked = render(
            &mut state,
            0.2,
            vec![
                egui::Event::PointerMoved(copy_center),
                egui::Event::PointerButton {
                    pos: copy_center,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::default(),
                },
            ],
        );
        assert!(clicked.platform_output.copied_text.is_empty());
    }
}
