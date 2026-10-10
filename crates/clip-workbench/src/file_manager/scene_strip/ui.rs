use super::super::filesystem::Entry;
use super::{Previews, backend};
use crate::{icons::Icon, ui_appearance::WorkbenchColors};
use egui::{Align2, FontId, Rect, Ui, pos2, vec2};

impl Previews {
    pub fn show(&mut self, ui: &mut Ui, row: Rect, entry: &Entry, colors: WorkbenchColors) {
        self.request(entry);
        let ready = self.files.get(&entry.path);
        let format = entry
            .path
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or("Scene")
            .to_uppercase();
        let details = format!("{format} · {} · {}", entry.size, entry.modified);
        super::super::ui::media_header(
            ui,
            Rect::from_min_size(row.min + vec2(6.0, 0.0), vec2(row.width() - 12.0, 24.0)),
            entry,
            Some(details),
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
            ui.painter().rect_filled(rect, 3.0, colors.recessed);
            let texture = ready.and_then(|ready| ready.textures[view].as_ref());
            if let Some(texture) = texture {
                ui.painter().image(
                    texture.id(),
                    rect,
                    Rect::from_min_max(egui::Pos2::ZERO, pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
            } else {
                Icon::Model3d.paint(
                    ui.painter(),
                    Rect::from_center_size(rect.center(), vec2(32.0, 32.0)),
                    colors.text_secondary,
                );
            }
            if ui.is_enabled()
                && ui
                    .ctx()
                    .pointer_hover_pos()
                    .is_some_and(|point| rect.intersect(ui.clip_rect()).contains(point))
            {
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
                    .unwrap_or_else(|| "Loading Gaussian scene…".into())
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
        ui.interact(
            info,
            egui::Id::new(("scene-strip-metadata", &entry.path)),
            egui::Sense::hover(),
        )
        .on_hover_text(&detail);
        if ready.is_some_and(|ready| ready.metadata.is_some())
            && let Some((view, rect, texture)) = hovered
        {
            self.hover.show(ui, rect, entry, view, texture);
        }
        if ready.is_some_and(|ready| !ready.complete) {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(50));
        }
    }
}
fn metadata_text(metadata: &backend::Metadata) -> String {
    let [x, y, z] = metadata.center;
    format!(
        "{} Gaussians · {}\nRadius {:.3} scene units\nCenter {:.3}, {:.3}, {:.3}",
        metadata.points,
        if metadata.linear {
            "Linear RGB"
        } else {
            "sRGB"
        },
        metadata.radius,
        x,
        y,
        z
    )
}
