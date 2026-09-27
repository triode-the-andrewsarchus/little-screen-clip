use super::super::*;

#[allow(dead_code)]
pub fn optimize_to_target_size(
    base_img: &RgbaImage,
    target_bytes: usize,
    format: TargetFormat,
) -> FileSizeOptimizationResult {
    optimize_to_target_size_with_progress(base_img, target_bytes, format, None)
}

pub fn optimize_to_target_size_with_progress(
    base_img: &RgbaImage,
    target_bytes: usize,
    format: TargetFormat,
    progress: Option<&dyn Fn(String)>,
) -> FileSizeOptimizationResult {
    let orig_w = base_img.width();
    let orig_h = base_img.height();
    let target_str = format_bytes_size(target_bytes);

    let notify = |msg: String| {
        if let Some(cb) = progress {
            cb(msg);
        }
    };

    match format {
        TargetFormat::Jpeg => {
            notify(format!(
                "Testing full resolution ({}×{}) at JPEG quality 95%...",
                orig_w, orig_h
            ));
            // 1. Try quality 95 at 100% scale
            if let Ok(bytes) = encode_jpeg_with_quality(base_img, 95) {
                if bytes.len() <= target_bytes {
                    notify(format!(
                        "Full resolution fits budget: {} <= {}",
                        format_bytes_size(bytes.len()),
                        target_str
                    ));
                    return FileSizeOptimizationResult {
                        target_bytes,
                        achieved_bytes: bytes.len(),
                        width: orig_w,
                        height: orig_h,
                        scale: 1.0,
                        quality: 95,
                        format,
                        encoded_data: bytes,
                        result_rgba: base_img.clone(),
                    };
                } else {
                    notify(format!(
                        "Quality 95% is {} (budget: {}). Searching JPEG quality [60..95]...",
                        format_bytes_size(bytes.len()),
                        target_str
                    ));
                }
            }

            // 2. Binary search quality in [60, 95] at scale 1.0
            let mut best_qual_at_100 = None;
            let mut low_q = 60u8;
            let mut high_q = 95u8;

            while low_q <= high_q {
                let mid_q = low_q + (high_q - low_q) / 2;
                notify(format!(
                    "Trying quality {}% at 100% resolution ({}×{})...",
                    mid_q, orig_w, orig_h
                ));
                if let Ok(bytes) = encode_jpeg_with_quality(base_img, mid_q) {
                    let len = bytes.len();
                    if len <= target_bytes {
                        notify(format!(
                            "Quality {}% fits budget: {} <= {}",
                            mid_q,
                            format_bytes_size(len),
                            target_str
                        ));
                        best_qual_at_100 = Some((mid_q, bytes));
                        low_q = mid_q + 1; // Try higher quality
                    } else {
                        notify(format!(
                            "Quality {}% exceeds budget: {} > {}",
                            mid_q,
                            format_bytes_size(len),
                            target_str
                        ));
                        if mid_q == 0 {
                            break;
                        }
                        high_q = mid_q - 1; // Try lower quality
                    }
                } else {
                    break;
                }
            }

            if let Some((q, bytes)) = best_qual_at_100 {
                notify(format!(
                    "Found optimal quality {}% at 100% resolution ({})",
                    q,
                    format_bytes_size(bytes.len())
                ));
                return FileSizeOptimizationResult {
                    target_bytes,
                    achieved_bytes: bytes.len(),
                    width: orig_w,
                    height: orig_h,
                    scale: 1.0,
                    quality: q,
                    format,
                    encoded_data: bytes,
                    result_rgba: base_img.clone(),
                };
            }

            // 3. At scale 1.0, even quality 60 exceeds target_bytes!
            notify(format!(
                "Exceeds budget at 100% resolution. Scaling down with Lanczos3 filter..."
            ));
            let mut best_scale = 0.5f32;
            let mut best_scaled_img = base_img.clone();
            let mut best_bytes = Vec::new();
            let mut best_w = orig_w;
            let mut best_h = orig_h;
            let mut best_q = 80u8;

            let mut low_s = 0.05f32;
            let mut high_s = 1.0f32;

            for iter in 1..=10 {
                let mid_s = (low_s + high_s) / 2.0;
                let candidate_w = ((orig_w as f32) * mid_s).round().max(16.0) as u32;
                let candidate_h = ((orig_h as f32) * mid_s).round().max(16.0) as u32;

                notify(format!(
                    "[Step {}/10] Resampling to {:.1}% ({}×{}) with Lanczos3...",
                    iter,
                    mid_s * 100.0,
                    candidate_w,
                    candidate_h
                ));

                let scaled = image::imageops::resize(
                    base_img,
                    candidate_w,
                    candidate_h,
                    image::imageops::FilterType::Lanczos3,
                );

                if let Ok(bytes) = encode_jpeg_with_quality(&scaled, 80) {
                    let len = bytes.len();
                    if len <= target_bytes {
                        notify(format!(
                            "Scale {:.1}% ({}×{}) fits: {} <= {}",
                            mid_s * 100.0,
                            candidate_w,
                            candidate_h,
                            format_bytes_size(len),
                            target_str
                        ));
                        best_scale = mid_s;
                        best_w = candidate_w;
                        best_h = candidate_h;
                        best_bytes = bytes;
                        best_scaled_img = scaled;
                        best_q = 80;
                        low_s = mid_s;
                    } else {
                        notify(format!(
                            "Scale {:.1}% ({}×{}) exceeds: {} > {}",
                            mid_s * 100.0,
                            candidate_w,
                            candidate_h,
                            format_bytes_size(len),
                            target_str
                        ));
                        high_s = mid_s;
                    }
                } else {
                    break;
                }
            }

            if !best_bytes.is_empty() {
                notify(format!(
                    "Fine-tuning JPEG quality on {}×{}...",
                    best_w, best_h
                ));
                for test_q in (81..=95).step_by(2) {
                    if let Ok(b) = encode_jpeg_with_quality(&best_scaled_img, test_q) {
                        let len = b.len();
                        if len <= target_bytes {
                            notify(format!(
                                "Tuned quality to {}%: {} <= {}",
                                test_q,
                                format_bytes_size(len),
                                target_str
                            ));
                            best_q = test_q;
                            best_bytes = b;
                        } else {
                            break;
                        }
                    }
                }
            } else {
                notify(format!("Applying minimal scale fallback..."));
                let min_w = ((orig_w as f32) * 0.1).round().max(16.0) as u32;
                let min_h = ((orig_h as f32) * 0.1).round().max(16.0) as u32;
                best_scaled_img = image::imageops::resize(
                    base_img,
                    min_w,
                    min_h,
                    image::imageops::FilterType::Triangle,
                );
                best_w = min_w;
                best_h = min_h;
                best_scale = 0.1;
                best_q = 40;
                if let Ok(b) = encode_jpeg_with_quality(&best_scaled_img, 40) {
                    best_bytes = b;
                }
            }

            notify(format!(
                "Complete: {} achieved at {}×{} (Quality {}%)",
                format_bytes_size(best_bytes.len()),
                best_w,
                best_h,
                best_q
            ));

            FileSizeOptimizationResult {
                target_bytes,
                achieved_bytes: best_bytes.len(),
                width: best_w,
                height: best_h,
                scale: best_scale,
                quality: best_q,
                format,
                encoded_data: best_bytes,
                result_rgba: best_scaled_img,
            }
        }
        TargetFormat::Png => {
            notify(format!(
                "Testing full resolution ({}×{}) as lossless PNG...",
                orig_w, orig_h
            ));
            let mut best_scale = 1.0f32;
            let mut best_scaled_img = base_img.clone();
            let mut best_bytes = Vec::new();
            let mut best_w = orig_w;
            let mut best_h = orig_h;

            // Check 100% first
            let mut cursor = std::io::Cursor::new(Vec::new());
            if base_img
                .write_to(&mut cursor, image::ImageFormat::Png)
                .is_ok()
            {
                let bytes = cursor.into_inner();
                if bytes.len() <= target_bytes {
                    notify(format!(
                        "Full resolution PNG fits budget: {} <= {}",
                        format_bytes_size(bytes.len()),
                        target_str
                    ));
                    return FileSizeOptimizationResult {
                        target_bytes,
                        achieved_bytes: bytes.len(),
                        width: orig_w,
                        height: orig_h,
                        scale: 1.0,
                        quality: 100,
                        format,
                        encoded_data: bytes,
                        result_rgba: base_img.clone(),
                    };
                } else {
                    notify(format!(
                        "PNG at 100% is {} > {}. Searching resolution scale...",
                        format_bytes_size(bytes.len()),
                        target_str
                    ));
                }
            }

            let mut low_s = 0.05f32;
            let mut high_s = 1.0f32;

            for iter in 1..=10 {
                let mid_s = (low_s + high_s) / 2.0;
                let candidate_w = ((orig_w as f32) * mid_s).round().max(16.0) as u32;
                let candidate_h = ((orig_h as f32) * mid_s).round().max(16.0) as u32;

                notify(format!(
                    "[Step {}/10] Resampling to {:.1}% ({}×{})...",
                    iter,
                    mid_s * 100.0,
                    candidate_w,
                    candidate_h
                ));

                let scaled = image::imageops::resize(
                    base_img,
                    candidate_w,
                    candidate_h,
                    image::imageops::FilterType::Lanczos3,
                );

                let mut cur = std::io::Cursor::new(Vec::new());
                if scaled.write_to(&mut cur, image::ImageFormat::Png).is_ok() {
                    let bytes = cur.into_inner();
                    let len = bytes.len();
                    if len <= target_bytes {
                        notify(format!(
                            "Scale {:.1}% fits: {} <= {}",
                            mid_s * 100.0,
                            format_bytes_size(len),
                            target_str
                        ));
                        best_scale = mid_s;
                        best_w = candidate_w;
                        best_h = candidate_h;
                        best_bytes = bytes;
                        best_scaled_img = scaled;
                        low_s = mid_s;
                    } else {
                        notify(format!(
                            "Scale {:.1}% exceeds: {} > {}",
                            mid_s * 100.0,
                            format_bytes_size(len),
                            target_str
                        ));
                        high_s = mid_s;
                    }
                }
            }

            notify(format!(
                "Complete: {} achieved at {}×{}",
                format_bytes_size(best_bytes.len()),
                best_w,
                best_h
            ));

            FileSizeOptimizationResult {
                target_bytes,
                achieved_bytes: best_bytes.len(),
                width: best_w,
                height: best_h,
                scale: best_scale,
                quality: 100,
                format,
                encoded_data: best_bytes,
                result_rgba: best_scaled_img,
            }
        }
    }
}

