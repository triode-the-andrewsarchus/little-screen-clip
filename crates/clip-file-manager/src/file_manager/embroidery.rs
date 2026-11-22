//! Embroidery stitch timelines in file rows and the dedicated viewer.
use super::{filesystem::Entry, media::PreviewData, shelf::preview::Ready};
use crate::{icons::Icon, shell::StudioShell, ui_appearance::WorkbenchColors};
use egui::{Color32, Painter, Rect, Shape, TextureHandle, Ui, pos2, vec2};
use little_clip_core::{
    BackgroundJobs, ImageThumbnail, JobError,
    embroidery::{Design, TimelineLayout},
    jobs::task::Task,
    palette::AppPalette,
};
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

const VIEWER_TILE: [u32; 2] = [256, 220];

fn details(design: &Design) -> String {
    let [width, height] = design.size_mm();
    format!(
        "{} stitches · {width:.1} × {height:.1} mm",
        design.stitch_count()
    )
}

pub(super) fn load_strip(path: &Path, cancelled: &AtomicBool) -> PreviewData {
    let mut data = PreviewData::new(String::new(), String::new(), Icon::Image);
    let timeline = Design::read(path, cancelled).and_then(|design| {
        data.detail = details(&design);
        design.timeline_on([176, 92], 60, [255; 3], cancelled)
    });
    match timeline {
        Ok(timeline) => {
            data.embroidery = Some(timeline.layout);
            data.image = Some(timeline.image);
        }
        Err(error) => data.detail = error,
    }
    data
}

pub(super) fn show_strip(
    ui: &mut Ui,
    row: Rect,
    entry: &Entry,
    columns: super::ui::ListColumns,
    ready: Option<&Ready>,
    colors: WorkbenchColors,
) {
    let rect = super::ui::information_strip(
        ui,
        row,
        entry,
        ready.map_or("Reading stitches…", |ready| &ready.data.detail),
        Some(columns),
        colors,
    );
    let Some((layout, texture)) =
        ready.and_then(|ready| Some((ready.data.embroidery.as_ref()?, ready.texture.as_ref()?)))
    else {
        return;
    };
    if rect.width() <= 8.0 || rect.height() < 16.0 {
        return;
    }
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 3.0, Color32::WHITE);
    let area = rect.shrink2(vec2(4.0, 3.0));
    let [width, height] = layout.design_size_mm;
    let capacity = sample_capacity(area, width, height);
    if capacity > 3 {
        paint_samples(&painter, area, layout, texture, capacity);
    } else {
        paint_row_scrubber(ui, &painter, rect, layout, texture, capacity >= 2, colors);
    }
}

fn sample_capacity(area: Rect, width: f32, height: f32) -> usize {
    // A degenerate design still occupies a finite tile on screen.
    let aspect = width.max(0.1) / height.max(0.1);
    (area.width() / (area.height() * aspect)) as usize
}

fn paint_samples(
    painter: &Painter,
    area: Rect,
    layout: &TimelineLayout,
    texture: &TextureHandle,
    capacity: usize,
) {
    let count = capacity.min(layout.colors().len());
    let width = area.width() / count as f32;
    for sample in 1..=count {
        let x = area.left() + (sample - 1) as f32 * width;
        let slot = Rect::from_min_size(pos2(x, area.top()), vec2(width, area.height()));
        let frame = layout.frame(sample as f32 / count as f32);
        paint_frame(painter, slot, layout, texture, frame);
    }
}

fn paint_row_scrubber(
    ui: &Ui,
    painter: &Painter,
    rect: Rect,
    layout: &TimelineLayout,
    texture: &TextureHandle,
    show_finished: bool,
    colors: WorkbenchColors,
) {
    let bar = timeline_bar(rect);
    let mut area = scrub_area(rect);
    let complete = layout.colors().len();
    let mut frame = complete;
    if show_finished {
        let width =
            (area.height() * layout.width as f32 / layout.height as f32).min(area.width() / 2.0);
        let split = area.right() - width;
        let finished = Rect::from_min_max(pos2(split, area.top()), area.max);
        paint_frame(painter, finished, layout, texture, complete);
        area.max.x = split;
        frame = layout.frame(0.5);
    }
    let hover = super::ui::preview_hovered(ui, rect)
        .then(|| ui.ctx().pointer_hover_pos())
        .flatten();
    if let Some(point) = hover {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
        frame = layout.frame((point.x - bar.left()) / bar.width());
    }
    paint_frame(painter, area, layout, texture, frame);
    draw_timeline(painter, bar, layout, frame, Color32::BLACK);
    if hover.is_some() {
        let text = format!("{} / {}", layout.stitches_at(frame), layout.stitch_count);
        let galley =
            painter.layout_no_wrap(text, egui::FontId::monospace(10.0), colors.text_primary);
        let label = Rect::from_min_size(
            pos2(area.right() - galley.size().x - 8.0, area.top()),
            galley.size() + vec2(8.0, 4.0),
        );
        painter.rect_filled(label, 3.0, colors.recessed);
        painter.galley(label.min + vec2(4.0, 2.0), galley, colors.text_primary);
    }
}

