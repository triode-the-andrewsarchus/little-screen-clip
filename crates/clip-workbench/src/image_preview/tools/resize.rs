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
    optimize_to_target_size_cancellable(base_img, target_bytes, format, progress, &|| false)
        .expect("synchronous optimization cannot be cancelled")
}

fn optimize_to_target_size_cancellable(
    base_img: &RgbaImage,
    target_bytes: usize,
    format: TargetFormat,
    progress: Option<&dyn Fn(String)>,
    cancelled: &dyn Fn() -> bool,
) -> Option<FileSizeOptimizationResult> {
    let search = TargetSizeSearch {
        image: base_img,
        target_bytes,
        target_str: format_bytes_size(target_bytes),
        progress,
        cancelled,
    };
    search.check().ok()?;
    match format {
        TargetFormat::Jpeg => search.optimize_jpeg().ok(),
        TargetFormat::Png => search.optimize_png().ok(),
    }
}

struct OptimizationCancelled;

type SearchResult<T> = Result<T, OptimizationCancelled>;

struct TargetSizeSearch<'a> {
    image: &'a RgbaImage,
    target_bytes: usize,
    target_str: String,
    progress: Option<&'a dyn Fn(String)>,
    cancelled: &'a dyn Fn() -> bool,
}

struct OptimizedCandidate {
    image: RgbaImage,
    bytes: Vec<u8>,
    scale: f32,
    quality: u8,
}

