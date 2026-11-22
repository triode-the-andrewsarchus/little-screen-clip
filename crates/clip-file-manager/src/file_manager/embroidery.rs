//! Finished embroidery and passive stitch-order scrubbing in file rows.
use super::{filesystem::Entry, media::PreviewData, shelf::preview::Ready};
use crate::{icons::Icon, shell::StudioShell, ui_appearance::WorkbenchColors};
use egui::{Color32, Rect, TextureHandle, Ui, pos2, vec2};
use little_clip_core::{
    ImageThumbnail,
    embroidery::{Design, TimelineLayout},
    palette::AppPalette,
};
use std::{path::Path, sync::atomic::AtomicBool};

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
        design.timeline([176, 92], 40, cancelled)
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
    let hover = super::ui::preview_hovered(ui, rect)
        .then(|| ui.ctx().pointer_hover_pos())
        .flatten();
    let frame = hover.map_or(layout.colors().len(), |point| {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
        layout.frame((point.x - rect.left()) / rect.width())
    });
    let label = hover.map(|_| format!("{} / {}", layout.stitches_at(frame), layout.stitch_count));
    paint(ui, rect, layout, texture, frame, label.as_deref(), colors);
}

fn paint(
    ui: &Ui,
    rect: Rect,
    layout: &TimelineLayout,
    texture: &TextureHandle,
    frame: usize,
    progress: Option<&str>,
    colors: WorkbenchColors,
) {
    if rect.width() < 8.0 || rect.height() < 16.0 {
        return;
    }
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 3.0, colors.recessed);
    let area = Rect::from_min_max(rect.min + vec2(4.0, 3.0), rect.max - vec2(4.0, 8.0));
    let fit = (area.width() / layout.width as f32).min(area.height() / layout.height as f32);
    let image = Rect::from_center_size(
        area.center(),
        vec2(layout.width as f32, layout.height as f32) * fit,
    );
    let [[u0, v0], [u1, v1]] = layout.uv(frame);
    painter.image(
        texture.id(),
        image,
        Rect::from_min_max(pos2(u0, v0), pos2(u1, v1)),
        Color32::WHITE,
    );

    let bar = Rect::from_min_max(
        pos2(rect.left() + 4.0, rect.bottom() - 5.0),
        rect.max - vec2(4.0, 2.0),
    );
    draw_timeline(&painter, bar, layout, frame, colors);
    if let Some(text) = progress {
        let galley = painter.layout_no_wrap(
            text.to_owned(),
            egui::FontId::monospace(10.0),
            colors.text_primary,
        );
        let label = Rect::from_min_size(
            pos2(area.right() - galley.size().x - 8.0, area.top()),
            galley.size() + vec2(8.0, 4.0),
        );
        painter.rect_filled(label, 3.0, colors.recessed);
        painter.galley(label.min + vec2(4.0, 2.0), galley, colors.text_primary);
    }
}

fn draw_timeline(
    painter: &egui::Painter,
    bar: Rect,
    layout: &TimelineLayout,
    frame: usize,
    colors: WorkbenchColors,
) {
    let width = bar.width() / layout.colors().len() as f32;
    for (index, &[r, g, b]) in layout.colors().iter().enumerate() {
        let color = Color32::from_rgb(r, g, b);
        let color = if index < frame {
            color
        } else {
            color.gamma_multiply(0.4)
        };
        let x = bar.left() + index as f32 * width;
        painter.rect_filled(
            Rect::from_min_size(pos2(x, bar.top()), vec2(width, bar.height())),
            0.0,
            color,
        );
    }
    let x = bar.left() + frame as f32 * width;
    painter.vline(
        x,
        bar.top() - 2.0..=bar.bottom() + 1.0,
        egui::Stroke::new(1.5, colors.text_primary),
    );
}

pub(super) struct Viewer {
    title: String,
    detail: String,
    layout: TimelineLayout,
    pixels: Option<ImageThumbnail>,
    texture: Option<TextureHandle>,
    frame: usize,
}

impl Viewer {
    pub fn load(path: &Path, cancelled: &AtomicBool) -> Result<Self, String> {
        let design = Design::read(path, cancelled)?;
        let timeline = design.timeline([256, 220], 63, cancelled)?;
        Ok(Self {
            title: super::display_path(Path::new(path.file_name().unwrap_or_default())),
            detail: details(&design),
            frame: timeline.layout.colors().len(),
            layout: timeline.layout,
            pixels: Some(timeline.image),
            texture: None,
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
        ui.add(
            egui::Slider::new(&mut self.frame, 0..=self.layout.colors().len())
                .text("Stitch progress")
                .custom_formatter(|frame, _| self.layout.stitches_at(frame as usize).to_string())
                .custom_parser(|value| {
                    let stitches = value.parse::<usize>().ok()?;
                    let fraction = stitches as f32 / self.layout.stitch_count as f32;
                    Some(self.layout.frame(fraction) as f64)
                }),
        );
        let (rect, _) = ui.allocate_exact_size(ui.available_size(), egui::Sense::hover());
        if let Some(texture) = &self.texture {
            paint(
                ui,
                rect,
                &self.layout,
                texture,
                self.frame,
                None,
                WorkbenchColors::from_palette(palette),
            );
        }
    }
}