fn scrub_area(rect: Rect) -> Rect {
    Rect::from_min_max(rect.min + vec2(4.0, 3.0), rect.max - vec2(4.0, 8.0))
}

fn timeline_bar(rect: Rect) -> Rect {
    Rect::from_min_max(
        pos2(rect.left() + 4.0, rect.bottom() - 5.0),
        rect.max - vec2(4.0, 2.0),
    )
}

fn paint_frame(
    painter: &Painter,
    area: Rect,
    layout: &TimelineLayout,
    texture: &TextureHandle,
    frame: usize,
) {
    let [[u0, v0], [u1, v1]] = layout.uv(frame);
    painter.image(
        texture.id(),
        fitted_frame(area, layout),
        Rect::from_min_max(pos2(u0, v0), pos2(u1, v1)),
        Color32::WHITE,
    );
}

fn fitted_frame(area: Rect, layout: &TimelineLayout) -> Rect {
    let size = vec2(layout.width as f32, layout.height as f32);
    let fit = (area.width() / size.x).min(area.height() / size.y);
    Rect::from_center_size(area.center(), size * fit)
}

fn draw_timeline(
    painter: &Painter,
    bar: Rect,
    layout: &TimelineLayout,
    frame: usize,
    marker: Color32,
) {
    painter.extend(thread_bands(bar, layout, frame));
    let x = egui::lerp(bar.x_range(), frame as f32 / layout.colors().len() as f32);
    painter.vline(
        x,
        bar.top() - 2.0..=bar.bottom() + 1.0,
        egui::Stroke::new(1.5, marker),
    );
}

fn thread_bands(bar: Rect, layout: &TimelineLayout, frame: usize) -> impl Iterator<Item = Shape> {
    let width = bar.width() / layout.colors().len() as f32;
    layout
        .colors()
        .iter()
        .enumerate()
        .map(move |(index, &[r, g, b])| {
            let color = Color32::from_rgb(r, g, b);
            let color = if index < frame {
                color
            } else {
                color.gamma_multiply(0.4)
            };
            let x = bar.left() + index as f32 * width;
            Shape::rect_filled(
                Rect::from_min_size(pos2(x, bar.top()), vec2(width, bar.height())),
                0.0,
                color,
            )
        })
}

