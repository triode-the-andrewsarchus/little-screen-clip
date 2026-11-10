use super::*;

pub(super) fn render_hex_table_header(
    ui: &mut egui::Ui,
    palette: &AppPalette,
    res: &BinaryDiffResult,
    columns: HexHeaderColumns,
    font_size: f32,
    scale: f32,
) {
    // Table column headers
    let (header_rect, _) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 28.0), egui::Sense::hover());
    ui.painter().rect_filled(
        header_rect,
        Rounding::ZERO,
        hex_to_c32(&palette.colors.surface_recessed),
    );
    let content_width = 4.0 * scale
        + columns.offset * 2.0
        + columns.hex * 2.0
        + columns.ascii * 2.0
        + columns.diff
        + 6.0 * 2.0 * scale
        + 2.0 * crate::ui_ext::SPACE_SM * scale;
    let header_ui_rect = egui::Rect::from_min_size(
        egui::pos2(header_rect.min.x, header_rect.min.y),
        Vec2::new(content_width, header_rect.height()),
    );
    let mut child_ui = ui.child_ui(
        header_ui_rect,
        egui::Layout::left_to_right(egui::Align::Center),
    );
    child_ui.set_clip_rect(header_rect);
    child_ui.spacing_mut().item_spacing = Vec2::new(2.0 * scale, 0.0);
    child_ui.add_space(4.0 * scale);

    let diff_header_color = crate::ui_ext::contrast_text_c32_on(
        hex_to_c32(&palette.colors.surface_recessed),
        hex_to_c32(&palette.colors.surface_recessed),
        Some(hex_to_c32(&palette.diff_mod)),
        palette,
    );
    let diff_header_hex = format!(
        "#{:02X}{:02X}{:02X}",
        diff_header_color.r(),
        diff_header_color.g(),
        diff_header_color.b()
    );
    let headers = [
        (
            "Offset L".to_owned(),
            columns.offset,
            &palette.colors.text_muted,
            false,
        ),
        (
            format!("Hex Bytes ({})", res.left_label),
            columns.hex,
            &palette.colors.text_secondary,
            false,
        ),
        (
            "ASCII".to_owned(),
            columns.ascii,
            &palette.colors.text_secondary,
            true,
        ),
        ("Diff".to_owned(), columns.diff, &diff_header_hex, false),
        (
            format!("Hex Bytes ({})", res.right_label),
            columns.hex,
            &palette.colors.text_secondary,
            false,
        ),
        (
            "ASCII".to_owned(),
            columns.ascii,
            &palette.colors.text_secondary,
            true,
        ),
        (
            "Offset R".to_owned(),
            columns.offset,
            &palette.colors.text_muted,
            false,
        ),
    ];
    for (label, width, color, gap_before) in headers {
        if gap_before {
            child_ui.add_space(crate::ui_ext::SPACE_SM * scale);
        }
        child_ui.allocate_ui_with_layout(
            Vec2::new(width, 28.0),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.label(
                    egui::RichText::new(label)
                        .font(FontId::monospace(font_size))
                        .strong()
                        .color(hex_to_c32(color)),
                );
            },
        );
    }
}

