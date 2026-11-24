use crate::file_manager::{eda, preview_job::Task};
use crate::file_source::FileSource;
use egui::{Color32, Rect, TextureHandle, Ui, Vec2, pos2, vec2};
use little_clip_core::{
    BackgroundJobs, ImageThumbnail,
    eda::{
        Document,
        comparison::{ArtworkComparison, compare_artwork},
        read_document_with_source,
    },
};
use std::{sync::atomic::Ordering, time::Duration};

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Mode {
    #[default]
    Changes,
    Overlay,
    A,
    B,
}

#[derive(Default)]
pub(super) struct View {
    mode: Mode,
    opacity: f32,
    only_changes: bool,
    tolerance: u8,
    zoom: f32,
    pan: Vec2,
    pending: Option<((usize, u8), Task<Result<Prepared, String>>)>,
    display: Option<((usize, u8), Result<Display, String>)>,
}

struct Prepared {
    raster: ArtworkComparison,
    notes: [usize; 2],
    roles: [String; 2],
}

struct Display {
    images: [TextureHandle; 3],
    bounds: [f64; 4],
    pitch: f32,
    added: usize,
    removed: usize,
    notes: [usize; 2],
    roles: [String; 2],
}

impl View {
    pub(super) fn new() -> Self {
        Self {
            opacity: 0.5,
            zoom: 1.0,
            ..Self::default()
        }
    }

    pub(super) fn cancel_pending(&mut self) {
        self.pending = None;
    }

