use super::*;
use little_clip_ops::creative_fx::byte_bend_sample;

const MAP_COLUMNS: usize = 28;
const MAP_ROWS: usize = 9;
const MAP_CELLS: usize = MAP_COLUMNS * MAP_ROWS;
const ADDRESS_SPACE: u32 = 65_536;

pub(in crate::image_preview) struct ByteBendEditor {
    pub(super) params: ByteBendParams,
    pub(super) audition: Option<ByteBendEdit>,
    pub(super) full_preview: bool,
    mode: ByteBendMode,
    span: u16,
    variation: u8,
    zoom: u32,
    location: u32,
    wheel: f32,
    map: Option<(u64, u32, u32, egui::TextureHandle)>,
}

impl Default for ByteBendEditor {
    fn default() -> Self {
        Self {
            params: ByteBendParams::default(),
            audition: None,
            full_preview: true,
            mode: ByteBendMode::FlipBits,
            span: 600,
            variation: 4,
            zoom: 1,
            location: ADDRESS_SPACE / 2,
            wheel: 0.0,
            map: None,
        }
    }
}

impl ByteBendEditor {
    pub(in crate::image_preview) fn clear_source(&mut self) {
        self.params.edits.clear();
        self.audition = None;
        self.map = None;
    }

    pub(super) fn preview_params(&self) -> ByteBendParams {
        let mut params = self.params.clone();
        params.edits.extend(self.audition);
        params
    }

    fn range(&self) -> (u32, u32) {
        let length = ADDRESS_SPACE / self.zoom;
        let start = self
            .location
            .saturating_sub(length / 2)
            .min(ADDRESS_SPACE - length);
        (start, length)
    }

    fn map_texture(
        &mut self,
        ctx: &egui::Context,
        image: &RgbaImage,
        revision: u64,
    ) -> egui::TextureId {
        let (start, length) = self.range();
        if let Some((cached_revision, cached_start, cached_length, texture)) = &self.map
            && (*cached_revision, *cached_start, *cached_length) == (revision, start, length)
        {
            return texture.id();
        }
        let pixels = (0..MAP_CELLS)
            .map(|cell| {
                let address = cell_address(start, length, cell);
                let sample = byte_bend_sample(image, address).map_or(0, |(_, value)| value);
                let signal = (sample as i8).unsigned_abs();
                Color32::from_rgb(24 + signal / 2, 33 + signal, 47 + signal)
            })
            .collect();
        let texture = ctx.load_texture(
            "byte_bend_map",
            egui::ColorImage::new([MAP_COLUMNS, MAP_ROWS], pixels),
            egui::TextureOptions::NEAREST,
        );
        let id = texture.id();
        self.map = Some((revision, start, length, texture));
        id
    }

    fn edit_at(&self, address: u16) -> ByteBendEdit {
        ByteBendEdit {
            address,
            span: self.span,
            mode: self.mode,
            variation: self.variation,
        }
    }

    fn change_variation(&mut self, ui: &egui::Ui, response: &egui::Response) {
        if !response.hovered() {
            self.wheel = 0.0;
            return;
        }
        let scroll = ui.input_mut(|input| std::mem::take(&mut input.smooth_scroll_delta.y));
        self.wheel += scroll;
        let steps = (self.wheel / 32.0).trunc() as i32;
        self.wheel -= steps as f32 * 32.0;
        self.variation = (i32::from(self.variation) + steps).rem_euclid(8) as u8;
    }
}

fn cell_address(start: u32, length: u32, cell: usize) -> u16 {
    (start + length * (2 * cell as u32 + 1) / (2 * MAP_CELLS as u32)) as u16
}

fn hovered_cell(rect: Rect, pointer: Pos2) -> usize {
    let col = ((pointer.x - rect.left()) / rect.width() * MAP_COLUMNS as f32) as usize;
    let row = ((pointer.y - rect.top()) / rect.height() * MAP_ROWS as f32) as usize;
    row.min(MAP_ROWS - 1) * MAP_COLUMNS + col.min(MAP_COLUMNS - 1)
}