impl Drop for ImagePreviewState {
    fn drop(&mut self) {
        self.cancel_ocr();
    }
}

#[derive(Debug)]
pub enum OptimizerMsg {
    Progress(String),
    Done(FileSizeOptimizationResult),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ResizeFilter {
    #[default]
    Lanczos3,
    Triangle,
    Nearest,
}

impl ResizeFilter {
    pub fn label(&self) -> &'static str {
        match self {
            ResizeFilter::Lanczos3 => "Lanczos3 (HQ)",
            ResizeFilter::Triangle => "Triangle (Smooth)",
            ResizeFilter::Nearest => "Nearest (Sharp)",
        }
    }

    pub fn to_image_filter(&self) -> image::imageops::FilterType {
        match self {
            ResizeFilter::Lanczos3 => image::imageops::FilterType::Lanczos3,
            ResizeFilter::Triangle => image::imageops::FilterType::Triangle,
            ResizeFilter::Nearest => image::imageops::FilterType::Nearest,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TargetFormat {
    #[default]
    Jpeg,
    Png,
}

impl TargetFormat {
    pub fn label(&self) -> &'static str {
        match self {
            TargetFormat::Jpeg => "JPEG",
            TargetFormat::Png => "PNG",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ResizeSubTab {
    #[default]
    Pixels,
    FileSize,
}

#[derive(Debug, Clone)]
pub struct FileSizeOptimizationResult {
    pub target_bytes: usize,
    pub achieved_bytes: usize,
    pub width: u32,
    pub height: u32,
    pub scale: f32,
    pub quality: u8,
    pub format: TargetFormat,
    pub encoded_data: Vec<u8>,
    pub result_rgba: RgbaImage,
}

#[derive(Debug, Clone)]
pub struct ResizeState {
    pub sub_tab: ResizeSubTab,

    // Target Pixels
    pub target_width: u32,
    pub target_height: u32,
    pub width_input: String,
    pub height_input: String,
    pub lock_aspect: bool,
    pub filter: ResizeFilter,
    pub aspect_ratio_base: f32,

    // Target File Size
    pub target_size_val: f32,
    pub target_size_unit_mb: bool,
    pub target_format: TargetFormat,
    pub last_result: Option<FileSizeOptimizationResult>,
    pub(crate) last_result_input_stamp: Option<ImageInputStamp>,
    pub(crate) optimizer_input_stamp: Option<ImageInputStamp>,
    #[allow(dead_code)]
    pub is_optimizing: bool,
}

impl Default for ResizeState {
    fn default() -> Self {
        Self {
            sub_tab: ResizeSubTab::Pixels,
            target_width: 1920,
            target_height: 1080,
            width_input: "1920".to_string(),
            height_input: "1080".to_string(),
            lock_aspect: true,
            filter: ResizeFilter::Lanczos3,
            aspect_ratio_base: 16.0 / 9.0,
            target_size_val: 2.0,
            target_size_unit_mb: true,
            target_format: TargetFormat::Jpeg,
            last_result: None,
            last_result_input_stamp: None,
            optimizer_input_stamp: None,
            is_optimizing: false,
        }
    }
}

impl ResizeState {
    pub fn sync_from_image(&mut self, img: &RgbaImage) {
        let w = img.width().max(1);
        let h = img.height().max(1);
        self.target_width = w;
        self.target_height = h;
        self.width_input = w.to_string();
        self.height_input = h.to_string();
        self.aspect_ratio_base = w as f32 / h as f32;
    }

    pub fn set_width(&mut self, new_w: u32) {
        self.target_width = new_w.max(1);
        self.width_input = self.target_width.to_string();
        if self.lock_aspect && self.aspect_ratio_base > 0.0 {
            let new_h = ((self.target_width as f32) / self.aspect_ratio_base)
                .round()
                .max(1.0) as u32;
            self.target_height = new_h;
            self.height_input = new_h.to_string();
        }
    }

    pub fn set_height(&mut self, new_h: u32) {
        self.target_height = new_h.max(1);
        self.height_input = self.target_height.to_string();
        if self.lock_aspect && self.aspect_ratio_base > 0.0 {
            let new_w = ((self.target_height as f32) * self.aspect_ratio_base)
                .round()
                .max(1.0) as u32;
            self.target_width = new_w;
            self.width_input = new_w.to_string();
        }
    }

    pub fn set_scale_preset(&mut self, factor: f32, current_w: u32, current_h: u32) {
        let new_w = ((current_w as f32) * factor).round().max(1.0) as u32;
        let new_h = ((current_h as f32) * factor).round().max(1.0) as u32;
        self.target_width = new_w;
        self.target_height = new_h;
        self.width_input = new_w.to_string();
        self.height_input = new_h.to_string();
    }

    pub fn target_bytes(&self) -> usize {
        let bytes = if self.target_size_unit_mb {
            self.target_size_val * 1024.0 * 1024.0
        } else {
            self.target_size_val * 1024.0
        };
        (bytes.max(1024.0).round()) as usize
    }
}

#[cfg(test)]
mod input_stamp_tests {
    use super::*;

    fn state() -> ImagePreviewState {
        let mut bytes = Vec::new();
        RgbaImage::new(4, 4)
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .unwrap();
        ImagePreviewState::from_bytes_with_meta(
            bytes,
            "image/png".into(),
            None,
            "optimizer stamp test".into(),
            &AppConfig::default(),
        )
        .unwrap()
    }

    fn result(color: [u8; 4]) -> FileSizeOptimizationResult {
        FileSizeOptimizationResult {
            target_bytes: 100,
            achieved_bytes: 90,
            width: 4,
            height: 4,
            scale: 1.0,
            quality: 80,
            format: TargetFormat::Png,
            encoded_data: vec![1, 2, 3],
            result_rgba: RgbaImage::from_pixel(4, 4, image::Rgba(color)),
        }
    }

    #[test]
    fn optimizer_discards_stale_result_and_keeps_current_result_applicable() {
        let mut stale = state();
        stale.resize_state.optimizer_input_stamp = Some(stale.input_stamp());
        stale.resize_state.is_optimizing = true;
        stale.optimizer_status = Some("working".into());
        let (tx, rx) = std::sync::mpsc::channel();
        stale.optimizer_rx = Some(rx);
        tx.send(OptimizerMsg::Done(result([1, 2, 3, 255]))).unwrap();
        stale.mark_pixels_changed();

        stale.poll_optimizer();

        assert!(stale.resize_state.last_result.is_none());
        assert!(!stale.resize_state.is_optimizing);
        assert!(stale.optimizer_status.is_none());
        assert!(stale.optimizer_rx.is_none());

        let mut current = state();
        current.resize_state.optimizer_input_stamp = Some(current.input_stamp());
        current.resize_state.is_optimizing = true;
        let (tx, rx) = std::sync::mpsc::channel();
        current.optimizer_rx = Some(rx);
        tx.send(OptimizerMsg::Done(result([7, 8, 9, 255]))).unwrap();

        current.poll_optimizer();

        assert!(current.resize_state.last_result.is_some());
        assert_eq!(
            current.resize_state.last_result_input_stamp,
            Some(current.input_stamp())
        );
        current.apply_optimized_result();
        assert_eq!(
            current.current_rgba.get_pixel(0, 0),
            &image::Rgba([7, 8, 9, 255])
        );
        assert_eq!(current.pixel_revision, 1);
        assert!(current.resize_state.last_result_input_stamp.is_none());
    }
}

impl ImagePreviewState {
    pub fn apply_pixel_resize(&mut self, target_w: u32, target_h: u32, filter: ResizeFilter) {
        let (w, h) = (target_w.clamp(1, 16384), target_h.clamp(1, 16384));
        if w == self.current_rgba.width() && h == self.current_rgba.height() {
            return;
        }

        // Burn any pending redactions first so they scale with the image
        if !self.redaction_state.boxes.is_empty() {
            self.apply_redactions();
        }

        self.undo_stack.push(self.current_rgba.clone());
        self.redo_stack.clear();

        let resized = image::imageops::resize(&self.current_rgba, w, h, filter.to_image_filter());
        self.current_rgba = resized;
        self.mark_pixels_changed();
        self.dirty_texture = true;
        self.resize_state.sync_from_image(&self.current_rgba);
        self.palette_state.is_extracted = false;

        self.show_toast(format!("↔ Resized to {}×{} px ({})", w, h, filter.label()));
    }

    /// Runs the target file size optimizer solving the quality vs. resolution trade-off asynchronously.

    pub fn run_target_file_size_optimizer(&mut self) {
        if self.resize_state.is_optimizing {
            return;
        }

        let target_bytes = self.resize_state.target_bytes();
        let format = self.resize_state.target_format;
        let base_img = self.current_display_image();
        self.resize_state.optimizer_input_stamp = Some(self.input_stamp());

        let (tx, rx) = std::sync::mpsc::channel::<OptimizerMsg>();
        self.optimizer_rx = Some(rx);
        self.resize_state.is_optimizing = true;
        self.optimizer_status = Some("Starting optimizer...".to_string());

        std::thread::spawn(move || {
            let tx_prog = tx.clone();
            let progress_cb = move |msg: String| {
                let _ = tx_prog.send(OptimizerMsg::Progress(msg));
            };

            let result = optimize_to_target_size_with_progress(
                &base_img,
                target_bytes,
                format,
                Some(&progress_cb),
            );

            let _ = tx.send(OptimizerMsg::Done(result));
        });
    }

    /// Polls the background file size optimizer channel, updating status messages
    /// and storing the final result when complete.

    pub fn poll_optimizer(&mut self) {
        if self.optimizer_rx.is_some()
            && self.resize_state.optimizer_input_stamp.as_ref() != Some(&self.input_stamp())
        {
            self.optimizer_rx = None;
            self.resize_state.optimizer_input_stamp = None;
            self.resize_state.is_optimizing = false;
            self.optimizer_status = None;
            return;
        }
        let mut done_res = None;
        let mut disconnected = false;
        if let Some(rx) = &self.optimizer_rx {
            loop {
                match rx.try_recv() {
                    Ok(OptimizerMsg::Progress(status)) => {
                        self.optimizer_status = Some(status);
                    }
                    Ok(OptimizerMsg::Done(res)) => {
                        done_res = Some(res);
                        break;
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => {
                        break;
                    }
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        disconnected = true;
                        break;
                    }
                }
            }
        }
        if let Some(res) = done_res {
            self.resize_state.last_result = Some(res);
            self.resize_state.last_result_input_stamp =
                self.resize_state.optimizer_input_stamp.take();
            self.resize_state.is_optimizing = false;
            self.optimizer_status = None;
            self.optimizer_rx = None;
            self.resize_state.optimizer_input_stamp = None;
        } else if disconnected {
            self.resize_state.is_optimizing = false;
            self.optimizer_status = None;
            self.optimizer_rx = None;
            self.resize_state.optimizer_input_stamp = None;
        }
    }

    /// Applies the optimized image result directly to current_rgba.

    pub fn apply_optimized_result(&mut self) {
        if self.resize_state.last_result_input_stamp.as_ref() != Some(&self.input_stamp()) {
            return;
        }
        let res = match self.resize_state.last_result.clone() {
            Some(r) => r,
            None => return,
        };

        if !self.redaction_state.boxes.is_empty() {
            self.apply_redactions();
        }

        self.undo_stack.push(self.current_rgba.clone());
        self.redo_stack.clear();

        self.current_rgba = res.result_rgba;
        self.mark_pixels_changed();
        self.redaction_state.boxes.clear();
        self.markup_state.items.clear();
        self.resize_state.last_result_input_stamp = None;
        self.file_size_bytes = res.achieved_bytes;
        self.dirty_texture = true;
        self.resize_state.sync_from_image(&self.current_rgba);
        self.palette_state.is_extracted = false;

        self.show_toast(format!(
            "✔ Applied optimized {}×{} image ({})",
            res.width,
            res.height,
            format_bytes_size(res.achieved_bytes)
        ));
    }

    /// Copies the optimized result directly to the Left Hand clipboard slot via daemon IPC
    /// and synchronizes with OS clipboard.

    pub fn copy_optimized_to_slot(&mut self) {
        if self.resize_state.last_result_input_stamp.as_ref() != Some(&self.input_stamp()) {
            return;
        }
        let res = match &self.resize_state.last_result {
            Some(r) => r,
            None => return,
        };

        let mime = match res.format {
            TargetFormat::Jpeg => "image/jpeg",
            TargetFormat::Png => "image/png",
        };

        let req = IpcRequest::PushImage {
            bytes: res.encoded_data.clone(),
            mime: mime.to_string(),
            hand: HandSide::Left,
        };
        let ipc_ok = crate::send_ipc_request(&req).is_ok();

        let bytes_for_cmd = res.encoded_data.clone();
        let mime_for_cmd = mime.to_string();
        std::thread::spawn(move || {
            if let Err(error) =
                little_clip_core::copy_bytes_to_system_clipboard(&bytes_for_cmd, &mime_for_cmd)
            {
                tracing::debug!(%error, "failed to copy optimized image to system clipboard");
            }
        });

        if ipc_ok {
            self.show_toast(format!(
                "📋 Copied optimized {} ({}) to Clipboard Slot (LH)",
                format_bytes_size(res.achieved_bytes),
                res.format.label()
            ));
        } else {
            self.show_toast(format!(
                "📋 Copied optimized {} ({}) to OS Clipboard",
                format_bytes_size(res.achieved_bytes),
                res.format.label()
            ));
        }
    }

    /// Extracts dominant colors from current_display_image into palette_state.

    pub(crate) fn render_resize_toolbar(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        palette: &AppPalette,
    ) {
        self.poll_optimizer();
        if self.resize_state.is_optimizing {
            ctx.request_repaint();
        }

        let accent = hex_to_c32(&palette.harmonies.base);
        let surface = hex_to_c32(&palette.colors.surface_panel);
        let text = hex_to_c32(&palette.colors.text_secondary);
        let muted = hex_to_c32(&palette.colors.text_muted);
        let success = hex_to_c32(&palette.diff_add);

        ui.label(egui::RichText::new("Resize").strong().color(accent));
        ui.add_space(crate::ui_ext::ITEM_SPACING_Y);

        // Stable, equal-width mode selector at the top of the rail.
        let mode_button_width = ((ui.available_width() - 6.0) * 0.5).max(70.0);
        ui.horizontal(|ui| {
            let dimensions = crate::mode_tab_button(
                "Dimensions",
                self.resize_state.sub_tab == ResizeSubTab::Pixels,
                palette,
            );
            if ui
                .add_sized(
                    [mode_button_width, crate::ui_ext::CONTROL_HEIGHT],
                    dimensions,
                )
                .clicked()
            {
                self.resize_state.sub_tab = ResizeSubTab::Pixels;
                self.resize_state.sync_from_image(&self.current_rgba);
            }

            let file_size = crate::mode_tab_button(
                "File size",
                self.resize_state.sub_tab == ResizeSubTab::FileSize,
                palette,
            );
            if ui
                .add_sized(
                    [mode_button_width, crate::ui_ext::CONTROL_HEIGHT],
                    file_size,
                )
                .clicked()
            {
                self.resize_state.sub_tab = ResizeSubTab::FileSize;
            }
        });

        ui.add_space(crate::ui_ext::SPACE_SM);
        ui.separator();
        ui.add_space(crate::ui_ext::SPACE_SM);

        match self.resize_state.sub_tab {
            ResizeSubTab::Pixels => {
                ui.label(
                    egui::RichText::new("OUTPUT DIMENSIONS")
                        .size(crate::ui_ext::BODY_FONT_SIZE)
                        .strong()
                        .color(text),
                );
                ui.add_space(crate::ui_ext::SPACE_XS);

                // Width and height stay together as one WxH control.
                ui.horizontal(|ui| {
                    let field_width = ((ui.available_width() - 24.0) * 0.5).max(56.0);

                    let mut width = self.resize_state.width_input.clone();
                    let width_response = ui.add_sized(
                        [field_width, crate::ui_ext::CONTROL_HEIGHT],
                        egui::TextEdit::singleline(&mut width)
                            .desired_width(field_width)
                            .hint_text("Width")
                            .font(egui::TextStyle::Monospace),
                    );
                    if width_response.changed() {
                        self.resize_state.width_input = width.clone();
                        if let Ok(value) = width.trim().parse::<u32>() {
                            self.resize_state.set_width(value);
                        }
                    }

                    ui.label(egui::RichText::new("×").strong().color(muted));

                    let mut height = self.resize_state.height_input.clone();
                    let height_response = ui.add_sized(
                        [field_width, crate::ui_ext::CONTROL_HEIGHT],
                        egui::TextEdit::singleline(&mut height)
                            .desired_width(field_width)
                            .hint_text("Height")
                            .font(egui::TextStyle::Monospace),
                    );
                    if height_response.changed() {
                        self.resize_state.height_input = height.clone();
                        if let Ok(value) = height.trim().parse::<u32>() {
                            self.resize_state.set_height(value);
                        }
                    }
                });
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("W")
                            .size(crate::ui_ext::BODY_FONT_SIZE)
                            .color(muted),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            egui::RichText::new("H")
                                .size(crate::ui_ext::BODY_FONT_SIZE)
                                .color(muted),
                        );
                    });
                });

                ui.add_space(crate::ui_ext::ITEM_SPACING_Y);
                let lock_icon = if self.resize_state.lock_aspect {
                    Icon::Lock
                } else {
                    Icon::Unlock
                };
                let lock_label = if self.resize_state.lock_aspect {
                    "Aspect ratio locked"
                } else {
                    "Aspect ratio unlocked"
                };
                let lock_button = crate::ui_ext::mode_tab_button_icon(
                    Some(lock_icon),
                    lock_label,
                    self.resize_state.lock_aspect,
                    palette,
                );
                if ui
                    .add_sized(
                        [ui.available_width(), crate::ui_ext::CONTROL_HEIGHT],
                        lock_button,
                    )
                    .on_hover_text("Keep width and height proportional")
                    .clicked()
                {
                    self.resize_state.lock_aspect = !self.resize_state.lock_aspect;
                    if self.resize_state.lock_aspect && self.resize_state.target_height > 0 {
                        self.resize_state.aspect_ratio_base = self.resize_state.target_width as f32
                            / self.resize_state.target_height as f32;
                    }
                }

                ui.add_space(crate::ui_ext::SPACE_MD);
                ui.label(
                    egui::RichText::new("SCALE PRESET")
                        .size(crate::ui_ext::BODY_FONT_SIZE)
                        .strong()
                        .color(text),
                );
                ui.add_space(crate::ui_ext::SPACE_XS);
                let current = (self.current_rgba.width(), self.current_rgba.height());
                let presets = [
                    ("25%", 0.25_f32),
                    ("50%", 0.5),
                    ("75%", 0.75),
                    ("100%", 1.0),
                    ("150%", 1.5),
                    ("200%", 2.0),
                ];
                let labels = presets.map(|(label, _)| label);
                let button_width =
                    crate::ui_ext::wrapped_equal_button_width(ui, &labels, false, presets.len());
                ui.horizontal_wrapped(|ui| {
                    for (label, factor) in presets {
                        let button = crate::compact_toolbar_button(label, surface, palette);
                        if ui
                            .add_sized([button_width, crate::ui_ext::CONTROL_HEIGHT], button)
                            .clicked()
                        {
                            self.resize_state
                                .set_scale_preset(factor, current.0, current.1);
                        }
                    }
                });

                ui.add_space(crate::ui_ext::SPACE_MD);
                ui.label(
                    egui::RichText::new("RESAMPLING")
                        .size(crate::ui_ext::BODY_FONT_SIZE)
                        .strong()
                        .color(text),
                );
                egui::ComboBox::from_id_source("resize_filter_combo")
                    .selected_text(self.resize_state.filter.label())
                    .width(ui.available_width())
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut self.resize_state.filter,
                            ResizeFilter::Lanczos3,
                            ResizeFilter::Lanczos3.label(),
                        );
                        ui.selectable_value(
                            &mut self.resize_state.filter,
                            ResizeFilter::Triangle,
                            ResizeFilter::Triangle.label(),
                        );
                        ui.selectable_value(
                            &mut self.resize_state.filter,
                            ResizeFilter::Nearest,
                            ResizeFilter::Nearest.label(),
                        );
                    });

