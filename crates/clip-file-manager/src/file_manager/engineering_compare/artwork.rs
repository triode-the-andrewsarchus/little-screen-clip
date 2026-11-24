use crate::file_manager::{
    eda::{
        self,
        raster_task::{REPAINT_INTERVAL, RasterTask, poll_current},
    },
    preview_job::Task,
};
use crate::file_source::FileSource;
use egui::{Color32, Rect, TextureHandle, Ui, Vec2, pos2, vec2};
use little_clip_core::{
    BackgroundJobs, ImageThumbnail,
    eda::{
        RasterRegion,
        comparison::{ArtworkComparison, ArtworkRaster, compare_artwork, read_artwork},
    },
};
use std::{
    sync::{Arc, LazyLock},
    task::Poll,
};

static GEOMETRY_JOBS: LazyLock<BackgroundJobs> = LazyLock::new(|| BackgroundJobs::new(1, 2));

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
    tolerance_um: u16,
    zoom: f32,
    pan: Vec2,
    pending: RasterTask<OverviewKey, Prepared>,
    display: Option<(OverviewKey, Result<Display, String>)>,
    detail: Detail,
}

type OverviewKey = ([usize; 2], u16);

struct Prepared {
    overview: ArtworkRaster,
    comparison: ArtworkComparison,
    roles: [String; 2],
}

struct Display {
    overview: Textures,
    comparison: Arc<ArtworkComparison>,
    roles: [String; 2],
}

struct Textures {
    images: [TextureHandle; 3],
    pitch: f64,
}

#[derive(Default)]
struct Detail {
    pending: RasterTask<RasterRegion, ArtworkRaster>,
    display: Option<(RasterRegion, Textures)>,
    failed: Option<(RasterRegion, String)>,
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
        self.detail.pending = None;
    }

    pub(super) fn ui(
        &mut self,
        ui: &mut Ui,
        pair: [usize; 2],
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
            egui::ComboBox::from_label("Edge tolerance")
                .selected_text(if self.tolerance_um == 0 {
                    "0 µm".to_owned()
                } else {
                    format!("{} µm", self.tolerance_um)
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.tolerance_um, 0, "0 µm");
                    for microns in [10, 25, 50, 100] {
                        ui.selectable_value(
                            &mut self.tolerance_um,
                            microns,
                            format!("{microns} µm"),
                        );
                    }
                })
                .response
                .on_hover_text("Physical edge tolerance. Geometry snaps to a fixed 0.1 µm grid.");
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
        let wanted = (pair, self.tolerance_um);
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
            let [a, b] = &display.roles;
            ui.label(format!("{a} / {b}"));
            ui.label("Geometry");
            ui.colored_label(
                Color32::from_rgb(92, 220, 247),
                format!("Added {}", area(display.comparison.added_mm2)),
            );
            ui.colored_label(
                Color32::from_rgb(248, 140, 78),
                format!("Removed {}", area(display.comparison.removed_mm2)),
            );
            ui.label(format!("Overview {:.4} mm/px", display.overview.pitch));
        });
        let (rect, response) = ui.allocate_exact_size(
            vec2(
                ui.available_width(),
                (ui.available_height() - 40.0).max(32.0),
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
        let size = display.overview.images[0].size_vec2();
        let fit = eda::eda_fit_rect(rect).size() / size;
        let scale = fit.x.min(fit.y) * self.zoom;
        let drawn = Rect::from_center_size(rect.center() + self.pan, size * scale);
        let canvas = Canvas {
            mode: if show_b { Mode::B } else { self.mode },
            opacity: self.opacity,
            only_changes: self.only_changes,
        };
        painter.rect_filled(rect, 0.0, background());
        canvas.paint(&painter, drawn, &display.overview);
        let Some(mapping) = eda::RasterMapping::new(
            drawn,
            display.overview.images[0].size().map(|n| n as u32),
            display.comparison.bounds_mm,
            false,
        ) else {
            return;
        };

        let local = rect.intersect(drawn);
        let wanted = local
            .is_positive()
            .then(|| eda::detail_region(local, &mapping, scale, ui.ctx().pixels_per_point()))
            .flatten();
        let mut detail_text = String::new();
        if let Some(region) = wanted {
            self.detail.update(ui.ctx(), region, &display.comparison);
            if let Some((region, textures)) = &self.detail.display {
                let detail_rect = mapping.region_rect(*region);
                let clip = detail_rect.intersect(local);
                if clip.is_positive() {
                    let painter = painter.with_clip_rect(clip);
                    painter.rect_filled(detail_rect, 0.0, background());
                    canvas.paint(&painter, detail_rect, textures);
                    detail_text = format!("Detail {:.4} mm/px", textures.pitch);
                }
            }
            if let Some((_, error)) = &self.detail.failed {
                detail_text = format!("Detail unavailable · {error}");
            } else if self.detail.pending.is_some() {
                detail_text.push_str(if detail_text.is_empty() {
                    "Rendering detail…"
                } else {
                    " · Updating…"
                });
            }
        } else {
            self.detail = Detail::default();
        }
        eda::paint_crosshair(&painter, rect, mapping.screen([0.0, 0.0]));
        if !detail_text.is_empty() {
            ui.label(detail_text);
        }
        if let Some(pointer) = response.hover_pos() {
            let [x, y] = mapping.world(pointer);
            ui.label(format!("X {x:.3} mm · Y {y:.3} mm"));
        }
    }

    fn update(&mut self, ctx: &egui::Context, wanted: OverviewKey, files: [&FileSource; 2]) {
        if self.display.as_ref().is_some_and(|(key, _)| *key == wanted) {
            return;
        }
        self.display = None;
        self.detail = Detail::default();
        match poll_current(ctx, &mut self.pending, &wanted) {
            Poll::Pending => return,
            Poll::Ready(Some(result)) => {
                self.display = Some((
                    wanted,
                    result.map(|prepared| Display::upload(ctx, prepared)),
                ));
                return;
            }
            Poll::Ready(None) => {}
        }
        let files = files.map(Clone::clone);
        self.pending = Some((
            wanted,
            Task::start_unbounded(&GEOMETRY_JOBS, move |cancel| {
                let [a, b] = files.map(|file| {
                    file.check()?;
                    let (document, observed) = read_artwork(file.path(), cancel)?;
                    if file != observed {
                        return Err("Comparison source changed".to_owned());
                    }
                    Ok(document)
                });
                let [a, b] = [a?, b?];
                let comparison = compare_artwork([&a, &b], f64::from(wanted.1) / 1000.0, cancel)?;
                let overview = comparison.render_overview(cancel)?;
                Ok(Prepared {
                    comparison,
                    overview,
                    roles: [a.layer_type, b.layer_type],
                })
            }),
        ));
        ctx.request_repaint_after(REPAINT_INTERVAL);
    }
}

