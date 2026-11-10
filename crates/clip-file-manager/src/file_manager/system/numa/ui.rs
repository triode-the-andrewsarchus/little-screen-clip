use super::Numa;
use crate::ui_appearance::WorkbenchColors;
use egui::{Align2, FontId, Rect, Sense, Ui, pos2, vec2};
pub(in crate::file_manager::system) fn paint(
    ui: &mut Ui,
    rect: Rect,
    data: &Numa,
    colors: WorkbenchColors,
    palette: &little_clip_core::palette::AppPalette,
) {
    let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    painter.rect_filled(rect, 3.0, colors.panel);
    if data.columns.is_empty() || data.rows.is_empty() {
        painter.text(
            rect.center(),
            Align2::CENTER_CENTER,
            "NUMA distance mapping unavailable",
            FontId::proportional(12.0),
            colors.text_secondary,
        );
        return;
    }
    if rect.width() < 60.0 || rect.height() < 12.0 {
        return;
    }
    let width = (rect.width() - 42.0).max(1.0) / data.columns.len() as f32;
    let height = (rect.height() - 10.0).max(1.0) / data.rows.len() as f32;
    for (r, row) in data.rows.iter().enumerate() {
        let y = rect.top() + 10.0 + r as f32 * height;
        if height >= 10.0 {
            painter
                .with_clip_rect(
                    Rect::from_min_size(pos2(rect.left(), y), vec2(41.0, height)).intersect(rect),
                )
                .text(
                    pos2(rect.left() + 2.0, y + height / 2.0),
                    Align2::LEFT_CENTER,
                    format!("N{}", row.id),
                    FontId::monospace(10.0),
                    colors.text_primary,
                );
        }
        for (c, id) in data.columns.iter().enumerate() {
            let value = row.values.as_ref().and_then(|v| v.get(c));
            let cell = Rect::from_min_size(
                pos2(rect.left() + 42.0 + c as f32 * width, y),
                vec2(width, height),
            )
            .intersect(rect)
            .shrink(width.min(height).min(1.0) * 0.25);
            distance_cell(ui, cell, row, *id, value, colors, palette);
            let label = format!("N{id}");
            let label_width = ui.fonts(|fonts| {
                fonts
                    .layout_no_wrap(label.clone(), FontId::monospace(9.0), colors.text_secondary)
                    .size()
                    .x
            });
            if r == 0 && width >= label_width + 2.0 {
                painter.text(
                    pos2(cell.center().x, rect.top() + 5.0),
                    Align2::CENTER_CENTER,
                    label,
                    FontId::monospace(9.0),
                    colors.text_secondary,
                );
            }
        }
    }
}
pub(in crate::file_manager::system) fn inspector(
    ui: &mut Ui,
    data: &Numa,
    colors: WorkbenchColors,
    palette: &little_clip_core::palette::AppPalette,
) {
    ui.heading("NUMA distance matrix");
    ui.label(&data.summary);
    ui.label(format!(
        "Coverage: {} · rows are sources; columns follow observed online-node order",
        data.coverage
    ));
    let (rect, _) = ui.allocate_exact_size(
        vec2(
            ui.available_width(),
            (data.rows.len() as f32 * 22.0 + 12.0).clamp(52.0, 380.0),
        ),
        Sense::hover(),
    );
    paint(ui, rect, data, colors, palette);
    ui.label("Relative distances are not nanoseconds or measured bandwidth. Detailed CPU/memory facts and navigation links follow below.");
}

fn distance_cell(
    ui: &mut Ui,
    cell: Rect,
    row: &super::Row,
    column: u32,
    value: Option<&u16>,
    colors: WorkbenchColors,
    palette: &little_clip_core::palette::AppPalette,
) {
    let painter = ui.painter().with_clip_rect(cell.intersect(ui.clip_rect()));
    let color = value.map_or(colors.recessed, |v| {
        colors
            .accent
            .gamma_multiply((1.0 - f32::from(*v) / 256.0).max(0.12))
    });
    let fill = if row.retained {
        color.gamma_multiply(0.5)
    } else {
        color
    };
    painter.rect_filled(cell, 1.0, fill);
    let label = value.map_or_else(|| "?".into(), u16::to_string);
    let label_width = ui.fonts(|fonts| {
        fonts
            .layout_no_wrap(label.clone(), FontId::monospace(10.0), colors.text_primary)
            .size()
            .x
    });
    if cell.width() >= label_width + 2.0 && cell.height() >= 12.0 {
        painter.text(
            cell.center(),
            Align2::CENTER_CENTER,
            label,
            FontId::monospace(10.0),
            crate::ui_ext::contrast_text_c32_on(
                fill,
                colors.panel,
                Some(colors.text_primary),
                palette,
            ),
        );
    }
    if !crate::file_manager::ui::popup_open(ui.ctx()) {
        ui.interact(cell,ui.id().with(("numa-distance",row.id,column)),Sense::hover()).on_hover_text(format!("node{} → node{column}: {} · {}{}\nRelative distance, not measured latency · observed {} ms ago",row.id,value.map_or_else(||"unavailable".into(),u16::to_string),row.outcome,if row.retained {" · last observation"} else {""},super::super::model::now_ms().saturating_sub(row.observed_ms)));
    }
}