    pub(super) fn ui(
        &mut self,
        ui: &mut Ui,
        pair: usize,
        files: [&FileSource; 2],
        names: [&str; 2],
    ) {
        ui.horizontal_wrapped(|ui| {
            for (mode, label) in [
                (Mode::Changes, "Changes"),
                (Mode::Overlay, "Overlay"),
                (Mode::A, "A"),
                (Mode::B, "B"),
            ] {
                clip_ui::buttons::choice(ui, &mut self.mode, mode, label);
            }
            match self.mode {
                Mode::Overlay => {
                    ui.add(egui::Slider::new(&mut self.opacity, 0.0..=1.0).text("B opacity"));
                }
                Mode::Changes => {
                    ui.checkbox(&mut self.only_changes, "Changes only");
                }
                _ => {}
            }
            egui::ComboBox::from_id_salt("artwork tolerance")
                .selected_text(if self.tolerance == 0 {
                    "Exact raster".to_owned()
                } else {
                    format!("{} px edge tolerance", self.tolerance)
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.tolerance, 0, "Exact raster");
                    for px in [1, 2, 4] {
                        ui.selectable_value(
                            &mut self.tolerance,
                            px,
                            format!("{px} px edge tolerance"),
                        );
                    }
                });
            if ui.button("Fit").clicked() {
                self.zoom = 1.0;
                self.pan = Vec2::ZERO;
            }
        });
        let show_b = ui
            .horizontal_wrapped(|ui| {
                ui.label(format!("A  {}    B  {}", names[0], names[1]));
                ui.button("Hold B")
                    .on_hover_text("Hold to temporarily view B at the same zoom and position.")
                    .is_pointer_button_down_on()
            })
            .inner;
        let wanted = (pair, self.tolerance);
        self.update(ui.ctx(), wanted, files);
        let Some((_, display)) = self.display.as_ref().filter(|(key, _)| *key == wanted) else {
            ui.label("Comparing fabrication artwork…");
            return;
        };
        let display = match display {
            Ok(display) => display,
            Err(error) => {
                ui.label(error);
                return;
            }
        };
        ui.horizontal_wrapped(|ui| {
            ui.label(format!("{} / {}", display.roles[0], display.roles[1]));
            ui.colored_label(
                Color32::from_rgb(92, 220, 247),
                format!("Added {} px", display.added),
            );
            ui.colored_label(
                Color32::from_rgb(248, 140, 78),
                format!("Removed {} px", display.removed),
            );
            ui.label(format!("{:.4} mm/px", display.pitch));
        });
        if display.notes != [0, 0] {
            ui.label(format!(
                "Rendering notes  A {} · B {}",
                display.notes[0], display.notes[1]
            ));
        }
        let (rect, response) = ui.allocate_exact_size(
            vec2(
                ui.available_width(),
                (ui.available_height() - 20.0).max(32.0),
            ),
            egui::Sense::click_and_drag(),
        );
        eda::eda_pan_zoom_canvas(
            ui,
            rect,
            rect.center(),
            &response,
            &mut self.zoom,
            &mut self.pan,
        );
        let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
        painter.rect_filled(rect, 0.0, Color32::from_rgb(18, 22, 26));
        let size = display.images[0].size_vec2();
        let fit = eda::eda_fit_rect(rect).size() / size;
        let drawn = Rect::from_center_size(
            rect.center() + self.pan,
            size * (fit.x.min(fit.y) * self.zoom),
        );
        let paint = |index: usize, color| {
            painter.image(
                display.images[index].id(),
                drawn,
                Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                color,
            );
        };
        match if show_b { Mode::B } else { self.mode } {
            Mode::Changes => {
                if !self.only_changes {
                    paint(0, Color32::from_white_alpha(70));
                    paint(1, Color32::from_white_alpha(70));
                }
                paint(2, Color32::WHITE);
            }
            Mode::Overlay => {
                paint(
                    0,
                    Color32::from_rgba_unmultiplied(
                        248,
                        140,
                        78,
                        ((1.0 - self.opacity) * 255.0) as u8,
                    ),
                );
                paint(
                    1,
                    Color32::from_rgba_unmultiplied(92, 220, 247, (self.opacity * 255.0) as u8),
                );
            }
            Mode::A => paint(0, Color32::WHITE),
            Mode::B => paint(1, Color32::WHITE),
        }
        if let Some(mapping) = eda::RasterMapping::new(
            drawn,
            display.images[0].size().map(|n| n as u32),
            display.bounds,
            false,
        ) {
            eda::paint_crosshair(&painter, rect, mapping.screen([0.0, 0.0]));
            if let Some(pointer) = response.hover_pos() {
                let [x, y] = mapping.world(pointer);
                ui.label(format!("X {x:.3} mm · Y {y:.3} mm"));
            }
        }
    }

    fn update(&mut self, ctx: &egui::Context, wanted: (usize, u8), files: [&FileSource; 2]) {
        if self.display.as_ref().is_some_and(|(key, _)| *key == wanted) {
            return;
        }
        self.display = None;
        if let Some((key, task)) = &self.pending {
            if *key != wanted {
                task.token.store(true, Ordering::Relaxed);
            }
            let Some(result) = task.poll() else {
                ctx.request_repaint_after(Duration::from_millis(20));
                return;
            };
            let current = *key == wanted && !task.token.load(Ordering::Relaxed);
            self.pending = None;
            if current {
                if matches!(result, Err(little_clip_core::JobError::QueueFull)) {
                    ctx.request_repaint_after(Duration::from_millis(50));
                    return;
                }
                self.display = Some((
                    wanted,
                    result
                        .map_err(|e| e.to_string())
                        .and_then(|r| r)
                        .map(|prepared| Display::upload(ctx, prepared)),
                ));
                return;
            }
        }
        let files = files.map(Clone::clone);
        self.pending = Some((
            wanted,
            Task::start_unbounded(BackgroundJobs::global(), move |cancel| {
                let [a, b] = files.map(|file| {
                    file.check()?;
                    let (document, observed) = read_document_with_source(file.path(), cancel)?;
                    if file != observed {
                        return Err("Comparison source changed".to_owned());
                    }
                    match document {
                        Document::Gerber(doc) => Ok(doc),
                        _ => Err("Expected a fabrication layer".to_owned()),
                    }
                });
                let [a, b] = [a?, b?];
                let raster = compare_artwork([&a, &b], wanted.1, cancel)?;
                Ok(Prepared {
                    raster,
                    notes: [a.issues.len(), b.issues.len()],
                    roles: [a.layer_type, b.layer_type],
                })
            }),
        ));
        ctx.request_repaint_after(Duration::from_millis(20));
    }
}

impl Display {
    fn upload(ctx: &egui::Context, prepared: Prepared) -> Self {
        let r = prepared.raster;
        let upload = |thumb: ImageThumbnail| {
            ctx.load_texture(
                "fabrication comparison",
                egui::ColorImage::from_rgba_premultiplied(
                    [thumb.width as usize, thumb.height as usize],
                    &thumb.premultiplied_rgba,
                ),
                egui::TextureOptions::LINEAR,
            )
        };
        Self {
            images: [r.before, r.after, r.changes].map(upload),
            bounds: r.bounds_mm,
            pitch: r.pitch_mm,
            added: r.added_pixels,
            removed: r.removed_pixels,
            notes: prepared.notes,
            roles: prepared.roles,
        }
    }
}
