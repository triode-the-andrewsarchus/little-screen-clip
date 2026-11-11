use super::super::*;
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

mod inpaint;
pub(crate) use inpaint::InpaintJob;
pub use inpaint::run_native_lama_inpaint;
mod ui;

#[cfg(test)]
mod tests;