impl ImagePreviewState {
    pub(super) fn render_byte_bend_controls(&mut self, ui: &mut egui::Ui, palette: &AppPalette) {
        let editor = &mut self.creative_fx_state.byte_bend;
        let mut staged_changed = false;
        ui.label("Hover to preview. Click to stage. Scroll to vary.");
        ui.horizontal(|ui| {
            ui.label("Map");
            for (zoom, label) in [(1, "Full"), (4, "4×"), (16, "16×")] {
                if ui.selectable_label(editor.zoom == zoom, label).clicked() {
                    editor.zoom = zoom;
                }
            }
        });
        if editor.zoom > 1 {
            ui.add(
                egui::Slider::new(&mut editor.location, 0..=ADDRESS_SPACE - 1)
                    .show_value(false)
                    .text("Browse"),
            );
        }
        let (start, length) = editor.range();
        let map = editor.map_texture(ui.ctx(), &self.current_rgba, self.pixel_revision);
        let (rect, response) = ui.allocate_exact_size(
            Vec2::new(ui.available_width().max(1.0), 82.0),
            egui::Sense::click(),
        );
        let painter = ui.painter_at(rect);
        painter.image(
            map,
            rect,
            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
            Color32::WHITE,
        );
        paint_staged_bytes(
            &painter,
            rect,
            start,
            length,
            &editor.params.edits,
            hex_to_c32(&palette.harmonies.base),
        );
        editor.change_variation(ui, &response);
        let hovered = response.hover_pos().map(|point| hovered_cell(rect, point));
        let candidate = hovered.map(|cell| editor.edit_at(cell_address(start, length, cell)));
        if let Some(cell) = hovered {
            let outline = map_cell_rect(rect, cell);
            painter.rect_stroke(
                outline,
                0,
                Stroke::new(1.25, Color32::WHITE),
                egui::StrokeKind::Inside,
            );
        }
        if response.clicked()
            && let Some(edit) = candidate
            && editor.params.edits.len() < MAX_BYTE_BEND_EDITS
        {
            editor.params.edits.push(edit);
            if editor.zoom == 1 {
                editor.location = u32::from(edit.address);
            }
            staged_changed = true;
        }
        let audition = candidate.filter(|candidate| editor.params.edits.last() != Some(candidate));
        let audition_changed = editor.audition != audition;
        editor.audition = audition;

        ui.horizontal(|ui| {
            if let Some((offset, byte)) =
                candidate.and_then(|edit| byte_bend_sample(&self.current_rgba, edit.address))
            {
                ui.monospace(format!("{offset:08X}  {byte:02X}"));
            } else {
                ui.label(format!("{} staged", editor.params.edits.len()));
            }
        });
        ui.horizontal(|ui| {
            ui.label("Mutation");
            egui::ComboBox::from_id_salt("byte_bend_mode")
                .selected_text(editor.mode.label())
                .show_ui(ui, |ui| {
                    for mode in [
                        ByteBendMode::FlipBits,
                        ByteBendMode::Repeat,
                        ByteBendMode::Dropout,
                    ] {
                        ui.selectable_value(&mut editor.mode, mode, mode.label());
                    }
                });
        });
        let mut percent = f32::from(editor.span) * 100.0 / ADDRESS_SPACE as f32;
        if ui
            .add(
                egui::Slider::new(&mut percent, 0.02..=6.25)
                    .text("Span")
                    .suffix("%")
                    .fixed_decimals(2),
            )
            .changed()
        {
            editor.span = (percent * ADDRESS_SPACE as f32 / 100.0).round() as u16;
        }
        ui.add(egui::Slider::new(&mut editor.variation, 0..=7).text("Variation"));
        ui.checkbox(&mut editor.full_preview, "Show full effect");
        ui.horizontal(|ui| {
            let staged = !editor.params.edits.is_empty();
            if ui
                .add_enabled(staged, egui::Button::new("Undo edit"))
                .clicked()
            {
                editor.params.edits.pop();
                staged_changed = true;
            }
            if ui.add_enabled(staged, egui::Button::new("Clear")).clicked() {
                editor.params.edits.clear();
                staged_changed = true;
            }
        });
        if editor.params.edits.len() == MAX_BYTE_BEND_EDITS {
            ui.label("Apply or undo an edit to stage more.");
        }
        if staged_changed {
            self.creative_fx_state.mark_custom();
        } else if audition_changed {
            self.creative_fx_state.mark_dirty();
        }
    }
}

fn map_cell_rect(rect: Rect, cell: usize) -> Rect {
    let size = Vec2::new(
        rect.width() / MAP_COLUMNS as f32,
        rect.height() / MAP_ROWS as f32,
    );
    let offset = Vec2::new(
        (cell % MAP_COLUMNS) as f32 * size.x,
        (cell / MAP_COLUMNS) as f32 * size.y,
    );
    Rect::from_min_size(rect.min + offset, size)
}

fn paint_staged_bytes(
    painter: &egui::Painter,
    rect: Rect,
    start: u32,
    length: u32,
    edits: &[ByteBendEdit],
    accent: Color32,
) {
    for edit in edits {
        let address = u32::from(edit.address);
        if (start..start + length).contains(&address) {
            let cell = ((address - start) * MAP_CELLS as u32 / length) as usize;
            painter.rect_stroke(
                map_cell_rect(rect, cell),
                0,
                Stroke::new(1.5, accent),
                egui::StrokeKind::Inside,
            );
        }
    }
}
