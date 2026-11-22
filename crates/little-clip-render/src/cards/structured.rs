use super::*;

impl ScreenRenderer {
    #[expect(
        clippy::too_many_arguments,
        reason = "Drawing keeps target, geometry, content and palette explicit at the rendering boundary"
    )]
    pub(super) fn draw_unit_conversion_card(
        &self,
        pixmap: &mut Pixmap,
        conv: &units::UnitConversion,
        rx: f32,
        ry: f32,
        rw: f32,
        _rh: f32,
        is_hero: bool,
    ) {
        let content_x = rx + 14.0;
        let clip_right = rx + rw - 10.0;
        let mut y = ry + 124.0;

        let bg_col = self.color_from_hex(&self.palette.colors.canvas);
        let f_high = self.contrast_color(
            bg_col,
            Some(self.color_from_hex(&self.palette.colors.text_primary)),
        );
        let f_low = self.contrast_color(
            bg_col,
            Some(self.color_from_hex(&self.palette.colors.text_muted)),
        );
        let base_c = self.contrast_color(
            bg_col,
            Some(self.color_from_hex(&self.palette.harmonies.base)),
        );
        let diff_add =
            self.contrast_color(bg_col, Some(self.color_from_hex(&self.palette.diff_add)));
        let diff_mod =
            self.contrast_color(bg_col, Some(self.color_from_hex(&self.palette.diff_mod)));

        self.draw_pill_badge(
            pixmap,
            "UNIT CONVERSION",
            content_x,
            y,
            14.0,
            diff_add,
            Self::hex_to_color_alpha(&self.palette.colors.surface_recessed, 230),
            clip_right,
        );
        y += 28.0;

        self.draw_text_clipped(
            pixmap,
            &format!("Input: {}", conv.original),
            content_x,
            y,
            if is_hero { 18.0 } else { 15.0 },
            f_low,
            false,
            clip_right,
        );
        y += 24.0;

        self.draw_text_clipped(
            pixmap,
            &conv.primary_conversion,
            content_x,
            y,
            if is_hero { 24.0 } else { 20.0 },
            f_high,
            true,
            clip_right,
        );
        y += if is_hero { 32.0 } else { 28.0 };

        if let Some(ref sec) = conv.secondary_conversion {
            self.draw_text_clipped(
                pixmap,
                &format!("≈ {}", sec),
                content_x,
                y,
                if is_hero { 18.0 } else { 15.0 },
                base_c,
                false,
                clip_right,
            );
            y += 26.0;
        }

        if let Some(ref rot) = conv.rule_of_thumb {
            self.draw_text_clipped(
                pixmap,
                &format!("* {}", rot),
                content_x,
                y,
                if is_hero { 16.0 } else { 13.0 },
                diff_mod,
                false,
                clip_right,
            );
        }
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "Drawing keeps target, geometry, content and palette explicit at the rendering boundary"
    )]
    pub(super) fn draw_network_preview_card(
        &self,
        pixmap: &mut Pixmap,
        net: &network_preview::NetworkPreview,
        rx: f32,
        ry: f32,
        rw: f32,
        _rh: f32,
        is_hero: bool,
    ) {
        let content_x = rx + 14.0;
        let clip_right = rx + rw - 10.0;
        let mut y = ry + 124.0;

        let bg_col = self.color_from_hex(&self.palette.colors.canvas);
        let f_high = self.contrast_color(
            bg_col,
            Some(self.color_from_hex(&self.palette.colors.text_primary)),
        );
        let f_low = self.contrast_color(
            bg_col,
            Some(self.color_from_hex(&self.palette.colors.text_muted)),
        );
        let base_c = self.contrast_color(
            bg_col,
            Some(self.color_from_hex(&self.palette.harmonies.base)),
        );
        let triadic_1 = self.contrast_color(
            bg_col,
            Some(self.color_from_hex(&self.palette.harmonies.triadic_1)),
        );
        let diff_add =
            self.contrast_color(bg_col, Some(self.color_from_hex(&self.palette.diff_add)));
        let diff_del =
            self.contrast_color(bg_col, Some(self.color_from_hex(&self.palette.diff_del)));

        match net {
            network_preview::NetworkPreview::Ip {
                ip,
                rfc_class,
                description,
                ping_status,
            } => {
                self.draw_pill_badge(
                    pixmap,
                    "IP ADDRESS",
                    content_x,
                    y,
                    14.0,
                    base_c,
                    Self::hex_to_color_alpha(&self.palette.colors.surface_recessed, 230),
                    clip_right,
                );
                y += 28.0;

                self.draw_text_clipped(
                    pixmap,
                    &ip.to_string(),
                    content_x,
                    y,
                    if is_hero { 24.0 } else { 20.0 },
                    f_high,
                    true,
                    clip_right,
                );
                y += if is_hero { 32.0 } else { 28.0 };

                self.draw_text_clipped(
                    pixmap,
                    rfc_class,
                    content_x,
                    y,
                    if is_hero { 17.0 } else { 14.0 },
                    triadic_1,
                    true,
                    clip_right,
                );
                y += 24.0;

                if let Some(desc) = description {
                    self.draw_text_clipped(
                        pixmap,
                        desc,
                        content_x,
                        y,
                        if is_hero { 15.0 } else { 13.0 },
                        f_low,
                        false,
                        clip_right,
                    );
                    y += 24.0;
                }

                if let Some(ping) = ping_status {
                    let is_online = ping.starts_with("Online");
                    let ping_col = if is_online { diff_add } else { diff_del };
                    self.draw_text_clipped(
                        pixmap,
                        &format!("Ping: {}", ping),
                        content_x,
                        y,
                        if is_hero { 16.0 } else { 14.0 },
                        ping_col,
                        true,
                        clip_right,
                    );
                }
            }
            network_preview::NetworkPreview::Mac {
                formatted,
                vendor,
                mac_type,
            } => {
                self.draw_pill_badge(
                    pixmap,
                    "MAC ADDRESS",
                    content_x,
                    y,
                    14.0,
                    triadic_1,
                    Self::hex_to_color_alpha(&self.palette.colors.surface_recessed, 230),
                    clip_right,
                );
                y += 28.0;

                self.draw_text_clipped(
                    pixmap,
                    formatted,
                    content_x,
                    y,
                    if is_hero { 22.0 } else { 18.0 },
                    f_high,
                    true,
                    clip_right,
                );
                y += if is_hero { 32.0 } else { 28.0 };

                self.draw_text_clipped(
                    pixmap,
                    &format!("Vendor: {}", vendor),
                    content_x,
                    y,
                    if is_hero { 17.0 } else { 14.0 },
                    base_c,
                    true,
                    clip_right,
                );
                y += 24.0;

                self.draw_text_clipped(
                    pixmap,
                    mac_type,
                    content_x,
                    y,
                    if is_hero { 15.0 } else { 13.0 },
                    f_low,
                    false,
                    clip_right,
                );
            }
        }
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "Drawing keeps target, geometry, content and palette explicit at the rendering boundary"
    )]
    pub(super) fn draw_timestamp_card(
        &self,
        pixmap: &mut Pixmap,
        ts: &timestamps::TimestampPreview,
        rx: f32,
        ry: f32,
        rw: f32,
        _rh: f32,
        is_hero: bool,
    ) {
        let content_x = rx + 14.0;
        let clip_right = rx + rw - 10.0;
        let mut y = ry + 124.0;

        let bg_col = self.color_from_hex(&self.palette.colors.canvas);
        let f_high = self.contrast_color(
            bg_col,
            Some(self.color_from_hex(&self.palette.colors.text_primary)),
        );
        let f_med = self.contrast_color(
            bg_col,
            Some(self.color_from_hex(&self.palette.colors.text_secondary)),
        );
        let f_low = self.contrast_color(
            bg_col,
            Some(self.color_from_hex(&self.palette.colors.text_muted)),
        );
        let diff_add =
            self.contrast_color(bg_col, Some(self.color_from_hex(&self.palette.diff_add)));
        let diff_mod =
            self.contrast_color(bg_col, Some(self.color_from_hex(&self.palette.diff_mod)));

        self.draw_pill_badge(
            pixmap,
            "TIMESTAMP",
            content_x,
            y,
            14.0,
            diff_mod,
            Self::hex_to_color_alpha(&self.palette.colors.surface_recessed, 230),
            clip_right,
        );
        y += 28.0;

        self.draw_text_clipped(
            pixmap,
            &ts.human_readable,
            content_x,
            y,
            if is_hero { 20.0 } else { 16.0 },
            f_high,
            true,
            clip_right,
        );
        y += if is_hero { 28.0 } else { 24.0 };

        self.draw_text_clipped(
            pixmap,
            &format!("Relative: {}", ts.relative),
            content_x,
            y,
            if is_hero { 17.0 } else { 14.0 },
            diff_add,
            true,
            clip_right,
        );
        y += 24.0;

        self.draw_text_clipped(
            pixmap,
            &format!("UTC: {}", ts.utc_iso),
            content_x,
            y,
            if is_hero { 15.0 } else { 13.0 },
            f_med,
            false,
            clip_right,
        );
        y += 24.0;

        self.draw_text_clipped(
            pixmap,
            &format!("Epoch: {} ({})", ts.epoch_sec, ts.day_info),
            content_x,
            y,
            if is_hero { 15.0 } else { 13.0 },
            f_low,
            false,
            clip_right,
        );
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "Drawing keeps target, geometry, content and palette explicit at the rendering boundary"
    )]
    pub(super) fn draw_spreadsheet_card(
        &self,
        pixmap: &mut Pixmap,
        _title: &str,
        sheet: &spreadsheet::SpreadsheetSheet,
        rx: f32,
        ry: f32,
        rw: f32,
        rh: f32,
        is_hero: bool,
    ) {
        let content_x = rx + 14.0;
        let clip_right = rx + rw - 10.0;
        let mut y = ry + 124.0;

        let badge_bg = self.color_from_hex(&self.palette.diff_add);
        let badge_fg = self.contrast_color(
            badge_bg,
            Some(self.color_from_hex(&self.palette.colors.text_primary)),
        );
        self.draw_pill_badge(
            pixmap,
            "SPREADSHEET",
            content_x,
            y,
            14.0,
            badge_fg,
            badge_bg,
            clip_right,
        );
        y += 28.0;

        let sheet_info = format!(
            "{} ({}r × {}c)",
            sheet.name, sheet.total_rows, sheet.total_cols
        );
        self.draw_text_clipped(
            pixmap,
            &sheet_info,
            content_x,
            y,
            if is_hero { 16.0 } else { 13.0 },
            self.color_from_hex(&self.palette.colors.text_secondary),
            false,
            clip_right,
        );
        y += 22.0;

        let available_width = clip_right - content_x;
        let row_height = if is_hero { 23.0 } else { 20.0 };
        let font_size = if is_hero { 14.0 } else { 12.0 };
        let rows = (((ry + rh - 28.0 - y) / row_height) as usize)
            .min(if is_hero { 9 } else { 7 })
            .min(sheet.total_rows);
        let columns = ((available_width / 78.0).floor().max(1.0) as usize)
            .min(if is_hero { 10 } else { 5 })
            .min(sheet.total_cols);
        if rows == 0 || columns == 0 || available_width <= 0.0 {
            return;
        }

        let column_width = available_width / columns as f32;
        let row_indices: Vec<_> = (0..rows).collect();
        let panel = self.color_from_hex(&self.palette.colors.surface_panel);
        let canvas = self.color_from_hex(&self.palette.colors.canvas);
        let recessed = self.color_from_hex(&self.palette.colors.surface_recessed);
        let foreground = self.color_from_hex(&self.palette.colors.text_primary);
        let accent = self.color_from_hex(&self.palette.harmonies.base);
        let mut border = Paint::default();
        border.set_color(self.color_from_hex(&self.palette.chrome_border));
        let stroke = Stroke {
            width: 0.7,
            ..Default::default()
        };
        for cell in sheet.grid_cells(&row_indices, 0..rows, 0..columns) {
            let (row, column) = cell.anchor;
            let width = (cell.columns.end.min(columns) - column) as f32 * column_width;
            let height = cell.rows.len() as f32 * row_height;
            let left = content_x + column as f32 * column_width;
            let top = y + cell.rows.start as f32 * row_height;
            let is_header = row == 0;
            let fill = if is_header {
                panel
            } else if row % 2 == 1 {
                recessed
            } else {
                canvas
            };
            if let Some(rect) = Rect::from_xywh(left, top, width, height) {
                let mut paint = Paint::default();
                paint.set_color(fill);
                pixmap.fill_rect(rect, &paint, Transform::identity(), None);
                pixmap.stroke_path(
                    &PathBuilder::from_rect(rect),
                    &border,
                    &stroke,
                    Transform::identity(),
                    None,
                );
            }

            let value = sheet.cell(row, column).lines().next().unwrap_or_default();
            let text_color =
                self.contrast_color(fill, Some(if is_header { accent } else { foreground }));
            self.draw_text_clipped(
                pixmap,
                value,
                left + 4.0,
                top + (height - font_size) / 2.0 - 1.0,
                font_size,
                text_color,
                is_header,
                left + width - 3.0,
            );
        }

        let footer_y = ry + rh - 22.0;
        self.draw_text_clipped(
            pixmap,
            "> Open in Workbench for full table",
            content_x,
            footer_y,
            12.0,
            self.color_from_hex(&self.palette.colors.text_muted),
            false,
            clip_right,
        );
    }
}