                ui.add_space(14.0);
                let apply = crate::action_button("Apply resize (Enter)", accent, palette);
                if ui
                    .add_sized([ui.available_width(), crate::ui_ext::CONTROL_HEIGHT], apply)
                    .on_hover_text("Resize the image to the dimensions above")
                    .clicked()
                {
                    self.apply_pixel_resize(
                        self.resize_state.target_width,
                        self.resize_state.target_height,
                        self.resize_state.filter,
                    );
                }
            }
            ResizeSubTab::FileSize => {
                ui.label(
                    egui::RichText::new("TARGET SIZE")
                        .size(crate::ui_ext::BODY_FONT_SIZE)
                        .strong()
                        .color(text),
                );
                ui.add_space(crate::ui_ext::SPACE_XS);

                let presets = [
                    ("250 KB", 250.0_f32, false),
                    ("500 KB", 500.0, false),
                    ("1 MB", 1.0, true),
                    ("2 MB", 2.0, true),
                ];
                let labels = presets.map(|(label, _, _)| label);
                let button_width =
                    crate::ui_ext::wrapped_equal_button_width(ui, &labels, false, presets.len());
                ui.horizontal_wrapped(|ui| {
                    for (label, value, megabytes) in presets {
                        let selected = (self.resize_state.target_size_val - value).abs() < 0.01
                            && self.resize_state.target_size_unit_mb == megabytes;
                        let fill = if selected { success } else { surface };
                        let button = crate::compact_toolbar_button(label, fill, palette);
                        if ui
                            .add_sized([button_width, crate::ui_ext::CONTROL_HEIGHT], button)
                            .clicked()
                        {
                            self.resize_state.target_size_val = value;
                            self.resize_state.target_size_unit_mb = megabytes;
                        }
                    }
                });

                ui.add_space(crate::ui_ext::SPACE_SM);
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("Custom")
                            .size(crate::ui_ext::BODY_FONT_SIZE)
                            .color(text),
                    );
                    ui.add(
                        egui::DragValue::new(&mut self.resize_state.target_size_val)
                            .speed(0.1)
                            .clamp_range(0.01..=1000.0),
                    );
                    let unit = if self.resize_state.target_size_unit_mb {
                        "MB"
                    } else {
                        "KB"
                    };
                    if ui
                        .add(crate::compact_toolbar_button(unit, surface, palette))
                        .on_hover_text("Switch between KB and MB")
                        .clicked()
                    {
                        self.resize_state.target_size_unit_mb =
                            !self.resize_state.target_size_unit_mb;
                    }
                });

                ui.add_space(crate::ui_ext::SPACE_MD);
                ui.label(
                    egui::RichText::new("OUTPUT FORMAT")
                        .size(crate::ui_ext::BODY_FONT_SIZE)
                        .strong()
                        .color(text),
                );
                egui::ComboBox::from_id_source("opt_format_combo")
                    .selected_text(self.resize_state.target_format.label())
                    .width(ui.available_width())
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut self.resize_state.target_format,
                            TargetFormat::Jpeg,
                            "JPEG — quality + resolution",
                        );
                        ui.selectable_value(
                            &mut self.resize_state.target_format,
                            TargetFormat::Png,
                            "PNG — lossless resolution",
                        );
                    });

                ui.add_space(14.0);
                if self.resize_state.is_optimizing {
                    ui.add_enabled(
                        false,
                        crate::action_button("Optimizing…", surface, palette).min_size(Vec2::new(
                            ui.available_width(),
                            crate::ui_ext::CONTROL_HEIGHT,
                        )),
                    );
                } else {
                    let run = crate::action_button("Run optimizer", success, palette);
                    if ui
                        .add_sized([ui.available_width(), crate::ui_ext::CONTROL_HEIGHT], run)
                        .on_hover_text("Fit quality and resolution to the target file size")
                        .clicked()
                    {
                        self.run_target_file_size_optimizer();
                    }
                }

                if self.resize_state.is_optimizing {
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        ui.add(egui::Spinner::new().size(14.0).color(accent));
                        ui.label(
                            egui::RichText::new(
                                self.optimizer_status
                                    .as_deref()
                                    .unwrap_or("Optimizing image file size…"),
                            )
                            .size(crate::ui_ext::BODY_FONT_SIZE)
                            .color(accent),
                        );
                    });
                } else if self.resize_state.last_result_input_stamp.as_ref()
                    == Some(&self.input_stamp())
                    && let Some(result) = self.resize_state.last_result.clone()
                {
                    ui.add_space(crate::ui_ext::SPACE_MD);
                    ui.separator();
                    ui.add_space(crate::ui_ext::SPACE_SM);
                    ui.label(
                        egui::RichText::new("OPTIMIZED RESULT")
                            .size(crate::ui_ext::BODY_FONT_SIZE)
                            .strong()
                            .color(success),
                    );
                    ui.label(
                        egui::RichText::new(format!(
                            "{} of {}",
                            format_bytes_size(result.achieved_bytes),
                            format_bytes_size(result.target_bytes)
                        ))
                        .strong()
                        .color(success),
                    );
                    ui.label(
                        egui::RichText::new(format!(
                            "{} × {} · {:.0}% scale · Q{}",
                            result.width,
                            result.height,
                            result.scale * 100.0,
                            result.quality
                        ))
                        .size(crate::ui_ext::BODY_FONT_SIZE)
                        .color(text),
                    );
                    ui.add_space(crate::ui_ext::ITEM_SPACING_Y);

                    let apply = crate::action_button("Apply to canvas", success, palette);
                    if ui
                        .add_sized([ui.available_width(), crate::ui_ext::CONTROL_HEIGHT], apply)
                        .clicked()
                    {
                        self.apply_optimized_result();
                    }
                    let copy = crate::secondary_button("Copy to slot", surface, palette);
                    if ui
                        .add_sized([ui.available_width(), crate::ui_ext::CONTROL_HEIGHT], copy)
                        .clicked()
                    {
                        self.copy_optimized_to_slot();
                    }
                }
            }
        }

        if !self.undo_stack.is_empty() {
            ui.add_space(crate::ui_ext::SPACE_MD);
            ui.separator();
            ui.add_space(crate::ui_ext::ITEM_SPACING_Y);
            let undo = crate::secondary_button("Undo resize (Ctrl+Z)", surface, palette);
            if ui
                .add_sized([ui.available_width(), crate::ui_ext::CONTROL_HEIGHT], undo)
                .clicked()
            {
                self.undo();
            }
        }
    }
}
