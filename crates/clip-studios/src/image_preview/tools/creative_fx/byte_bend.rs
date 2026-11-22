use super::*;
use egui::scroll_area::{DragScroll, ScrollSource};
use little_clip_ops::creative_fx::byte_bend_sample;

const MAP_COLUMNS: usize = 28;
const MAP_ROWS: usize = 96;
const MAP_CELLS: usize = MAP_COLUMNS * MAP_ROWS;
const MIN_MAP_HEIGHT: f32 = 96.0;
const DEFAULT_SPAN: u16 = 600;
const MIN_SPAN: u16 = 13;
const MAX_SPAN: u16 = 4_096;
const HOLD_SECONDS: f64 = 0.18;

struct HeldEdit {
    edit: ByteBendEdit,
    started: f64,
    adjusting: bool,
}

pub(in crate::image_preview) struct ByteBendEditor {
    pub(super) params: ByteBendParams,
    pub(super) audition: Option<ByteBendEdit>,
    pub(super) full_preview: bool,
    held: Option<HeldEdit>,
    suppressed_hover: Option<u16>,
    map: Option<(u64, egui::TextureHandle)>,
}

impl Default for ByteBendEditor {
    fn default() -> Self {
        Self {
            params: ByteBendParams::default(),
            audition: None,
            full_preview: true,
            held: None,
            suppressed_hover: None,
            map: None,
        }
    }
}

impl ByteBendEditor {
    pub(super) fn clear_interaction(&mut self) {
        self.audition = None;
        self.held = None;
        self.suppressed_hover = None;
    }

    pub(in crate::image_preview) fn clear_source(&mut self) {
        self.params.edits.clear();
        self.clear_interaction();
        self.map = None;
    }

    pub(super) fn preview_params(&self) -> ByteBendParams {
        let mut params = self.params.clone();
        if let Some(edit) = self.audition {
            params.edits.retain(|staged| staged.address != edit.address);
            params.edits.push(edit);
        }
        params
    }

    fn stage(&mut self, edit: ByteBendEdit) {
        self.params
            .edits
            .retain(|staged| staged.address != edit.address);
        self.params.edits.push(edit);
    }

    fn edit_at(&self, address: u16) -> Option<ByteBendEdit> {
        let previous = self
            .params
            .edits
            .iter()
            .find(|edit| edit.address == address);
        if self.params.edits.len() >= MAX_BYTE_BEND_EDITS && previous.is_none() {
            return None;
        }
        let mut modes = [
            ByteBendMode::FlipBits,
            ByteBendMode::Repeat,
            ByteBendMode::Dropout,
        ];
        fastrand::shuffle(&mut modes);
        let mode = modes
            .into_iter()
            .find(|mode| previous.is_none_or(|edit| *mode != edit.mode))?;
        let variation = if let Some(edit) = previous {
            (edit.variation + fastrand::u8(1..8)) % 8
        } else {
            fastrand::u8(0..8)
        };
        Some(ByteBendEdit {
            address,
            span: previous.map_or(DEFAULT_SPAN, |edit| edit.span),
            mode,
            variation,
        })
    }