impl TargetSizeSearch<'_> {
    fn check(&self) -> SearchResult<()> {
        if (self.cancelled)() {
            Err(OptimizationCancelled)
        } else {
            Ok(())
        }
    }

    fn notify(&self, message: String) {
        if let Some(progress) = self.progress {
            progress(message);
        }
    }

    fn finish(
        &self,
        candidate: OptimizedCandidate,
        format: TargetFormat,
    ) -> FileSizeOptimizationResult {
        FileSizeOptimizationResult {
            target_bytes: self.target_bytes,
            achieved_bytes: candidate.bytes.len(),
            width: candidate.image.width(),
            height: candidate.image.height(),
            scale: candidate.scale,
            quality: candidate.quality,
            format,
            encoded_data: Arc::new(candidate.bytes),
            result_rgba: candidate.image,
        }
    }

    fn optimize_jpeg(&self) -> SearchResult<FileSizeOptimizationResult> {
        self.notify(format!(
            "Testing full resolution ({}×{}) at JPEG quality 95%...",
            self.image.width(),
            self.image.height()
        ));
        if let Some(candidate) = self.jpeg_full_resolution()? {
            return Ok(self.finish(candidate, TargetFormat::Jpeg));
        }

        self.notify(
            "Exceeds budget at 100% resolution. Scaling down with Lanczos3 filter...".into(),
        );
        let mut best = self.jpeg_scale_search()?;
        if best.bytes.is_empty() {
            self.jpeg_minimal_fallback(&mut best)?;
        } else {
            self.jpeg_tune_quality(&mut best)?;
        }
        self.check()?;
        self.notify(format!(
            "Complete: {} achieved at {}×{} (Quality {}%)",
            format_bytes_size(best.bytes.len()),
            best.image.width(),
            best.image.height(),
            best.quality
        ));
        Ok(self.finish(best, TargetFormat::Jpeg))
    }

    fn jpeg_full_resolution(&self) -> SearchResult<Option<OptimizedCandidate>> {
        if let Ok(bytes) = encode_jpeg_with_quality(self.image, 95) {
            self.check()?;
            if bytes.len() <= self.target_bytes {
                self.notify(format!(
                    "Full resolution fits budget: {} <= {}",
                    format_bytes_size(bytes.len()),
                    self.target_str
                ));
                return Ok(Some(OptimizedCandidate {
                    image: self.image.clone(),
                    bytes,
                    scale: 1.0,
                    quality: 95,
                }));
            }
            self.notify(format!(
                "Quality 95% is {} (budget: {}). Searching JPEG quality [60..95]...",
                format_bytes_size(bytes.len()),
                self.target_str
            ));
        }

        let Some((quality, bytes)) = self.jpeg_quality_search()? else {
            return Ok(None);
        };
        self.check()?;
        self.notify(format!(
            "Found optimal quality {}% at 100% resolution ({})",
            quality,
            format_bytes_size(bytes.len())
        ));
        Ok(Some(OptimizedCandidate {
            image: self.image.clone(),
            bytes,
            scale: 1.0,
            quality,
        }))
    }

    fn jpeg_quality_search(&self) -> SearchResult<Option<(u8, Vec<u8>)>> {
        let mut best = None;
        let mut low_q = 60u8;
        let mut high_q = 95u8;
        while low_q <= high_q {
            self.check()?;
            let mid_q = low_q + (high_q - low_q) / 2;
            self.notify(format!(
                "Trying quality {}% at 100% resolution ({}×{})...",
                mid_q,
                self.image.width(),
                self.image.height()
            ));
            let Ok(bytes) = encode_jpeg_with_quality(self.image, mid_q) else {
                break;
            };
            self.check()?;
            let len = bytes.len();
            if len <= self.target_bytes {
                self.notify(format!(
                    "Quality {}% fits budget: {} <= {}",
                    mid_q,
                    format_bytes_size(len),
                    self.target_str
                ));
                best = Some((mid_q, bytes));
                low_q = mid_q + 1;
            } else {
                self.notify(format!(
                    "Quality {}% exceeds budget: {} > {}",
                    mid_q,
                    format_bytes_size(len),
                    self.target_str
                ));
                if mid_q == 0 {
                    break;
                }
                high_q = mid_q - 1;
            }
        }
        Ok(best)
    }

    fn jpeg_scale_search(&self) -> SearchResult<OptimizedCandidate> {
        let mut best = OptimizedCandidate {
            image: self.image.clone(),
            bytes: Vec::new(),
            scale: 0.5,
            quality: 80,
        };
        let mut low_s = 0.05f32;
        let mut high_s = 1.0f32;
        for iter in 1..=10 {
            self.check()?;
            let mid_s = (low_s + high_s) / 2.0;
            let width = ((self.image.width() as f32) * mid_s).round().max(16.0) as u32;
            let height = ((self.image.height() as f32) * mid_s).round().max(16.0) as u32;
            self.notify(format!(
                "[Step {}/10] Resampling to {:.1}% ({}×{}) with Lanczos3...",
                iter,
                mid_s * 100.0,
                width,
                height
            ));
            let scaled = image::imageops::resize(
                self.image,
                width,
                height,
                image::imageops::FilterType::Lanczos3,
            );
            self.check()?;
            let Ok(bytes) = encode_jpeg_with_quality(&scaled, 80) else {
                break;
            };
            self.check()?;
            let len = bytes.len();
            if len <= self.target_bytes {
                self.notify(format!(
                    "Scale {:.1}% ({}×{}) fits: {} <= {}",
                    mid_s * 100.0,
                    width,
                    height,
                    format_bytes_size(len),
                    self.target_str
                ));
                best = OptimizedCandidate {
                    image: scaled,
                    bytes,
                    scale: mid_s,
                    quality: 80,
                };
                low_s = mid_s;
            } else {
                self.notify(format!(
                    "Scale {:.1}% ({}×{}) exceeds: {} > {}",
                    mid_s * 100.0,
                    width,
                    height,
                    format_bytes_size(len),
                    self.target_str
                ));
                high_s = mid_s;
            }
        }
        Ok(best)
    }

    fn jpeg_tune_quality(&self, best: &mut OptimizedCandidate) -> SearchResult<()> {
        self.notify(format!(
            "Fine-tuning JPEG quality on {}×{}...",
            best.image.width(),
            best.image.height()
        ));
        for quality in (81..=95).step_by(2) {
            self.check()?;
            if let Ok(bytes) = encode_jpeg_with_quality(&best.image, quality) {
                if bytes.len() > self.target_bytes {
                    break;
                }
                self.notify(format!(
                    "Tuned quality to {}%: {} <= {}",
                    quality,
                    format_bytes_size(bytes.len()),
                    self.target_str
                ));
                best.quality = quality;
                best.bytes = bytes;
            }
        }
        Ok(())
    }

    fn jpeg_minimal_fallback(&self, best: &mut OptimizedCandidate) -> SearchResult<()> {
        self.notify("Applying minimal scale fallback...".into());
        let width = ((self.image.width() as f32) * 0.1).round().max(16.0) as u32;
        let height = ((self.image.height() as f32) * 0.1).round().max(16.0) as u32;
        best.image = image::imageops::resize(
            self.image,
            width,
            height,
            image::imageops::FilterType::Triangle,
        );
        self.check()?;
        best.scale = 0.1;
        best.quality = 40;
        if let Ok(bytes) = encode_jpeg_with_quality(&best.image, 40) {
            best.bytes = bytes;
        }
        Ok(())
    }

    fn optimize_png(&self) -> SearchResult<FileSizeOptimizationResult> {
        self.notify(format!(
            "Testing full resolution ({}×{}) as lossless PNG...",
            self.image.width(),
            self.image.height()
        ));
        if let Some(candidate) = self.png_full_resolution()? {
            return Ok(self.finish(candidate, TargetFormat::Png));
        }
        let best = self.png_scale_search()?;
        self.check()?;
        self.notify(format!(
            "Complete: {} achieved at {}×{}",
            format_bytes_size(best.bytes.len()),
            best.image.width(),
            best.image.height()
        ));
        Ok(self.finish(best, TargetFormat::Png))
    }

    fn png_full_resolution(&self) -> SearchResult<Option<OptimizedCandidate>> {
        let mut cursor = std::io::Cursor::new(Vec::new());
        if self
            .image
            .write_to(&mut cursor, image::ImageFormat::Png)
            .is_ok()
        {
            self.check()?;
            let bytes = cursor.into_inner();
            if bytes.len() <= self.target_bytes {
                self.notify(format!(
                    "Full resolution PNG fits budget: {} <= {}",
                    format_bytes_size(bytes.len()),
                    self.target_str
                ));
                return Ok(Some(OptimizedCandidate {
                    image: self.image.clone(),
                    bytes,
                    scale: 1.0,
                    quality: 100,
                }));
            }
            self.notify(format!(
                "PNG at 100% is {} > {}. Searching resolution scale...",
                format_bytes_size(bytes.len()),
                self.target_str
            ));
        }
        Ok(None)
    }

    fn png_scale_search(&self) -> SearchResult<OptimizedCandidate> {
        let mut best = OptimizedCandidate {
            image: self.image.clone(),
            bytes: Vec::new(),
            scale: 1.0,
            quality: 100,
        };
        let mut low_s = 0.05f32;
        let mut high_s = 1.0f32;
        for iter in 1..=10 {
            self.check()?;
            let mid_s = (low_s + high_s) / 2.0;
            let width = ((self.image.width() as f32) * mid_s).round().max(16.0) as u32;
            let height = ((self.image.height() as f32) * mid_s).round().max(16.0) as u32;
            self.notify(format!(
                "[Step {}/10] Resampling to {:.1}% ({}×{})...",
                iter,
                mid_s * 100.0,
                width,
                height
            ));
            let scaled = image::imageops::resize(
                self.image,
                width,
                height,
                image::imageops::FilterType::Lanczos3,
            );
            self.check()?;
            let mut cursor = std::io::Cursor::new(Vec::new());
            if scaled
                .write_to(&mut cursor, image::ImageFormat::Png)
                .is_ok()
            {
                self.check()?;
                let bytes = cursor.into_inner();
                let len = bytes.len();
                if len <= self.target_bytes {
                    self.notify(format!(
                        "Scale {:.1}% fits: {} <= {}",
                        mid_s * 100.0,
                        format_bytes_size(len),
                        self.target_str
                    ));
                    best = OptimizedCandidate {
                        image: scaled,
                        bytes,
                        scale: mid_s,
                        quality: 100,
                    };
                    low_s = mid_s;
                } else {
                    self.notify(format!(
                        "Scale {:.1}% exceeds: {} > {}",
                        mid_s * 100.0,
                        format_bytes_size(len),
                        self.target_str
                    ));
                    high_s = mid_s;
                }
            }
        }
        Ok(best)
    }
}

impl ImagePreviewState {
    pub(crate) fn suspend_pending_work(&mut self) {
        self.cancel_pixel_resize();
        if let Some(ticket) = self.palette_ticket.take() {
            ticket.cancel();
        }
        self.palette_rx = None;
        self.film_sim_state.cancel_render();
        self.creative_fx_state.cancel_render();
        self.cancel_optimizer();
        self.cancel_optipng_process();
        self.cancel_ocr();
        self.cancel_barcode_scan();
        self.cancel_decorrelation_stretch();
        self.cancel_image_tool_process();
        self.film_sim_state.cancel_thumbnail_batch();
        self.cancel_creative_fx_thumbnail_batch();
        if let Some(control) = self.creative_fx_state.depth_process_control.take() {
            control.cancel();
        }
        if let Some(ticket) = self.creative_fx_state.depth_job_ticket.take() {
            ticket.cancel();
        }
        if let Some(control) = self.creative_fx_state.alpha_process_control.take() {
            control.cancel();
        }
        if let Some(ticket) = self.creative_fx_state.alpha_job_ticket.take() {
            ticket.cancel();
        }
    }
}