impl Display {
    fn upload(ctx: &egui::Context, prepared: Prepared) -> Self {
        Self {
            comparison: Arc::new(prepared.comparison),
            overview: Textures::upload(ctx, prepared.overview),
            roles: prepared.roles,
        }
    }
}

impl Textures {
    fn upload(ctx: &egui::Context, raster: ArtworkRaster) -> Self {
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
            images: [raster.before, raster.after, raster.changes].map(upload),
            pitch: raster.pitch_mm,
        }
    }
}

impl Detail {
    fn update(
        &mut self,
        ctx: &egui::Context,
        wanted: RasterRegion,
        comparison: &Arc<ArtworkComparison>,
    ) {
        if self
            .display
            .as_ref()
            .is_some_and(|(region, _)| *region == wanted)
        {
            self.pending = None;
            self.failed = None;
            return;
        }
        if self
            .failed
            .as_ref()
            .is_some_and(|(region, _)| *region == wanted)
        {
            return;
        }
        self.failed = None;
        match poll_current(ctx, &mut self.pending, &wanted) {
            Poll::Pending => return,
            Poll::Ready(Some(Ok(raster))) => {
                self.display = Some((wanted, Textures::upload(ctx, raster)));
                return;
            }
            Poll::Ready(Some(Err(error))) => {
                self.failed = Some((wanted, error));
                return;
            }
            Poll::Ready(None) => {}
        }
        let comparison = Arc::clone(comparison);
        self.pending = Some((
            wanted,
            Task::start_unbounded(BackgroundJobs::global(), move |cancel| {
                comparison.render_region(wanted, cancel)
            }),
        ));
        ctx.request_repaint_after(REPAINT_INTERVAL);
    }
}

struct Canvas {
    mode: Mode,
    opacity: f32,
    only_changes: bool,
}

impl Canvas {
    fn paint(&self, painter: &egui::Painter, rect: Rect, textures: &Textures) {
        let paint = |index: usize, color| {
            painter.image(
                textures.images[index].id(),
                rect,
                Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                color,
            );
        };
        match self.mode {
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
    }
}

fn area(mm2: f64) -> String {
    if mm2 == 0.0 {
        "0 mm²".into()
    } else if mm2 < 0.01 {
        format!("{:.3} µm²", mm2 * 1_000_000.0)
    } else {
        format!("{mm2:.4} mm²")
    }
}

fn background() -> Color32 {
    Color32::from_rgb(18, 22, 26)
}
