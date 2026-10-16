use super::{Point, Series, Telemetry};
use crate::ui_appearance::WorkbenchColors;
use egui::{Align2, FontId, Rect, Sense, Ui, pos2, vec2};
use little_clip_core::palette::AppPalette;
fn tooltip(row: &Series) -> String {
    let mut text = row.label.clone();
    for point in std::iter::once(&row.current).chain(&row.markers) {
        text.push_str(&format!(
            "\n{}: {} · {}{}\n{} · observed {} ms ago",
            point.label,
            point.display,
            point.outcome,
            if point.retained {
                " (last observation)"
            } else {
                ""
            },
            point.source.display(),
            super::super::model::now_ms().saturating_sub(point.observed_ms)
        ));
    }
    for note in &row.notes {
        text.push_str(&format!("\n{note}"));
    }
    text
}
fn position(point: &Point, range: (f64, f64), rect: Rect) -> Option<f32> {
    point.value.filter(|value| value.is_finite()).map(|value| {
        rect.left()
            + (((value - range.0) / (range.1 - range.0)).clamp(0.0, 1.0) as f32) * rect.width()
    })
}
fn cell(
    ui: &mut Ui,
    rect: Rect,
    row: &Series,
    index: usize,
    colors: WorkbenchColors,
    palette: &AppPalette,
) {
    let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    painter.rect_filled(rect, 3.0, colors.panel);
    painter.text(
        pos2(rect.left() + 5.0, rect.top() + 7.0),
        Align2::LEFT_CENTER,
        &row.label,
        FontId::proportional(11.0),
        colors.text_secondary,
    );
    let display = if row.current.label == "Full/design estimate" {
        format!(
            "{}{}",
            row.current.display,
            if row.current.retained {
                " · last observation"
            } else {
                ""
            }
        )
    } else {
        format!(
            "{}: {}{}",
            row.current.label,
            row.current.display,
            if row.current.retained {
                " · last observation"
            } else {
                ""
            }
        )
    };
    painter.text(
        pos2(rect.left() + 5.0, rect.top() + 22.0),
        Align2::LEFT_CENTER,
        display,
        FontId::monospace(12.0),
        colors.text_primary,
    );
    if let Some(range) = row
        .range
        .filter(|(min, max)| min.is_finite() && max.is_finite() && min < max)
    {
        let bar = Rect::from_min_max(
            pos2(rect.left() + 5.0, rect.bottom() - 12.0),
            pos2(rect.right() - 5.0, rect.bottom() - 5.0),
        );
        painter.rect_filled(bar, 1.0, colors.recessed);
        if let Some(x) = position(&row.current, range, bar) {
            let color = if row.alarm {
                colors.danger
            } else {
                colors.accent
            };
            let fill = if row.current.retained {
                color.gamma_multiply(0.5)
            } else {
                color
            };
            painter.rect_filled(
                Rect::from_min_max(bar.min, pos2(x, bar.bottom())),
                1.0,
                fill,
            );
            painter.line_segment(
                [pos2(x, bar.top() - 3.0), pos2(x, bar.bottom() + 2.0)],
                egui::Stroke::new(
                    2.0_f32,
                    crate::ui_ext::contrast_text_c32_on(
                        fill,
                        colors.panel,
                        Some(colors.text_primary),
                        palette,
                    ),
                ),
            );
        }
        for marker in row
            .markers
            .iter()
            .filter(|marker| marker.unit == row.current.unit)
        {
            if let Some(x) = position(marker, range, bar) {
                painter.line_segment(
                    [pos2(x, bar.top() - 2.0), pos2(x, bar.bottom() + 1.0)],
                    egui::Stroke::new(1.0_f32, colors.text_secondary),
                );
            }
        }
    }
    if row.current.value.is_none() && !row.markers.is_empty() {
        painter.text(
            pos2(rect.left() + 5.0, rect.bottom() - 9.0),
            Align2::LEFT_CENTER,
            row.markers
                .iter()
                .map(|point| {
                    format!(
                        "{}: {}{}",
                        point.label,
                        point.display,
                        if point.retained {
                            " (last observation)"
                        } else {
                            ""
                        }
                    )
                })
                .collect::<Vec<_>>()
                .join(" · "),
            FontId::proportional(11.0),
            colors.text_secondary,
        );
    }
    ui.interact(
        rect,
        ui.id()
            .with(("telemetry-series", index, &row.current.source)),
        Sense::hover(),
    )
    .on_hover_text(tooltip(row));
}
pub(in crate::file_manager::system) fn paint(
    ui: &mut Ui,
    rect: Rect,
    data: &Telemetry,
    colors: WorkbenchColors,
    palette: &AppPalette,
) {
    let count = data.series.len().min(4);
    if count == 0 {
        ui.painter()
            .with_clip_rect(rect.intersect(ui.clip_rect()))
            .text(
                rect.center(),
                Align2::CENTER_CENTER,
                "No measurement available",
                FontId::proportional(12.0),
                colors.text_secondary,
            );
        return;
    }
    let width = rect.width() / count as f32;
    for (index, row) in data.series.iter().take(count).enumerate() {
        cell(
            ui,
            Rect::from_min_size(
                pos2(rect.left() + index as f32 * width, rect.top()),
                vec2(width - 4.0, rect.height()),
            ),
            row,
            index,
            colors,
            palette,
        );
    }
}
pub(in crate::file_manager::system) fn inspector(
    ui: &mut Ui,
    data: &Telemetry,
    colors: WorkbenchColors,
    palette: &AppPalette,
) {
    ui.heading("Measurements and limits");
    ui.label(&data.summary);
    ui.label(format!(
        "Coverage: {} · {} represented series",
        data.coverage,
        data.series.len()
    ));
    for (index, row) in data.series.iter().take(24).enumerate() {
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 52.0), Sense::hover());
        cell(ui, rect, row, index, colors, palette);
        for marker in &row.markers {
            ui.label(format!(
                "{}: {}{}",
                marker.label,
                marker.display,
                if marker.retained {
                    " (last observation)"
                } else {
                    ""
                }
            ));
        }
        if let Some((min, max)) = row.range {
            ui.label(format!(
                "Display span: {min:.2}–{max:.2} {}",
                row.current.unit
            ));
        }
        for note in &row.notes {
            ui.label(note);
        }
        ui.add_space(4.0);
    }
    if data.series.len() > 24 {
        ui.label("Additional series remain in the collected facts below");
    }
}