impl Drop for ImagePreviewState {
    fn drop(&mut self) {
        self.suspend_pending_work();
    }
}

#[derive(Debug)]
pub enum OptimizerMsg {
    Progress(String),
    Done(FileSizeOptimizationResult),
    Failed(String),
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

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct OptimizerRequestStamp {
    image: ImageInputStamp,
    target_bytes: usize,
    format: TargetFormat,
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
    pub encoded_data: Arc<Vec<u8>>,
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
    pub(crate) last_result_request_stamp: Option<OptimizerRequestStamp>,
    pub(crate) optimizer_request_stamp: Option<OptimizerRequestStamp>,
    #[allow(dead_code)]
    pub is_optimizing: bool,
}

#[derive(Debug, Clone, PartialEq)]
struct PixelResizeStamp {
    image: ImageInputStamp,
    width: u32,
    height: u32,
    filter: ResizeFilter,
}

#[derive(Clone)]
struct PixelResizeRequest {
    source: Arc<RgbaImage>,
    redactions: Vec<RedactionBox>,
    stamp: PixelResizeStamp,
}

struct PixelResizeMessage {
    stamp: PixelResizeStamp,
    result: Result<RgbaImage, String>,
}

#[derive(Default)]
pub struct PixelResizeJobState {
    rx: Option<
        std::sync::mpsc::Receiver<Result<Option<PixelResizeMessage>, little_clip_core::JobError>>,
    >,
    ticket: Option<little_clip_core::JobTicket>,
    cancel: Option<Arc<std::sync::atomic::AtomicBool>>,
    stamp: Option<PixelResizeStamp>,
    pub(crate) retry_at: Option<std::time::Instant>,
    retry_request: Option<PixelResizeRequest>,
    pub(crate) pending: bool,
    pub(crate) status: Option<String>,
    pub(crate) error: Option<String>,
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
            last_result_request_stamp: None,
            optimizer_request_stamp: None,
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
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    fn wait_for_pixel_resize(state: &mut ImagePreviewState, jobs: &BackgroundJobs) {
        let ctx = egui::Context::default();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while (state.pixel_resize_job.pending || state.pixel_resize_job.retry_at.is_some())
            && std::time::Instant::now() < deadline
        {
            state.poll_pixel_resize(&ctx, jobs);
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        state.poll_pixel_resize(&ctx, jobs);
        assert!(!state.pixel_resize_job.pending);
        assert!(state.pixel_resize_job.error.is_none());
    }

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

    #[test]
    fn bounded_pixel_resize_matches_direct_filter_output_and_preserves_markup() {
        let jobs = BackgroundJobs::new(1, 1);
        let mut state = state();
        let source = RgbaImage::from_fn(7, 5, |x, y| {
            image::Rgba([(x * 31) as u8, (y * 47) as u8, (x * 7 + y * 9) as u8, 255])
        });
        state.current_rgba = Arc::new(source.clone());
        state.markup_state.items.push(MarkupShape::Rectangle {
            rect: Rect::from_min_max(Pos2::new(0.1, 0.2), Pos2::new(0.8, 0.9)),
            color: Color32::RED,
            width: 2.0,
            filled: false,
            rounding: 0.0,
        });
        let markup_before = state.markup_state.items.clone();
        let expected =
            image::imageops::resize(&source, 11, 9, ResizeFilter::Triangle.to_image_filter());

        state.apply_pixel_resize_on(11, 9, ResizeFilter::Triangle, &jobs);
        assert!(state.pixel_resize_job.pending);
        wait_for_pixel_resize(&mut state, &jobs);

        assert_eq!(state.current_rgba.as_ref(), &expected);
        assert_eq!(state.markup_state.items, markup_before);
        assert_eq!(state.mime, "image/png");
        assert_eq!(state.undo_stack.len(), 1);
        assert_eq!(
            (state.current_rgba.width(), state.current_rgba.height()),
            (11, 9)
        );
    }

    #[test]
    fn resize_with_pending_redactions_matches_legacy_output_and_keeps_two_undo_steps() {
        let jobs = BackgroundJobs::new(1, 1);
        let mut state = state();
        let source = RgbaImage::from_fn(8, 6, |x, y| {
            image::Rgba([(x * 20) as u8, (y * 30) as u8, 90, 255])
        });
        state.current_rgba = Arc::new(source.clone());
        let redactions = vec![RedactionBox {
            normalized_rect: Rect::from_min_max(Pos2::new(0.25, 0.25), Pos2::new(0.75, 0.75)),
        }];
        state.redaction_state.boxes = redactions.clone();
        state.active_mode = ImageToolMode::Resize;
        let baked = super::super::redact::compose_redactions_only(&source, &redactions);
        let expected =
            image::imageops::resize(&baked, 4, 3, ResizeFilter::Lanczos3.to_image_filter());

        state.apply_pixel_resize_on(4, 3, ResizeFilter::Lanczos3, &jobs);
        wait_for_pixel_resize(&mut state, &jobs);
        assert_eq!(state.current_rgba.as_ref(), &expected);
        assert!(state.redaction_state.boxes.is_empty());
        assert_eq!(state.undo_stack.len(), 2);
        state.undo();
        assert_eq!(state.current_rgba.as_ref(), &baked);
        state.undo();
        assert_eq!(state.current_rgba.as_ref(), &source);
    }

    #[test]
    fn pixel_resize_queue_full_shows_retry_and_reuses_source_arc() {
        let jobs = BackgroundJobs::new(1, 1);
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let (_, blocker_rx) = jobs.submit(std::time::Duration::MAX, move || {
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        });
        started_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap();
        let (_, filler_rx) = jobs.submit(std::time::Duration::MAX, || ());
        let mut state = state();
        let before = state.current_rgba.clone();
        state.apply_pixel_resize_on(8, 7, ResizeFilter::Nearest, &jobs);
        let retained = Arc::clone(
            &state
                .pixel_resize_job
                .retry_request
                .as_ref()
                .unwrap()
                .source,
        );
        state.poll_pixel_resize(&egui::Context::default(), &jobs);
        assert!(!state.pixel_resize_job.pending);
        assert!(state.pixel_resize_job.retry_at.is_some());
        assert!(
            state
                .pixel_resize_job
                .status
                .as_deref()
                .unwrap()
                .contains("busy")
        );
        assert!(Arc::ptr_eq(
            &retained,
            &state
                .pixel_resize_job
                .retry_request
                .as_ref()
                .unwrap()
                .source
        ));
        assert!(Arc::ptr_eq(&before, &state.current_rgba));
        let retry_at = state.pixel_resize_job.retry_at.unwrap();
        state.poll_pixel_resize(&egui::Context::default(), &jobs);
        assert_eq!(state.pixel_resize_job.retry_at, Some(retry_at));

        release_tx.send(()).unwrap();
        blocker_rx.recv().unwrap().unwrap();
        filler_rx.recv().unwrap().unwrap();
        state.pixel_resize_job.retry_at = Some(std::time::Instant::now());
        state.poll_pixel_resize(&egui::Context::default(), &jobs);
        assert!(state.pixel_resize_job.pending);
        assert!(Arc::ptr_eq(
            &retained,
            &state
                .pixel_resize_job
                .retry_request
                .as_ref()
                .unwrap()
                .source
        ));
        wait_for_pixel_resize(&mut state, &jobs);
        assert!(state.pixel_resize_job.status.is_none());
    }

    #[test]
    fn stale_resize_settings_cancel_queued_work_and_viewer_drop_cancels_ticket() {
        let jobs = BackgroundJobs::new(1, 2);
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let (_, blocker_rx) = jobs.submit(std::time::Duration::MAX, move || {
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        });
        started_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap();
        let mut active = state();
        active.apply_pixel_resize_on(9, 8, ResizeFilter::Nearest, &jobs);
        let cancel = active.pixel_resize_job.cancel.as_ref().unwrap().clone();
        let worker_rx = active.pixel_resize_job.rx.take().unwrap();
        active.resize_state.filter = ResizeFilter::Triangle;
        active.poll_pixel_resize(&egui::Context::default(), &jobs);
        assert!(cancel.load(Ordering::Acquire));
        assert!(!active.pixel_resize_job.pending);
        release_tx.send(()).unwrap();
        blocker_rx.recv().unwrap().unwrap();
        assert!(matches!(
            worker_rx
                .recv_timeout(std::time::Duration::from_secs(1))
                .unwrap(),
            Err(little_clip_core::JobError::Cancelled)
        ));

        let jobs = BackgroundJobs::new(1, 2);
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let (_, blocker_rx) = jobs.submit(std::time::Duration::MAX, move || {
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        });
        started_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap();
        let mut dropped = state();
        dropped.apply_pixel_resize_on(9, 8, ResizeFilter::Nearest, &jobs);
        let cancel = dropped.pixel_resize_job.cancel.as_ref().unwrap().clone();
        let worker_rx = dropped.pixel_resize_job.rx.take().unwrap();
        drop(dropped);
        assert!(cancel.load(Ordering::Acquire));
        release_tx.send(()).unwrap();
        blocker_rx.recv().unwrap().unwrap();
        assert!(matches!(
            worker_rx
                .recv_timeout(std::time::Duration::from_secs(1))
                .unwrap(),
            Err(little_clip_core::JobError::Cancelled)
        ));
    }

    fn result(color: [u8; 4]) -> FileSizeOptimizationResult {
        FileSizeOptimizationResult {
            target_bytes: 1024,
            achieved_bytes: 90,
            width: 4,
            height: 4,
            scale: 1.0,
            quality: 80,
            format: TargetFormat::Png,
            encoded_data: Arc::new(vec![1, 2, 3]),
            result_rgba: RgbaImage::from_pixel(4, 4, image::Rgba(color)),
        }
    }

    #[test]
    fn optimizer_discards_stale_result_and_keeps_current_result_applicable() {
        let mut stale = state();
        stale.resize_state.target_size_unit_mb = false;
        stale.resize_state.target_size_val = 1.0;
        stale.resize_state.target_format = TargetFormat::Png;
        stale.resize_state.optimizer_request_stamp = Some(stale.current_optimizer_request_stamp());
        stale.resize_state.is_optimizing = true;
        stale.optimizer_status = Some("working".into());
        let cancelled = Arc::new(AtomicBool::new(false));
        stale.optimizer_cancel = Some(cancelled.clone());
        let (tx, rx) = std::sync::mpsc::channel();
        stale.optimizer_rx = Some(rx);
        tx.send(OptimizerMsg::Done(result([1, 2, 3, 255]))).unwrap();
        stale.mark_pixels_changed();

        stale.poll_optimizer();

        assert!(stale.resize_state.last_result.is_none());
        assert!(!stale.resize_state.is_optimizing);
        assert!(stale.optimizer_status.is_none());
        assert!(stale.optimizer_rx.is_none());
        assert!(cancelled.load(Ordering::Acquire));

        let mut current = state();
        current.resize_state.target_size_unit_mb = false;
        current.resize_state.target_size_val = 1.0;
        current.resize_state.target_format = TargetFormat::Png;
        current.resize_state.optimizer_request_stamp =
            Some(current.current_optimizer_request_stamp());
        current.resize_state.is_optimizing = true;
        let (tx, rx) = std::sync::mpsc::channel();
        current.optimizer_rx = Some(rx);
        tx.send(OptimizerMsg::Done(result([7, 8, 9, 255]))).unwrap();

        current.poll_optimizer();

        assert!(current.resize_state.last_result.is_some());
        assert_eq!(
            current.resize_state.last_result_request_stamp,
            Some(current.current_optimizer_request_stamp())
        );
        let original_pixel = current.current_rgba.get_pixel(0, 0).to_owned();
        current.resize_state.target_size_val = 2.0;
        assert!(!current.optimized_result_is_current());
        current.apply_optimized_result();
        assert_eq!(current.current_rgba.get_pixel(0, 0), &original_pixel);
        current.resize_state.target_size_val = 1.0;
        current.apply_optimized_result();
        assert_eq!(
            current.current_rgba.get_pixel(0, 0),
            &image::Rgba([7, 8, 9, 255])
        );
        assert_eq!(current.pixel_revision, 1);
        assert!(current.resize_state.last_result_request_stamp.is_none());
    }

    #[test]
    fn optimizer_discards_completion_when_target_format_or_size_changes() {
        for change_format in [false, true] {
            let mut state = state();
            state.resize_state.target_size_unit_mb = false;
            state.resize_state.target_size_val = 1.0;
            state.resize_state.target_format = TargetFormat::Png;
            state.resize_state.optimizer_request_stamp =
                Some(state.current_optimizer_request_stamp());
            state.resize_state.is_optimizing = true;
            state.optimizer_status = Some("working".into());
            let cancelled = Arc::new(AtomicBool::new(false));
            state.optimizer_cancel = Some(cancelled.clone());
            let (tx, rx) = std::sync::mpsc::channel();
            state.optimizer_rx = Some(rx);
            tx.send(OptimizerMsg::Done(result([1, 2, 3, 255]))).unwrap();

            if change_format {
                state.resize_state.target_format = TargetFormat::Jpeg;
            } else {
                state.resize_state.target_size_val = 2.0;
            }
            state.poll_optimizer();

            assert!(state.resize_state.last_result.is_none());
            assert!(!state.resize_state.is_optimizing);
            assert!(state.optimizer_status.is_none());
            assert!(state.optimizer_rx.is_none());
            assert!(cancelled.load(Ordering::Acquire));
        }
    }

    #[test]
    fn dropping_image_preview_cancels_optimizer() {
        let cancelled = Arc::new(AtomicBool::new(false));
        let mut state = state();
        state.optimizer_cancel = Some(cancelled.clone());
        drop(state);
        assert!(cancelled.load(Ordering::Acquire));
    }

    #[test]
    fn optimizer_stops_between_resample_and_encode() {
        let img = RgbaImage::from_fn(64, 64, |x, y| {
            image::Rgba([(x * 17) as u8, (y * 31) as u8, (x ^ y) as u8, 255])
        });
        let cancelled = AtomicBool::new(false);
        let progress = |status: String| {
            if status.contains("Resampling") {
                cancelled.store(true, Ordering::Release);
            }
        };

        let result = optimize_to_target_size_cancellable(
            &img,
            1,
            TargetFormat::Png,
            Some(&progress),
            &|| cancelled.load(Ordering::Acquire),
        );

        assert!(cancelled.load(Ordering::Acquire));
        assert!(result.is_none());
    }

    #[test]
    fn optimizer_stops_after_jpeg_quality_search_is_cancelled() {
        let img = RgbaImage::from_fn(64, 64, |x, y| {
            image::Rgba([(x * 17) as u8, (y * 31) as u8, (x ^ y) as u8, 255])
        });
        let cancelled = AtomicBool::new(false);
        let progress = |status: String| {
            if status.contains("Trying quality") {
                cancelled.store(true, Ordering::Release);
            }
        };

        let result = optimize_to_target_size_cancellable(
            &img,
            1,
            TargetFormat::Jpeg,
            Some(&progress),
            &|| cancelled.load(Ordering::Acquire),
        );

        assert!(cancelled.load(Ordering::Acquire));
        assert!(result.is_none());
    }

    #[test]
    fn bounded_optimizer_matches_direct_tiny_png_output_and_streams_progress() {
        let mut state = state();
        state.resize_state.target_format = TargetFormat::Png;
        state.resize_state.target_size_unit_mb = false;
        state.resize_state.target_size_val = 1.0;
        let image = state.current_display_image();
        let expected = optimize_to_target_size_with_progress(
            &image,
            state.resize_state.target_bytes(),
            TargetFormat::Png,
            None,
        );
        let jobs = little_clip_core::BackgroundJobs::new(1, 1);
        state.active_mode = ImageToolMode::Resize;
        state.resize_state.sub_tab = ResizeSubTab::FileSize;
        state.run_target_file_size_optimizer_on(&jobs);
        let saw_progress = matches!(
            state
                .optimizer_rx
                .as_ref()
                .unwrap()
                .recv_timeout(std::time::Duration::from_secs(1))
                .unwrap(),
            OptimizerMsg::Progress(status) if status.contains("full resolution")
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while state.resize_state.is_optimizing && std::time::Instant::now() < deadline {
            state.poll_optimizer_on(&jobs);
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        state.poll_optimizer_on(&jobs);
        let actual = state.resize_state.last_result.as_ref().unwrap();
        assert!(saw_progress);
        assert_eq!(actual.encoded_data, expected.encoded_data);
        assert_eq!(actual.result_rgba, expected.result_rgba);
        assert_eq!(actual.width, expected.width);
        assert_eq!(actual.height, expected.height);
    }

    #[test]
    fn optimizer_queue_full_preserves_result_and_retries_after_delay() {
        let jobs = little_clip_core::BackgroundJobs::new(1, 1);
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let (_, blocker_rx) = jobs.submit(std::time::Duration::MAX, move || {
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        });
        started_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap();
        let (_, queued_rx) = jobs.submit(std::time::Duration::MAX, || ());
        let mut state = state();
        state.active_mode = ImageToolMode::Resize;
        state.resize_state.sub_tab = ResizeSubTab::FileSize;
        state.resize_state.last_result = Some(result([2, 3, 4, 255]));
        state.resize_state.last_result_request_stamp =
            Some(state.current_optimizer_request_stamp());
        state.run_target_file_size_optimizer_on(&jobs);
        let retained_pixels =
            std::sync::Arc::clone(&state.optimizer_retry_snapshot.as_ref().unwrap().pixels);
        state.poll_optimizer_on(&jobs);

        assert!(!state.resize_state.is_optimizing);
        assert!(state.optimizer_retry_at.is_some());
        assert!(std::sync::Arc::ptr_eq(
            &retained_pixels,
            &state.optimizer_retry_snapshot.as_ref().unwrap().pixels
        ));
        assert!(
            state
                .optimizer_status
                .as_deref()
                .is_some_and(|status| status.contains("busy"))
        );
        assert_eq!(
            state
                .resize_state
                .last_result
                .as_ref()
                .unwrap()
                .encoded_data
                .as_slice(),
            [1, 2, 3]
        );
        let retry_deadline = state.optimizer_retry_at.unwrap();
        state.poll_optimizer_on(&jobs);
        assert!(!state.resize_state.is_optimizing);
        assert_eq!(state.optimizer_retry_at, Some(retry_deadline));

        release_tx.send(()).unwrap();
        blocker_rx.recv().unwrap().unwrap();
        queued_rx.recv().unwrap().unwrap();
        state.optimizer_retry_at = Some(std::time::Instant::now());
        state.poll_optimizer_on(&jobs);
        assert!(state.resize_state.is_optimizing);
        assert!(state.optimizer_retry_at.is_none());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while state.resize_state.is_optimizing && std::time::Instant::now() < deadline {
            state.poll_optimizer_on(&jobs);
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        state.poll_optimizer_on(&jobs);
        assert!(state.resize_state.last_result.is_some());
    }

    #[test]
    fn optimizer_drop_cancels_queued_ticket_and_cooperative_flag() {
        let jobs = little_clip_core::BackgroundJobs::new(1, 2);
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let (_, blocker_rx) = jobs.submit(std::time::Duration::MAX, move || {
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        });
        started_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap();
        let (_, queued_rx) = jobs.submit(std::time::Duration::MAX, || ());
        let mut state = state();
        state.run_target_file_size_optimizer_on(&jobs);
        let cancel = state.optimizer_cancel.as_ref().unwrap().clone();
        let worker_rx = state.optimizer_job_rx.take().unwrap();
        drop(state);
        assert!(cancel.load(Ordering::Acquire));

        release_tx.send(()).unwrap();
        blocker_rx.recv().unwrap().unwrap();
        queued_rx.recv().unwrap().unwrap();
        assert_eq!(
            worker_rx
                .recv_timeout(std::time::Duration::from_secs(1))
                .unwrap(),
            Err(little_clip_core::JobError::Cancelled)
        );
    }

    #[test]
    fn optimizer_rejects_stale_retry_and_completion() {
        let mut retry = state();
        retry.active_mode = ImageToolMode::Resize;
        retry.resize_state.sub_tab = ResizeSubTab::FileSize;
        retry.optimizer_retry_stamp = Some(retry.current_optimizer_request_stamp());
        retry.optimizer_retry_at =
            Some(std::time::Instant::now() - std::time::Duration::from_millis(1));
        retry.optimizer_status = Some("waiting".into());
        retry.resize_state.target_size_val += 1.0;
        retry.poll_optimizer_on(&little_clip_core::BackgroundJobs::new(1, 1));
        assert!(retry.optimizer_retry_at.is_none());
        assert!(!retry.resize_state.is_optimizing);
        assert!(retry.optimizer_status.is_none());

        let mut stale = state();
        stale.resize_state.target_size_unit_mb = false;
        stale.resize_state.target_size_val = 1.0;
        stale.resize_state.target_format = TargetFormat::Png;
        stale.resize_state.optimizer_request_stamp = Some(stale.current_optimizer_request_stamp());
        stale.resize_state.is_optimizing = true;
        let cancelled = Arc::new(AtomicBool::new(false));
        stale.optimizer_cancel = Some(cancelled.clone());
        let (tx, rx) = std::sync::mpsc::channel();
        stale.optimizer_rx = Some(rx);
        tx.send(OptimizerMsg::Done(result([9, 8, 7, 255]))).unwrap();
        stale.resize_state.target_size_val = 2.0;
        stale.poll_optimizer_on(&little_clip_core::BackgroundJobs::new(1, 1));
        assert!(stale.resize_state.last_result.is_none());
        assert!(cancelled.load(Ordering::Acquire));
    }
}

impl ImagePreviewState {
    pub(crate) fn cancel_optimizer(&mut self) {
        if let Some(ticket) = self.optimizer_ticket.take() {
            ticket.cancel();
        }
        if let Some(cancelled) = self.optimizer_cancel.take() {
            cancelled.store(true, std::sync::atomic::Ordering::Release);
        }
        self.optimizer_rx = None;
        self.optimizer_job_rx = None;
        self.optimizer_retry_at = None;
        self.optimizer_retry_stamp = None;
        self.optimizer_retry_snapshot = None;
        self.resize_state.optimizer_request_stamp = None;
        self.resize_state.is_optimizing = false;
        self.optimizer_status = None;
    }

    pub fn apply_pixel_resize(&mut self, target_w: u32, target_h: u32, filter: ResizeFilter) {
        self.apply_pixel_resize_on(target_w, target_h, filter, BackgroundJobs::global());
    }

    fn apply_pixel_resize_on(
        &mut self,
        target_w: u32,
        target_h: u32,
        filter: ResizeFilter,
        jobs: &BackgroundJobs,
    ) {
        let (w, h) = (target_w.clamp(1, 16384), target_h.clamp(1, 16384));
        if (w == self.current_rgba.width() && h == self.current_rgba.height())
            || self.pixel_resize_job.pending
        {
            return;
        }

        self.cancel_pixel_resize();
        self.resize_state.target_width = w;
        self.resize_state.target_height = h;
        self.resize_state.width_input = w.to_string();
        self.resize_state.height_input = h.to_string();
        self.resize_state.filter = filter;
        let snapshot = self.display_image_snapshot_request();
        let request = PixelResizeRequest {
            source: snapshot.pixels,
            redactions: self.redaction_state.boxes.clone(),
            stamp: PixelResizeStamp {
                image: snapshot.input_stamp,
                width: w,
                height: h,
                filter,
            },
        };
        self.submit_pixel_resize(request, jobs);
    }

    fn current_pixel_resize_stamp(&self) -> PixelResizeStamp {
        PixelResizeStamp {
            image: self.input_stamp(),
            width: self.resize_state.target_width.clamp(1, 16384),
            height: self.resize_state.target_height.clamp(1, 16384),
            filter: self.resize_state.filter,
        }
    }

    fn submit_pixel_resize(&mut self, request: PixelResizeRequest, jobs: &BackgroundJobs) {
        let worker_request = request.clone();
        let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let (ticket, rx) = jobs.submit(std::time::Duration::MAX, move || {
            if worker_cancel.load(std::sync::atomic::Ordering::Acquire) {
                return None;
            }
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let prepared = super::redact::compose_redactions_only(
                    &worker_request.source,
                    &worker_request.redactions,
                );
                image::imageops::resize(
                    &prepared,
                    worker_request.stamp.width,
                    worker_request.stamp.height,
                    worker_request.stamp.filter.to_image_filter(),
                )
            }))
            .map_err(|_| "Resize worker panicked".to_string());
            if worker_cancel.load(std::sync::atomic::Ordering::Acquire) {
                return None;
            }
            Some(PixelResizeMessage {
                stamp: worker_request.stamp,
                result,
            })
        });
        self.pixel_resize_job.ticket = Some(ticket);
        self.pixel_resize_job.rx = Some(rx);
        self.pixel_resize_job.cancel = Some(cancel);
        self.pixel_resize_job.stamp = Some(request.stamp.clone());
        self.pixel_resize_job.retry_request = Some(request);
        self.pixel_resize_job.retry_at = None;
        self.pixel_resize_job.pending = true;
        self.pixel_resize_job.status = Some("Resizing image…".into());
        self.pixel_resize_job.error = None;
    }

    pub(crate) fn poll_pixel_resize(&mut self, ctx: &egui::Context, jobs: &BackgroundJobs) {
        if self.pixel_resize_job.pending
            && self.pixel_resize_job.stamp.as_ref() != Some(&self.current_pixel_resize_stamp())
        {
            self.cancel_pixel_resize();
            return;
        }
        if self.pixel_resize_job.retry_at.is_some()
            && self
                .pixel_resize_job
                .retry_request
                .as_ref()
                .map(|r| &r.stamp)
                != Some(&self.current_pixel_resize_stamp())
        {
            self.cancel_pixel_resize();
            return;
        }
        if let Some(retry_at) = self.pixel_resize_job.retry_at {
            if std::time::Instant::now() < retry_at {
                ctx.request_repaint_after(
                    retry_at.saturating_duration_since(std::time::Instant::now()),
                );
                return;
            }
            if let Some(request) = self.pixel_resize_job.retry_request.take() {
                self.submit_pixel_resize(request, jobs);
            }
            return;
        }
        let Some(rx) = self.pixel_resize_job.rx.take() else {
            return;
        };
        match rx.try_recv() {
            Ok(Ok(Some(message))) => {
                if message.stamp != self.current_pixel_resize_stamp() {
                    self.clear_pixel_resize_job();
                    return;
                }
                match message.result {
                    Ok(resized) => {
                        let redactions = self
                            .pixel_resize_job
                            .retry_request
                            .as_ref()
                            .map(|request| !request.redactions.is_empty())
                            .unwrap_or(false);
                        self.clear_pixel_resize_job();
                        if redactions {
                            self.apply_redactions();
                        }
                        self.undo_stack.push(self.current_rgba.clone());
                        self.redo_stack.clear();
                        self.current_rgba = Arc::new(resized);
                        self.mark_pixels_changed();
                        self.dirty_texture = true;
                        self.resize_state.sync_from_image(&self.current_rgba);
                        self.palette_state.is_extracted = false;
                        self.show_toast(format!(
                            "↔ Resized to {}×{} px ({})",
                            message.stamp.width,
                            message.stamp.height,
                            message.stamp.filter.label()
                        ));
                    }
                    Err(error) => {
                        self.clear_pixel_resize_job();
                        self.pixel_resize_job.error = Some(error);
                        self.pixel_resize_job.status = None;
                    }
                }
            }
            Ok(Err(little_clip_core::JobError::QueueFull)) => {
                self.pixel_resize_job.ticket = None;
                self.pixel_resize_job.rx = None;
                self.pixel_resize_job.cancel = None;
                self.pixel_resize_job.stamp = None;
                self.pixel_resize_job.pending = false;
                self.pixel_resize_job.status =
                    Some("Background workers busy; retrying resize…".into());
                self.pixel_resize_job.retry_at =
                    Some(std::time::Instant::now() + std::time::Duration::from_millis(500));
                ctx.request_repaint_after(std::time::Duration::from_millis(500));
            }
            Ok(Err(little_clip_core::JobError::Cancelled)) => self.clear_pixel_resize_job(),
            Ok(Err(error)) => {
                self.clear_pixel_resize_job();
                self.pixel_resize_job.error = Some(format!("Resize worker failed: {error}"));
            }
            Ok(Ok(None)) => self.clear_pixel_resize_job(),
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                self.pixel_resize_job.rx = Some(rx);
                ctx.request_repaint_after(std::time::Duration::from_millis(16));
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.clear_pixel_resize_job();
                self.pixel_resize_job.error = Some("Resize worker stopped unexpectedly".into());
            }
        }
    }