    fn map_texture(
        &mut self,
        ctx: &egui::Context,
        image: &RgbaImage,
        revision: u64,
    ) -> egui::TextureId {
        if let Some((cached_revision, texture)) = &self.map
            && *cached_revision == revision
        {
            return texture.id();
        }
        let pixels = (0..MAP_CELLS)
            .map(|cell| {
                let sample = byte_bend_sample(image, self.params.profile, cell_address(cell))
                    .map_or(0, |(_, value)| value);
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
        self.map = Some((revision, texture));
        id
    }

    fn show_map(
        &mut self,
        ui: &mut egui::Ui,
        image: &RgbaImage,
        revision: u64,
        accent: Color32,
    ) -> bool {
        let height = ui.available_height().max(1.0);
        let edit_before = self.audition;
        let changed = egui::ScrollArea::vertical()
            .id_salt("byte_bend_scroll")
            .max_height(height)
            .min_scrolled_height(MIN_MAP_HEIGHT.min(height))
            .auto_shrink([false, false])
            .scroll_source(ScrollSource {
                drag: DragScroll::Never,
                mouse_wheel: self.held.is_none(),
                ..Default::default()
            })
            .show(ui, |ui| {
                self.show_map_grid(ui, image, revision, accent, height)
            })
            .inner;
        changed || edit_before != self.audition
    }

    fn show_map_grid(
        &mut self,
        ui: &mut egui::Ui,
        image: &RgbaImage,
        revision: u64,
        accent: Color32,
        height: f32,
    ) -> bool {
        let width = ui.available_width().max(1.0);
        let grid_height = MAP_ROWS as f32 * (width / MAP_COLUMNS as f32).max(8.0);
        let size = Vec2::new(width, grid_height.max(height));
        let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click_and_drag());
        let response = response.on_hover_cursor(egui::CursorIcon::Crosshair);
        let map = self.map_texture(ui.ctx(), image, revision);
        let painter = ui.painter_at(rect);
        painter.image(
            map,
            rect,
            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
            Color32::WHITE,
        );
        let hover = response.hover_pos().map(|point| map_cell(rect, point));
        let visible = rect.intersect(ui.clip_rect());
        let changed = self.interact(ui, &response, visible, image, hover.map(cell_address));
        for edit in &self.params.edits {
            outline_cell(&painter, rect, address_cell(edit.address), accent);
        }
        if let Some(cell) = self
            .held
            .as_ref()
            .map(|hold| address_cell(hold.edit.address))
            .or(hover)
        {
            outline_cell(&painter, rect, cell, Color32::WHITE);
        }
        changed
    }

    fn interact(
        &mut self,
        ui: &egui::Ui,
        response: &egui::Response,
        visible: Rect,
        image: &RgbaImage,
        hovered: Option<u16>,
    ) -> bool {
        if self.suppressed_hover != hovered {
            self.suppressed_hover = None;
        }
        if response.secondary_clicked()
            && let Some(address) = hovered
        {
            self.suppressed_hover = Some(address);
            self.audition = None;
            self.held = None;
            return self.params.restore_at(image, address);
        }

        if self.held.is_none() {
            self.held = self.begin_hold(ui, response, visible);
        }
        if let Some(hold) = self.held.take() {
            self.suppressed_hover = None;
            let staged = self.update_hold(ui, response, visible, hold);
            if staged {
                self.suppressed_hover = hovered;
            }
            return staged;
        }

        self.audition = hovered
            .filter(|address| self.suppressed_hover != Some(*address))
            .filter(|address| {
                !self
                    .params
                    .edits
                    .iter()
                    .any(|edit| edit.address == *address)
            })
            .filter(|_| self.params.edits.len() < MAX_BYTE_BEND_EDITS)
            .map(|address| ByteBendEdit {
                address,
                span: DEFAULT_SPAN,
                mode: ByteBendMode::FlipBits,
                variation: 4,
            });
        false
    }

    fn begin_hold(
        &self,
        ui: &egui::Ui,
        response: &egui::Response,
        visible: Rect,
    ) -> Option<HeldEdit> {
        let primary = egui::PointerButton::Primary;
        let (pressed_here, pointer, started) = ui.input(|input| {
            (
                input.pointer.button_pressed(primary) && response.is_pointer_button_down_on(),
                input.pointer.press_origin().or(input.pointer.latest_pos()),
                input.time,
            )
        });
        if !pressed_here && !response.clicked_by(primary) {
            return None;
        }
        let point = pointer.filter(|point| visible.contains(*point))?;
        let edit = self.edit_at(cell_address(map_cell(response.rect, point)))?;
        Some(HeldEdit {
            edit,
            started,
            adjusting: false,
        })
    }

    fn update_hold(
        &mut self,
        ui: &egui::Ui,
        response: &egui::Response,
        visible: Rect,
        mut hold: HeldEdit,
    ) -> bool {
        let primary = egui::PointerButton::Primary;
        let (now, down, released, point) = ui.input(|input| {
            (
                input.time,
                input.pointer.button_down(primary),
                input.pointer.button_released(primary),
                input.pointer.latest_pos().filter(|_| input.focused),
            )
        });
        let Some(point) = point else {
            self.audition = None;
            return false;
        };
        let until_adjusting = HOLD_SECONDS - (now - hold.started);
        hold.adjusting |= until_adjusting <= 0.0 || response.dragged_by(primary);
        if hold.adjusting {
            adjust_edit(&mut hold.edit, visible, point);
        }
        if down {
            if !hold.adjusting {
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_secs_f64(until_adjusting));
            }
            self.audition = Some(hold.edit);
            self.held = Some(hold);
            return false;
        }
        self.audition = None;
        let stage = response.clicked_by(primary) || (released && hold.adjusting);
        if stage {
            self.stage(hold.edit);
        }
        stage
    }
}

