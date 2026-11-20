mod inference;
mod jobs;
mod preprocess;
#[cfg(test)]
mod tests;

use super::super::*;
use little_clip_core::jobs::task::Task;
use std::sync::Mutex;
use std::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DeskewMethod {
    #[default]
    Manual,
    UvDoc,
    SyntheticDoc,
}

impl DeskewMethod {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Manual => "Manual · 4 points",
            Self::UvDoc => "UVDoc",
            Self::SyntheticDoc => "SyntheticDoc",
        }
    }

    fn input_size(self) -> (usize, usize) {
        match self {
            Self::UvDoc => (488, 712),
            _ => (512, 720),
        }
    }
}

type RunControl = Arc<Mutex<Option<Arc<ort::session::RunOptions>>>>;

#[derive(Clone, PartialEq)]
struct Stamp {
    image: ImageInputStamp,
    method: DeskewMethod,
    illumination: bool,
}

struct Preview {
    original: Arc<RgbaImage>,
    result: Arc<RgbaImage>,
    stamp: Stamp,
    duration: std::time::Duration,
}

#[derive(Default)]
pub struct DocumentUnwarpState {
    pub(super) illumination: bool,
    task: Option<Task<Result<Preview, String>>>,
    control: RunControl,
    request: Option<(DisplayImageSnapshotRequest, Stamp)>,
    retry_at: Option<Instant>,
    preview: Option<Preview>,
    texture: Option<egui::TextureHandle>,
    pub(super) show_original: bool,
    error: Option<String>,
}

impl DocumentUnwarpState {
    pub(crate) fn preview_ready(&self) -> bool {
        self.preview.is_some()
    }
    pub(crate) fn pending(&self) -> bool {
        self.task.is_some() || self.retry_at.is_some()
    }

    fn cancel(&mut self) {
        self.task = None;
        if let Ok(options) = self.control.lock()
            && let Some(options) = options.as_ref()
        {
            let _ = options.terminate();
        }
        self.request = None;
        self.retry_at = None;
    }

    fn clear_preview(&mut self) {
        self.preview = None;
        self.texture = None;
        self.show_original = false;
    }
}

impl ImagePreviewState {
    pub(super) fn render_deskew_method(&mut self, ui: &mut egui::Ui, palette: &AppPalette) {
        ui.label(egui::RichText::new("Deskew method").strong());
        for method in [
            DeskewMethod::Manual,
            DeskewMethod::UvDoc,
            DeskewMethod::SyntheticDoc,
        ] {
            if ui
                .add(crate::ui_ext::mode_tab_button_icon(
                    Some(Icon::Grid),
                    method.label(),
                    self.deskew_state.method == method,
                    palette,
                ))
                .clicked()
            {
                self.select_deskew_method(method);
            }
        }
        ui.separator();
        ui.add_space(crate::ui_ext::SPACE_XS);
    }

    pub(super) fn select_deskew_method(&mut self, method: DeskewMethod) {
        if self.deskew_state.method != method {
            self.cancel_document_unwarp();
            self.deskew_state.cancel_clicking_points();
            self.deskew_state.dragging_corner = None;
            self.deskew_state.method = method;
        }
    }

    pub(super) fn render_document_unwarp_toolbar(
        &mut self,
        ui: &mut egui::Ui,
        palette: &AppPalette,
    ) {
        let method = self.deskew_state.method;
        ui.label("Flatten a curved or wrinkled document.");
        if method == DeskewMethod::SyntheticDoc {
            ui.label("Crop roughly around the page first for best results.");
            if ui
                .checkbox(
                    &mut self.document_unwarp.illumination,
                    "Correct illumination",
                )
                .changed()
            {
                self.cancel_document_unwarp();
            }
        }
        ui.add_space(crate::ui_ext::SPACE_SM);
        let pending = self.document_unwarp.pending();
        if ui
            .add_enabled(
                !pending && !self.document_unwarp.preview_ready(),
                clip_ui::buttons::ButtonRole::Primary.button("Preview Unwarp", palette),
            )
            .clicked()
        {
            self.start_document_unwarp(BackgroundJobs::global());
        }
        if pending {
            ui.horizontal_wrapped(|ui| {
                ui.spinner();
                ui.label(format!("Flattening with {}…", method.label()));
            });
            if ui.button("Cancel").clicked() {
                self.cancel_document_unwarp();
            }
        }
        if self.document_unwarp.preview.is_some() {
            if ui
                .checkbox(&mut self.document_unwarp.show_original, "Show original")
                .changed()
            {
                self.document_unwarp.texture = None;
            }
            if let Some(preview) = &self.document_unwarp.preview {
                ui.label(format!(
                    "{} × {} px · {:.2} s",
                    preview.result.width(),
                    preview.result.height(),
                    preview.duration.as_secs_f32()
                ));
            }
            if ui
                .add(clip_ui::buttons::ButtonRole::Primary.button("Apply Unwarp (Enter)", palette))
                .clicked()
            {
                self.apply_document_unwarp();
            }
            if ui.button("Discard Preview").clicked() {
                self.cancel_document_unwarp();
            }
        }
        if let Some(error) = &self.document_unwarp.error {
            ui.colored_label(hex_to_c32(&palette.diff_del), error);
        }
        ui.add_space(crate::ui_ext::SPACE_SM);
        ui.label("Runs locally · model included");
    }

    pub(crate) fn render_document_unwarp_preview(
        &mut self,
        painter: &egui::Painter,
        rect: Rect,
        ctx: &egui::Context,
    ) {
        let state = &mut self.document_unwarp;
        let Some(preview) = &state.preview else {
            return;
        };
        let pixels = if state.show_original {
            &preview.original
        } else {
            &preview.result
        };
        if state.texture.is_none() {
            let image = egui::ColorImage::from_rgba_unmultiplied(
                [pixels.width() as usize, pixels.height() as usize],
                pixels.as_raw(),
            );
            state.texture = Some(ctx.load_texture(
                "document_unwarp_preview",
                image,
                image_texture_options(pixels),
            ));
        }
        if let Some(texture) = &state.texture {
            painter.image(
                texture.id(),
                rect,
                Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                Color32::WHITE,
            );
        }
    }
}
