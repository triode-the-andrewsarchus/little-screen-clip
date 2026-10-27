//! Inline provenance leaves existing preview layouts and metadata columns intact.
use super::Search;
use crate::{
    file_manager::filesystem::{self, Entry},
    ui_appearance::WorkbenchColors,
};
use egui::{Rect, Response, Ui};

impl Search {
    pub(in crate::file_manager) fn result_height(&self, entry: &Entry) -> f32 {
        if self.sources.is_none() {
            return 0.0;
        }
        if self
            .explanations
            .get(&entry.path)
            .and_then(|detail| detail.passages.first())
            .is_some_and(|passage| !passage.text.is_empty())
        {
            44.0
        } else {
            24.0
        }
    }
    pub(in crate::file_manager) fn result_row(
        &self,
        ui: &mut Ui,
        rect: Rect,
        entry: &Entry,
        colors: WorkbenchColors,
    ) -> (Rect, Option<Response>) {
        let height = self.result_height(entry);
        if height == 0.0 {
            return (rect, None);
        }
        let mut body = rect;
        body.max.y -= height;
        let footer = Rect::from_min_max(egui::pos2(rect.left(), body.bottom()), rect.max);
        ui.painter().rect_filled(footer, 0.0, colors.recessed);
        let current = self.explanation_id == Some(self.request_id);
        let detail = current
            .then(|| self.explanations.get(&entry.path))
            .flatten();
        let passage = detail.and_then(|detail| detail.passages.first());
        let label = if !current {
            "Previous search result"
        } else {
            passage.map_or("Filename match", |passage| passage.kind.as_str())
        };
        let location = passage.map_or("", |passage| passage.location.as_str());
        let coverage = detail
            .filter(|detail| !detail.coverage.is_empty() && detail.coverage != "complete")
            .map_or("", |detail| detail.coverage.as_str());
        let parent =
            filesystem::display_path(entry.path.parent().unwrap_or(std::path::Path::new("/")));
        let availability = if self.is_offline(&entry.path) {
            "Unavailable · last-known"
        } else {
            ""
        };
        let heading = [availability, label, location, coverage, &parent]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(" · ");
        let painter = ui
            .painter()
            .with_clip_rect(ui.clip_rect().intersect(footer.shrink(3.0)));
        painter.text(
            footer.left_top() + egui::vec2(6.0, 3.0),
            egui::Align2::LEFT_TOP,
            heading,
            egui::FontId::proportional(12.0),
            colors.text_secondary,
        );
        if let Some(passage) = passage.filter(|passage| !passage.text.is_empty()) {
            painter.text(
                footer.left_top() + egui::vec2(6.0, 23.0),
                egui::Align2::LEFT_TOP,
                &passage.text,
                egui::FontId::proportional(13.0),
                colors.text_primary,
            );
        }
        let response = ui.interact(
            footer,
            egui::Id::new(("search_explanation", &entry.path)),
            egui::Sense::click_and_drag(),
        );
        self.result_hover(&response, entry);
        (body, Some(response))
    }
    pub(in crate::file_manager) fn result_hover(&self, response: &Response, entry: &Entry) {
        if self.sources.is_none() {
            return;
        }
        response.clone().on_hover_ui(|ui| {
            ui.set_max_width(600.0);
            ui.label(filesystem::display_path(&entry.path));
            if self.explanation_id != Some(self.request_id) {
                ui.label("Previous search result. Await the current search or refresh it.");
                return;
            }
            if self.is_offline(&entry.path) {
                ui.strong("Unavailable · last-known match, not freshly verified");
                ui.label("Reconnect and refresh before opening, copying or collecting this file. Preview reads are disabled.");
            }
            let Some(detail) = self
                .explanations
                .get(&entry.path)
                .filter(|detail| !detail.passages.is_empty())
            else {
                ui.label("Filename match; no content passage contributed.");
                return;
            };
            if !detail.coverage.is_empty() {
                ui.label(format!("Extraction coverage: {}", detail.coverage));
            }
            for passage in &detail.passages {
                ui.separator();
                ui.strong(format!("{} · {}", passage.kind, passage.location));
                if !passage.text.is_empty() {
                    ui.label(&passage.text);
                }
            }
        });
    }
}