fn viewer_timeline(
    ui: &mut Ui,
    rect: Rect,
    layout: &TimelineLayout,
    frame: &mut usize,
) -> egui::Response {
    let painter = ui.painter_at(rect);
    let bands = painter.add(Shape::Noop);
    let mut timeline = ui.new_child(
        egui::UiBuilder::new()
            .id_salt("embroidery-timeline")
            .max_rect(rect)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    let spacing = timeline.spacing_mut();
    spacing.interact_size.y = rect.height();
    spacing.slider_width = rect.width();
    spacing.slider_rail_height = 0.0;
    let response = timeline.add(
        egui::Slider::new(frame, 0..=layout.colors().len())
            .handle_shape(egui::style::HandleShape::Circle)
            .show_value(false)
            .smart_aim(false)
            .trailing_fill(false),
    );
    // Match the inset egui gives a circular slider handle at either end.
    let handle_radius = response.rect.height() / 2.5;
    let bar = Rect::from_center_size(
        response.rect.center(),
        vec2(response.rect.width() - 2.0 * handle_radius, 12.0),
    );
    painter.set(
        bands,
        Shape::Vec(thread_bands(bar, layout, *frame).collect()),
    );
    let stitches = format!(
        "{} / {} stitches",
        layout.stitches_at(*frame),
        layout.stitch_count
    );
    ui.ctx().accesskit_node_builder(response.id, |node| {
        node.set_label("Stitch progress");
        node.set_value(stitches.clone());
    });
    response
        .on_hover_cursor(egui::CursorIcon::ResizeHorizontal)
        .on_hover_ui(|ui| {
            ui.label(stitches);
        })
}

pub(super) struct Viewer {
    title: String,
    detail: String,
    layout: TimelineLayout,
    pixels: Option<ImageThumbnail>,
    texture: Option<TextureHandle>,
    progress: f32,
    design: Arc<Design>,
    detail_frame: DetailFrame,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct FrameRequest {
    size: [u32; 2],
    stitches: usize,
}

#[derive(Default)]
struct DetailFrame {
    pending: Option<(FrameRequest, Task<Result<egui::ColorImage, String>>)>,
    ready: Option<(FrameRequest, Option<TextureHandle>)>,
}

impl DetailFrame {
    fn texture(
        &mut self,
        ctx: &egui::Context,
        design: &Arc<Design>,
        wanted: FrameRequest,
    ) -> Option<&TextureHandle> {
        if let Some((request, task)) = &self.pending
            && *request != wanted
        {
            task.token.store(true, Ordering::Relaxed);
        }
        if let Some((request, task)) = &self.pending
            && let Some(result) = task.poll()
        {
            let request = *request;
            let cancelled = task.token.load(Ordering::Relaxed);
            self.pending = None;
            if request == wanted && !cancelled {
                if matches!(result, Err(JobError::QueueFull)) {
                    ctx.request_repaint_after(Duration::from_millis(50));
                    return None;
                }
                let image = result.ok().and_then(Result::ok);
                self.ready = Some((
                    request,
                    image.map(|image| {
                        ctx.load_texture("embroidery-detail", image, egui::TextureOptions::LINEAR)
                    }),
                ));
            }
        }
        if let Some((_, texture)) = self
            .ready
            .as_ref()
            .filter(|(request, _)| *request == wanted)
        {
            return texture.as_ref();
        }
        // Keep the job slot until the worker acknowledges superseded requests.
        if self.pending.is_none() {
            let design = Arc::clone(design);
            self.pending = Some((
                wanted,
                Task::start_unbounded(BackgroundJobs::global(), move |cancelled| {
                    let raster = design.detailed_prefix(
                        VIEWER_TILE,
                        wanted.size,
                        wanted.stitches,
                        cancelled,
                    )?;
                    Ok(egui::ColorImage::from_rgba_premultiplied(
                        [raster.width as usize, raster.height as usize],
                        &raster.premultiplied_rgba,
                    ))
                }),
            ));
        }
        ctx.request_repaint_after(Duration::from_millis(16));
        None
    }
}

impl Viewer {
    pub fn load(path: &Path, cancelled: &AtomicBool) -> Result<Self, String> {
        let design = Design::read(path, cancelled)?;
        let timeline = design.timeline(VIEWER_TILE, 63, cancelled)?;
        Ok(Self {
            title: super::display_path(Path::new(path.file_name().unwrap_or_default())),
            detail: details(&design),
            progress: timeline.layout.colors().len() as f32,
            layout: timeline.layout,
            pixels: Some(timeline.image),
            texture: None,
            design: Arc::new(design),
            detail_frame: DetailFrame::default(),
        })
    }

    pub fn ui(&mut self, ui: &mut Ui, close: &mut bool, palette: &AppPalette) {
        StudioShell::new(&self.title, Icon::Image, palette, close).render_header(ui, |_| {});
        if let Some(pixels) = self.pixels.take() {
            self.texture = Some(ui.ctx().load_texture(
                "embroidery-viewer",
                egui::ColorImage::from_rgba_premultiplied(
                    [pixels.width as usize, pixels.height as usize],
                    &pixels.premultiplied_rgba,
                ),
                egui::TextureOptions::LINEAR,
            ));
        }
        ui.label(&self.detail);
        let (rect, _) = ui.allocate_exact_size(ui.available_size(), egui::Sense::hover());
        if let Some(texture) = &self.texture
            && rect.width() >= 40.0
            && rect.height() >= 40.0
        {
            let painter = ui.painter_at(rect);
            painter.rect_filled(rect, 3.0, WorkbenchColors::from_palette(palette).recessed);
            if super::ui::preview_hovered(ui, rect) {
                let scroll = clip_ui::ui_ext::wheel_delta_including_start(ui.ctx()).y;
                if scroll != 0.0 {
                    // Retain fractional movement for fine touchpad gestures.
                    self.progress = (self.progress + scroll / 16.0)
                        .clamp(0.0, self.layout.colors().len() as f32);
                    ui.input_mut(|input| input.smooth_scroll_delta.y = 0.0);
                }
            }
            let timeline = Rect::from_min_max(
                pos2(rect.left() + 4.0, rect.bottom() - 32.0),
                rect.max - vec2(4.0, 4.0),
            );
            let mut frame = self.progress.round() as usize;
            let response = viewer_timeline(ui, timeline, &self.layout, &mut frame);
            if response.changed() || response.dragged() {
                self.progress = frame as f32;
            }
            let area = Rect::from_min_max(
                rect.min + vec2(4.0, 3.0),
                timeline.right_top() - vec2(0.0, 4.0),
            );
            let display = fitted_frame(area, &self.layout);
            let pixels = display.size() * ui.ctx().pixels_per_point();
            let texture_limit = ui.input(|input| input.max_texture_side) as f32;
            let size = [pixels.x, pixels.y].map(|px| px.ceil().min(texture_limit) as u32);
            if size[0] > self.layout.width
                && size[1] > self.layout.height
                && let Some(detail) = self.detail_frame.texture(
                    ui.ctx(),
                    &self.design,
                    FrameRequest {
                        size,
                        stitches: self.layout.stitches_at(frame),
                    },
                )
            {
                let inset = vec2(
                    0.5 / self.layout.width as f32,
                    0.5 / self.layout.height as f32,
                );
                painter.image(
                    detail.id(),
                    display,
                    Rect::from_min_max(pos2(0.0, 0.0) + inset, pos2(1.0, 1.0) - inset),
                    Color32::WHITE,
                );
            } else {
                paint_frame(&painter, area, &self.layout, texture, frame);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::{Harness, kittest::Queryable};

    #[test]
    fn strip_capacity_follows_the_stitched_aspect_ratio() {
        let area = Rect::from_min_size(pos2(0.0, 0.0), vec2(200.0, 10.0));
        for (width, count) in [
            (2.0, 10),
            (5.0, 4),
            (6.0, 3),
            (9.0, 2),
            (11.0, 1),
            (21.0, 0),
        ] {
            assert_eq!(sample_capacity(area, width, 1.0), count);
        }
    }

    #[test]
    fn viewer_thread_bar_seeks_and_scrolls_the_same_progress() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../little-clip-core/tests/fixtures/embroidery/Wildflower.pes");
        let viewer = Viewer::load(&path, &AtomicBool::new(false)).unwrap();
        let complete = viewer.layout.colors().len() as f32;
        let palette = AppPalette::default();
        let mut harness = Harness::builder()
            .with_size(vec2(640.0, 480.0))
            .with_context_setup(egui_extras::install_image_loaders)
            .build_ui_state(
                move |ui, viewer| viewer.ui(ui, &mut false, &palette),
                viewer,
            );
        harness.run();
        assert_eq!(harness.state().progress, complete);

        let bar = harness.get_by_label("Stitch progress").rect();
        harness.get_by_label("Stitch progress").click();
        harness.run();
        assert!((harness.state().progress - complete / 2.0).abs() <= 0.5);

        harness.drag_at(bar.center());
        harness.run();
        harness.hover_at(bar.right_center());
        harness.run();
        assert_eq!(harness.state().progress, complete);
        harness.drop_at(bar.right_center());
        harness.run();

        harness.get_by_label("Stitch progress").focus();
        harness.run();
        harness.key_press(egui::Key::ArrowLeft);
        harness.run();
        assert_eq!(harness.state().progress, complete - 1.0);

        harness.hover_at(bar.center() - vec2(0.0, 120.0));
        harness.run();
        harness.event(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: vec2(0.0, -48.0),
            phase: egui::TouchPhase::Start,
            source: egui::MouseWheelSource::Unknown,
            modifiers: egui::Modifiers::NONE,
        });
        harness.run();
        assert_eq!(harness.state().progress, complete - 4.0);
        harness.event(clip_ui::test_support::scroll(vec2(0.0, 120.0)));
        harness.run();
        assert_eq!(harness.state().progress, complete);
    }

    #[test]
    fn viewer_detail_ignores_an_obsolete_frame_and_resolution() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../little-clip-core/tests/fixtures/embroidery/Wildflower.pes");
        let design = Arc::new(Design::read(&path, &AtomicBool::new(false)).unwrap());
        let ctx = egui::Context::default();
        let mut detail = DetailFrame::default();
        let obsolete = FrameRequest {
            size: [480, 700],
            stitches: 0,
        };
        let wanted = FrameRequest {
            size: [1300, 1900],
            stitches: design.stitch_count(),
        };
        assert!(detail.texture(&ctx, &design, obsolete).is_none());
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(texture) = detail.texture(&ctx, &design, wanted) {
                assert_eq!(texture.size()[1], 1900);
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "detail frame never arrived"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}
