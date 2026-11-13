//! Worker-prepared page fields alongside the existing source minimap.
use little_clip_core::{markdown_metadata, palette::AppPalette};
use serde_json::Value;

pub(in crate::file_manager) struct Summary {
    pub(super) fields: Vec<(String, String)>,
    format: &'static str,
    error: Option<String>,
}

impl Summary {
    pub(super) fn parse(source: &str) -> Option<Self> {
        let metadata = markdown_metadata::parse(source)?;
        let mut fields = Vec::new();
        if let Ok(Value::Object(values)) = &metadata.value {
            for name in [
                "title",
                "date",
                "draft",
                "tags",
                "categories",
                "publishDate",
                "lastmod",
                "weight",
            ] {
                if let Some((key, value)) = values
                    .iter()
                    .find(|(key, _)| key.eq_ignore_ascii_case(name))
                {
                    let mut text = match value {
                        Value::String(text) => text.clone(),
                        _ => value.to_string(),
                    };
                    if text.chars().count() > 160 {
                        text = text.chars().take(160).collect();
                        text.push('…');
                    }
                    fields.push((key.clone(), text));
                }
            }
            if fields.is_empty() {
                fields.extend(metadata.display_fields().into_iter().take(4));
            }
        }
        Some(Self {
            fields,
            format: metadata.format.name(),
            error: metadata
                .value
                .err()
                .map(|error| error.chars().take(1024).collect()),
        })
    }

    /// Paints within the row's original height and returns its remaining source area.
    pub(in crate::file_manager) fn show(
        &self,
        ui: &mut egui::Ui,
        rect: egui::Rect,
        id: egui::Id,
        palette: &AppPalette,
    ) -> egui::Rect {
        if rect.width() < 360.0
            || rect.height() < 36.0
            || self.fields.is_empty() && self.error.is_none()
        {
            return rect;
        }
        let width = (rect.width() * 0.36).clamp(160.0, 380.0);
        let panel = egui::Rect::from_min_size(rect.min, egui::vec2(width, rect.height()));
        let painter = ui.painter_at(panel.intersect(ui.clip_rect()));
        let color = crate::hex_to_c32(&palette.colors.text_primary);
        let rows = if let Some(error) = &self.error {
            vec![
                (
                    format!("{} metadata", self.format),
                    ui.visuals().error_fg_color,
                ),
                (error.clone(), color),
            ]
        } else if self.fields.is_empty() {
            vec![(format!("{} metadata", self.format), color)]
        } else {
            self.fields
                .iter()
                .map(|(key, value)| {
                    let text = if key.eq_ignore_ascii_case("title") {
                        value.clone()
                    } else {
                        format!("{key}: {value}")
                    };
                    (text, color)
                })
                .collect()
        };
        for (index, (text, color)) in rows.into_iter().enumerate() {
            let top = panel.top() + 3.0 + index as f32 * 16.0;
            if top + 13.0 > panel.bottom() {
                break;
            }
            let mut job = egui::text::LayoutJob::simple_singleline(
                text,
                egui::FontId::proportional(13.0),
                color,
            );
            job.wrap.max_width = panel.width() - 12.0;
            job.wrap.max_rows = 1;
            let galley = painter.layout_job(job);
            painter.galley(egui::pos2(panel.left() + 5.0, top), galley, color);
        }
        painter.line_segment(
            [panel.right_top(), panel.right_bottom()],
            ui.visuals().widgets.noninteractive.bg_stroke,
        );
        ui.interact(panel, id, egui::Sense::hover())
            .on_hover_ui(|ui| {
                ui.label(format!("{} front matter", self.format));
                if let Some(error) = &self.error {
                    ui.colored_label(ui.visuals().error_fg_color, error);
                }
                for (key, value) in &self.fields {
                    ui.label(format!("{key}: {value}"));
                }
            });
        egui::Rect::from_min_max(panel.right_top() + egui::vec2(8.0, 0.0), rect.max)
    }
}
