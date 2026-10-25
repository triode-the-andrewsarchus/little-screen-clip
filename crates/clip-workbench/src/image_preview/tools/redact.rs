use super::super::*;
use std::process::Command;
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RedactToolType {
    #[default]
    BlackBar,
    AverageColor,
    LamaInpaint,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RedactionStyle {
    #[default]
    Black,
    AverageColor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InpaintSelectionMode {
    #[default]
    Rectangle,
    Brush,
    PointSam,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RedactionBox {
    /// Normalized coordinates in [0.0, 1.0] range relative to image
    pub normalized_rect: Rect,
    pub style: RedactionStyle,
}

impl Default for RedactionBox {
    fn default() -> Self {
        Self {
            normalized_rect: Rect::from_min_max(Pos2::ZERO, Pos2::ZERO),
            style: RedactionStyle::Black,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RedactionDrag {
    Creating(Pos2, Pos2),
    Moving(usize, Pos2, Rect),
    Resizing(usize, CropHandle, Pos2, Rect),
    InpaintRect(Pos2, Pos2, bool),
    InpaintBrush(bool),
}

#[derive(Debug, Clone)]
pub struct RedactionState {
    pub tool_type: RedactToolType,
    pub inpaint_mode: InpaintSelectionMode,
    pub boxes: Vec<RedactionBox>,
    pub selected_idx: Option<usize>,
    pub active_drag: Option<RedactionDrag>,

    pub inpaint_mask: Option<image::GrayImage>,
    pub brush_radius: f32,
    pub grow_selection_px: f32,
    pub last_brush_pos: Option<Pos2>,
    pub inpaint_in_progress: bool,
    pub inpaint_status: Option<String>,
    pub inpaint_error: Option<String>,
}

impl Default for RedactionState {
    fn default() -> Self {
        Self {
            tool_type: RedactToolType::BlackBar,
            inpaint_mode: InpaintSelectionMode::Rectangle,
            boxes: Vec::new(),
            selected_idx: None,
            active_drag: None,
            inpaint_mask: None,
            brush_radius: 24.0,
            grow_selection_px: 6.0,
            last_brush_pos: None,
            inpaint_in_progress: false,
            inpaint_status: None,
            inpaint_error: None,
        }
    }
}

#[derive(Debug)]
pub enum LamaInpaintMsg {
    Progress(String),
    Success {
        result_rgba: RgbaImage,
        duration_ms: u128,
    },
    Error(String),
}

const BAYER_4X4: [[f32; 4]; 4] = [
    [-0.5000, 0.0000, -0.3750, 0.1250],
    [0.2500, -0.2500, 0.3750, -0.1250],
    [-0.3125, 0.1875, -0.4375, 0.0625],
    [0.4375, -0.0625, 0.3125, -0.1875],
];

/// Extracts the 1D perimeter of a rectangle in clockwise order starting from top-left,
/// and applies a 1D circular median filter (window size 5) to eliminate rogue text pixels
/// touching the boundary.
fn extract_clean_perimeter(
    image: &RgbaImage,
    min_x: u32,
    max_x: u32,
    min_y: u32,
    max_y: u32,
) -> (Vec<[f32; 4]>, [f32; 4]) {
    if min_x >= max_x || min_y >= max_y {
        return (Vec::new(), [0.0, 0.0, 0.0, 255.0]);
    }

    let w = max_x - min_x;
    let h = max_y - min_y;
    let last_x = max_x.saturating_sub(1);
    let last_y = max_y.saturating_sub(1);

    let mut raw: Vec<[f32; 4]> = Vec::new();

    if w <= 2 || h <= 2 {
        for y in min_y..max_y {
            for x in min_x..max_x {
                let p = image.get_pixel(x, y);
                raw.push([p[0] as f32, p[1] as f32, p[2] as f32, p[3] as f32]);
            }
        }
    } else {
        // Clockwise perimeter:
        // 1. Top edge: (min_x..last_x, min_y)
        for x in min_x..last_x {
            let p = image.get_pixel(x, min_y);
            raw.push([p[0] as f32, p[1] as f32, p[2] as f32, p[3] as f32]);
        }
        // 2. Right edge: (last_x, min_y..last_y)
        for y in min_y..last_y {
            let p = image.get_pixel(last_x, y);
            raw.push([p[0] as f32, p[1] as f32, p[2] as f32, p[3] as f32]);
        }
        // 3. Bottom edge: (last_x down to min_x + 1, last_y)
        for x in (min_x + 1..=last_x).rev() {
            let p = image.get_pixel(x, last_y);
            raw.push([p[0] as f32, p[1] as f32, p[2] as f32, p[3] as f32]);
        }
        // 4. Left edge: (min_x, last_y down to min_y + 1)
        for y in (min_y + 1..=last_y).rev() {
            let p = image.get_pixel(min_x, y);
            raw.push([p[0] as f32, p[1] as f32, p[2] as f32, p[3] as f32]);
        }
    }

    let n = raw.len();
    if n == 0 {
        return (Vec::new(), [0.0, 0.0, 0.0, 255.0]);
    }

    // Apply circular median filter of window size 5 to reject rogue text spikes
    let mut clean = Vec::with_capacity(n);
    let mut sum_r = 0.0;
    let mut sum_g = 0.0;
    let mut sum_b = 0.0;
    let mut sum_a = 0.0;

    if n < 5 {
        for px in &raw {
            sum_r += px[0];
            sum_g += px[1];
            sum_b += px[2];
            sum_a += px[3];
            clean.push(*px);
        }
    } else {
        for i in 0..n {
            let mut wr = [0.0; 5];
            let mut wg = [0.0; 5];
            let mut wb = [0.0; 5];
            let mut wa = [0.0; 5];
            for (k, offset) in [-2, -1, 0, 1, 2].iter().enumerate() {
                let idx = ((i as isize + offset).rem_euclid(n as isize)) as usize;
                wr[k] = raw[idx][0];
                wg[k] = raw[idx][1];
                wb[k] = raw[idx][2];
                wa[k] = raw[idx][3];
            }
            wr.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            wg.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            wb.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            wa.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

            let med = [wr[2], wg[2], wb[2], wa[2]];
            sum_r += med[0];
            sum_g += med[1];
            sum_b += med[2];
            sum_a += med[3];
            clean.push(med);
        }
    }

    let count = clean.len() as f32;
    let center = [sum_r / count, sum_g / count, sum_b / count, sum_a / count];
    (clean, center)
}

/// Fills the specified rectangle with a 100% opaque, destructive synthetic gradient
/// projected inward from the perimeter edge background colors.
///
/// Under no circumstances does this redaction rely on blurring the underlying image pixels.
/// The original image data inside the box is completely overwritten and eliminated.
/// The "center blur" refers strictly to smoothing the apex of the synthetic conical gradient
/// itself (blending the incoming edge rays into their collective mean towards the geometric center)
/// to eliminate pinch/spoke artifacts where the rays meet.
pub fn fill_conical_edge_gradient_redaction(
    target: &mut RgbaImage,
    source: &RgbaImage,
    min_x: u32,
    max_x: u32,
    min_y: u32,
    max_y: u32,
) {
    if min_x >= max_x || min_y >= max_y {
        return;
    }

    let (perimeter, center) = extract_clean_perimeter(source, min_x, max_x, min_y, max_y);
    if perimeter.is_empty() {
        return;
    }

    let w = max_x - min_x;
    let h = max_y - min_y;

    if w <= 2 || h <= 2 {
        let cr = center[0].round().clamp(0.0, 255.0) as u8;
        let cg = center[1].round().clamp(0.0, 255.0) as u8;
        let cb = center[2].round().clamp(0.0, 255.0) as u8;
        let ca = center[3].round().clamp(0.0, 255.0) as u8;
        for y in min_y..max_y {
            for x in min_x..max_x {
                target.put_pixel(x, y, image::Rgba([cr, cg, cb, ca]));
            }
        }
        return;
    }

    let p_len = perimeter.len() as f32;
    let last_x = (max_x - 1) as f32;
    let last_y = (max_y - 1) as f32;
    let f_min_x = min_x as f32;
    let f_min_y = min_y as f32;

    let cx = (f_min_x + last_x) * 0.5;
    let cy = (f_min_y + last_y) * 0.5;
    let wh = (last_x - f_min_x) * 0.5;
    let hh = (last_y - f_min_y) * 0.5;

    let seg_top = (max_x - min_x - 1) as f32;
    let seg_right = (max_y - min_y - 1) as f32;
    let seg_bottom = seg_top;

    for y in min_y..max_y {
        let py = y as f32;
        let dy = py - cy;

        for x in min_x..max_x {
            let px = x as f32;
            let dx = px - cx;

            if dx.abs() < 1e-5 && dy.abs() < 1e-5 {
                let dither = BAYER_4X4[(y % 4) as usize][(x % 4) as usize];
                let r = (center[0] + dither).round().clamp(0.0, 255.0) as u8;
                let g = (center[1] + dither).round().clamp(0.0, 255.0) as u8;
                let b = (center[2] + dither).round().clamp(0.0, 255.0) as u8;
                let a = center[3].round().clamp(0.0, 255.0) as u8;
                target.put_pixel(x, y, image::Rgba([r, g, b, a]));
                continue;
            }

            let tx = if dx.abs() > 1e-5 { wh / dx.abs() } else { 1e9 };
            let ty = if dy.abs() > 1e-5 { hh / dy.abs() } else { 1e9 };
            let t = tx.min(ty);
            let d = (1.0 / t).clamp(0.0, 1.0);

            let ex = cx + t * dx;
            let ey = cy + t * dy;

            // Map (ex, ey) to perimeter index s
            let s = if (ey - f_min_y).abs() < 0.01 {
                ex - f_min_x
            } else if (ex - last_x).abs() < 0.01 {
                seg_top + (ey - f_min_y)
            } else if (ey - last_y).abs() < 0.01 {
                seg_top + seg_right + (last_x - ex)
            } else {
                seg_top + seg_right + seg_bottom + (last_y - ey)
            };

            let s_clamped = s.rem_euclid(p_len);
            let idx0 = (s_clamped.floor() as usize) % perimeter.len();
            let idx1 = (idx0 + 1) % perimeter.len();
            let frac = s_clamped - s_clamped.floor();

            let edge_r = (1.0 - frac) * perimeter[idx0][0] + frac * perimeter[idx1][0];
            let edge_g = (1.0 - frac) * perimeter[idx0][1] + frac * perimeter[idx1][1];
            let edge_b = (1.0 - frac) * perimeter[idx0][2] + frac * perimeter[idx1][2];
            let edge_a = (1.0 - frac) * perimeter[idx0][3] + frac * perimeter[idx1][3];

            // Center blur: smoothstep transition from edge to center
            let factor = d * d * (3.0 - 2.0 * d);
            let blended_r = factor * edge_r + (1.0 - factor) * center[0];
            let blended_g = factor * edge_g + (1.0 - factor) * center[1];
            let blended_b = factor * edge_b + (1.0 - factor) * center[2];
            let blended_a = factor * edge_a + (1.0 - factor) * center[3];

            let dither = BAYER_4X4[(y % 4) as usize][(x % 4) as usize];
            let r = (blended_r + dither).round().clamp(0.0, 255.0) as u8;
            let g = (blended_g + dither).round().clamp(0.0, 255.0) as u8;
            let b = (blended_b + dither).round().clamp(0.0, 255.0) as u8;
            let a = blended_a.round().clamp(0.0, 255.0) as u8;

            target.put_pixel(x, y, image::Rgba([r, g, b, a]));
        }
    }
}

/// Computes the representative background color of the perimeter edges
/// for the specified normalized rect.
pub fn compute_box_average_color(image: &RgbaImage, norm_rect: Rect) -> Color32 {
    let img_w = image.width() as f32;
    let img_h = image.height() as f32;
    let px_min_x = (norm_rect.min.x * img_w).floor().clamp(0.0, img_w) as u32;
    let px_max_x = (norm_rect.max.x * img_w).ceil().clamp(0.0, img_w) as u32;
    let px_min_y = (norm_rect.min.y * img_h).floor().clamp(0.0, img_h) as u32;
    let px_max_y = (norm_rect.max.y * img_h).ceil().clamp(0.0, img_h) as u32;

    let (_, center) = extract_clean_perimeter(image, px_min_x, px_max_x, px_min_y, px_max_y);
    Color32::from_rgba_unmultiplied(
        center[0].round().clamp(0.0, 255.0) as u8,
        center[1].round().clamp(0.0, 255.0) as u8,
        center[2].round().clamp(0.0, 255.0) as u8,
        center[3].round().clamp(0.0, 255.0) as u8,
    )
}

pub fn stamp_circle_on_mask(
    mask: &mut image::GrayImage,
    center_norm: Pos2,
    radius_px: f32,
    value: u8,
) {
    let (w, h) = mask.dimensions();
    let cx = center_norm.x * w as f32;
    let cy = center_norm.y * h as f32;
    let min_x = ((cx - radius_px).floor().max(0.0) as u32).min(w);
    let max_x = ((cx + radius_px).ceil().max(0.0) as u32).min(w);
    let min_y = ((cy - radius_px).floor().max(0.0) as u32).min(h);
    let max_y = ((cy + radius_px).ceil().max(0.0) as u32).min(h);
    let r2 = radius_px * radius_px;

    for y in min_y..max_y {
        let dy = y as f32 + 0.5 - cy;
        let dy2 = dy * dy;
        for x in min_x..max_x {
            let dx = x as f32 + 0.5 - cx;
            if dx * dx + dy2 <= r2 {
                mask.put_pixel(x, y, image::Luma([value]));
            }
        }
    }
}

pub fn stamp_line_on_mask(
    mask: &mut image::GrayImage,
    p0: Pos2,
    p1: Pos2,
    radius_px: f32,
    value: u8,
) {
    let (w, h) = mask.dimensions();
    let p0_px = Pos2::new(p0.x * w as f32, p0.y * h as f32);
    let p1_px = Pos2::new(p1.x * w as f32, p1.y * h as f32);
    let dist = p0_px.distance(p1_px);
    let step = (radius_px * 0.4).max(1.0);
    let n_steps = (dist / step).ceil() as usize;

    for i in 0..=n_steps {
        let t = if n_steps == 0 {
            0.0
        } else {
            i as f32 / n_steps as f32
        };
        let curr = Pos2::new(p0.x + t * (p1.x - p0.x), p0.y + t * (p1.y - p0.y));
        stamp_circle_on_mask(mask, curr, radius_px, value);
    }
}

pub fn stamp_rect_on_mask(mask: &mut image::GrayImage, rect_norm: Rect, value: u8) {
    let (w, h) = mask.dimensions();
    let min_x = (rect_norm.min.x.clamp(0.0, 1.0) * w as f32).round() as u32;
    let max_x = (rect_norm.max.x.clamp(0.0, 1.0) * w as f32).round() as u32;
    let min_y = (rect_norm.min.y.clamp(0.0, 1.0) * h as f32).round() as u32;
    let max_y = (rect_norm.max.y.clamp(0.0, 1.0) * h as f32).round() as u32;
    let (min_x, max_x) = (min_x.min(max_x), min_x.max(max_x));
    let (min_y, max_y) = (min_y.min(max_y), min_y.max(max_y));
    let max_x = max_x.min(w);
    let max_y = max_y.min(h);

    for y in min_y..max_y {
        for x in min_x..max_x {
            mask.put_pixel(x, y, image::Luma([value]));
        }
    }
}

impl ImagePreviewState {
    pub fn ensure_inpaint_mask(&mut self) -> &mut image::GrayImage {
        let (w, h) = self.current_rgba.dimensions();
        if self
            .redaction_state
            .inpaint_mask
            .as_ref()
            .map(|m| m.dimensions())
            != Some((w, h))
        {
            self.redaction_state.inpaint_mask = Some(image::GrayImage::new(w, h));
            self.inpaint_mask_texture = None;
        }
        self.redaction_state.inpaint_mask.as_mut().unwrap()
    }

    pub fn inpaint_has_mask(&self) -> bool {
        self.redaction_state
            .inpaint_mask
            .as_ref()
            .is_some_and(|m| m.pixels().any(|p| p[0] > 0))
    }

    pub fn clear_inpaint_mask(&mut self) {
        if let Some(mask) = self.redaction_state.inpaint_mask.as_mut() {
            mask.fill(0);
        }
        self.inpaint_mask_texture = None;
        self.sam_state.reset_hover();
        self.show_toast(crate::ui_ext::StatusMessage::new(
            crate::icons::Icon::Trash,
            "Cleared inpaint mask",
        ));
    }

    pub fn set_inpaint_mode(&mut self, mode: InpaintSelectionMode) {
        self.redaction_state.inpaint_mode = mode;
        if mode == InpaintSelectionMode::PointSam {
            self.ensure_sam_worker();
            self.sync_sam_image();
        } else {
            self.sam_state.reset_hover();
        }
    }

    pub fn ensure_sam_worker(&mut self) {
        if self.sam_state.worker.is_none() {
            match super::sam::SamWorkerHandle::spawn_native() {
                Ok(worker) => {
                    self.sam_state.worker = Some(worker);
                    self.sam_state.error_text = None;
                }
                Err(err) => {
                    self.sam_state.error_text = Some(err);
                }
            }
        }
    }

    pub fn sync_sam_image(&mut self) {
        self.ensure_sam_worker();
        if self.sam_state.loaded_revision != Some(self.pixel_revision) && !self.sam_state.is_loading
        {
            if let Some(ref worker) = self.sam_state.worker {
                if worker
                    .send(super::sam::SamWorkerCommand::EncodeImage {
                        image: self.current_rgba.clone(),
                        revision: self.pixel_revision,
                    })
                    .is_ok()
                {
                    self.sam_state.is_loading = true;
                    self.sam_state.error_text = None;
                }
            }
        }
    }

    pub fn poll_sam(&mut self, ctx: &egui::Context) {
        let mut msgs = Vec::new();
        let mut disconnected = false;
        if let Some(ref worker) = self.sam_state.worker {
            loop {
                match worker.try_recv() {
                    Ok(msg) => msgs.push(msg),
                    Err(std::sync::mpsc::TryRecvError::Empty) => break,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        disconnected = true;
                        break;
                    }
                }
            }
        }
        if disconnected {
            self.sam_state.worker = None;
            self.sam_state.is_loading = false;
            self.sam_state.in_flight = false;
        }

        for msg in msgs {
            match msg {
                super::sam::SamWorkerResponse::Ready => {
                    self.sam_state.is_ready = true;
                    self.sam_state.error_text = None;
                    self.sync_sam_image();
                }
                super::sam::SamWorkerResponse::ImageLoaded { revision } => {
                    self.sam_state.loaded_revision = Some(revision);
                    self.sam_state.is_loading = false;
                    self.sam_state.error_text = None;
                    if let Some(norm_pos) = self.sam_state.hovered_pos {
                        self.sam_state.last_query_norm_pos = None;
                        self.sam_on_hover(norm_pos, ctx);
                    }
                }
                super::sam::SamWorkerResponse::Mask { query_id, mask } => {
                    self.sam_state.in_flight = false;
                    if query_id == self.sam_state.latest_query_id {
                        self.sam_state.raw_hovered_mask = Some(mask.clone());
                        let final_mask = if self.redaction_state.grow_selection_px > 0.0 {
                            super::sam::dilate_binary_mask(
                                &mask,
                                self.redaction_state.grow_selection_px,
                            )
                        } else {
                            mask
                        };
                        super::sam::update_hovered_mask_texture(
                            &final_mask,
                            &mut self.sam_state.hovered_mask_texture,
                            ctx,
                        );
                        self.sam_state.hovered_mask = Some(final_mask);
                        ctx.request_repaint();

                        if let Some((_, val)) = self.sam_state.pending_click.take() {
                            self.commit_sam_hover(val, ctx);
                        }
                    }
                }
                super::sam::SamWorkerResponse::Error(err) => {
                    self.sam_state.error_text = Some(err);
                    self.sam_state.is_loading = false;
                    self.sam_state.in_flight = false;
                }
            }
        }
    }

    pub fn sam_on_hover(&mut self, norm_pos: Pos2, _ctx: &egui::Context) {
        if !self.sam_state.is_ready || self.sam_state.is_loading {
            self.sam_state.hovered_pos = Some(norm_pos);
            return;
        }

        if let Some(last_pos) = self.sam_state.last_query_norm_pos {
            let dx = (norm_pos.x - last_pos.x).abs();
            let dy = (norm_pos.y - last_pos.y).abs();
            if dx < 0.003 && dy < 0.003 {
                return;
            }
        }

        if self.sam_state.in_flight {
            return;
        }

        let px = norm_pos.x * self.current_rgba.width() as f32;
        let py = norm_pos.y * self.current_rgba.height() as f32;

        self.sam_state.latest_query_id = self.sam_state.latest_query_id.wrapping_add(1);
        let qid = self.sam_state.latest_query_id;

        if let Some(ref worker) = self.sam_state.worker {
            if worker
                .send(super::sam::SamWorkerCommand::PredictPoint {
                    x: px,
                    y: py,
                    query_id: qid,
                })
                .is_ok()
            {
                self.sam_state.in_flight = true;
                self.sam_state.last_query_norm_pos = Some(norm_pos);
                self.sam_state.hovered_pos = Some(norm_pos);
                self.sam_state.hovered_point_px = Some((px, py));
            }
        }
    }

    pub fn commit_sam_hover(&mut self, val: u8, ctx: &egui::Context) {
        if let Some(mask) = self.sam_state.hovered_mask.clone() {
            let target = self.ensure_inpaint_mask();
            super::sam::stamp_sam_mask_on_inpaint_mask(target, &mask, val);
            self.update_inpaint_mask_texture(ctx);
            if val > 0 {
                self.show_toast("✦ Object added to inpaint mask");
            } else {
                self.show_toast("Object erased from inpaint mask");
            }
        } else if let Some(norm_p) = self.sam_state.hovered_pos {
            self.sam_state.pending_click = Some((norm_p, val));
        }
    }

    pub fn refresh_sam_hover_preview(&mut self, ctx: &egui::Context) {
        if let Some(ref raw) = self.sam_state.raw_hovered_mask {
            let final_mask = if self.redaction_state.grow_selection_px > 0.0 {
                super::sam::dilate_binary_mask(raw, self.redaction_state.grow_selection_px)
            } else {
                raw.clone()
            };
            super::sam::update_hovered_mask_texture(
                &final_mask,
                &mut self.sam_state.hovered_mask_texture,
                ctx,
            );
            self.sam_state.hovered_mask = Some(final_mask);
            ctx.request_repaint();
        }
    }

    pub fn grow_current_inpaint_mask(&mut self, ctx: &egui::Context) {
        let grow_px = self.redaction_state.grow_selection_px.max(1.0);
        if let Some(mask) = self.redaction_state.inpaint_mask.as_mut() {
            let grown = super::sam::dilate_binary_mask(mask, grow_px);
            *mask = grown;
            self.update_inpaint_mask_texture(ctx);
            self.show_toast(format!(
                "Grew inpaint mask by {} px",
                grow_px.round() as u32
            ));
        }
    }

    pub fn adjust_brush_radius(&mut self, delta: f32) {
        let prev = self.redaction_state.brush_radius;
        let new_radius = (prev + delta).clamp(4.0, 128.0);
        if (new_radius - prev).abs() > 0.01 {
            self.redaction_state.brush_radius = new_radius;
            self.show_toast(format!(
                "Brush radius: {} px (diameter {} px)",
                new_radius.round() as u32,
                (new_radius * 2.0).round() as u32
            ));
        }
    }

    pub fn update_inpaint_mask_texture(&mut self, ctx: &egui::Context) {
        let Some(mask) = &self.redaction_state.inpaint_mask else {
            self.inpaint_mask_texture = None;
            return;
        };
        let (w, h) = mask.dimensions();
        let mut pixels = Vec::with_capacity((w * h) as usize);
        for p in mask.pixels() {
            if p[0] > 0 {
                // Semi-transparent magenta overlay
                pixels.push(Color32::from_rgba_unmultiplied(235, 30, 120, 140));
            } else {
                pixels.push(Color32::TRANSPARENT);
            }
        }
        let color_image = egui::ColorImage {
            size: [w as usize, h as usize],
            pixels,
        };
        if let Some(tex) = &mut self.inpaint_mask_texture {
            tex.set(color_image, egui::TextureOptions::NEAREST);
        } else {
            self.inpaint_mask_texture =
                Some(ctx.load_texture("inpaint_mask", color_image, egui::TextureOptions::NEAREST));
        }
    }

    pub fn apply_redactions(&mut self) {
        if self.redaction_state.boxes.is_empty() {
            return;
        }

        self.undo_stack.push(self.current_rgba.clone());
        self.redo_stack.clear();

        let count = self.redaction_state.boxes.len();
        self.current_rgba = Arc::new(compose_redactions_only(
            &self.current_rgba,
            &self.redaction_state.boxes,
        ));

        self.redaction_state.boxes.clear();
        self.redaction_state.selected_idx = None;
        self.redaction_state.active_drag = None;
        self.dirty_texture = true;
        self.mark_pixels_changed();

        // Automatically ensure metadata is stripped when redactions are permanently burned
        if let Ok(clean) = strip_metadata(&self.current_rgba, &self.mime) {
            self.raw_bytes = std::sync::Arc::new(clean);
            self.file_size_bytes = self.raw_bytes.len();
            self.metadata = None;
        }

        self.show_toast(format!("█ Burned {} redaction bars into image", count));
    }

    /// Deletes the currently selected redaction bar.
    pub fn delete_selected_redaction(&mut self) {
        if let Some(idx) = self.redaction_state.selected_idx {
            if idx < self.redaction_state.boxes.len() {
                self.redaction_state.boxes.remove(idx);
                self.redaction_state.selected_idx = None;
                self.redaction_state.active_drag = None;
                self.show_toast(crate::ui_ext::StatusMessage::new(
                    crate::icons::Icon::Trash,
                    "Redaction bar removed",
                ));
            }
        }
    }

    /// Clears all pending redaction bars without burning.
    pub fn clear_all_redactions(&mut self) {
        let count = self.redaction_state.boxes.len();
        if count > 0 {
            self.redaction_state.boxes.clear();
            self.redaction_state.selected_idx = None;
            self.redaction_state.active_drag = None;
            self.show_toast(crate::ui_ext::StatusMessage::new(
                crate::icons::Icon::Close,
                format!("Cleared {} pending redaction bars", count),
            ));
        }
    }

    pub(crate) fn render_redact_toolbar(&mut self, ui: &mut egui::Ui, palette: &AppPalette) {
        let btn_bg = hex_to_c32(&palette.colors.surface_panel);
        let btn_border = hex_to_c32(&palette.colors.surface_recessed);

        ui.vertical(|ui| {
            // Mode selector: Black Bar | Average Color | LaMa Inpaint
            ui.horizontal_wrapped(|ui| {
                let modes = [
                    (RedactToolType::BlackBar, "Black Bar"),
                    (RedactToolType::AverageColor, "Average Color"),
                    (RedactToolType::LamaInpaint, "LaMa Inpaint"),
                ];

                for (mode, label) in modes {
                    let is_active = self.redaction_state.tool_type == mode;
                    let fill = if is_active {
                        hex_to_c32(&palette.harmonies.base)
                    } else {
                        btn_bg
                    };
                    let stroke_col = if is_active {
                        hex_to_c32(&palette.harmonies.base)
                    } else {
                        btn_border
                    };
                    let text_col = if is_active {
                        contrast_text_c32(fill, None, palette)
                    } else {
                        hex_to_c32(&palette.colors.text_primary)
                    };

                    let btn = egui::Button::new(
                        egui::RichText::new(label)
                            .size(crate::ui_ext::BODY_FONT_SIZE)
                            .strong()
                            .color(text_col),
                    )
                    .fill(fill)
                    .stroke(Stroke::new(1.0_f32, stroke_col))
                    .rounding(Rounding::same(crate::ui_ext::CONTROL_CORNER_RADIUS))
                    .min_size(Vec2::new(0.0, crate::ui_ext::CONTROL_HEIGHT));

                    if ui.add(btn).clicked() {
                        self.redaction_state.tool_type = mode;
                    }
                }
            });

            ui.add_space(crate::ui_ext::SPACE_SM);

            match self.redaction_state.tool_type {
                RedactToolType::BlackBar | RedactToolType::AverageColor => {
                    let title = match self.redaction_state.tool_type {
                        RedactToolType::BlackBar => "Privacy Redaction (Black Bar):",
                        RedactToolType::AverageColor => "Color Redaction (Average Color):",
                        _ => unreachable!(),
                    };
                    ui.label(
                        egui::RichText::new(title)
                            .strong()
                            .size(crate::ui_ext::BODY_FONT_SIZE)
                            .color(hex_to_c32(&palette.diff_del)),
                    );

                    let box_count = self.redaction_state.boxes.len();
                    let count_str = if box_count == 0 {
                        "Drag on image to draw redaction box".to_string()
                    } else if box_count == 1 {
                        "1 bar placed".to_string()
                    } else {
                        format!("{} bars placed", box_count)
                    };
                    ui.label(
                        egui::RichText::new(count_str)
                            .size(crate::ui_ext::BODY_FONT_SIZE)
                            .color(hex_to_c32(&palette.colors.text_secondary)),
                    );

                    // Selected box style switcher
                    if let Some(sel_idx) = self.redaction_state.selected_idx {
                        if sel_idx < self.redaction_state.boxes.len() {
                            ui.add_space(crate::ui_ext::SPACE_XS);
                            ui.label(
                                egui::RichText::new("Selected Bar Style:")
                                    .size(crate::ui_ext::BODY_FONT_SIZE)
                                    .color(hex_to_c32(&palette.colors.text_secondary)),
                            );
                            ui.horizontal(|ui| {
                                let cur_style = self.redaction_state.boxes[sel_idx].style;
                                if ui
                                    .selectable_label(cur_style == RedactionStyle::Black, "Black")
                                    .clicked()
                                {
                                    self.redaction_state.boxes[sel_idx].style =
                                        RedactionStyle::Black;
                                }
                                if ui
                                    .selectable_label(
                                        cur_style == RedactionStyle::AverageColor,
                                        "Average Color",
                                    )
                                    .clicked()
                                {
                                    self.redaction_state.boxes[sel_idx].style =
                                        RedactionStyle::AverageColor;
                                }
                            });
                        }
                    }

                    ui.add_space(crate::ui_ext::SPACE_SM);

                    // Burn / Apply Redactions button
                    let has_boxes = !self.redaction_state.boxes.is_empty();
                    let burn_bg = if has_boxes {
                        hex_to_c32(&palette.diff_del)
                    } else {
                        btn_bg
                    };
                    let burn_btn = crate::ui_ext::action_button_icon(
                        Some(Icon::Check),
                        "Burn / Apply (Enter)",
                        burn_bg,
                        palette,
                    );

                    if ui
                        .add_enabled(has_boxes, burn_btn)
                        .on_hover_text("Permanently rasterize redactions into image pixels (Enter)")
                        .clicked()
                    {
                        self.apply_redactions();
                    }

                    // Delete Selected button
                    let has_selection = self.redaction_state.selected_idx.is_some();
                    let del_btn = crate::ui_ext::danger_button_icon(
                        Some(Icon::Trash),
                        "Delete Selected (Del)",
                        btn_bg,
                        palette,
                    );

                    if ui.add_enabled(has_selection, del_btn).clicked() {
                        self.delete_selected_redaction();
                    }

                    // Clear All button
                    let clear_btn = crate::ui_ext::secondary_button_icon(
                        Some(Icon::Close),
                        "Clear All",
                        btn_bg,
                        palette,
                    );

                    if ui.add_enabled(has_boxes, clear_btn).clicked() {
                        self.clear_all_redactions();
                    }

                    ui.add_space(crate::ui_ext::SPACE_SM);

                    // Strip Metadata / EXIF button
                    let has_meta = self.metadata.is_some();
                    let strip_btn = crate::ui_ext::secondary_button_icon(
                        Some(Icon::Shield),
                        "Strip Metadata / EXIF",
                        btn_bg,
                        palette,
                    );

                    if ui
                        .add(strip_btn)
                        .on_hover_text(if has_meta {
                            "Permanently remove camera, GPS, timestamps, and all EXIF tags from this image"
                        } else {
                            "Image is already clean (No EXIF metadata detected)"
                        })
                        .clicked()
                    {
                        self.strip_metadata();
                    }
                }
                RedactToolType::LamaInpaint => {
                    ui.label(
                        egui::RichText::new("LaMa AI Inpainting:")
                            .strong()
                            .size(crate::ui_ext::BODY_FONT_SIZE)
                            .color(hex_to_c32(&palette.harmonies.base)),
                    );

                    // Selection Mode Toggle: Block Select | Paint | Point (SAM)
                    ui.horizontal(|ui| {
                        let modes = [
                            (InpaintSelectionMode::Rectangle, "Block Select"),
                            (InpaintSelectionMode::Brush, "Paint"),
                            (InpaintSelectionMode::PointSam, "Point (SAM)"),
                        ];
                        for (mode, label) in modes {
                            let is_active = self.redaction_state.inpaint_mode == mode;
                            if ui.selectable_label(is_active, label).clicked() {
                                self.set_inpaint_mode(mode);
                            }
                        }
                    });

                    // If Brush mode: radius slider and shortcut reminder
                    if self.redaction_state.inpaint_mode == InpaintSelectionMode::Brush {
                        ui.add_space(crate::ui_ext::SPACE_XS);
                        ui.horizontal(|ui| {
                            ui.label("Brush Size:");
                            let mut rad = self.redaction_state.brush_radius;
                            if ui
                                .add(
                                    egui::Slider::new(&mut rad, 4.0..=128.0)
                                        .step_by(1.0)
                                        .text("px"),
                                )
                                .changed()
                            {
                                self.redaction_state.brush_radius = rad;
                            }
                        });
                        ui.label(
                            egui::RichText::new(
                                "Tip: (+/=) increase size, (-) decrease size\nRight-click drag to erase",
                            )
                            .size(crate::ui_ext::BODY_FONT_SIZE - 2.0)
                            .color(hex_to_c32(&palette.colors.text_secondary)),
                        );
                    } else if self.redaction_state.inpaint_mode == InpaintSelectionMode::PointSam {
                        ui.add_space(crate::ui_ext::SPACE_XS);
                        if self.sam_state.is_loading {
                            ui.label(
                                egui::RichText::new("Loading Segment Anything neural network...")
                                    .italics()
                                    .size(crate::ui_ext::BODY_FONT_SIZE - 2.0)
                                    .color(hex_to_c32(&palette.colors.text_secondary)),
                            );
                        } else if let Some(ref err) = self.sam_state.error_text {
                            ui.colored_label(hex_to_c32(&palette.diff_del), format!("SAM: {err}"));
                        } else {
                            ui.label(
                                egui::RichText::new(
                                    "Tip: Hover to preview object (blue)\nClick to add to inpaint mask; Right-click to erase",
                                )
                                .size(crate::ui_ext::BODY_FONT_SIZE - 2.0)
                                .color(hex_to_c32(&palette.colors.text_secondary)),
                            );
                        }
                    }

                    ui.add_space(crate::ui_ext::SPACE_XS);
                    ui.horizontal(|ui| {
                        ui.label("Grow Selection:");
                        let mut grow = self.redaction_state.grow_selection_px;
                        if ui
                            .add(
                                egui::Slider::new(&mut grow, 0.0..=64.0)
                                    .step_by(1.0)
                                    .text("px"),
                            )
                            .on_hover_text(
                                "Expand selection mask outward by N pixels to give inpainting room to work with",
                            )
                            .changed()
                        {
                            self.redaction_state.grow_selection_px = grow;
                            self.refresh_sam_hover_preview(ui.ctx());
                        }
                    });

                    ui.add_space(crate::ui_ext::SPACE_SM);

                    let has_mask = self.inpaint_has_mask();
                    let in_progress = self.redaction_state.inpaint_in_progress;

                    // Inpaint Run Button
                    let run_label = if in_progress {
                        "Inpainting... (Running LaMa)"
                    } else {
                        "Inpaint with LaMa (Enter)"
                    };
                    let inpaint_btn = crate::ui_ext::action_button_icon(
                        Some(Icon::Wand),
                        run_label,
                        hex_to_c32(&palette.harmonies.base),
                        palette,
                    );

                    if ui
                        .add_enabled(has_mask && !in_progress, inpaint_btn)
                        .on_hover_text(
                            "Remove covered objects and fill background using LaMa AI (Enter)",
                        )
                        .clicked()
                    {
                        self.run_lama_inpaint();
                    }

                    // Action buttons: Clear Mask & Grow Mask
                    ui.horizontal(|ui| {
                        let clear_mask_btn = crate::ui_ext::secondary_button_icon(
                            Some(Icon::Close),
                            "Clear Mask",
                            btn_bg,
                            palette,
                        );
                        if ui.add_enabled(has_mask, clear_mask_btn).clicked() {
                            self.clear_inpaint_mask();
                        }

                        let grow_mask_btn = crate::ui_ext::secondary_button_icon(
                            Some(Icon::Plus),
                            "Grow Mask",
                            btn_bg,
                            palette,
                        );
                        if ui
                            .add_enabled(has_mask && !in_progress, grow_mask_btn)
                            .on_hover_text(
                                "Expand current inpaint mask by the grow-selection slider radius",
                            )
                            .clicked()
                        {
                            self.grow_current_inpaint_mask(ui.ctx());
                        }
                    });

                    if let Some(ref st) = self.redaction_state.inpaint_status {
                        ui.add_space(crate::ui_ext::SPACE_XS);
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label(
                                egui::RichText::new(st)
                                    .size(crate::ui_ext::BODY_FONT_SIZE - 2.0)
                                    .color(hex_to_c32(&palette.colors.text_secondary)),
                            );
                        });
                    }

                    if let Some(ref err) = self.redaction_state.inpaint_error {
                        ui.add_space(crate::ui_ext::SPACE_XS);
                        ui.label(
                            egui::RichText::new(err)
                                .size(crate::ui_ext::BODY_FONT_SIZE - 2.0)
                                .color(hex_to_c32(&palette.diff_del)),
                        );
                    }
                }
            }

            // Undo button
            ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                if !self.undo_stack.is_empty() {
                    let undo_fg =
                        contrast_text_c32(btn_bg, Some(hex_to_c32(&palette.diff_mod)), palette);
                    let undo_btn = egui::Button::new(
                        egui::RichText::new("⟲ Undo (Ctrl+Z)")
                            .size(crate::ui_ext::BODY_FONT_SIZE)
                            .strong()
                            .color(undo_fg),
                    )
                    .fill(btn_bg)
                    .stroke(Stroke::new(1.0_f32, btn_border))
                    .rounding(Rounding::same(crate::ui_ext::CONTROL_CORNER_RADIUS))
                    .min_size(Vec2::new(0.0, crate::ui_ext::CONTROL_HEIGHT));
                    if ui.add(undo_btn).clicked() {
                        self.undo();
                    }
                }
            });
        });
    }

    pub(crate) fn render_redact_overlay(
        &mut self,
        ui: &mut egui::Ui,
        painter: &egui::Painter,
        response: &egui::Response,
        img_rect: Rect,
        palette: &AppPalette,
    ) {
        let img_w = self.current_rgba.width();
        let img_h = self.current_rgba.height();
        let handle_radius = 5.0;
        let hit_dist = 12.0;

        let to_screen = |norm_p: Pos2| -> Pos2 {
            Pos2::new(
                img_rect.min.x + norm_p.x * img_rect.width(),
                img_rect.min.y + norm_p.y * img_rect.height(),
            )
        };
        let to_screen_rect = |norm_r: Rect| -> Rect {
            Rect::from_min_max(to_screen(norm_r.min), to_screen(norm_r.max))
        };
        let to_norm = |screen_p: Pos2| -> Pos2 {
            Pos2::new(
                ((screen_p.x - img_rect.min.x) / img_rect.width()).clamp(0.0, 1.0),
                ((screen_p.y - img_rect.min.y) / img_rect.height()).clamp(0.0, 1.0),
            )
        };

        let diff_del = hex_to_c32(&palette.diff_del);
        let b_low_badge = hex_to_c32_alpha(&palette.colors.surface_recessed, 230);
        let badge_text_col = contrast_text_c32(
            b_low_badge,
            Some(hex_to_c32(&palette.colors.text_primary)),
            palette,
        );

        // 1. Paint inpainting mask texture if present
        if let Some(ref tex) = self.inpaint_mask_texture {
            painter.image(
                tex.id(),
                img_rect,
                Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                Color32::WHITE,
            );
        }

        // 1b. Paint Segment Anything hovered object preview (transparent blue) if active
        if self.redaction_state.tool_type == RedactToolType::LamaInpaint
            && self.redaction_state.inpaint_mode == InpaintSelectionMode::PointSam
        {
            if let Some(ref tex) = self.sam_state.hovered_mask_texture {
                painter.image(
                    tex.id(),
                    img_rect,
                    Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                    Color32::WHITE,
                );
            }
            if let Some(norm_p) = self.sam_state.hovered_pos {
                let screen_p = to_screen(norm_p);
                painter.circle_filled(screen_p, 3.5, Color32::from_rgb(30, 144, 255));
                painter.circle_stroke(screen_p, 5.5, Stroke::new(1.5, Color32::WHITE));
            }
        }

        // 2. Draw existing redaction boxes
        for (i, b) in self.redaction_state.boxes.iter().enumerate() {
            let box_screen_rect = to_screen_rect(b.normalized_rect);

            let fill_color = match b.style {
                RedactionStyle::Black => Color32::BLACK,
                RedactionStyle::AverageColor => {
                    compute_box_average_color(&self.current_rgba, b.normalized_rect)
                }
            };

            painter.rect_filled(box_screen_rect, Rounding::ZERO, fill_color);

            let is_selected = self.redaction_state.selected_idx == Some(i);
            if is_selected {
                painter.rect_stroke(
                    box_screen_rect,
                    Rounding::ZERO,
                    Stroke::new(2.0_f32, diff_del),
                );

                let handles = get_box_handles(box_screen_rect);
                for (_, pos, _) in &handles {
                    let handle_rect =
                        Rect::from_center_size(*pos, Vec2::splat(handle_radius * 2.0));
                    painter.rect_filled(
                        handle_rect,
                        Rounding::same(1.0),
                        hex_to_c32(&palette.colors.text_primary),
                    );
                    painter.rect_stroke(
                        handle_rect,
                        Rounding::same(1.0),
                        Stroke::new(1.0_f32, hex_to_c32(&palette.colors.surface_recessed)),
                    );
                }

                let px_w = (b.normalized_rect.width() * img_w as f32).round() as u32;
                let px_h = (b.normalized_rect.height() * img_h as f32).round() as u32;
                let style_desc = match b.style {
                    RedactionStyle::Black => "Black",
                    RedactionStyle::AverageColor => "Avg Color",
                };
                let badge_str = format!("{} × {} px ({})", px_w, px_h, style_desc);
                let badge_pos = Pos2::new(
                    box_screen_rect.center().x,
                    (box_screen_rect.min.y - 14.0).max(img_rect.min.y + 10.0),
                );
                let galley = ui.painter().layout_no_wrap(
                    badge_str,
                    egui::FontId::proportional(crate::ui_ext::BODY_FONT_SIZE),
                    badge_text_col,
                );
                let badge_size = galley.size() + Vec2::new(12.0, 4.0);
                let ideal_badge_min = badge_pos - badge_size * 0.5;
                let badge_rect =
                    crate::ui_ext::clamp_overlay_rect(ideal_badge_min, badge_size, img_rect);
                painter.rect_filled(badge_rect, Rounding::same(4.0), b_low_badge);
                painter.rect_stroke(
                    badge_rect,
                    Rounding::same(4.0),
                    Stroke::new(1.0_f32, diff_del),
                );
                painter.galley(badge_rect.min + Vec2::new(6.0, 2.0), galley, badge_text_col);
            } else {
                painter.rect_stroke(
                    box_screen_rect,
                    Rounding::ZERO,
                    Stroke::new(
                        1.0_f32,
                        hex_to_c32_alpha(&palette.colors.text_secondary, 60),
                    ),
                );
            }
        }

        // 3. Draw in-progress creating box / inpaint rectangle
        match self.redaction_state.active_drag {
            Some(RedactionDrag::Creating(start_norm, curr_norm)) => {
                let start_s = to_screen(start_norm);
                let curr_s = to_screen(curr_norm);
                let in_progress_rect = Rect::from_two_pos(start_s, curr_s);
                let norm_r = Rect::from_two_pos(start_norm, curr_norm);

                let fill = match self.redaction_state.tool_type {
                    RedactToolType::BlackBar => Color32::from_rgba_unmultiplied(0, 0, 0, 210),
                    RedactToolType::AverageColor => {
                        let avg = compute_box_average_color(&self.current_rgba, norm_r);
                        Color32::from_rgba_unmultiplied(avg.r(), avg.g(), avg.b(), 210)
                    }
                    _ => Color32::from_rgba_unmultiplied(0, 0, 0, 210),
                };

                painter.rect_filled(in_progress_rect, Rounding::ZERO, fill);
                painter.rect_stroke(
                    in_progress_rect,
                    Rounding::ZERO,
                    Stroke::new(1.5_f32, diff_del),
                );
            }
            Some(RedactionDrag::InpaintRect(start_norm, curr_norm, is_erase)) => {
                let start_s = to_screen(start_norm);
                let curr_s = to_screen(curr_norm);
                let in_progress_rect = Rect::from_two_pos(start_s, curr_s);
                let fill = if is_erase {
                    Color32::from_rgba_unmultiplied(60, 60, 60, 120)
                } else {
                    Color32::from_rgba_unmultiplied(235, 30, 120, 140)
                };
                let stroke_color = if is_erase {
                    Color32::WHITE
                } else {
                    hex_to_c32(&palette.harmonies.base)
                };
                painter.rect_filled(in_progress_rect, Rounding::ZERO, fill);
                painter.rect_stroke(
                    in_progress_rect,
                    Rounding::ZERO,
                    Stroke::new(1.5_f32, stroke_color),
                );
            }
            _ => {}
        }

        // 4. Cursor rendering & icon
        if let Some(ptr_pos) = response.hover_pos() {
            if self.redaction_state.tool_type == RedactToolType::LamaInpaint {
                match self.redaction_state.inpaint_mode {
                    InpaintSelectionMode::Rectangle => {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
                    }
                    InpaintSelectionMode::Brush => {
                        // Render circular brush outline at pointer position
                        if img_rect.contains(ptr_pos) {
                            ui.ctx().set_cursor_icon(egui::CursorIcon::None);
                            let scale_x = img_rect.width() / img_w as f32;
                            let screen_radius = self.redaction_state.brush_radius * scale_x;
                            // High contrast double circle
                            painter.circle_stroke(
                                ptr_pos,
                                screen_radius + 0.5,
                                Stroke::new(1.5_f32, Color32::BLACK),
                            );
                            painter.circle_stroke(
                                ptr_pos,
                                screen_radius,
                                Stroke::new(1.0_f32, Color32::WHITE),
                            );
                        } else {
                            ui.ctx().set_cursor_icon(egui::CursorIcon::Default);
                        }
                    }
                    InpaintSelectionMode::PointSam => {
                        if img_rect.contains(ptr_pos) {
                            ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
                            let norm = to_norm(ptr_pos);
                            self.sam_on_hover(norm, ui.ctx());
                        } else {
                            ui.ctx().set_cursor_icon(egui::CursorIcon::Default);
                            self.sam_state.reset_hover();
                        }
                    }
                }
            } else if let Some(selected_idx) = self.redaction_state.selected_idx {
                if selected_idx < self.redaction_state.boxes.len() {
                    let box_screen_rect =
                        to_screen_rect(self.redaction_state.boxes[selected_idx].normalized_rect);
                    let handles = get_box_handles(box_screen_rect);
                    let hovered_handle = handles.iter().find_map(|(_, pos, cur)| {
                        if pos.distance(ptr_pos) <= hit_dist {
                            Some(*cur)
                        } else {
                            None
                        }
                    });

                    if let Some(cur) = hovered_handle {
                        ui.ctx().set_cursor_icon(cur);
                    } else if box_screen_rect.contains(ptr_pos) {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::Move);
                    } else {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
                    }
                } else {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
                }
            } else {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
            }
        }

        // 5. Handle Pointer & Drag Events
        let is_secondary_down = ui.input(|i| i.pointer.button_down(egui::PointerButton::Secondary));

        if response.drag_started() {
            if let Some(ptr) = response.interact_pointer_pos() {
                if self.redaction_state.tool_type == RedactToolType::LamaInpaint {
                    match self.redaction_state.inpaint_mode {
                        InpaintSelectionMode::Rectangle => {
                            if img_rect.contains(ptr) {
                                let norm = to_norm(ptr);
                                self.redaction_state.active_drag =
                                    Some(RedactionDrag::InpaintRect(norm, norm, is_secondary_down));
                            }
                        }
                        InpaintSelectionMode::Brush => {
                            if img_rect.contains(ptr) {
                                let norm = to_norm(ptr);
                                self.redaction_state.active_drag =
                                    Some(RedactionDrag::InpaintBrush(is_secondary_down));
                                self.redaction_state.last_brush_pos = Some(norm);
                                let val = if is_secondary_down { 0 } else { 255 };
                                let radius = self.redaction_state.brush_radius;
                                {
                                    let mask = self.ensure_inpaint_mask();
                                    stamp_circle_on_mask(mask, norm, radius, val);
                                }
                                self.update_inpaint_mask_texture(ui.ctx());
                            }
                        }
                        InpaintSelectionMode::PointSam => {}
                    }
                } else {
                    let mut handled = false;
                    if let Some(sel_idx) = self.redaction_state.selected_idx {
                        if sel_idx < self.redaction_state.boxes.len() {
                            let init_norm = self.redaction_state.boxes[sel_idx].normalized_rect;
                            let box_screen_rect = to_screen_rect(init_norm);
                            let handles = get_box_handles(box_screen_rect);

                            if let Some((h, _, _)) = handles
                                .iter()
                                .find(|(_, pos, _)| pos.distance(ptr) <= hit_dist)
                            {
                                self.redaction_state.active_drag =
                                    Some(RedactionDrag::Resizing(sel_idx, *h, ptr, init_norm));
                                handled = true;
                            } else if box_screen_rect.contains(ptr) {
                                self.redaction_state.active_drag =
                                    Some(RedactionDrag::Moving(sel_idx, ptr, init_norm));
                                handled = true;
                            }
                        }
                    }

                    if !handled {
                        let found_idx = self
                            .redaction_state
                            .boxes
                            .iter()
                            .enumerate()
                            .rev()
                            .find_map(|(i, b)| {
                                let r = to_screen_rect(b.normalized_rect);
                                if r.contains(ptr) { Some(i) } else { None }
                            });

                        if let Some(idx) = found_idx {
                            self.redaction_state.selected_idx = Some(idx);
                            let init_norm = self.redaction_state.boxes[idx].normalized_rect;
                            self.redaction_state.active_drag =
                                Some(RedactionDrag::Moving(idx, ptr, init_norm));
                        } else if img_rect.contains(ptr) {
                            let start_norm = to_norm(ptr);
                            self.redaction_state.selected_idx = None;
                            self.redaction_state.active_drag =
                                Some(RedactionDrag::Creating(start_norm, start_norm));
                        }
                    }
                }
            }
        }

        if response.dragged() {
            if let Some(ptr) = response.interact_pointer_pos() {
                match self.redaction_state.active_drag {
                    Some(RedactionDrag::Creating(start_norm, _)) => {
                        let curr_norm = to_norm(ptr);
                        self.redaction_state.active_drag =
                            Some(RedactionDrag::Creating(start_norm, curr_norm));
                    }
                    Some(RedactionDrag::InpaintRect(start_norm, _, is_erase)) => {
                        let curr_norm = to_norm(ptr);
                        self.redaction_state.active_drag =
                            Some(RedactionDrag::InpaintRect(start_norm, curr_norm, is_erase));
                    }
                    Some(RedactionDrag::InpaintBrush(is_erase)) => {
                        let curr_norm = to_norm(ptr);
                        let radius = self.redaction_state.brush_radius;
                        let val = if is_erase { 0 } else { 255 };
                        if let Some(last_pos) = self.redaction_state.last_brush_pos {
                            let mask = self.ensure_inpaint_mask();
                            stamp_line_on_mask(mask, last_pos, curr_norm, radius, val);
                        }
                        self.redaction_state.last_brush_pos = Some(curr_norm);
                        self.update_inpaint_mask_texture(ui.ctx());
                    }
                    Some(RedactionDrag::Moving(idx, start_ptr, init_norm)) => {
                        let delta_s = ptr - start_ptr;
                        let delta_norm_x = delta_s.x / img_rect.width();
                        let delta_norm_y = delta_s.y / img_rect.height();
                        let box_w = init_norm.width();
                        let box_h = init_norm.height();
                        let new_min_x = (init_norm.min.x + delta_norm_x).clamp(0.0, 1.0 - box_w);
                        let new_min_y = (init_norm.min.y + delta_norm_y).clamp(0.0, 1.0 - box_h);
                        if idx < self.redaction_state.boxes.len() {
                            self.redaction_state.boxes[idx].normalized_rect = Rect::from_min_size(
                                Pos2::new(new_min_x, new_min_y),
                                Vec2::new(box_w, box_h),
                            );
                        }
                    }
                    Some(RedactionDrag::Resizing(idx, handle, start_ptr, init_norm)) => {
                        let delta_s = ptr - start_ptr;
                        let min_w = 4.0 / img_w as f32;
                        let min_h = 4.0 / img_h as f32;
                        let new_rect =
                            resize_norm_rect(init_norm, handle, delta_s, img_rect, min_w, min_h);
                        if idx < self.redaction_state.boxes.len() {
                            self.redaction_state.boxes[idx].normalized_rect = new_rect;
                        }
                    }
                    None => {}
                }
            }
        }

        if response.drag_stopped() {
            match self.redaction_state.active_drag {
                Some(RedactionDrag::Creating(start_norm, end_norm)) => {
                    let min_x = start_norm.x.min(end_norm.x);
                    let max_x = start_norm.x.max(end_norm.x);
                    let min_y = start_norm.y.min(end_norm.y);
                    let max_y = start_norm.y.max(end_norm.y);
                    let norm_w = max_x - min_x;
                    let norm_h = max_y - min_y;

                    let min_norm_w = 4.0 / img_w as f32;
                    let min_norm_h = 4.0 / img_h as f32;
                    if norm_w >= min_norm_w && norm_h >= min_norm_h {
                        let style = match self.redaction_state.tool_type {
                            RedactToolType::AverageColor => RedactionStyle::AverageColor,
                            _ => RedactionStyle::Black,
                        };
                        self.redaction_state.boxes.push(RedactionBox {
                            normalized_rect: Rect::from_min_max(
                                Pos2::new(min_x, min_y),
                                Pos2::new(max_x, max_y),
                            ),
                            style,
                        });
                        self.redaction_state.selected_idx =
                            Some(self.redaction_state.boxes.len() - 1);
                    }
                }
                Some(RedactionDrag::InpaintRect(start_norm, end_norm, is_erase)) => {
                    let min_x = start_norm.x.min(end_norm.x);
                    let max_x = start_norm.x.max(end_norm.x);
                    let min_y = start_norm.y.min(end_norm.y);
                    let max_y = start_norm.y.max(end_norm.y);
                    let norm_r =
                        Rect::from_min_max(Pos2::new(min_x, min_y), Pos2::new(max_x, max_y));
                    let val = if is_erase { 0 } else { 255 };
                    {
                        let mask = self.ensure_inpaint_mask();
                        stamp_rect_on_mask(mask, norm_r, val);
                    }
                    self.update_inpaint_mask_texture(ui.ctx());
                }
                _ => {}
            }
            self.redaction_state.active_drag = None;
            self.redaction_state.last_brush_pos = None;
        }

        // Clicking without dragging on empty background deselects in box mode
        if response.clicked() && self.redaction_state.tool_type != RedactToolType::LamaInpaint {
            if let Some(ptr) = response.interact_pointer_pos() {
                let hit_any = self
                    .redaction_state
                    .boxes
                    .iter()
                    .enumerate()
                    .rev()
                    .find_map(|(i, b)| {
                        if to_screen_rect(b.normalized_rect).contains(ptr) {
                            Some(i)
                        } else {
                            None
                        }
                    });
                self.redaction_state.selected_idx = hit_any;
            }
        }

        // Point-to-select SAM mask click actions
        if self.redaction_state.tool_type == RedactToolType::LamaInpaint
            && self.redaction_state.inpaint_mode == InpaintSelectionMode::PointSam
        {
            if response.clicked() {
                self.commit_sam_hover(255, ui.ctx());
            } else if response.secondary_clicked() {
                self.commit_sam_hover(0, ui.ctx());
            }
        }
    }

    /// Triggers LaMa inpainting on background worker.
    pub fn run_lama_inpaint(&mut self) {
        self.run_lama_inpaint_on(BackgroundJobs::global());
    }

    pub fn run_lama_inpaint_on(&mut self, jobs: &BackgroundJobs) {
        if self.redaction_state.inpaint_in_progress {
            return;
        }

        if !self.inpaint_has_mask() {
            self.show_toast(crate::ui_ext::StatusMessage::new(
                crate::icons::Icon::Alert,
                "Draw an inpainting mask first (brush or rectangle)",
            ));
            return;
        }

        if !self.redaction_state.boxes.is_empty() {
            self.apply_redactions();
        }

        let current_image = self.current_rgba.as_ref().clone();
        let mask_image = match self.redaction_state.inpaint_mask {
            Some(ref m) => m.clone(),
            None => return,
        };

        let (cmd, args) = if let Some(ref custom_cmd) = self.inpaint_cmd {
            let parts: Vec<String> = custom_cmd
                .split_whitespace()
                .map(|s| s.to_string())
                .collect();
            if parts.is_empty() {
                ("scripts/inpaint_lama.py".to_string(), Vec::new())
            } else {
                (parts[0].clone(), parts[1..].to_vec())
            }
        } else if std::path::Path::new("scripts/inpaint_lama.py").exists() {
            ("scripts/inpaint_lama.py".to_string(), Vec::new())
        } else {
            (
                "python3".to_string(),
                vec!["scripts/inpaint_lama.py".to_string()],
            )
        };

        let (tx, rx) = std::sync::mpsc::channel::<LamaInpaintMsg>();
        self.inpaint_rx = Some(rx);
        self.redaction_state.inpaint_in_progress = true;
        self.redaction_state.inpaint_status = Some("Starting LaMa inpainting...".to_string());
        self.redaction_state.inpaint_error = None;

        let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
        self.inpaint_cancel = Some(cancel.clone());
        self.inpaint_child_pid = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let child_pid = self.inpaint_child_pid.clone();
        let lama_url = self.lama_model_url.clone();

        let (ticket, job_rx) = jobs.submit(std::time::Duration::MAX, move || {
            let start = std::time::Instant::now();
            let _ = tx.send(LamaInpaintMsg::Progress(
                "Running LaMa inpainting neural network...".to_string(),
            ));

            // Priority 1: Native Rust ORT inference if model is available
            let model_path = crate::onnx_runtime::ensure_model(
                crate::onnx_runtime::BIG_LAMA_MODEL_NAME,
                lama_url.as_deref(),
            )
            .ok();

            if let Some(ref model_path) = model_path {
                match run_native_lama_inpaint(&current_image, &mask_image, model_path) {
                    Ok(result_rgba) => {
                        let _ = tx.send(LamaInpaintMsg::Success {
                            result_rgba,
                            duration_ms: start.elapsed().as_millis(),
                        });
                        return;
                    }
                    Err(native_err) => {
                        let _ = tx.send(LamaInpaintMsg::Progress(format!(
                            "Native ORT failed ({native_err}), trying fallback..."
                        )));
                    }
                }
            }

            // Priority 2: Fallback to scripts/inpaint_lama.py
            let temp_in =
                std::env::temp_dir().join(format!("clip_inpaint_in_{}.png", uuid::Uuid::new_v4()));
            let temp_mask = std::env::temp_dir()
                .join(format!("clip_inpaint_mask_{}.png", uuid::Uuid::new_v4()));
            let temp_out =
                std::env::temp_dir().join(format!("clip_inpaint_out_{}.png", uuid::Uuid::new_v4()));

            if let Err(err) = current_image.save_with_format(&temp_in, image::ImageFormat::Png) {
                let _ = tx.send(LamaInpaintMsg::Error(format!(
                    "Failed to save temp image: {err}"
                )));
                return;
            }

            if let Err(err) = mask_image.save_with_format(&temp_mask, image::ImageFormat::Png) {
                let _ = std::fs::remove_file(&temp_in);
                let _ = tx.send(LamaInpaintMsg::Error(format!(
                    "Failed to save temp mask: {err}"
                )));
                return;
            }

            let mut process_cmd = Command::new(&cmd);
            process_cmd.args(&args);
            if let Some(ref path) = model_path {
                process_cmd.arg("--model").arg(path);
            } else if let Some(ref url) = lama_url {
                process_cmd.arg("--model-url").arg(url);
                process_cmd.env("LAMA_MODEL_URL", url);
            }
            process_cmd.arg(&temp_in);
            process_cmd.arg(&temp_mask);
            process_cmd.arg(&temp_out);

            #[cfg(unix)]
            {
                use std::os::unix::process::CommandExt;
                process_cmd.process_group(0);
            }

            let mut child = match process_cmd
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
            {
                Ok(child) => child,
                Err(err) => {
                    let _ = std::fs::remove_file(&temp_in);
                    let _ = std::fs::remove_file(&temp_mask);
                    let _ = std::fs::remove_file(&temp_out);
                    let _ = tx.send(LamaInpaintMsg::Error(format!(
                        "Failed to launch inpainting process '{cmd}': {err}"
                    )));
                    return;
                }
            };

            let pid = child.id();
            child_pid.store(pid, std::sync::atomic::Ordering::SeqCst);

            let status = child.wait();
            let _ = std::fs::remove_file(&temp_in);
            let _ = std::fs::remove_file(&temp_mask);

            if cancel.load(std::sync::atomic::Ordering::SeqCst) {
                let _ = std::fs::remove_file(&temp_out);
                let _ = tx.send(LamaInpaintMsg::Error("Inpainting cancelled".to_string()));
                return;
            }

            match status {
                Ok(st) if st.success() => match image::open(&temp_out) {
                    Ok(dyn_img) => {
                        let _ = std::fs::remove_file(&temp_out);
                        let _ = tx.send(LamaInpaintMsg::Success {
                            result_rgba: dyn_img.to_rgba8(),
                            duration_ms: start.elapsed().as_millis(),
                        });
                    }
                    Err(err) => {
                        let _ = std::fs::remove_file(&temp_out);
                        let _ = tx.send(LamaInpaintMsg::Error(format!(
                            "Failed to read inpaint output: {err}"
                        )));
                    }
                },
                Ok(st) => {
                    let mut stderr_msg = String::new();
                    if let Some(mut err_pipe) = child.stderr.take() {
                        use std::io::Read;
                        let _ = err_pipe.read_to_string(&mut stderr_msg);
                    }
                    let _ = std::fs::remove_file(&temp_out);
                    let _ = tx.send(LamaInpaintMsg::Error(format!(
                        "LaMa process exited with code {:?}: {}",
                        st.code(),
                        stderr_msg.trim()
                    )));
                }
                Err(err) => {
                    let _ = std::fs::remove_file(&temp_out);
                    let _ = tx.send(LamaInpaintMsg::Error(format!("Worker failed: {err}")));
                }
            }
        });

        self.inpaint_ticket = Some(ticket);
        self.inpaint_job_rx = Some(job_rx);
    }

    pub fn poll_lama_inpaint(&mut self) {
        self.poll_lama_inpaint_on(BackgroundJobs::global());
    }

    pub fn poll_lama_inpaint_on(&mut self, _jobs: &BackgroundJobs) {
        let mut success = None;
        let mut error = None;

        if let Some(rx) = &self.inpaint_rx {
            loop {
                match rx.try_recv() {
                    Ok(LamaInpaintMsg::Progress(st)) => {
                        self.redaction_state.inpaint_status = Some(st);
                    }
                    Ok(LamaInpaintMsg::Success {
                        result_rgba,
                        duration_ms,
                    }) => {
                        success = Some((result_rgba, duration_ms));
                        break;
                    }
                    Ok(LamaInpaintMsg::Error(err)) => {
                        error = Some(err);
                        break;
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => break,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => break,
                }
            }
        }

        if let Some((result_rgba, duration_ms)) = success {
            self.undo_stack.push(self.current_rgba.clone());
            self.redo_stack.clear();
            self.current_rgba = Arc::new(result_rgba);
            self.mark_pixels_changed();
            self.dirty_texture = true;
            if let Some(mask) = self.redaction_state.inpaint_mask.as_mut() {
                mask.fill(0);
            }
            self.inpaint_mask_texture = None;
            self.redaction_state.inpaint_in_progress = false;
            self.redaction_state.inpaint_status = None;
            self.redaction_state.inpaint_error = None;
            self.inpaint_rx = None;
            self.inpaint_job_rx = None;
            self.inpaint_ticket = None;
            self.inpaint_cancel = None;
            self.show_toast(format!(
                "✦ LaMa inpainting complete ({:.2}s)",
                duration_ms as f64 / 1000.0
            ));
        } else if let Some(err) = error {
            self.redaction_state.inpaint_in_progress = false;
            self.redaction_state.inpaint_status = None;
            self.redaction_state.inpaint_error = Some(err.clone());
            self.inpaint_rx = None;
            self.inpaint_job_rx = None;
            self.inpaint_ticket = None;
            self.inpaint_cancel = None;
            self.show_toast(crate::ui_ext::StatusMessage::new(
                crate::icons::Icon::Alert,
                format!("Inpainting failed: {err}"),
            ));
        }
    }
}

pub fn run_native_lama_inpaint(
    image: &RgbaImage,
    mask: &image::GrayImage,
    model_path: &std::path::Path,
) -> Result<RgbaImage, String> {
    let (orig_w, orig_h) = image.dimensions();
    if orig_w == 0 || orig_h == 0 {
        return Err("Image has zero dimension".to_string());
    }

    let pad_h = (8 - orig_h % 8) % 8;
    let pad_w = (8 - orig_w % 8) % 8;
    let padded_w = orig_w + pad_w;
    let padded_h = orig_h + pad_h;

    let mut img_arr = ndarray::Array4::<f32>::zeros((1, 3, padded_h as usize, padded_w as usize));
    let mut mask_arr = ndarray::Array4::<f32>::zeros((1, 1, padded_h as usize, padded_w as usize));

    for y in 0..padded_h {
        let src_y = if y < orig_h {
            y
        } else {
            orig_h.saturating_sub(1)
        };
        for x in 0..padded_w {
            let src_x = if x < orig_w {
                x
            } else {
                orig_w.saturating_sub(1)
            };
            let px = image.get_pixel(src_x, src_y);
            img_arr[[0, 0, y as usize, x as usize]] = px[0] as f32 / 255.0;
            img_arr[[0, 1, y as usize, x as usize]] = px[1] as f32 / 255.0;
            img_arr[[0, 2, y as usize, x as usize]] = px[2] as f32 / 255.0;

            let m = mask.get_pixel(src_x, src_y)[0];
            mask_arr[[0, 0, y as usize, x as usize]] = if m > 0 { 1.0 } else { 0.0 };
        }
    }

    let mut session = crate::onnx_runtime::load_session(model_path)
        .map_err(|e| format!("ORT model load failed: {e}"))?;

    let img_tensor = ort::value::Tensor::from_array(img_arr)
        .map_err(|e| format!("ORT image tensor conversion failed: {e}"))?;
    let mask_tensor = ort::value::Tensor::from_array(mask_arr)
        .map_err(|e| format!("ORT mask tensor conversion failed: {e}"))?;

    let outputs = session
        .run(ort::inputs![img_tensor, mask_tensor])
        .map_err(|e| format!("ORT inference failed: {e}"))?;

    let out_tensor = outputs[0]
        .try_extract_array::<f32>()
        .map_err(|e| format!("Failed to extract output tensor: {e}"))?;

    let mut result = image.clone();
    for y in 0..orig_h {
        for x in 0..orig_w {
            let m = mask.get_pixel(x, y)[0];
            if m > 0 {
                let orig_alpha = image.get_pixel(x, y)[3];
                let r = (out_tensor[[0, 0, y as usize, x as usize]].clamp(0.0, 1.0) * 255.0).round()
                    as u8;
                let g = (out_tensor[[0, 1, y as usize, x as usize]].clamp(0.0, 1.0) * 255.0).round()
                    as u8;
                let b = (out_tensor[[0, 2, y as usize, x as usize]].clamp(0.0, 1.0) * 255.0).round()
                    as u8;
                result.put_pixel(x, y, image::Rgba([r, g, b, orig_alpha]));
            }
        }
    }

    Ok(result)
}

pub(crate) fn compose_redactions_only(source: &RgbaImage, boxes: &[RedactionBox]) -> RgbaImage {
    let mut image = source.clone();
    let img_w = image.width() as f32;
    let img_h = image.height() as f32;
    for b in boxes {
        let px_min_x = (b.normalized_rect.min.x * img_w).round().clamp(0.0, img_w) as u32;
        let px_max_x = (b.normalized_rect.max.x * img_w).round().clamp(0.0, img_w) as u32;
        let px_min_y = (b.normalized_rect.min.y * img_h).round().clamp(0.0, img_h) as u32;
        let px_max_y = (b.normalized_rect.max.y * img_h).round().clamp(0.0, img_h) as u32;

        match b.style {
            RedactionStyle::Black => {
                for y in px_min_y..px_max_y {
                    for x in px_min_x..px_max_x {
                        image.put_pixel(x, y, image::Rgba([0, 0, 0, 255]));
                    }
                }
            }
            RedactionStyle::AverageColor => {
                fill_conical_edge_gradient_redaction(
                    &mut image, source, px_min_x, px_max_x, px_min_y, px_max_y,
                );
            }
        }
    }
    image
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{RawInput, Shape};

    #[test]
    fn test_average_color_computation_and_redaction() {
        // Create an image: left half is Red (200, 0, 0), right half is Blue (0, 0, 200)
        let mut img = RgbaImage::new(100, 100);
        for y in 0..100 {
            for x in 0..50 {
                img.put_pixel(x, y, image::Rgba([200, 0, 0, 255]));
            }
            for x in 50..100 {
                img.put_pixel(x, y, image::Rgba([0, 0, 200, 255]));
            }
        }

        // Box covering entire left half (x: 0..0.5, y: 0..1.0)
        let left_box = Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(0.5, 1.0));
        let left_avg = compute_box_average_color(&img, left_box);
        assert_eq!(left_avg, Color32::from_rgb(200, 0, 0));

        let composed_left = compose_redactions_only(
            &img,
            &[RedactionBox {
                normalized_rect: left_box,
                style: RedactionStyle::AverageColor,
            }],
        );
        // Interior of left half should remain pure red (200, 0, 0)
        assert_eq!(
            *composed_left.get_pixel(25, 25),
            image::Rgba([200, 0, 0, 255])
        );

        // Black text on off-white background
        let mut doc_img = RgbaImage::from_pixel(100, 100, image::Rgba([245, 245, 240, 255]));
        // Add black text in the center (x: 30..70, y: 35..65)
        for y in 35..65 {
            for x in 30..70 {
                doc_img.put_pixel(x, y, image::Rgba([0, 0, 0, 255]));
            }
        }
        // Redaction box enclosing the text with margin (x: 20..80, y: 25..75)
        let text_box = Rect::from_min_max(Pos2::new(0.2, 0.25), Pos2::new(0.8, 0.75));
        let edge_color = compute_box_average_color(&doc_img, text_box);
        assert_eq!(edge_color, Color32::from_rgb(245, 245, 240));

        let redacted_doc = compose_redactions_only(
            &doc_img,
            &[RedactionBox {
                normalized_rect: text_box,
                style: RedactionStyle::AverageColor,
            }],
        );

        // Center where text was should now match the off-white background exactly, not tainted by black text
        assert_eq!(
            *redacted_doc.get_pixel(50, 50),
            image::Rgba([245, 245, 240, 255])
        );
        assert_eq!(
            *redacted_doc.get_pixel(35, 45),
            image::Rgba([245, 245, 240, 255])
        );

        // Test gradient background redaction: top is dark gray (100, 100, 100), bottom is light gray (200, 200, 200)
        let mut grad_img = RgbaImage::new(100, 100);
        for y in 0..100 {
            let v = (100.0 + (y as f32 / 99.0) * 100.0).round() as u8;
            for x in 0..100 {
                grad_img.put_pixel(x, y, image::Rgba([v, v, v, 255]));
            }
        }
        // Black text in the center
        for y in 40..60 {
            for x in 30..70 {
                grad_img.put_pixel(x, y, image::Rgba([0, 0, 0, 255]));
            }
        }
        let grad_box = Rect::from_min_max(Pos2::new(0.2, 0.2), Pos2::new(0.8, 0.8));
        let redacted_grad = compose_redactions_only(
            &grad_img,
            &[RedactionBox {
                normalized_rect: grad_box,
                style: RedactionStyle::AverageColor,
            }],
        );
        // Top edge of box (y = 20) matches background (~120)
        let top_px = redacted_grad.get_pixel(50, 20);
        assert!((top_px[0] as i16 - 120).abs() <= 2);
        // Bottom edge of box (y = 79) matches background (~180)
        let bot_px = redacted_grad.get_pixel(50, 79);
        assert!((bot_px[0] as i16 - 180).abs() <= 2);
        // Center (y = 50) where black text was is smoothly blurred to ~150 with zero black contamination
        let mid_px = redacted_grad.get_pixel(50, 50);
        assert!((mid_px[0] as i16 - 150).abs() <= 3);

        // Compose with Black style
        let composed_black = compose_redactions_only(
            &img,
            &[RedactionBox {
                normalized_rect: left_box,
                style: RedactionStyle::Black,
            }],
        );
        assert_eq!(
            *composed_black.get_pixel(25, 25),
            image::Rgba([0, 0, 0, 255])
        );
    }

    #[test]
    fn test_mask_stamping_and_erasing() {
        let mut mask = image::GrayImage::new(100, 100);

        // Stamp a rectangle in the center (40..60, 40..60)
        let rect = Rect::from_min_max(Pos2::new(0.4, 0.4), Pos2::new(0.6, 0.6));
        stamp_rect_on_mask(&mut mask, rect, 255);
        assert_eq!(mask.get_pixel(50, 50)[0], 255);
        assert_eq!(mask.get_pixel(10, 10)[0], 0);

        // Erase a smaller circle in the center
        stamp_circle_on_mask(&mut mask, Pos2::new(0.5, 0.5), 5.0, 0);
        assert_eq!(mask.get_pixel(50, 50)[0], 0);
        assert_eq!(mask.get_pixel(42, 42)[0], 255);
    }

    #[test]
    fn test_brush_size_adjustment() {
        let png = RgbaImage::new(10, 10);
        let mut cursor = std::io::Cursor::new(Vec::new());
        png.write_to(&mut cursor, image::ImageFormat::Png).unwrap();
        let mut state = ImagePreviewState::from_bytes_with_meta(
            cursor.into_inner(),
            "image/png".into(),
            None,
            "Brush size test".into(),
            &little_clip_core::config::AppConfig::default(),
        )
        .unwrap();

        assert_eq!(state.redaction_state.brush_radius, 24.0);
        state.adjust_brush_radius(4.0);
        assert_eq!(state.redaction_state.brush_radius, 28.0);
        state.adjust_brush_radius(-8.0);
        assert_eq!(state.redaction_state.brush_radius, 20.0);

        // Check clamping
        state.adjust_brush_radius(500.0);
        assert_eq!(state.redaction_state.brush_radius, 128.0);
        state.adjust_brush_radius(-500.0);
        assert_eq!(state.redaction_state.brush_radius, 4.0);
    }

    #[test]
    fn selected_redaction_size_badge_stays_inside_compact_and_wide_viewports() {
        for viewport_size in [Vec2::new(280.0, 220.0), Vec2::new(1040.0, 640.0)] {
            let mut png = Vec::new();
            RgbaImage::new(320, 240)
                .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
                .unwrap();
            let mut state = ImagePreviewState::from_bytes_with_meta(
                png,
                "image/png".into(),
                None,
                "Redact badge test".into(),
                &little_clip_core::config::AppConfig::default(),
            )
            .unwrap();
            state.redaction_state.boxes.push(RedactionBox {
                normalized_rect: Rect::from_min_max(Pos2::new(0.2, 0.2), Pos2::new(0.8, 0.8)),
                style: RedactionStyle::Black,
            });
            state.redaction_state.selected_idx = Some(0);

            let ctx = egui::Context::default();
            let palette = AppPalette::default();
            let mut viewport = None;
            let output = ctx.run(
                RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, viewport_size)),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        let image_rect = ui.max_rect();
                        let response = ui.allocate_rect(image_rect, egui::Sense::click_and_drag());
                        state.render_redact_overlay(
                            ui,
                            &ui.painter().clone(),
                            &response,
                            image_rect,
                            &palette,
                        );
                        viewport = Some(image_rect);
                    });
                },
            );

            let viewport = viewport.expect("overlay viewport was allocated");
            let badge = output.shapes.iter().find_map(|shape| match &shape.shape {
                Shape::Text(text) if text.galley.text().starts_with("192 × 144 px") => {
                    Some(Rect::from_min_size(text.pos, text.galley.size()))
                }
                _ => None,
            });
            let badge = badge.expect("selected redaction renders its size badge");
            assert!(
                viewport.contains_rect(badge),
                "badge {badge:?} must stay within {viewport:?} at {viewport_size:?}"
            );
        }
    }

    #[test]
    fn test_native_lama_inpaint_execution() {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
        let model_path =
            std::path::PathBuf::from(home).join(".cache/little-screen-clip/models/big-lama.onnx");
        if !model_path.exists() {
            return;
        }

        // 64x64 test image: solid green (40, 200, 40) with a blue patch in the center
        let mut img = RgbaImage::from_pixel(64, 64, image::Rgba([40, 200, 40, 255]));
        for y in 24..40 {
            for x in 24..40 {
                img.put_pixel(x, y, image::Rgba([40, 40, 200, 255]));
            }
        }

        // Mask covering the blue patch
        let mut mask = image::GrayImage::new(64, 64);
        for y in 20..44 {
            for x in 20..44 {
                mask.put_pixel(x, y, image::Luma([255]));
            }
        }

        let result = run_native_lama_inpaint(&img, &mask, &model_path).unwrap();
        assert_eq!(result.dimensions(), (64, 64));

        // Outside the mask, pixels must be exactly identical to original
        assert_eq!(*result.get_pixel(10, 10), image::Rgba([40, 200, 40, 255]));
        // Inside the mask, the blue patch should have been replaced (no longer [40, 40, 200])
        assert_ne!(*result.get_pixel(32, 32), image::Rgba([40, 40, 200, 255]));
    }

    #[test]
    fn test_inpaint_selection_modes_combine_onto_same_mask() {
        let mut mask = image::GrayImage::new(100, 100);

        // 1. Block Select: stamp a rectangle from norm (0.1, 0.1) to (0.3, 0.3)
        let rect = Rect::from_min_max(Pos2::new(0.1, 0.1), Pos2::new(0.3, 0.3));
        stamp_rect_on_mask(&mut mask, rect, 255);

        // Check rectangle was stamped (e.g. at pixel 20, 20)
        assert_eq!(mask.get_pixel(20, 20)[0], 255);
        assert_eq!(mask.get_pixel(50, 50)[0], 0);

        // 2. Paint Brush: stamp a circle at norm (0.5, 0.5) with radius 10px
        stamp_circle_on_mask(&mut mask, Pos2::new(0.5, 0.5), 10.0, 255);
        assert_eq!(mask.get_pixel(50, 50)[0], 255);

        // 3. Point-to-Select (SAM): stamp a segmented object at (70..90, 70..90)
        let mut sam_mask = image::GrayImage::new(100, 100);
        for y in 70..90 {
            for x in 70..90 {
                sam_mask.put_pixel(x, y, image::Luma([255]));
            }
        }
        super::sam::stamp_sam_mask_on_inpaint_mask(&mut mask, &sam_mask, 255);
        assert_eq!(mask.get_pixel(80, 80)[0], 255);

        // All three modes are present simultaneously on the exact same mask!
        assert_eq!(mask.get_pixel(20, 20)[0], 255); // Block Select
        assert_eq!(mask.get_pixel(50, 50)[0], 255); // Paint Brush
        assert_eq!(mask.get_pixel(80, 80)[0], 255); // SAM Object
        assert_eq!(mask.get_pixel(5, 5)[0], 0); // Background

        // 4. SAM Erase: erase a portion of the block select rectangle (15..25, 15..25)
        let mut erase_sam = image::GrayImage::new(100, 100);
        for y in 15..25 {
            for x in 15..25 {
                erase_sam.put_pixel(x, y, image::Luma([255]));
            }
        }
        super::sam::stamp_sam_mask_on_inpaint_mask(&mut mask, &erase_sam, 0);

        // Erased portion is now 0
        assert_eq!(mask.get_pixel(20, 20)[0], 0);
        // Rest of rectangle remains 255
        assert_eq!(mask.get_pixel(12, 12)[0], 255);
        // Paint brush and SAM object remain untouched!
        assert_eq!(mask.get_pixel(50, 50)[0], 255);
        assert_eq!(mask.get_pixel(80, 80)[0], 255);
    }

    #[test]
    fn test_point_sam_mode_hover_and_commit() {
        let app_config = little_clip_core::AppConfig::default();
        let mut png = Vec::new();
        RgbaImage::from_pixel(100, 100, image::Rgba([120, 120, 120, 255]))
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let mut state = ImagePreviewState::from_bytes_with_meta(
            png,
            "image/png".into(),
            None,
            "SAM test".into(),
            &app_config,
        )
        .unwrap();

        state.redaction_state.tool_type = RedactToolType::LamaInpaint;
        state.set_inpaint_mode(InpaintSelectionMode::PointSam);

        assert_eq!(
            state.redaction_state.inpaint_mode,
            InpaintSelectionMode::PointSam
        );

        // Simulate a SAM mask arriving from worker
        let mut sam_mask = image::GrayImage::new(100, 100);
        for y in 30..60 {
            for x in 30..60 {
                sam_mask.put_pixel(x, y, image::Luma([255]));
            }
        }
        state.sam_state.hovered_mask = Some(sam_mask);
        state.sam_state.hovered_pos = Some(Pos2::new(0.45, 0.45));

        let ctx = egui::Context::default();
        // Commit the hover mask into inpaint_mask
        state.commit_sam_hover(255, &ctx);

        let mask = state.ensure_inpaint_mask();
        assert_eq!(mask.get_pixel(45, 45)[0], 255);
        assert_eq!(mask.get_pixel(10, 10)[0], 0);
        assert!(state.inpaint_has_mask());
    }

    #[test]
    fn test_grow_selection_expands_sam_and_inpaint_mask() {
        let app_config = little_clip_core::AppConfig::default();
        let mut png = Vec::new();
        RgbaImage::from_pixel(100, 100, image::Rgba([120, 120, 120, 255]))
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let mut state = ImagePreviewState::from_bytes_with_meta(
            png,
            "image/png".into(),
            None,
            "Grow test".into(),
            &app_config,
        )
        .unwrap();

        state.redaction_state.tool_type = RedactToolType::LamaInpaint;
        state.redaction_state.grow_selection_px = 5.0;

        // 1. Check raw hover mask dilation via refresh_sam_hover_preview
        let mut raw_mask = image::GrayImage::new(100, 100);
        // Small 10x10 patch at (45..55, 45..55)
        for y in 45..55 {
            for x in 45..55 {
                raw_mask.put_pixel(x, y, image::Luma([255]));
            }
        }
        state.sam_state.raw_hovered_mask = Some(raw_mask);
        let ctx = egui::Context::default();
        state.refresh_sam_hover_preview(&ctx);

        let dilated_hover = state.sam_state.hovered_mask.as_ref().unwrap();
        // Point (40, 50) is 5 pixels away from x=45, should now be included
        assert_eq!(dilated_hover.get_pixel(40, 50)[0], 255);
        // Point (35, 50) is 10 pixels away, should be 0
        assert_eq!(dilated_hover.get_pixel(35, 50)[0], 0);

        // 2. Check growing an existing inpaint mask
        let mask = state.ensure_inpaint_mask();
        mask.put_pixel(50, 50, image::Luma([255]));
        state.grow_current_inpaint_mask(&ctx);

        let grown_mask = state.ensure_inpaint_mask();
        // Center is still 255
        assert_eq!(grown_mask.get_pixel(50, 50)[0], 255);
        // 5 pixels away is now 255
        assert_eq!(grown_mask.get_pixel(50, 45)[0], 255);
        assert_eq!(grown_mask.get_pixel(55, 50)[0], 255);
        // 10 pixels away is 0
        assert_eq!(grown_mask.get_pixel(50, 40)[0], 0);
    }
}
