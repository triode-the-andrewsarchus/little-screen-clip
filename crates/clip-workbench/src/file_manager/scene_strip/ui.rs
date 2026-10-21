use super::super::filesystem::Entry;
use super::{Previews, backend};
use crate::ui_appearance::WorkbenchColors;
use egui::{Align2, FontId, Rect, Ui, pos2, vec2};

impl Previews {
    pub fn show(
        &mut self,
        ui: &mut Ui,
        row: Rect,
        entry: &Entry,
        columns: Option<super::super::ui::ListColumns>,
        colors: WorkbenchColors,
        palette: &little_clip_core::palette::AppPalette,
    ) {
        self.request(entry, palette);
        let ready = self.files.get(&entry.path);
        let format = entry
            .path
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or("Scene")
            .to_uppercase();
        super::super::ui::media_header(
            ui,
            Rect::from_min_size(row.min + vec2(6.0, 0.0), vec2(row.width() - 12.0, 24.0)),
            entry,
            Some(format),
            columns,
            colors,
        );
        let body = Rect::from_min_max(row.min + vec2(6.0, 32.0), row.max - vec2(6.0, 10.0));
        let width = body.height() * 16.0 / 9.0;
        let views = ((body.width() - 190.0) / (width + 8.0))
            .floor()
            .clamp(1.0, 4.0) as usize;
        let mut hovered = None;
        for view in 0..views {
            let rect = Rect::from_min_size(
                body.min + vec2(view as f32 * (width + 8.0), 0.0),
                vec2(width, body.height()),
            );
            let texture = ready.and_then(|ready| ready.textures[view].as_ref());
            super::super::ui::model_thumbnail(ui, rect, texture, colors);
            if super::super::ui::preview_hovered(ui, rect) {
                hovered = Some((view, rect, texture));
            }
        }
        let info = Rect::from_min_max(
            pos2(body.left() + views as f32 * (width + 8.0) + 4.0, body.top()),
            body.max,
        );
        let detail = ready
            .and_then(|ready| ready.metadata.as_ref())
            .map(metadata_text)
            .unwrap_or_else(|| {
                ready
                    .and_then(|ready| ready.error.clone())
                    .unwrap_or_else(|| "Loading 3D preview…".into())
            });
        ui.painter()
            .with_clip_rect(info.intersect(ui.clip_rect()))
            .text(
                info.left_top(),
                Align2::LEFT_TOP,
                &detail,
                FontId::proportional(13.0),
                colors.text_secondary,
            );
        if !super::super::ui::popup_open(ui.ctx()) {
            ui.interact(
                info,
                egui::Id::new(("scene-strip-metadata", &entry.path)),
                egui::Sense::hover(),
            )
            .on_hover_text(&detail);
        }
        if ready.is_some_and(|ready| ready.metadata.is_some())
            && let Some((view, rect, texture)) = hovered
        {
            self.hover.show(ui, rect, entry, view, texture, palette);
        }
        if ready.is_some_and(|ready| !ready.complete) {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(50));
        }
    }
}
fn metadata_text(metadata: &backend::Metadata) -> String {
    match metadata {
        backend::Metadata::Gaussian {
            points,
            center: [x, y, z],
            radius,
            linear,
        } => format!(
            "{points} Gaussians · {}\nRadius {radius:.3} scene units\nCenter {x:.3}, {y:.3}, {z:.3}",
            if *linear { "Linear RGB" } else { "sRGB" }
        ),
        backend::Metadata::Mesh {
            triangles,
            preview_triangles,
            dimensions: [x, y, z],
            units,
            colored,
            z_up,
        } => format!(
            "{triangles} triangles · {}\nSize {x:.3} × {y:.3} × {z:.3} {units}\nPreview {preview_triangles} triangles · {}",
            if *colored {
                "Material colors"
            } else {
                "Shaded surface"
            },
            if *z_up { "Z up" } else { "Y up" }
        ),
    }
}