    fn clear_pixel_resize_job(&mut self) {
        self.pixel_resize_job.ticket = None;
        self.pixel_resize_job.rx = None;
        self.pixel_resize_job.cancel = None;
        self.pixel_resize_job.stamp = None;
        self.pixel_resize_job.retry_at = None;
        self.pixel_resize_job.retry_request = None;
        self.pixel_resize_job.pending = false;
        self.pixel_resize_job.status = None;
    }

    pub(crate) fn cancel_pixel_resize(&mut self) {
        if let Some(ticket) = self.pixel_resize_job.ticket.take() {
            ticket.cancel();
        }
        if let Some(cancel) = self.pixel_resize_job.cancel.take() {
            cancel.store(true, std::sync::atomic::Ordering::Release);
        }
        self.pixel_resize_job.rx = None;
        self.pixel_resize_job.stamp = None;
        self.pixel_resize_job.retry_at = None;
        self.pixel_resize_job.retry_request = None;
        self.pixel_resize_job.pending = false;
        self.pixel_resize_job.status = None;
    }

    fn current_optimizer_request_stamp(&self) -> OptimizerRequestStamp {
        OptimizerRequestStamp {
            image: self.input_stamp(),
            target_bytes: self.resize_state.target_bytes(),
            format: self.resize_state.target_format,
        }
    }