pub(super) fn format_cell_hex(
    cell: &AlignedCell,
    is_left: bool,
    color_mode: ByteColorMode,
    palette: &AppPalette,
) -> (String, Color32, Option<Color32>) {
    let surface = hex_to_c32(&palette.colors.canvas);
    let readable =
        |preferred| crate::ui_ext::contrast_text_c32_on(surface, surface, Some(preferred), palette);
    match (cell.byte_val, cell.kind) {
        (Some(b), DiffKind::Match) => match color_mode {
            ByteColorMode::Digits => (format!("{:02X}", b), readable(byte_subtle_color(b)), None),
            ByteColorMode::Background => {
                let bg = byte_subtle_bg_opaque(b);
                (
                    format!("{:02X}", b),
                    crate::contrast_text_c32(bg, None, palette),
                    Some(bg),
                )
            }
        },
        (Some(b), DiffKind::Mismatch) => match color_mode {
            ByteColorMode::Digits => (
                format!("{:02X}", b),
                readable(hex_to_c32(&palette.diff_mod)),
                None,
            ),
            ByteColorMode::Background => {
                let bg = hex_to_c32(&palette.diff_mod);
                (
                    format!("{:02X}", b),
                    crate::contrast_text_c32(bg, None, palette),
                    Some(bg),
                )
            }
        },
        (Some(b), DiffKind::LeftOnly) => {
            if is_left {
                match color_mode {
                    ByteColorMode::Digits => (
                        format!("{:02X}", b),
                        readable(hex_to_c32(&palette.diff_del)),
                        None,
                    ),
                    ByteColorMode::Background => {
                        let bg = hex_to_c32(&palette.diff_del);
                        (
                            format!("{:02X}", b),
                            crate::contrast_text_c32(bg, None, palette),
                            Some(bg),
                        )
                    }
                }
            } else {
                (
                    "--".to_string(),
                    hex_to_c32(&palette.colors.text_muted),
                    None,
                )
            }
        }
        (Some(b), DiffKind::RightOnly) => {
            if !is_left {
                match color_mode {
                    ByteColorMode::Digits => (
                        format!("{:02X}", b),
                        readable(hex_to_c32(&palette.diff_add)),
                        None,
                    ),
                    ByteColorMode::Background => {
                        let bg = hex_to_c32(&palette.diff_add);
                        (
                            format!("{:02X}", b),
                            crate::contrast_text_c32(bg, None, palette),
                            Some(bg),
                        )
                    }
                }
            } else {
                (
                    "--".to_string(),
                    hex_to_c32(&palette.colors.text_muted),
                    None,
                )
            }
        }
        (None, _) => (
            "--".to_string(),
            hex_to_c32(&palette.colors.text_muted),
            None,
        ),
    }
}

pub(super) fn format_cell_ascii(
    cell: &AlignedCell,
    is_left: bool,
    color_mode: ByteColorMode,
    palette: &AppPalette,
) -> (char, Color32) {
    let surface = hex_to_c32(&palette.colors.canvas);
    let readable =
        |preferred| crate::ui_ext::contrast_text_c32_on(surface, surface, Some(preferred), palette);
    match (cell.byte_val, cell.kind) {
        (Some(b), DiffKind::Match) => {
            let col = match color_mode {
                ByteColorMode::Digits => readable(byte_subtle_color(b)),
                ByteColorMode::Background => hex_to_c32(&palette.colors.text_secondary),
            };
            (byte_to_ascii_char(b), col)
        }
        (Some(b), DiffKind::Mismatch) => (
            byte_to_ascii_char(b),
            readable(hex_to_c32(&palette.diff_mod)),
        ),
        (Some(b), DiffKind::LeftOnly) => {
            if is_left {
                (
                    byte_to_ascii_char(b),
                    readable(hex_to_c32(&palette.diff_del)),
                )
            } else {
                ('·', hex_to_c32(&palette.colors.text_muted))
            }
        }
        (Some(b), DiffKind::RightOnly) => {
            if !is_left {
                (
                    byte_to_ascii_char(b),
                    readable(hex_to_c32(&palette.diff_add)),
                )
            } else {
                ('·', hex_to_c32(&palette.colors.text_muted))
            }
        }
        (None, _) => ('·', hex_to_c32(&palette.colors.text_muted)),
    }
}

pub(super) fn format_bytes_size(len: usize) -> String {
    if len < 1024 {
        format!("{} B", len)
    } else if len < 1024 * 1024 {
        format!("{:.1} KB", len as f64 / 1024.0)
    } else {
        format!("{:.2} MB", len as f64 / (1024.0 * 1024.0))
    }
}
