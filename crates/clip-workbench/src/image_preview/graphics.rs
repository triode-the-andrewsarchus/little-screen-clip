use super::*;

pub(super) fn image_texture_options(pixels: &RgbaImage) -> egui::TextureOptions {
    if pixels.width() < 500 && pixels.height() < 500 {
        egui::TextureOptions::NEAREST
    } else {
        egui::TextureOptions::LINEAR
    }
}

pub(super) fn image_studio_footer_hint(mode: ImageToolMode) -> &'static str {
    match mode {
        ImageToolMode::View => {
            "🖱 Scroll: Zoom | Drag: Pan | Ctrl+C: Copy Slot | Ctrl+S: Save As | 0: Fit"
        }
        ImageToolMode::Crop => {
            "🖱 Drag crop box/handles | Enter: Apply | Ctrl+C: Copy Slot | Ctrl+S: Save As | Esc: Cancel"
        }
        ImageToolMode::Deskew => {
            "🎯 Click 4 points (TL → TR → BR → BL) | 🖱 Drag handles | Enter: Apply | Esc: Close"
        }
        ImageToolMode::Transform => {
            "🖱 Rotate / Flip buttons in the left panel | Ctrl+Z: Undo | Ctrl+C: Copy Slot | Ctrl+S: Save As"
        }
        ImageToolMode::Redact => {
            "🖱 Drag on image: Place black bar | 🛡️ Strip EXIF | Del: Remove | Enter: Burn/Apply | Ctrl+C: Copy Slot"
        }
        ImageToolMode::Markup => {
            "🖱 Drag: Draw shape | 1-8: Tools | Del: Delete | Esc: Deselect | Enter: Flatten Layers | Ctrl+C: Copy Slot"
        }
        ImageToolMode::PickColor => {
            "🖱 Hover: Magnifying loupe | Click: Copy Hex code to clipboard | Ctrl+C: Copy Slot"
        }
        ImageToolMode::Convert => {
            "🖱 Drag vertical divider: Before/After comparison | Enter: Apply to Canvas | Ctrl+S: Save As"
        }
        ImageToolMode::Ocr => {
            "🖱 Choose engine and prompt on the left | Enter: Run OCR | Results appear on the right"
        }
        ImageToolMode::Barcode => {
            "🖱 Click code card or box to inspect | Enter: Rescan | 🌐 Open Link | 📋 Copy Payload"
        }
        ImageToolMode::CreativeFx => {
            "🖱 Split slider: Before/After comparison | Double-click: Fit | Sliders: Double-click to reset"
        }
        _ => {
            "🖱 Scroll: Zoom | Drag: Pan | Ctrl+C: Copy to Slot | Ctrl+S: Save As | 0: Fit | Esc: Close"
        }
    }
}

pub(super) fn chip_badge(ui: &mut egui::Ui, text: &str, text_col: Color32, bg_col: Color32) {
    let frame = egui::Frame::none()
        .fill(bg_col)
        .rounding(Rounding::same(crate::ui_ext::CONTROL_CORNER_RADIUS))
        .inner_margin(egui::Margin::symmetric(6.0, 2.0));
    frame.show(ui, |ui| {
        ui.label(
            egui::RichText::new(text)
                .size(crate::ui_ext::BODY_FONT_SIZE)
                .strong()
                .color(text_col),
        );
    });
}

pub(super) fn draw_checkerboard(
    painter: &egui::Painter,
    rect: Rect,
    _zoom: f32,
    dark_col: Color32,
    light_col: Color32,
) {
    let cell_size = 16.0;

    painter.rect_filled(rect, Rounding::ZERO, dark_col);

    let min_x = rect.min.x;
    let max_x = rect.max.x;
    let min_y = rect.min.y;
    let max_y = rect.max.y;

    let cols = ((max_x - min_x) / cell_size).ceil() as usize;
    let rows = ((max_y - min_y) / cell_size).ceil() as usize;

    for r in 0..rows {
        for c in 0..cols {
            if (r + c) % 2 == 1 {
                let x0 = (min_x + c as f32 * cell_size).min(max_x);
                let x1 = (min_x + (c + 1) as f32 * cell_size).min(max_x);
                let y0 = (min_y + r as f32 * cell_size).min(max_y);
                let y1 = (min_y + (r + 1) as f32 * cell_size).min(max_y);
                painter.rect_filled(
                    Rect::from_min_max(Pos2::new(x0, y0), Pos2::new(x1, y1)),
                    Rounding::ZERO,
                    light_col,
                );
            }
        }
    }
}

pub(super) fn format_aspect_ratio(w: u32, h: u32) -> String {
    if h == 0 {
        return "0:0".to_string();
    }
    let ratio = w as f32 / h as f32;
    if (ratio - 16.0 / 9.0).abs() < 0.03 {
        "16:9".to_string()
    } else if (ratio - 9.0 / 16.0).abs() < 0.03 {
        "9:16".to_string()
    } else if (ratio - 4.0 / 3.0).abs() < 0.03 {
        "4:3".to_string()
    } else if (ratio - 1.0).abs() < 0.02 {
        "1:1".to_string()
    } else {
        format!("{:.2}:1", ratio)
    }
}

pub(super) fn format_bytes_size(bytes: usize) -> String {
    if bytes < 1024 {
        format!("{} B", bytes)
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.2} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}