    fn optimized_result_is_current(&self) -> bool {
        self.resize_state.last_result_request_stamp.as_ref()
            == Some(&self.current_optimizer_request_stamp())
    }

    /// Runs the target file size optimizer solving the quality vs. resolution trade-off asynchronously.

    pub fn run_target_file_size_optimizer(&mut self) {
        self.run_target_file_size_optimizer_on(little_clip_core::BackgroundJobs::global());
    }

    fn run_target_file_size_optimizer_on(&mut self, jobs: &little_clip_core::BackgroundJobs) {
        if self.resize_state.is_optimizing {
            return;
        }

        let request_stamp = self.current_optimizer_request_stamp();
        let target_bytes = request_stamp.target_bytes;
        let format = request_stamp.format;
        let snapshot = self
            .optimizer_retry_snapshot
            .take()
            .unwrap_or_else(|| self.display_image_snapshot_request());
        self.optimizer_retry_snapshot = Some(snapshot.clone());
        self.resize_state.optimizer_request_stamp = Some(request_stamp);

        let (progress_tx, rx) = std::sync::mpsc::channel::<OptimizerMsg>();
        let cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let worker_cancelled = cancelled.clone();
        self.optimizer_cancel = Some(cancelled);
        self.optimizer_rx = Some(rx);
        self.optimizer_retry_at = None;
        self.optimizer_retry_stamp = None;
        self.resize_state.is_optimizing = true;
        self.optimizer_status = Some("Starting optimizer...".to_string());

        let (ticket, job_rx) = jobs.submit(std::time::Duration::MAX, move || {
            let tx_prog = progress_tx.clone();
            let progress_cb = move |msg: String| {
                let _ = tx_prog.send(OptimizerMsg::Progress(msg));
            };

            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                optimize_to_target_size_cancellable(
                    &snapshot.compose(),
                    target_bytes,
                    format,
                    Some(&progress_cb),
                    &|| worker_cancelled.load(std::sync::atomic::Ordering::Acquire),
                )
            }));

