use super::Cpu;
use crate::ui_appearance::WorkbenchColors;
use egui::{Align2, Color32, FontId, Rect, Sense, Ui, pos2, vec2};
pub(in crate::file_manager::system) fn paint(
    ui: &mut Ui,
    rect: Rect,
    cpu: &Cpu,
    colors: WorkbenchColors,
    palette: &little_clip_core::palette::AppPalette,
) {
    let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    painter.rect_filled(rect, 3.0, colors.panel);
    if cpu.bins.is_empty() {
        painter.text(
            rect.center(),
            Align2::CENTER_CENTER,
            "CPU membership unavailable",
            FontId::proportional(12.0),
            colors.text_secondary,
        );
        return;
    }
    let columns = (rect.width() / 22.0).floor().max(1.0) as usize;
    let rows = cpu.bins.len().div_ceil(columns);
    for (index, bin) in cpu.bins.iter().enumerate() {
        let width = rect.width() / columns.min(cpu.bins.len()) as f32;
        let height = rect.height() / rows as f32;
        let cell = Rect::from_min_size(
            pos2(
                rect.left() + (index % columns) as f32 * width,
                rect.top() + (index / columns) as f32 * height,
            ),
            vec2(width, height),
        )
        .shrink(1.0);
        let online = bin.counts.first().copied().flatten();
        let focus = cpu
            .focus
            .and_then(|index| bin.counts.get(index).copied().flatten());
        let color = match (focus, online) {
            (Some(count), _) if count > 0 => colors
                .accent
                .gamma_multiply(0.25 + 0.75 * count as f32 / bin.total as f32),
            (_, Some(0)) => colors.text_secondary.gamma_multiply(0.25),
            (_, Some(count)) => Color32::from_rgb(67, 133, 158)
                .gamma_multiply(0.25 + 0.75 * count as f32 / bin.total as f32),
            _ => colors.text_secondary.gamma_multiply(0.5),
        };
        let fill = if cpu.retained {
            color.gamma_multiply(0.5)
        } else {
            color
        };
        painter.rect_filled(cell, 2.0, fill);
        let foreground = crate::ui_ext::contrast_text_c32_on(
            fill,
            colors.panel,
            Some(colors.text_primary),
            palette,
        );
        if bin.selected {
            painter.rect_stroke(cell, 2.0, egui::Stroke::new(2.0_f32, foreground));
        }
        if bin.total == 1 && cell.height() >= 13.0 && cell.width() > 16.0 {
            painter.text(
                cell.center(),
                Align2::CENTER_CENTER,
                bin.first.to_string(),
                FontId::monospace(10.0),
                foreground,
            );
        }
        let mut tooltip = format!(
            "CPUs {}–{} · {} logical IDs in this cell",
            bin.first, bin.last, bin.total
        );
        for (label, count) in cpu.memberships.iter().zip(&bin.counts) {
            tooltip.push_str(&format!(
                "\n{label}: {}",
                count.map_or_else(
                    || "unavailable".into(),
                    |count| format!("{count}/{}", bin.total)
                )
            ));
        }
        tooltip.push_str(&format!(
            "\nCoverage: {}{}",
            cpu.coverage,
            if cpu.retained {
                " · last observation"
            } else {
                ""
            }
        ));
        ui.interact(
            cell,
            ui.id().with(("cpu-membership", index)),
            Sense::hover(),
        )
        .on_hover_text(tooltip);
    }
}
pub(in crate::file_manager::system) fn inspector(
    ui: &mut Ui,
    cpu: &Cpu,
    colors: WorkbenchColors,
    palette: &little_clip_core::palette::AppPalette,
) {
    ui.heading("CPU membership");
    ui.label(&cpu.summary);
    if let Some(label) = cpu.focus.and_then(|index| cpu.memberships.get(index)) {
        ui.label(format!("Accent highlights: {label}"));
    }
    ui.label(format!(
        "Coverage: {}{}",
        cpu.coverage,
        if cpu.retained {
            " · last observation"
        } else {
            ""
        }
    ));
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 72.0), Sense::hover());
    paint(ui, rect, cpu, colors, palette);
    for (index, label) in cpu.memberships.iter().enumerate() {
        ui.label(format!(
            "{label}: {}",
            cpu.count(index)
                .map_or_else(|| "unavailable".into(), |count| count.to_string())
        ));
    }
}