fn adjust_edit(edit: &mut ByteBendEdit, rect: Rect, point: Pos2) {
    let x = ((point.x - rect.left()) / rect.width().max(1.0)).clamp(0.0, 1.0);
    let y = ((rect.bottom() - point.y) / rect.height().max(1.0)).clamp(0.0, 1.0);
    edit.span = egui::lerp(f32::from(MIN_SPAN)..=f32::from(MAX_SPAN), x).round() as u16;
    edit.variation = (7.0 * y).round() as u8;
}

fn cell_address(cell: usize) -> u16 {
    ((2 * cell + 1) * 65_536 / (2 * MAP_CELLS)) as u16
}

fn address_cell(address: u16) -> usize {
    usize::from(address) * MAP_CELLS / 65_536
}

fn map_cell(rect: Rect, point: Pos2) -> usize {
    let col = ((point.x - rect.left()) / rect.width() * MAP_COLUMNS as f32) as usize;
    let row = ((point.y - rect.top()) / rect.height() * MAP_ROWS as f32) as usize;
    row.min(MAP_ROWS - 1) * MAP_COLUMNS + col.min(MAP_COLUMNS - 1)
}

fn outline_cell(painter: &egui::Painter, rect: Rect, cell: usize, color: Color32) {
    let size = Vec2::new(
        rect.width() / MAP_COLUMNS as f32,
        rect.height() / MAP_ROWS as f32,
    );
    let corner =
        rect.min + size * Vec2::new((cell % MAP_COLUMNS) as f32, (cell / MAP_COLUMNS) as f32);
    painter.rect_stroke(
        Rect::from_min_size(corner, size),
        0,
        Stroke::new(1.25, color),
        egui::StrokeKind::Inside,
    );
}

impl ImagePreviewState {
    pub(super) fn render_byte_bend_controls(&mut self, ui: &mut egui::Ui, palette: &AppPalette) {
        let accent = hex_to_c32(&palette.harmonies.base);
        ui.horizontal_wrapped(|ui| {
            creative_fx_icon(CreativeFxType::ByteBend).ui(ui, 16.0, accent);
            ui.label(egui::RichText::new("Byte Bend").strong().color(accent));
            ui.label(self.creative_fx_state.byte_bend.params.profile.label());
        });

        ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
            self.render_creative_fx_apply_button(ui, palette, ui.available_width());

            let editor = &mut self.creative_fx_state.byte_bend;
            let staged = !editor.params.edits.is_empty();
            let mut changed = false;
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add_enabled(staged, egui::Button::new("Undo edit"))
                    .clicked()
                {
                    editor.params.edits.pop();
                    changed = true;
                }
                if ui.add_enabled(staged, egui::Button::new("Clear")).clicked() {
                    editor.params.edits.clear();
                    changed = true;
                }
            });
            ui.horizontal_wrapped(|ui| {
                ui.checkbox(
                    &mut self.creative_fx_state.byte_bend.full_preview,
                    "Show full effect",
                );
                self.show_byte_bend_mask(ui, palette);
            });
            ui.horizontal(|ui| {
                let editor = &self.creative_fx_state.byte_bend;
                if let Some(hold) = &editor.held {
                    let edit = hold.edit;
                    let span = f32::from(edit.span) * 100.0 / 65_536.0;
                    ui.monospace(format!(
                        "{}  {span:.2}%  {}",
                        edit.mode.label(),
                        edit.variation
                    ));
                } else {
                    ui.monospace(format!(
                        "{}/{} edits",
                        editor.params.edits.len(),
                        MAX_BYTE_BEND_EDITS
                    ));
                }
            });

            changed |= ui
                .with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                    self.creative_fx_state.byte_bend.show_map(
                        ui,
                        &self.current_rgba,
                        self.pixel_revision,
                        accent,
                    )
                })
                .inner;
            if changed {
                self.creative_fx_state.mark_dirty();
                ui.ctx().request_repaint();
            }
        });
    }

    fn show_byte_bend_mask(&mut self, ui: &mut egui::Ui, palette: &AppPalette) {
        ui.menu_button("Mask", |ui| {
            let ctx = ui.ctx().clone();
            egui::ScrollArea::vertical()
                .max_height(320.0)
                .show(ui, |ui| {
                    ui.set_width(230.0);
                    self.render_creative_fx_mask_sources(ui, &ctx, palette, 230.0);
                    self.render_creative_fx_mask_options(
                        ui,
                        palette,
                        CreativeFxType::ByteBend,
                        230.0,
                        162.0,
                    );
                });
        });
    }
}