            if !worker_cancelled.load(std::sync::atomic::Ordering::Acquire) {
                match result {
                    Ok(Some(result)) => {
                        let _ = progress_tx.send(OptimizerMsg::Done(result));
                    }
                    Ok(None) => {}
                    Err(_) => {
                        let _ = progress_tx
                            .send(OptimizerMsg::Failed("Optimizer worker panicked".into()));
                    }
                }
            }
        });
        self.optimizer_ticket = Some(ticket);
        self.optimizer_job_rx = Some(job_rx);
    }

    /// Polls the background file size optimizer channel, updating status messages
    /// and storing the final result when complete.

    pub fn poll_optimizer(&mut self) {
        self.poll_optimizer_on(little_clip_core::BackgroundJobs::global());
    }

    fn poll_optimizer_on(&mut self, jobs: &little_clip_core::BackgroundJobs) {
        if self.optimizer_rx.is_some()
            && self.resize_state.optimizer_request_stamp.as_ref()
                != Some(&self.current_optimizer_request_stamp())
        {
            self.cancel_optimizer();
            return;
        }
        if self.optimizer_retry_at.is_some()
            && self.optimizer_retry_stamp.as_ref() != Some(&self.current_optimizer_request_stamp())
        {
            self.optimizer_retry_at = None;
            self.optimizer_retry_stamp = None;
            self.optimizer_retry_snapshot = None;
            self.optimizer_status = None;
        }
        if let Some(retry_at) = self.optimizer_retry_at {
            if std::time::Instant::now() < retry_at {
                return;
            }
            self.optimizer_retry_at = None;
            self.run_target_file_size_optimizer_on(jobs);
            return;
        }
        let mut done_res = None;
        let mut worker_error = None;
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
                    Ok(OptimizerMsg::Failed(error)) => {
                        worker_error = Some(error);
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
        let mut queue_full = false;
        let mut job_failed = None;
        if let Some(job_rx) = self.optimizer_job_rx.take() {
            match job_rx.try_recv() {
                Ok(Ok(())) => {
                    self.optimizer_ticket = None;
                    self.optimizer_cancel = None;
                    self.optimizer_job_rx = None;
                }
                Ok(Err(little_clip_core::JobError::QueueFull)) => {
                    queue_full = true;
                    self.optimizer_ticket = None;
                    self.optimizer_cancel = None;
                    self.optimizer_job_rx = None;
                }
                Ok(Err(little_clip_core::JobError::Cancelled)) => {
                    self.optimizer_ticket = None;
                    self.optimizer_cancel = None;
                    self.optimizer_job_rx = None;
                    self.resize_state.is_optimizing = false;
                    self.resize_state.optimizer_request_stamp = None;
                    self.optimizer_rx = None;
                    self.optimizer_status = None;
                    self.optimizer_retry_snapshot = None;
                    return;
                }
                Ok(Err(error)) => {
                    job_failed = Some(error.to_string());
                    self.optimizer_ticket = None;
                    self.optimizer_cancel = None;
                    self.optimizer_job_rx = None;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    self.optimizer_job_rx = Some(job_rx);
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    disconnected = true;
                    self.optimizer_ticket = None;
                    self.optimizer_cancel = None;
                    self.optimizer_job_rx = None;
                }
            }
        }
        if queue_full {
            self.resize_state.is_optimizing = false;
            self.resize_state.optimizer_request_stamp = None;
            self.optimizer_rx = None;
            self.optimizer_status = Some("Background workers are busy; retrying shortly".into());
            self.optimizer_retry_stamp = Some(self.current_optimizer_request_stamp());
            self.optimizer_retry_at =
                Some(std::time::Instant::now() + std::time::Duration::from_millis(500));
            return;
        }
        if let Some(error) = job_failed {
            self.resize_state.is_optimizing = false;
            self.resize_state.optimizer_request_stamp = None;
            self.optimizer_rx = None;
            self.optimizer_status = Some(format!("Optimizer worker failed: {error}"));
            self.optimizer_retry_snapshot = None;
            return;
        }
        if let Some(error) = worker_error {
            self.resize_state.is_optimizing = false;
            self.resize_state.optimizer_request_stamp = None;
            self.optimizer_rx = None;
            self.optimizer_cancel = None;
            self.optimizer_ticket = None;
            self.optimizer_job_rx = None;
            self.optimizer_status = Some(error);
            self.optimizer_retry_snapshot = None;
            return;
        }
        if let Some(res) = done_res {
            self.resize_state.last_result = Some(res);
            self.resize_state.last_result_request_stamp =
                self.resize_state.optimizer_request_stamp.take();
            self.resize_state.is_optimizing = false;
            self.optimizer_status = None;
            self.optimizer_rx = None;
            self.optimizer_cancel = None;
            self.optimizer_ticket = None;
            self.optimizer_job_rx = None;
            self.resize_state.optimizer_request_stamp = None;
            self.optimizer_retry_snapshot = None;
        } else if disconnected {
            self.resize_state.is_optimizing = false;
            self.optimizer_status = None;
            self.optimizer_rx = None;
            self.optimizer_cancel = None;
            self.optimizer_ticket = None;
            self.optimizer_job_rx = None;
            self.resize_state.optimizer_request_stamp = None;
            self.optimizer_retry_snapshot = None;
        }
    }

    /// Applies the optimized image result directly to current_rgba.

    pub fn apply_optimized_result(&mut self) {
        if !self.optimized_result_is_current() {
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

        self.current_rgba = Arc::new(res.result_rgba);
        self.mark_pixels_changed();
        self.redaction_state.boxes.clear();
        self.markup_state.items.clear();
        self.resize_state.last_result_request_stamp = None;
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
        self.copy_optimized_to_slot_on(BackgroundJobs::global());
    }

    fn copy_optimized_to_slot_on(&mut self, jobs: &BackgroundJobs) {
        if !self.optimized_result_is_current() {
            return;
        }
        let (bytes, mime, achieved_bytes, format_label) = match &self.resize_state.last_result {
            Some(r) => (
                Arc::clone(&r.encoded_data),
                Self::format_label_or_mime(r.format),
                r.achieved_bytes,
                r.format.label(),
            ),
            None => return,
        };
        self.start_optimized_copy_to_slot(bytes, mime, achieved_bytes, format_label, jobs);
    }

    fn format_label_or_mime(format: TargetFormat) -> &'static str {
        match format {
            TargetFormat::Jpeg => "image/jpeg",
            TargetFormat::Png => "image/png",
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
                if let Some(status) = &self.pixel_resize_job.status {
                    ui.label(
                        egui::RichText::new(status)
                            .size(crate::ui_ext::BODY_FONT_SIZE)
                            .color(muted),
                    );
                }
                if let Some(error) = &self.pixel_resize_job.error {
                    ui.label(
                        egui::RichText::new(error)
                            .size(crate::ui_ext::BODY_FONT_SIZE)
                            .color(hex_to_c32(&palette.diff_del)),
                    );
                }
                let apply = crate::action_button("Apply resize (Enter)", accent, palette);
                if ui
                    .add_enabled_ui(!self.pixel_resize_job.pending, |ui| {
                        ui.add_sized([ui.available_width(), crate::ui_ext::CONTROL_HEIGHT], apply)
                    })
                    .inner
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

                // Settings can change during this frame, after the top-level poll.
                self.poll_optimizer();

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
                } else if let Some(status) = self.optimizer_status.as_deref() {
                    ui.add_space(crate::ui_ext::SPACE_SM);
                    ui.label(
                        egui::RichText::new(status)
                            .size(crate::ui_ext::BODY_FONT_SIZE)
                            .color(accent),
                    );
                } else if self.optimized_result_is_current()
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
