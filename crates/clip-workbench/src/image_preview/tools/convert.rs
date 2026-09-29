use super::super::*;
use little_clip_core::{BackgroundJobs, JobError, JobTicket};

fn run_optipng_optimization(
    png_bytes: &[u8],
    control: &ImageToolProcessControl,
) -> Result<Vec<u8>, String> {
    use std::io::Write;
    let mut temp_file = tempfile::Builder::new()
        .suffix(".png")
        .tempfile()
        .map_err(|e| format!("Tempfile creation failed: {}", e))?;
    temp_file
        .write_all(png_bytes)
        .map_err(|e| format!("Tempfile write failed: {}", e))?;
    let temp_path = temp_file.path().to_path_buf();

    let mut command = Command::new("optipng");
    command.arg("-o2").arg("-quiet").arg(&temp_path);
    let status = super::background_removal::run_image_tool_status_command(command, control)
        .map_err(|error| {
            format!(
                "Failed to run optipng: {}. Ensure optipng is installed.",
                error
            )
        })?;

    if !status.success() {
        return Err(format!(
            "optipng failed with exit status {:?}",
            status.code()
        ));
    }

    std::fs::read(&temp_path).map_err(|e| format!("Failed to read optipng output: {}", e))
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ConvertTargetFormat {
    #[default]
    Png,
    Jpeg,
    Webp,
}

impl ConvertTargetFormat {
    pub fn extension(&self) -> &'static str {
        match self {
            ConvertTargetFormat::Png => "png",
            ConvertTargetFormat::Jpeg => "jpg",
            ConvertTargetFormat::Webp => "webp",
        }
    }
}

pub struct ConvertState {
    pub target_format: ConvertTargetFormat,
    pub lossless: bool,
    pub quality: u8,
    pub comparison_split: f32,
    pub is_dragging_split: bool,
    pub encoded_bytes: Vec<u8>,
    pub converted_rgba: Option<RgbaImage>,
    pub converted_texture: Option<egui::TextureHandle>,
    pub dirty_texture: bool,
    pub is_running_optipng: bool,
    pub optipng_savings: Option<(usize, usize)>,
    pub is_encoding: bool,
    pub params_dirty_at: Option<std::time::Instant>,
    pub request_generation: u64,
    pub completed_generation: u64,
    render_cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    render_decode_control: Option<std::sync::Arc<ImageToolProcessControl>>,
    pub(crate) completed_input_stamp: Option<ImageInputStamp>,
    optipng_request: Option<OptipngRequestStamp>,
    optipng_process_control: Option<std::sync::Arc<ImageToolProcessControl>>,
    optipng_job_ticket: Option<JobTicket>,
    pub(crate) optipng_retry_at: Option<std::time::Instant>,
    optipng_queue_status: Option<String>,
    optipng_retry_work: Option<OptipngRetryWork>,
    render_input_stamp: Option<ImageInputStamp>,
    render_snapshot: Option<DisplayImageSnapshotRequest>,
    pub(crate) render_error_input_stamp: Option<ImageInputStamp>,
    pub render_error: Option<String>,
    render_queue_status: Option<String>,
    pub(crate) render_retry_at: Option<std::time::Instant>,
    render_retry_input_stamp: Option<ImageInputStamp>,
    render_job_ticket: Option<JobTicket>,
    render_rx: Option<std::sync::mpsc::Receiver<Result<ConvertWorkerMsg, JobError>>>,
    optipng_rx: Option<std::sync::mpsc::Receiver<Result<OptipngWorkerMsg, JobError>>>,
}

struct ConvertWorkerMsg {
    generation: u64,
    input_stamp: ImageInputStamp,
    result: Result<(Vec<u8>, RgbaImage), String>,
}

struct OptipngWorkerMsg {
    generation: u64,
    input_stamp: ImageInputStamp,
    original_len: usize,
    result: Result<Vec<u8>, String>,
}

type OptipngOptimizer =
    dyn Fn(&[u8], &ImageToolProcessControl) -> Result<Vec<u8>, String> + Send + Sync;

#[derive(Clone)]
struct OptipngRetryWork {
    request: OptipngRequestStamp,
    original_bytes: Vec<u8>,
    optimizer: std::sync::Arc<OptipngOptimizer>,
}

#[derive(Debug, Clone, PartialEq)]
struct OptipngRequestStamp {
    generation: u64,
    input_stamp: ImageInputStamp,
    target_format: ConvertTargetFormat,
    quality: u8,
    lossless: bool,
}

impl Default for ConvertState {
    fn default() -> Self {
        Self {
            target_format: ConvertTargetFormat::Webp,
            lossless: false,
            quality: 80,
            comparison_split: 0.50,
            is_dragging_split: false,
            encoded_bytes: Vec::new(),
            converted_rgba: None,
            converted_texture: None,
            dirty_texture: false,
            is_running_optipng: false,
            optipng_savings: None,
            is_encoding: false,
            params_dirty_at: None,
            request_generation: 0,
            completed_generation: 0,
            render_cancel: None,
            render_decode_control: None,
            completed_input_stamp: None,
            optipng_request: None,
            optipng_process_control: None,
            optipng_job_ticket: None,
            optipng_retry_at: None,
            optipng_queue_status: None,
            optipng_retry_work: None,
            render_input_stamp: None,
            render_snapshot: None,
            render_error_input_stamp: None,
            render_error: None,
            render_queue_status: None,
            render_retry_at: None,
            render_retry_input_stamp: None,
            render_job_ticket: None,
            render_rx: None,
            optipng_rx: None,
        }
    }
}

impl ConvertState {
    pub fn is_pending(&self) -> bool {
        self.params_dirty_at.is_some()
            || self.is_encoding
            || self.is_running_optipng
            || self.optipng_retry_at.is_some()
            || self.render_retry_at.is_some()
    }

    pub fn output_is_current(&self) -> bool {
        self.completed_generation == self.request_generation && self.render_error.is_none()
    }

    pub fn output_matches(&self, input_stamp: &ImageInputStamp) -> bool {
        self.output_is_current() && self.completed_input_stamp.as_ref() == Some(input_stamp)
    }
}

fn encode_conversion(
    base_img: RgbaImage,
    format: ConvertTargetFormat,
    quality: u8,
    lossless: bool,
    cancel: Option<&std::sync::atomic::AtomicBool>,
    decoder: Option<(&std::path::Path, &ImageToolProcessControl)>,
) -> Result<(Vec<u8>, RgbaImage), String> {
    let cancelled = || cancel.is_some_and(|flag| flag.load(std::sync::atomic::Ordering::Acquire));
    if cancelled() {
        return Err("cancelled".to_string());
    }
    let bytes = match format {
        ConvertTargetFormat::Png => encode_png_conversion(&base_img, cancel),
        ConvertTargetFormat::Jpeg => encode_jpeg_conversion(&base_img, quality, cancel),
        ConvertTargetFormat::Webp => encode_webp_cancellable(&base_img, lossless, quality, cancel),
    }?;
    if cancelled() {
        return Err("cancelled".to_string());
    }
    // The RGBA PNG stream is lossless; the other codecs must preserve the
    // image crate's exact output, but their opaque decoders run in a child.
    let decoded = if format == ConvertTargetFormat::Png {
        base_img
    } else if let Some((program, control)) = decoder {
        crate::conversion_decode_worker::decode_in_child(
            &bytes,
            base_img.as_raw().len(),
            program,
            control,
        )?
        .unwrap_or(base_img)
    } else {
        image::load_from_memory(&bytes)
            .map(|image| image.to_rgba8())
            .unwrap_or(base_img)
    };
    if cancelled() {
        return Err("cancelled".to_string());
    }
    Ok((bytes, decoded))
}

fn encode_png_conversion(
    img: &RgbaImage,
    cancel: Option<&std::sync::atomic::AtomicBool>,
) -> Result<Vec<u8>, String> {
    let writer = CancellableVecWriter::new(cancel);
    let output = writer.output.clone();
    write_png_stream(img, writer, cancel)?;
    let mut output = output.lock().expect("conversion output lock poisoned");
    Ok(std::mem::take(&mut *output))
}

fn write_png_stream<W: std::io::Write>(
    img: &RgbaImage,
    writer: W,
    cancel: Option<&std::sync::atomic::AtomicBool>,
) -> Result<(), String> {
    use std::io::Write;

    if img.width() == 0 || img.height() == 0 {
        return Err("PNG dimensions must be nonzero".to_string());
    }
    let mut encoder = png::Encoder::new(writer, img.width(), img.height());
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_deflate_compression(png::DeflateCompression::FdeflateUltraFast);
    encoder.set_filter(png::Filter::Adaptive);
    let mut png_writer = encoder.write_header().map_err(|error| {
        if cancel.is_some_and(|flag| flag.load(std::sync::atomic::Ordering::Acquire)) {
            "cancelled".to_string()
        } else {
            error.to_string()
        }
    })?;
    let mut stream = png_writer
        .stream_writer_with_size(CONVERSION_OUTPUT_CHUNK_SIZE)
        .map_err(|error| {
            if cancel.is_some_and(|flag| flag.load(std::sync::atomic::Ordering::Acquire)) {
                "cancelled".to_string()
            } else {
                error.to_string()
            }
        })?;
    let row_bytes = img.width() as usize * 4;
    for row in img.as_raw().chunks_exact(row_bytes) {
        if cancel.is_some_and(|flag| flag.load(std::sync::atomic::Ordering::Acquire)) {
            return Err("cancelled".to_string());
        }
        stream.write_all(row).map_err(|error| {
            if cancel.is_some_and(|flag| flag.load(std::sync::atomic::Ordering::Acquire)) {
                "cancelled".to_string()
            } else {
                error.to_string()
            }
        })?;
    }
    stream.finish().map_err(|error| {
        if cancel.is_some_and(|flag| flag.load(std::sync::atomic::Ordering::Acquire)) {
            "cancelled".to_string()
        } else {
            error.to_string()
        }
    })?;
    if cancel.is_some_and(|flag| flag.load(std::sync::atomic::Ordering::Acquire)) {
        return Err("cancelled".to_string());
    }
    png_writer.finish().map_err(|error| {
        if cancel.is_some_and(|flag| flag.load(std::sync::atomic::Ordering::Acquire)) {
            "cancelled".to_string()
        } else {
            error.to_string()
        }
    })?;
    if cancel.is_some_and(|flag| flag.load(std::sync::atomic::Ordering::Acquire)) {
        return Err("cancelled".to_string());
    }
    Ok(())
}

struct CancellableVecWriter<'a> {
    bytes: Vec<u8>,
    output: std::sync::Arc<std::sync::Mutex<Vec<u8>>>,
    cancel: Option<&'a std::sync::atomic::AtomicBool>,
    bytes_since_cancel_check: usize,
}

const CONVERSION_OUTPUT_CHUNK_SIZE: usize = 64 * 1024;
const CONVERSION_CANCEL_CHECK_INTERVAL: usize = 4 * 1024;

impl CancellableVecWriter<'_> {
    fn new(cancel: Option<&std::sync::atomic::AtomicBool>) -> CancellableVecWriter<'_> {
        CancellableVecWriter {
            bytes: Vec::with_capacity(CONVERSION_OUTPUT_CHUNK_SIZE),
            output: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            cancel,
            bytes_since_cancel_check: 0,
        }
    }

    fn is_cancelled(&self) -> bool {
        self.cancel
            .is_some_and(|flag| flag.load(std::sync::atomic::Ordering::Acquire))
    }

    fn flush_chunk(&mut self) {
        if self.bytes.is_empty() {
            return;
        }
        self.output
            .lock()
            .expect("conversion output lock poisoned")
            .extend_from_slice(&self.bytes);
        self.bytes.clear();
    }
}

impl std::io::Write for CancellableVecWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self.bytes_since_cancel_check == 0 && self.is_cancelled() {
            return Err(std::io::Error::from(std::io::ErrorKind::Interrupted));
        }
        let mut written = 0;
        while written < bytes.len() {
            let check_after = CONVERSION_CANCEL_CHECK_INTERVAL - self.bytes_since_cancel_check;
            let count = check_after.min(bytes.len() - written);
            self.bytes
                .extend_from_slice(&bytes[written..written + count]);
            written += count;
            self.bytes_since_cancel_check += count;

            if self.bytes_since_cancel_check == CONVERSION_CANCEL_CHECK_INTERVAL {
                self.bytes_since_cancel_check = 0;
                if self.is_cancelled() {
                    return Ok(written);
                }
            }
            if self.bytes.len() >= CONVERSION_OUTPUT_CHUNK_SIZE {
                self.flush_chunk();
            }
        }
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        if self.is_cancelled() {
            Err(std::io::Error::from(std::io::ErrorKind::Interrupted))
        } else {
            Ok(())
        }
    }
}

impl Drop for CancellableVecWriter<'_> {
    fn drop(&mut self) {
        self.flush_chunk();
    }
}

fn encode_jpeg_conversion(
    img: &RgbaImage,
    quality: u8,
    cancel: Option<&std::sync::atomic::AtomicBool>,
) -> Result<Vec<u8>, String> {
    let mut rgb = image::RgbImage::new(img.width(), img.height());
    for (x, y, pixel) in img.enumerate_pixels() {
        if x == 0 && cancel.is_some_and(|flag| flag.load(std::sync::atomic::Ordering::Acquire)) {
            return Err("cancelled".to_string());
        }
        let alpha = pixel[3] as f32 / 255.0;
        let r = ((pixel[0] as f32 * alpha) + (255.0 * (1.0 - alpha))).round() as u8;
        let g = ((pixel[1] as f32 * alpha) + (255.0 * (1.0 - alpha))).round() as u8;
        let b = ((pixel[2] as f32 * alpha) + (255.0 * (1.0 - alpha))).round() as u8;
        rgb.put_pixel(x, y, image::Rgb([r, g, b]));
    }
    let writer = CancellableVecWriter::new(cancel);
    let bytes = writer.output.clone();
    let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(writer, quality);
    encoder
        .encode(
            rgb.as_raw(),
            rgb.width(),
            rgb.height(),
            image::ExtendedColorType::Rgb8,
        )
        .map_err(|error| error.to_string())?;
    drop(encoder);
    let mut output = bytes.lock().expect("conversion output lock poisoned");
    Ok(std::mem::take(&mut *output))
}

impl Drop for ConvertState {
    fn drop(&mut self) {
        if let Some(ticket) = self.optipng_job_ticket.take() {
            ticket.cancel();
        }
        if let Some(control) = self.optipng_process_control.take() {
            control.cancel();
        }
        self.optipng_retry_at = None;
        self.optipng_retry_work = None;
        if let Some(ticket) = self.render_job_ticket.take() {
            ticket.cancel();
        }
        if let Some(cancel) = self.render_cancel.take() {
            cancel.store(true, std::sync::atomic::Ordering::Release);
        }
        if let Some(control) = self.render_decode_control.take() {
            control.cancel();
        }
        self.render_retry_at = None;
        self.render_retry_input_stamp = None;
    }
}

impl ImagePreviewState {
    #[cfg(test)]
    pub fn update_conversion(&mut self) {
        let base_img = self.current_display_image();
        let input_stamp = self.input_stamp();
        let fmt = self.convert_state.target_format;
        let quality = self.convert_state.quality;
        let lossless = self.convert_state.lossless;
        self.convert_state.request_generation =
            self.convert_state.request_generation.wrapping_add(1);
        self.convert_state.params_dirty_at = None;
        self.convert_state.render_rx = None;
        self.convert_state.is_encoding = false;
        self.convert_state.render_input_stamp = None;

        match encode_conversion(base_img, fmt, quality, lossless, None, None) {
            Ok((bytes, decoded_rgba)) => {
                self.convert_state.encoded_bytes = bytes;
                self.convert_state.converted_rgba = Some(decoded_rgba);
                self.convert_state.dirty_texture = true;
                self.convert_state.optipng_savings = None;
                self.convert_state.render_error = None;
                self.convert_state.render_error_input_stamp = None;
                self.convert_state.completed_generation = self.convert_state.request_generation;
                self.convert_state.completed_input_stamp = Some(input_stamp);
            }
            Err(e) => {
                self.convert_state.render_error = Some(e.clone());
                self.convert_state.render_error_input_stamp = Some(input_stamp);
                self.show_toast(format!("Conversion failed: {}", e));
            }
        }
    }

    pub fn schedule_conversion(&mut self) {
        self.cancel_optipng_process();
        self.cancel_conversion_worker();
        self.convert_state.request_generation =
            self.convert_state.request_generation.wrapping_add(1);
        self.convert_state.params_dirty_at = Some(std::time::Instant::now());
        self.convert_state.optipng_savings = None;
        self.convert_state.render_error = None;
        self.convert_state.render_error_input_stamp = None;
        self.convert_state.render_queue_status = None;
        self.convert_state.render_retry_input_stamp = None;
    }

    pub fn run_conversion_worker(&mut self) {
        self.run_conversion_worker_on(BackgroundJobs::global());
    }

    fn run_conversion_worker_on(&mut self, jobs: &BackgroundJobs) {
        if self.convert_state.is_encoding || self.current_rgba.is_empty() {
            return;
        }

        let format = self.convert_state.target_format;
        let decode_program = if cfg!(test) || format == ConvertTargetFormat::Png {
            None
        } else {
            match std::env::current_exe() {
                Ok(program) => Some(program),
                Err(error) => {
                    self.convert_state.render_error =
                        Some(format!("Conversion decoder unavailable: {error}"));
                    self.convert_state.render_error_input_stamp = Some(self.input_stamp());
                    self.convert_state.params_dirty_at = None;
                    return;
                }
            }
        };
        let decode_control = decode_program
            .as_ref()
            .map(|_| std::sync::Arc::new(ImageToolProcessControl::new()));

        let snapshot = self
            .convert_state
            .render_snapshot
            .take()
            .unwrap_or_else(|| self.display_image_snapshot_request());
        let input_stamp = self.input_stamp();
        let quality = self.convert_state.quality;
        let lossless = self.convert_state.lossless;
        let generation = self.convert_state.request_generation;
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        self.convert_state.render_cancel = Some(cancel.clone());
        self.convert_state.render_decode_control = decode_control.clone();
        self.convert_state.render_input_stamp = Some(input_stamp.clone());
        self.convert_state.render_snapshot = Some(snapshot.clone());
        self.convert_state.is_encoding = true;
        self.convert_state.params_dirty_at = None;
        self.convert_state.render_retry_at = None;
        self.convert_state.render_queue_status = None;
        self.convert_state.render_retry_input_stamp = None;
        let (ticket, rx) = jobs.submit(std::time::Duration::MAX, move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                encode_conversion(
                    snapshot.compose(),
                    format,
                    quality,
                    lossless,
                    Some(&cancel),
                    decode_program.as_deref().zip(decode_control.as_deref()),
                )
            }))
            .unwrap_or_else(|_| Err("Conversion worker panicked".to_string()));
            ConvertWorkerMsg {
                generation,
                input_stamp,
                result,
            }
        });
        self.convert_state.render_job_ticket = Some(ticket);
        self.convert_state.render_rx = Some(rx);
    }

    pub fn poll_conversion(&mut self) {
        self.poll_conversion_on(BackgroundJobs::global());
    }

    fn poll_conversion_on(&mut self, jobs: &BackgroundJobs) {
        self.poll_optipng(jobs);
        if self.convert_state.is_encoding
            && self.convert_state.render_input_stamp.as_ref() != Some(&self.input_stamp())
        {
            self.cancel_conversion_worker();
            if self.active_mode == ImageToolMode::Convert {
                self.convert_state.params_dirty_at = Some(std::time::Instant::now());
            }
            return;
        }
        if self.convert_state.render_retry_at.is_some()
            && self.convert_state.render_retry_input_stamp.as_ref() != Some(&self.input_stamp())
        {
            self.convert_state.render_retry_at = None;
            self.convert_state.render_retry_input_stamp = None;
            self.convert_state.render_queue_status = None;
            self.convert_state.render_snapshot = None;
            if self.active_mode == ImageToolMode::Convert {
                self.convert_state.params_dirty_at = Some(std::time::Instant::now());
            }
        }
        if self
            .convert_state
            .render_retry_at
            .is_some_and(|retry_at| std::time::Instant::now() < retry_at)
        {
            return;
        }
        if self
            .convert_state
            .render_retry_at
            .is_some_and(|retry_at| std::time::Instant::now() >= retry_at)
        {
            self.convert_state.render_retry_at = None;
            self.run_conversion_worker_on(jobs);
        }
        let Some(rx) = self.convert_state.render_rx.take() else {
            return;
        };

        let received = rx.try_recv();
        if !matches!(&received, Err(std::sync::mpsc::TryRecvError::Empty)) {
            self.convert_state.render_decode_control = None;
        }
        match received {
            Ok(Ok(message)) => {
                self.convert_state.is_encoding = false;
                self.convert_state.render_cancel = None;
                self.convert_state.render_job_ticket = None;
                self.convert_state.render_input_stamp = None;
                self.convert_state.render_snapshot = None;
                self.convert_state.render_queue_status = None;
                self.convert_state.render_retry_input_stamp = None;
                if message.generation != self.convert_state.request_generation
                    || message.input_stamp != self.input_stamp()
                {
                    if self.active_mode == ImageToolMode::Convert {
                        self.convert_state.params_dirty_at = Some(std::time::Instant::now());
                    }
                    return;
                }
                match message.result {
                    Ok((bytes, rgba)) => {
                        self.convert_state.encoded_bytes = bytes;
                        self.convert_state.converted_rgba = Some(rgba);
                        self.convert_state.dirty_texture = true;
                        self.convert_state.completed_generation = message.generation;
                        self.convert_state.completed_input_stamp = Some(message.input_stamp);
                        self.convert_state.render_error = None;
                        self.convert_state.render_error_input_stamp = None;
                    }
                    Err(error) => {
                        self.convert_state.render_error = Some(error.clone());
                        self.convert_state.render_error_input_stamp =
                            Some(message.input_stamp.clone());
                        self.show_toast(format!("Conversion failed: {}", error));
                    }
                }
            }
            Ok(Err(JobError::QueueFull)) => {
                self.convert_state.is_encoding = false;
                self.convert_state.render_cancel = None;
                self.convert_state.render_job_ticket = None;
                self.convert_state.render_input_stamp = None;
                self.convert_state.render_retry_at =
                    Some(std::time::Instant::now() + std::time::Duration::from_millis(500));
                self.convert_state.render_retry_input_stamp = Some(self.input_stamp());
                self.convert_state.render_queue_status =
                    Some("Waiting for background workers; retrying shortly".to_string());
            }
            Ok(Err(JobError::Cancelled)) => {
                self.convert_state.is_encoding = false;
                self.convert_state.render_cancel = None;
                self.convert_state.render_job_ticket = None;
                self.convert_state.render_input_stamp = None;
                self.convert_state.render_snapshot = None;
            }
            Ok(Err(error)) => {
                self.convert_state.is_encoding = false;
                self.convert_state.render_cancel = None;
                self.convert_state.render_job_ticket = None;
                self.convert_state.render_input_stamp = None;
                self.convert_state.render_snapshot = None;
                self.convert_state.render_error =
                    Some(format!("Conversion worker failed: {error}"));
                self.convert_state.render_error_input_stamp = Some(self.input_stamp());
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                self.convert_state.render_rx = Some(rx);
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.convert_state.is_encoding = false;
                self.convert_state.render_job_ticket = None;
                self.convert_state.render_retry_at = None;
                self.convert_state.render_retry_input_stamp = None;
                let was_cancelled = self
                    .convert_state
                    .render_cancel
                    .take()
                    .is_some_and(|cancel| cancel.load(std::sync::atomic::Ordering::Acquire));
                self.convert_state.render_snapshot = None;
                if was_cancelled {
                    self.convert_state.render_input_stamp = None;
                } else {
                    self.convert_state.render_error_input_stamp =
                        self.convert_state.render_input_stamp.take();
                    self.convert_state.render_error = Some("Conversion worker stopped".to_string());
                }
            }
        }
    }

    fn cancel_conversion_worker(&mut self) {
        if let Some(ticket) = self.convert_state.render_job_ticket.take() {
            ticket.cancel();
        }
        if let Some(cancel) = self.convert_state.render_cancel.take() {
            cancel.store(true, std::sync::atomic::Ordering::Release);
        }
        if let Some(control) = self.convert_state.render_decode_control.take() {
            control.cancel();
        }
        self.convert_state.render_rx = None;
        self.convert_state.render_input_stamp = None;
        self.convert_state.render_snapshot = None;
        self.convert_state.is_encoding = false;
        self.convert_state.render_retry_at = None;
        self.convert_state.render_queue_status = None;
        self.convert_state.render_retry_input_stamp = None;
    }

    pub fn apply_converted_to_canvas(&mut self) {
        if !self.convert_state.output_matches(&self.input_stamp()) {
            return;
        }
        if let Some(converted) = self.convert_state.converted_rgba.clone() {
            self.undo_stack.push(self.current_rgba.clone());
            self.redo_stack.clear();
            self.current_rgba = Arc::new(converted);
            self.mark_pixels_changed();
            self.redaction_state.boxes.clear();
            self.markup_state.items.clear();
            self.convert_state.completed_input_stamp = None;
            self.dirty_texture = true;
            self.resize_state.sync_from_image(&self.current_rgba);
            self.palette_state.is_extracted = false;
            self.schedule_conversion();
            self.show_toast("✔ Applied converted image to canvas");
        }
    }

    pub fn run_optipng_conversion(&mut self) {
        self.run_optipng_conversion_with(run_optipng_optimization);
    }

    fn run_optipng_conversion_with<F>(&mut self, optimizer: F)
    where
        F: Fn(&[u8], &ImageToolProcessControl) -> Result<Vec<u8>, String> + Send + Sync + 'static,
    {
        self.run_optipng_conversion_with_on(
            std::sync::Arc::new(optimizer),
            BackgroundJobs::global(),
        );
    }

    fn run_optipng_conversion_with_on(
        &mut self,
        optimizer: std::sync::Arc<OptipngOptimizer>,
        jobs: &BackgroundJobs,
    ) {
        if self.convert_state.is_running_optipng || self.convert_state.optipng_retry_at.is_some() {
            return;
        }
        if !self.convert_state.output_matches(&self.input_stamp()) {
            return;
        }
        if self.convert_state.target_format != ConvertTargetFormat::Png {
            self.show_toast("⚠ OptiPNG is only applicable for PNG format");
            return;
        }

        let original_bytes = self.convert_state.encoded_bytes.clone();
        if original_bytes.is_empty() {
            self.show_toast("⚠ No PNG bytes available to optimize");
            return;
        }

        let request = OptipngRequestStamp {
            generation: self.convert_state.request_generation,
            input_stamp: self.input_stamp(),
            target_format: self.convert_state.target_format,
            quality: self.convert_state.quality,
            lossless: self.convert_state.lossless,
        };
        let work = OptipngRetryWork {
            request,
            original_bytes,
            optimizer,
        };
        self.start_optipng_job_on(work, jobs);
    }

    fn start_optipng_job_on(&mut self, work: OptipngRetryWork, jobs: &BackgroundJobs) {
        let control = std::sync::Arc::new(ImageToolProcessControl::new());
        let worker_control = control.clone();
        let worker_optimizer = work.optimizer.clone();
        let original_bytes = work.original_bytes.clone();
        let original_len = original_bytes.len();
        let request = work.request.clone();
        let (ticket, rx) = jobs.submit(std::time::Duration::MAX, move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                worker_optimizer(&original_bytes, &worker_control)
            }))
            .unwrap_or_else(|_| Err("OptiPNG worker panicked".to_string()));
            OptipngWorkerMsg {
                generation: request.generation,
                input_stamp: request.input_stamp,
                original_len,
                result,
            }
        });
        self.convert_state.optipng_request = Some(work.request.clone());
        self.convert_state.optipng_process_control = Some(control.clone());
        self.convert_state.optipng_job_ticket = Some(ticket);
        self.convert_state.optipng_rx = Some(rx);
        self.convert_state.is_running_optipng = true;
        self.convert_state.optipng_retry_work = Some(work);
        self.convert_state.optipng_retry_at = None;
        self.convert_state.optipng_queue_status = None;
    }

    pub(crate) fn cancel_optipng_process(&mut self) {
        if let Some(ticket) = self.convert_state.optipng_job_ticket.take() {
            ticket.cancel();
        }
        if let Some(control) = self.convert_state.optipng_process_control.take() {
            control.cancel();
        }
        self.convert_state.optipng_rx = None;
        self.convert_state.optipng_request = None;
        self.convert_state.is_running_optipng = false;
        self.convert_state.optipng_retry_at = None;
        self.convert_state.optipng_retry_work = None;
        self.convert_state.optipng_queue_status = None;
    }

    fn poll_optipng(&mut self, jobs: &BackgroundJobs) {
        let current_stamp = self.input_stamp();
        let request_is_stale = self
            .convert_state
            .optipng_request
            .as_ref()
            .is_some_and(|request| {
                request.generation != self.convert_state.request_generation
                    || request.input_stamp != current_stamp
                    || request.target_format != self.convert_state.target_format
                    || request.quality != self.convert_state.quality
                    || request.lossless != self.convert_state.lossless
                    || !self.convert_state.output_matches(&current_stamp)
            });
        if request_is_stale {
            self.cancel_optipng_process();
            return;
        }
        if let Some(retry_at) = self.convert_state.optipng_retry_at {
            if std::time::Instant::now() < retry_at {
                return;
            }
            if let Some(work) = self.convert_state.optipng_retry_work.clone() {
                self.start_optipng_job_on(work, jobs);
            }
            return;
        }
        let Some(rx) = self.convert_state.optipng_rx.take() else {
            return;
        };
        match rx.try_recv() {
            Ok(Ok(message)) => {
                self.convert_state.is_running_optipng = false;
                self.convert_state.optipng_request = None;
                self.convert_state.optipng_process_control = None;
                self.convert_state.optipng_job_ticket = None;
                self.convert_state.optipng_retry_work = None;
                self.convert_state.optipng_retry_at = None;
                self.convert_state.optipng_queue_status = None;
                if message.generation != self.convert_state.request_generation
                    || message.input_stamp != current_stamp
                    || self.convert_state.target_format != ConvertTargetFormat::Png
                    || !self.convert_state.output_matches(&current_stamp)
                {
                    return;
                }
                match message.result {
                    Ok(bytes) => {
                        let optimized_len = bytes.len();
                        self.convert_state.encoded_bytes = bytes;
                        self.convert_state.optipng_savings =
                            Some((message.original_len, optimized_len));
                        let pct =
                            if message.original_len > 0 && optimized_len < message.original_len {
                                (message.original_len - optimized_len) as f32
                                    / message.original_len as f32
                                    * 100.0
                            } else {
                                0.0
                            };
                        self.show_toast(format!("✔ OptiPNG complete! Saved {:.1}%", pct));
                    }
                    Err(error) => self.show_toast(format!("❌ OptiPNG error: {}", error)),
                }
            }
            Ok(Err(JobError::QueueFull)) => {
                self.convert_state.is_running_optipng = false;
                self.convert_state.optipng_process_control = None;
                self.convert_state.optipng_job_ticket = None;
                self.convert_state.optipng_rx = None;
                self.convert_state.optipng_retry_at =
                    Some(std::time::Instant::now() + std::time::Duration::from_millis(500));
                self.convert_state.optipng_queue_status =
                    Some("Background workers are busy; retrying OptiPNG shortly".to_string());
            }
            Ok(Err(JobError::Cancelled)) => {
                self.convert_state.is_running_optipng = false;
                self.convert_state.optipng_request = None;
                self.convert_state.optipng_process_control = None;
                self.convert_state.optipng_job_ticket = None;
                self.convert_state.optipng_retry_work = None;
                self.convert_state.optipng_rx = None;
            }
            Ok(Err(error)) => {
                self.convert_state.is_running_optipng = false;
                self.convert_state.optipng_request = None;
                self.convert_state.optipng_process_control = None;
                self.convert_state.optipng_job_ticket = None;
                self.convert_state.optipng_retry_work = None;
                self.convert_state.optipng_rx = None;
                if self.convert_state.target_format == ConvertTargetFormat::Png
                    && self.convert_state.output_matches(&current_stamp)
                {
                    self.show_toast(format!("❌ OptiPNG error: {error}"));
                }
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                self.convert_state.optipng_rx = Some(rx);
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.convert_state.is_running_optipng = false;
                self.convert_state.optipng_request = None;
                self.convert_state.optipng_process_control = None;
                self.convert_state.optipng_job_ticket = None;
                self.convert_state.optipng_retry_work = None;
                if self.convert_state.target_format == ConvertTargetFormat::Png
                    && self.convert_state.output_matches(&current_stamp)
                {
                    self.show_toast("❌ OptiPNG error: worker stopped");
                }
            }
        }
    }

    pub(crate) fn render_convert_toolbar(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        palette: &AppPalette,
    ) {
        if self.convert_state.is_running_optipng {
            ctx.request_repaint_after(std::time::Duration::from_millis(16));
        }
        let accent = hex_to_c32(&palette.harmonies.base);
        let surface = hex_to_c32(&palette.colors.surface_panel);
        let border = hex_to_c32(&palette.colors.surface_recessed);
        let text = hex_to_c32(&palette.colors.text_secondary);
        let muted = hex_to_c32(&palette.colors.text_muted);
        let success = hex_to_c32(&palette.diff_add);
        let warning = hex_to_c32(&palette.diff_mod);
        let mut changed = false;

        ui.label(
            egui::RichText::new("Convert")
                .size(16.0)
                .strong()
                .color(accent),
        );
        ui.label(
            egui::RichText::new("Choose an output format and compare it on the canvas.")
                .size(crate::ui_ext::BODY_FONT_SIZE)
                .color(muted),
        );
        ui.add_space(crate::ui_ext::ITEM_SPACING_Y);
        ui.allocate_ui_with_layout(
            Vec2::new(ui.available_width(), crate::ui_ext::CONTROL_HEIGHT),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                if self.convert_state.render_error_input_stamp.as_ref() == Some(&self.input_stamp())
                    && let Some(error) = &self.convert_state.render_error
                {
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(error)
                                .size(crate::ui_ext::BODY_FONT_SIZE)
                                .color(warning),
                        )
                        .truncate(true),
                    );
                } else if let Some(status) = &self.convert_state.render_queue_status {
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(status)
                                .size(crate::ui_ext::BODY_FONT_SIZE)
                                .color(warning),
                        )
                        .truncate(true),
                    );
                } else if let Some(status) = &self.convert_state.optipng_queue_status {
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(status)
                                .size(crate::ui_ext::BODY_FONT_SIZE)
                                .color(warning),
                        )
                        .truncate(true),
                    );
                } else if self.convert_state.is_running_optipng {
                    ui.add(egui::Spinner::new().size(13.0).color(accent));
                    ui.label(
                        egui::RichText::new("Optimizing PNG…")
                            .size(crate::ui_ext::BODY_FONT_SIZE)
                            .color(accent),
                    );
                } else if self.convert_state.is_pending() {
                    ui.add(egui::Spinner::new().size(13.0).color(accent));
                    ui.label(
                        egui::RichText::new(if self.convert_state.is_encoding {
                            "Encoding preview…"
                        } else {
                            "Waiting for changes…"
                        })
                        .size(crate::ui_ext::BODY_FONT_SIZE)
                        .color(accent),
                    );
                }
            },
        );

        ui.add_space(crate::ui_ext::SPACE_MD);
        ui.label(
            egui::RichText::new("OUTPUT FORMAT")
                .size(crate::ui_ext::BODY_FONT_SIZE)
                .strong()
                .color(text),
        );
        ui.add_space(crate::ui_ext::SPACE_XS);

        let formats = [
            (ConvertTargetFormat::Png, "PNG"),
            (ConvertTargetFormat::Jpeg, "JPEG"),
            (ConvertTargetFormat::Webp, "WebP"),
        ];
        let format_width = ((ui.available_width() - 8.0) / 3.0).max(54.0);
        ui.horizontal(|ui| {
            for (format, label) in formats {
                let active = self.convert_state.target_format == format;
                let fill = if active { accent } else { surface };
                let foreground = contrast_text_c32(fill, Some(text), palette);
                let button = egui::Button::new(
                    egui::RichText::new(label)
                        .size(crate::ui_ext::BODY_FONT_SIZE)
                        .strong()
                        .color(foreground),
                )
                .fill(fill)
                .stroke(Stroke::new(
                    1.0_f32,
                    if active { foreground } else { border },
                ))
                .rounding(Rounding::same(crate::ui_ext::CONTROL_CORNER_RADIUS));
                if ui
                    .add_sized([format_width, crate::ui_ext::CONTROL_HEIGHT], button)
                    .clicked()
                    && !active
                {
                    self.convert_state.target_format = format;
                    match format {
                        ConvertTargetFormat::Png => self.convert_state.lossless = true,
                        ConvertTargetFormat::Jpeg => self.convert_state.lossless = false,
                        ConvertTargetFormat::Webp => {}
                    }
                    changed = true;
                }
            }
        });

        ui.add_space(crate::ui_ext::SPACE_MD);
        ui.separator();
        ui.add_space(crate::ui_ext::SPACE_SM);
        ui.label(
            egui::RichText::new("ENCODING")
                .size(crate::ui_ext::BODY_FONT_SIZE)
                .strong()
                .color(text),
        );
        ui.add_space(crate::ui_ext::SPACE_XS);

        match self.convert_state.target_format {
            ConvertTargetFormat::Png => {
                ui.label(
                    egui::RichText::new("Lossless PNG")
                        .size(crate::ui_ext::BODY_FONT_SIZE)
                        .strong()
                        .color(text),
                );
                ui.label(
                    egui::RichText::new("Preserves every pixel and transparency.")
                        .size(crate::ui_ext::BODY_FONT_SIZE)
                        .color(muted),
                );
            }
            ConvertTargetFormat::Jpeg => {
                ui.label(
                    egui::RichText::new("Lossy JPEG")
                        .size(crate::ui_ext::BODY_FONT_SIZE)
                        .strong()
                        .color(text),
                );
                ui.label(
                    egui::RichText::new("Small files; transparency is flattened.")
                        .size(crate::ui_ext::BODY_FONT_SIZE)
                        .color(muted),
                );
            }
            ConvertTargetFormat::Webp => {
                let mut lossless = self.convert_state.lossless;
                if ui
                    .checkbox(
                        &mut lossless,
                        egui::RichText::new("Lossless WebP")
                            .size(crate::ui_ext::BODY_FONT_SIZE)
                            .strong()
                            .color(text),
                    )
                    .on_hover_text("Preserve every pixel using WebP lossless encoding")
                    .changed()
                {
                    self.convert_state.lossless = lossless;
                    changed = true;
                }
                ui.label(
                    egui::RichText::new(if lossless {
                        "Preserves every pixel and transparency."
                    } else {
                        "Compresses for a smaller output file."
                    })
                    .size(crate::ui_ext::BODY_FONT_SIZE)
                    .color(muted),
                );
            }
        }

        if !self.convert_state.lossless {
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("Quality")
                        .size(crate::ui_ext::BODY_FONT_SIZE)
                        .strong()
                        .color(text),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        egui::RichText::new(format!("{}%", self.convert_state.quality))
                            .size(crate::ui_ext::BODY_FONT_SIZE)
                            .monospace()
                            .color(accent),
                    );
                });
            });
            let mut quality = self.convert_state.quality;
            ui.spacing_mut().slider_width = ui.available_width();
            if ui
                .add(egui::Slider::new(&mut quality, 1..=100).show_value(false))
                .on_hover_text("Higher quality produces a larger file")
                .changed()
            {
                self.convert_state.quality = quality;
                changed = true;
            }
        }

        if changed {
            self.schedule_conversion();
        }

        if self.convert_state.target_format == ConvertTargetFormat::Png {
            ui.add_space(10.0);
            let optimize_label = if self.convert_state.is_running_optipng {
                "Optimizing…"
            } else {
                "Run OptiPNG"
            };
            let optimize = crate::secondary_button(optimize_label, warning, palette);
            if ui
                .add_enabled_ui(
                    !self.convert_state.is_running_optipng
                        && self.convert_state.optipng_retry_at.is_none()
                        && self.convert_state.output_matches(&self.input_stamp()),
                    |ui| {
                        ui.add_sized(
                            [ui.available_width(), crate::ui_ext::CONTROL_HEIGHT],
                            optimize,
                        )
                    },
                )
                .inner
                .on_hover_text("Losslessly reduce the encoded PNG file size")
                .clicked()
            {
                self.run_optipng_conversion();
            }
        }

        ui.add_space(crate::ui_ext::SPACE_MD);
        ui.separator();
        ui.add_space(crate::ui_ext::SPACE_SM);
        ui.label(
            egui::RichText::new("FILE SIZE")
                .size(crate::ui_ext::BODY_FONT_SIZE)
                .strong()
                .color(text),
        );
        ui.add_space(crate::ui_ext::SPACE_XS);

        let original_size = self.file_size_bytes;
        let converted_size = if self.convert_state.output_matches(&self.input_stamp()) {
            self.convert_state.encoded_bytes.len()
        } else {
            0
        };
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("Original")
                    .size(crate::ui_ext::BODY_FONT_SIZE)
                    .color(muted),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    egui::RichText::new(format_bytes_size(original_size))
                        .size(crate::ui_ext::BODY_FONT_SIZE)
                        .monospace()
                        .color(text),
                );
            });
        });
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("Converted")
                    .size(crate::ui_ext::BODY_FONT_SIZE)
                    .color(muted),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    egui::RichText::new(if converted_size == 0 {
                        "—".to_string()
                    } else {
                        format_bytes_size(converted_size)
                    })
                    .size(crate::ui_ext::BODY_FONT_SIZE)
                    .monospace()
                    .color(text),
                );
            });
        });

        if converted_size > 0 && original_size > 0 {
            let delta =
                (converted_size as f64 - original_size as f64) / original_size as f64 * 100.0;
            let delta_color = if delta <= 0.0 { success } else { warning };
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("Difference")
                        .size(crate::ui_ext::BODY_FONT_SIZE)
                        .color(muted),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        egui::RichText::new(format!("{:+.1}%", delta))
                            .size(crate::ui_ext::BODY_FONT_SIZE)
                            .strong()
                            .monospace()
                            .color(delta_color),
                    );
                });
            });
        }

        if self.convert_state.output_matches(&self.input_stamp())
            && let Some((before, after)) = self.convert_state.optipng_savings
        {
            let saved = before.saturating_sub(after);
            ui.label(
                egui::RichText::new(format!("OptiPNG saved {}", format_bytes_size(saved)))
                    .size(crate::ui_ext::BODY_FONT_SIZE)
                    .strong()
                    .color(success),
            );
        }

        ui.add_space(crate::ui_ext::SPACE_SM);
        ui.label(
            egui::RichText::new("Drag the canvas divider to compare before and after.")
                .size(crate::ui_ext::BODY_FONT_SIZE)
                .italics()
                .color(muted),
        );

        ui.add_space(14.0);
        let output_ready = self.convert_state.output_matches(&self.input_stamp());
        let extension = self.convert_state.target_format.extension();
        let save = crate::action_button(format!("Save as .{}", extension), accent, palette);
        if ui
            .add_enabled_ui(output_ready, |ui| {
                ui.add_sized([ui.available_width(), crate::ui_ext::CONTROL_HEIGHT], save)
            })
            .inner
            .on_hover_text(format!("Export the converted image as .{}", extension))
            .clicked()
        {
            self.open_save_dialog_with_ext(extension);
        }

        ui.add_space(crate::ui_ext::SPACE_XS);
        let apply = crate::secondary_button("Apply to canvas (Enter)", success, palette);
        if ui
            .add_enabled_ui(output_ready, |ui| {
                ui.add_sized([ui.available_width(), crate::ui_ext::CONTROL_HEIGHT], apply)
            })
            .inner
            .on_hover_text("Use the converted image as the active editor canvas")
            .clicked()
        {
            self.apply_converted_to_canvas();
        }

        ui.add_space(crate::ui_ext::SPACE_SM);
        ui.label(
            egui::RichText::new("COPY TO SLOT")
                .size(crate::ui_ext::BODY_FONT_SIZE)
                .strong()
                .color(text),
        );
        ui.add_space(crate::ui_ext::SPACE_XS);
        let slot_width = ((ui.available_width() - 6.0) * 0.5).max(70.0);
        ui.horizontal(|ui| {
            let left = crate::secondary_button("Left hand", surface, palette);
            if ui
                .add_sized([slot_width, crate::ui_ext::CONTROL_HEIGHT], left)
                .on_hover_text("Copy the converted image to the Left Hand slot")
                .clicked()
            {
                self.copy_to_hand_slot(HandSide::Left);
            }

            let right = crate::secondary_button("Right hand", surface, palette);
            if ui
                .add_sized([slot_width, crate::ui_ext::CONTROL_HEIGHT], right)
                .on_hover_text("Copy the converted image to the Right Hand slot")
                .clicked()
            {
                self.copy_to_hand_slot(HandSide::Right);
            }
        });

        // A control can become dirty after the top-level polling pass for this
        // frame. Ensure the debounce timer and worker continue without needing
        // another pointer event to wake the UI.
        let retry_at = self
            .convert_state
            .render_retry_at
            .into_iter()
            .chain(self.convert_state.optipng_retry_at)
            .min();
        if let Some(retry_at) = retry_at {
            ctx.request_repaint_after(
                retry_at.saturating_duration_since(std::time::Instant::now()),
            );
        } else if self.convert_state.is_pending() {
            ctx.request_repaint_after(std::time::Duration::from_millis(16));
        }
    }

    pub(crate) fn render_convert_comparison(
        &mut self,
        ui: &mut egui::Ui,
        painter: &egui::Painter,
        response: &egui::Response,
        img_rect: Rect,
        _canvas_rect: Rect,
        ctx: &egui::Context,
        palette: &AppPalette,
    ) {
        // Ensure converted texture is loaded
        if self.convert_state.output_matches(&self.input_stamp())
            && (self.convert_state.dirty_texture || self.convert_state.converted_texture.is_none())
        {
            if let Some(ref conv_rgba) = self.convert_state.converted_rgba {
                let color_img = egui::ColorImage::from_rgba_unmultiplied(
                    [conv_rgba.width() as usize, conv_rgba.height() as usize],
                    conv_rgba.as_raw(),
                );
                self.convert_state.converted_texture = Some(ctx.load_texture(
                    "converted_preview_tex",
                    color_img,
                    egui::TextureOptions::LINEAR,
                ));
                self.convert_state.dirty_texture = false;
            }
        }

        let split = self.convert_state.comparison_split.clamp(0.0, 1.0);
        let split_x = img_rect.min.x + split * img_rect.width();

        // 1. Draw Left Side (Original / Before)
        if let Some(ref orig_tex) = self.texture {
            let left_rect = Rect::from_min_max(img_rect.min, Pos2::new(split_x, img_rect.max.y));
            let left_uv = Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(split, 1.0));
            let mut left_mesh = egui::Mesh::with_texture(orig_tex.id());
            left_mesh.add_rect_with_uv(left_rect, left_uv, Color32::WHITE);
            painter.add(egui::Shape::mesh(left_mesh));
        }

        // 2. Draw Right Side (Converted / After)
        let conv_tex_id = if self.convert_state.output_matches(&self.input_stamp()) {
            self.convert_state
                .converted_texture
                .as_ref()
                .map(|t| t.id())
                .or_else(|| self.texture.as_ref().map(|t| t.id()))
        } else {
            self.texture.as_ref().map(|t| t.id())
        };
        if let Some(tex_id) = conv_tex_id {
            let right_rect = Rect::from_min_max(Pos2::new(split_x, img_rect.min.y), img_rect.max);
            let right_uv = Rect::from_min_max(Pos2::new(split, 0.0), Pos2::new(1.0, 1.0));
            let mut right_mesh = egui::Mesh::with_texture(tex_id);
            right_mesh.add_rect_with_uv(right_rect, right_uv, Color32::WHITE);
            painter.add(egui::Shape::mesh(right_mesh));
        }

        // 3. Draw Crisp Divider Line
        painter.line_segment(
            [
                Pos2::new(split_x, img_rect.min.y),
                Pos2::new(split_x, img_rect.max.y),
            ],
            Stroke::new(3.0_f32, Color32::from_black_alpha(150)),
        );
        painter.line_segment(
            [
                Pos2::new(split_x, img_rect.min.y),
                Pos2::new(split_x, img_rect.max.y),
            ],
            Stroke::new(1.5_f32, Color32::WHITE),
        );

        // 4. Draggable Center Knob
        let knob_center = Pos2::new(split_x, img_rect.center().y);
        let base_c = hex_to_c32(&palette.harmonies.base);
        let knob_radius = crate::ui_ext::paint_split_handle(painter, knob_center, palette);

        // 5. Draggable Handle Interaction
        let ptr_pos = response.hover_pos();
        let near_divider =
            crate::ui_ext::split_handle_hit_test(ptr_pos, img_rect, knob_center, knob_radius, 16.0);

        if near_divider || self.convert_state.is_dragging_split {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
        }

        if near_divider && response.drag_started() {
            self.convert_state.is_dragging_split = true;
        }

        if self.convert_state.is_dragging_split {
            if response.dragged() {
                if let Some(pos) = ptr_pos {
                    let new_split = ((pos.x - img_rect.min.x) / img_rect.width()).clamp(0.01, 0.99);
                    self.convert_state.comparison_split = new_split;
                }
            }
            if response.drag_stopped() {
                self.convert_state.is_dragging_split = false;
            }
        }

        // 6. Squoosh-Style Floating HUD Badges
        let orig_size_str = format_bytes_size(self.file_size_bytes);
        let conv_size_str = if self.convert_state.output_matches(&self.input_stamp()) {
            format_bytes_size(self.convert_state.encoded_bytes.len())
        } else {
            "—".to_string()
        };
        let orig_fmt = self.mime.replace("image/", "").to_uppercase();
        let conv_fmt = self.convert_state.target_format.extension().to_uppercase();

        let bounds = img_rect.intersect(painter.clip_rect());
        let gap = 8.0;
        let max_width = ((bounds.width() - gap * 3.0) * 0.5).max(1.0);
        let before_value = format!("{} · {}", orig_fmt, orig_size_str);
        let after_value = format!("{} · {}", conv_fmt, conv_size_str);
        crate::ui_ext::CanvasBadge {
            heading: "BEFORE",
            value: Some(&before_value),
            accent: None,
        }
        .paint(
            painter,
            bounds.min + Vec2::splat(gap),
            egui::Align2::LEFT_TOP,
            bounds,
            max_width,
            palette,
        );
        crate::ui_ext::CanvasBadge {
            heading: "AFTER",
            value: Some(&after_value),
            accent: Some(base_c),
        }
        .paint(
            painter,
            Pos2::new(bounds.right() - gap, bounds.top() + gap),
            egui::Align2::RIGHT_TOP,
            bounds,
            max_width,
            palette,
        );
    }
}

#[cfg(test)]
mod input_stamp_tests {
    use super::*;

    #[test]
    fn cancellable_webp_bytes_match_the_existing_encoder_for_lossy_and_lossless() {
        let image = RgbaImage::from_fn(37, 23, |x, y| {
            image::Rgba([
                (x.wrapping_mul(17) ^ y.wrapping_mul(3)) as u8,
                (y.wrapping_mul(29) ^ x.wrapping_mul(5)) as u8,
                ((x + y).wrapping_mul(11)) as u8,
                ((x * 7 + y * 13) % 256) as u8,
            ])
        });
        for (lossless, quality) in [(false, 83), (true, 100)] {
            let expected = super::super::super::encode_webp(&image, lossless, quality).unwrap();
            let actual =
                super::super::super::encode_webp_cancellable(&image, lossless, quality, None)
                    .unwrap();
            assert_eq!(actual, expected, "lossless={lossless}, quality={quality}");
        }
    }

    #[test]
    fn webp_progress_hook_aborts_when_preview_cancel_token_becomes_stale() {
        let image = RgbaImage::from_fn(512, 512, |x, y| {
            image::Rgba([
                (x.wrapping_mul(37) ^ y.wrapping_mul(7)) as u8,
                (y.wrapping_mul(53) ^ x.wrapping_mul(13)) as u8,
                ((x * 19 + y * 31) % 256) as u8,
                255,
            ])
        });
        let cancelled = std::sync::atomic::AtomicBool::new(false);
        let callbacks = std::sync::atomic::AtomicUsize::new(0);
        let result = super::super::super::encode_webp_cancellable_with_progress(
            &image,
            false,
            86,
            Some(&cancelled),
            Some(&callbacks),
            true,
        );
        assert_eq!(result.unwrap_err(), "cancelled");
        assert!(callbacks.load(std::sync::atomic::Ordering::Relaxed) > 0);
        assert!(cancelled.load(std::sync::atomic::Ordering::Acquire));
    }

    #[cfg(unix)]
    fn start_sleeping_optipng(
        state: &mut ImagePreviewState,
        label: &str,
    ) -> (
        u32,
        std::sync::Arc<ImageToolProcessControl>,
        std::path::PathBuf,
    ) {
        let script =
            std::env::temp_dir().join(format!("clip-optipng-{label}-{}.py", std::process::id()));
        std::fs::write(&script, "import time\ntime.sleep(30)\n").unwrap();
        let command_script = script.clone();
        state.run_optipng_conversion_with(move |_, control| {
            let mut command = Command::new("python3");
            command
                .arg(&command_script)
                .arg("-o2")
                .arg("-quiet")
                .arg("fixture.png");
            let status =
                super::super::background_removal::run_image_tool_status_command(command, control)?;
            if !status.success() {
                return Err(format!("fake optipng exited with {:?}", status.code()));
            }
            Ok(Vec::new())
        });
        let control = state
            .convert_state
            .optipng_process_control
            .as_ref()
            .unwrap()
            .clone();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while control.child_pid.load(std::sync::atomic::Ordering::SeqCst) == 0
            && std::time::Instant::now() < deadline
        {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let pid = control.child_pid.load(std::sync::atomic::Ordering::SeqCst);
        assert_ne!(pid, 0, "OptiPNG child PID was not published");
        (pid, control, script)
    }

    #[cfg(unix)]
    fn assert_optipng_exited(pid: u32) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while std::time::Instant::now() < deadline {
            let result = unsafe { libc::kill(pid as i32, 0) };
            if result == -1 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("OptiPNG child {pid} remained alive after cancellation");
    }

    #[cfg(unix)]
    fn state_with_png_conversion() -> ImagePreviewState {
        let mut state = state();
        state.active_mode = ImageToolMode::Convert;
        state.convert_state.target_format = ConvertTargetFormat::Png;
        state.update_conversion();
        state
    }

    #[cfg(unix)]
    #[test]
    fn optipng_process_stops_on_settings_input_change_and_viewer_drop() {
        let mut settings = state_with_png_conversion();
        let (pid, control, script) = start_sleeping_optipng(&mut settings, "settings");
        settings.schedule_conversion();
        assert!(control.cancel.load(std::sync::atomic::Ordering::SeqCst));
        assert_optipng_exited(pid);
        let _ = std::fs::remove_file(script);

        let mut input = state_with_png_conversion();
        let (pid, control, script) = start_sleeping_optipng(&mut input, "input");
        input.mark_pixels_changed();
        assert!(control.cancel.load(std::sync::atomic::Ordering::SeqCst));
        assert_optipng_exited(pid);
        let _ = std::fs::remove_file(script);

        let mut dropped = state_with_png_conversion();
        let (pid, control, script) = start_sleeping_optipng(&mut dropped, "drop");
        drop(dropped);
        assert!(control.cancel.load(std::sync::atomic::Ordering::SeqCst));
        assert_optipng_exited(pid);
        let _ = std::fs::remove_file(script);
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
            "conversion stamp test".into(),
            &AppConfig::default(),
        )
        .unwrap()
    }

    #[test]
    fn bounded_conversion_matches_direct_encoder_output_for_tiny_png() {
        let mut state = state();
        state.convert_state.target_format = ConvertTargetFormat::Png;
        state.convert_state.quality = 77;
        let image = state.current_display_image();
        let expected =
            encode_conversion(image, ConvertTargetFormat::Png, 77, false, None, None).unwrap();
        let jobs = BackgroundJobs::new(1, 1);
        state.run_conversion_worker_on(&jobs);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while state.convert_state.is_encoding && std::time::Instant::now() < deadline {
            state.poll_conversion_on(&jobs);
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        state.poll_conversion_on(&jobs);
        assert!(!state.convert_state.is_encoding);
        assert_eq!(state.convert_state.encoded_bytes, expected.0);
        assert_eq!(
            state.convert_state.converted_rgba.as_ref(),
            Some(&expected.1)
        );
        assert!(state.convert_state.render_error.is_none());
    }

    #[test]
    fn conversion_queue_saturation_retries_after_delay() {
        let jobs = BackgroundJobs::new(1, 1);
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let (_, blocker_rx) = jobs.submit(std::time::Duration::from_secs(5), move || {
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        });
        started_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap();
        let (_, queued_rx) = jobs.submit(std::time::Duration::from_secs(5), || ());
        let mut state = state();
        state.convert_state.target_format = ConvertTargetFormat::Png;
        state.run_conversion_worker_on(&jobs);
        let retained_pixels =
            std::sync::Arc::clone(&state.convert_state.render_snapshot.as_ref().unwrap().pixels);
        state.poll_conversion_on(&jobs);
        assert!(state.convert_state.render_retry_at.is_some());
        assert!(std::sync::Arc::ptr_eq(
            &retained_pixels,
            &state.convert_state.render_snapshot.as_ref().unwrap().pixels
        ));
        assert_eq!(
            state.convert_state.render_queue_status.as_deref(),
            Some("Waiting for background workers; retrying shortly")
        );
        assert!(state.convert_state.render_error.is_none());

        release_tx.send(()).unwrap();
        blocker_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap()
            .unwrap();
        queued_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap()
            .unwrap();
        state.convert_state.render_retry_at = Some(std::time::Instant::now());
        assert!(std::sync::Arc::ptr_eq(
            &retained_pixels,
            &state.convert_state.render_snapshot.as_ref().unwrap().pixels
        ));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while (state.convert_state.is_encoding || state.convert_state.render_retry_at.is_some())
            && std::time::Instant::now() < deadline
        {
            state.poll_conversion_on(&jobs);
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        state.poll_conversion_on(&jobs);
        assert!(state.convert_state.render_retry_at.is_none());
        assert!(state.convert_state.render_queue_status.is_none());
        assert!(state.convert_state.output_matches(&state.input_stamp()));
    }

    #[test]
    fn stale_conversion_retry_is_discarded_after_image_change() {
        let jobs = BackgroundJobs::new(1, 1);
        let mut state = state();
        state.active_mode = ImageToolMode::Convert;
        state.convert_state.render_retry_at =
            Some(std::time::Instant::now() + std::time::Duration::from_millis(500));
        state.convert_state.render_retry_input_stamp = Some(state.input_stamp());
        state.convert_state.render_queue_status = Some("retrying".into());
        state.mark_pixels_changed();

        state.poll_conversion_on(&jobs);

        assert!(state.convert_state.render_retry_at.is_none());
        assert!(state.convert_state.render_retry_input_stamp.is_none());
        assert!(state.convert_state.render_queue_status.is_none());
        assert!(state.convert_state.params_dirty_at.is_some());
        assert!(!state.convert_state.is_encoding);
    }

    #[test]
    fn dropping_convert_state_cancels_queued_job_and_worker_flag() {
        let jobs = BackgroundJobs::new(1, 1);
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let (_, blocker_rx) = jobs.submit(std::time::Duration::from_secs(5), move || {
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        });
        started_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap();

        let ran = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let worker_ran = ran.clone();
        let (ticket, result_rx) = jobs.submit(std::time::Duration::from_secs(5), move || {
            worker_ran.store(true, std::sync::atomic::Ordering::Release);
        });
        let mut state = state();
        let cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        state.convert_state.render_cancel = Some(cancelled.clone());
        state.convert_state.render_job_ticket = Some(ticket);
        drop(state);
        assert!(cancelled.load(std::sync::atomic::Ordering::Acquire));

        release_tx.send(()).unwrap();
        blocker_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap()
            .unwrap();
        assert_eq!(
            result_rx
                .recv_timeout(std::time::Duration::from_secs(1))
                .unwrap(),
            Err(JobError::Cancelled)
        );
        assert!(!ran.load(std::sync::atomic::Ordering::Acquire));
    }

    #[test]
    fn conversion_discards_stale_success_and_stale_failure() {
        for result in [
            Ok((
                vec![1],
                RgbaImage::from_pixel(4, 4, image::Rgba([1, 2, 3, 255])),
            )),
            Err("stale encode error".into()),
        ] {
            let mut state = state();
            state.active_mode = ImageToolMode::Convert;
            let old_stamp = state.input_stamp();
            state.convert_state.render_input_stamp = Some(old_stamp.clone());
            state.convert_state.is_encoding = true;
            let (tx, rx) = std::sync::mpsc::channel::<Result<ConvertWorkerMsg, JobError>>();
            state.convert_state.render_rx = Some(rx);
            tx.send(Ok(ConvertWorkerMsg {
                generation: state.convert_state.request_generation,
                input_stamp: old_stamp,
                result,
            }))
            .unwrap();
            state.mark_pixels_changed();

            state.poll_conversion();

            assert!(!state.convert_state.is_encoding);
            assert!(state.convert_state.render_rx.is_none());
            assert!(state.convert_state.render_error.is_none());
            assert!(state.convert_state.encoded_bytes.is_empty());
            assert!(state.convert_state.converted_rgba.is_none());
            assert!(state.status_toast.is_none());
        }
    }

    #[test]
    fn conversion_worker_is_cancelled_when_settings_or_input_go_stale() {
        let mut settings = state();
        settings.run_conversion_worker();
        let settings_decoder = std::sync::Arc::new(ImageToolProcessControl::new());
        settings.convert_state.render_decode_control = Some(settings_decoder.clone());
        let cancel = settings
            .convert_state
            .render_cancel
            .as_ref()
            .unwrap()
            .clone();
        settings.schedule_conversion();
        assert!(cancel.load(std::sync::atomic::Ordering::Acquire));
        assert!(
            settings_decoder
                .cancel
                .load(std::sync::atomic::Ordering::Acquire)
        );
        assert!(!settings.convert_state.is_encoding);
        assert!(settings.convert_state.render_rx.is_none());

        let mut input = state();
        input.run_conversion_worker();
        let input_decoder = std::sync::Arc::new(ImageToolProcessControl::new());
        input.convert_state.render_decode_control = Some(input_decoder.clone());
        let cancel = input.convert_state.render_cancel.as_ref().unwrap().clone();
        input.mark_pixels_changed();
        input.poll_conversion();
        assert!(cancel.load(std::sync::atomic::Ordering::Acquire));
        assert!(
            input_decoder
                .cancel
                .load(std::sync::atomic::Ordering::Acquire)
        );
        assert!(!input.convert_state.is_encoding);
        assert!(input.convert_state.render_rx.is_none());

        let mut dropped = state();
        let drop_decoder = std::sync::Arc::new(ImageToolProcessControl::new());
        dropped.convert_state.render_decode_control = Some(drop_decoder.clone());
        drop(dropped);
        assert!(
            drop_decoder
                .cancel
                .load(std::sync::atomic::Ordering::Acquire)
        );
    }

    #[test]
    fn conversion_worker_cancellation_is_checked_before_encoding() {
        let image = RgbaImage::new(4, 4);
        let cancel = std::sync::atomic::AtomicBool::new(true);
        assert_eq!(
            encode_conversion(
                image,
                ConvertTargetFormat::Png,
                80,
                false,
                Some(&cancel),
                None
            )
            .unwrap_err(),
            "cancelled"
        );
    }

    #[cfg(unix)]
    #[test]
    fn process_decode_failure_uses_the_original_rgba_preview() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().unwrap();
        let script = directory.path().join("decode-fallback.sh");
        std::fs::write(&script, "#!/bin/sh\nprintf '\\000' > \"$3\"\n").unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let image = RgbaImage::from_pixel(3, 2, image::Rgba([11, 29, 47, 83]));
        let control = ImageToolProcessControl::new();
        let (_, preview) = encode_conversion(
            image.clone(),
            ConvertTargetFormat::Jpeg,
            80,
            false,
            None,
            Some((&script, &control)),
        )
        .unwrap();
        assert_eq!(preview, image);
    }

    #[test]
    fn disconnected_cancelled_conversion_does_not_report_failure() {
        let mut state = state();
        state.convert_state.is_encoding = true;
        state.convert_state.render_input_stamp = Some(state.input_stamp());
        let (tx, rx) = std::sync::mpsc::channel::<Result<ConvertWorkerMsg, JobError>>();
        drop(tx);
        state.convert_state.render_rx = Some(rx);
        state.convert_state.render_cancel = Some(std::sync::Arc::new(
            std::sync::atomic::AtomicBool::new(true),
        ));

        state.poll_conversion();

        assert!(!state.convert_state.is_encoding);
        assert!(state.convert_state.render_error.is_none());
        assert!(state.convert_state.render_error_input_stamp.is_none());
    }

    #[test]
    fn cancellable_jpeg_path_preserves_the_existing_encoded_output() {
        let image = RgbaImage::from_fn(3, 2, |x, y| {
            image::Rgba([x as u8 * 30, y as u8 * 50, 90, (x + y) as u8 * 40])
        });
        assert_eq!(
            encode_jpeg_conversion(&image, 73, None).unwrap(),
            encode_jpeg_with_quality(&image, 73).unwrap()
        );
    }

    #[test]
    fn streaming_png_matches_existing_encoder_bytes_and_rgba() {
        let image = RgbaImage::from_fn(37, 19, |x, y| {
            image::Rgba([
                (x * 17 + y * 3) as u8,
                (y * 13 + x * 5) as u8,
                (x * 7 + y * 11) as u8,
                ((x * 19 + y * 23) % 256) as u8,
            ])
        });
        let streamed = encode_png_conversion(&image, None).unwrap();
        let existing = encode_png(&image).unwrap();

        assert_eq!(streamed, existing);
        let decoded = image::load_from_memory(&streamed).unwrap().to_rgba8();
        assert_eq!(decoded.dimensions(), image.dimensions());
        assert_eq!(decoded, image);
    }

    #[test]
    fn streaming_png_preserves_pixels_for_buffered_encoder_fallback_inputs() {
        let image = RgbaImage::from_fn(4, 4, |x, y| {
            image::Rgba([
                (x * 31 + y * 7) as u8,
                (y * 23 + x * 5) as u8,
                (x * 11 + y * 17) as u8,
                (x * 29 + y * 13) as u8,
            ])
        });
        let streamed = encode_png_conversion(&image, None).unwrap();
        let buffered = encode_png(&image).unwrap();

        assert_ne!(streamed, buffered);
        for encoded in [streamed, buffered] {
            let decoded = image::load_from_memory(&encoded).unwrap().to_rgba8();
            assert_eq!(decoded.dimensions(), (4, 4));
            assert_eq!(decoded, image);
        }
    }

    #[test]
    fn streaming_png_rejects_zero_dimensions() {
        for image in [
            RgbaImage::new(0, 2),
            RgbaImage::new(2, 0),
            RgbaImage::new(0, 0),
        ] {
            assert_eq!(
                encode_png_conversion(&image, None).unwrap_err(),
                "PNG dimensions must be nonzero"
            );
        }
    }

    struct CancelAfterPngOutput<'a> {
        inner: CancellableVecWriter<'a>,
        cancel: &'a std::sync::atomic::AtomicBool,
        bytes_written: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        threshold: usize,
    }

    impl std::io::Write for CancelAfterPngOutput<'_> {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            let written = std::io::Write::write(&mut self.inner, bytes)?;
            let total = self
                .bytes_written
                .fetch_add(written, std::sync::atomic::Ordering::AcqRel)
                + written;
            if total >= self.threshold {
                self.cancel
                    .store(true, std::sync::atomic::Ordering::Release);
            }
            Ok(written)
        }

        fn flush(&mut self) -> std::io::Result<()> {
            std::io::Write::flush(&mut self.inner)
        }
    }

    #[test]
    fn png_stream_cancellation_after_idat_output_discards_partial_result() {
        let cancel = std::sync::atomic::AtomicBool::new(false);
        let inner = CancellableVecWriter::new(Some(&cancel));
        let output = inner.output.clone();
        let bytes_written = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut sink = CancelAfterPngOutput {
            inner,
            cancel: &cancel,
            bytes_written: bytes_written.clone(),
            threshold: 1024,
        };
        let image = RgbaImage::from_fn(512, 128, |x, y| {
            let seed = x
                .wrapping_mul(1_664_525)
                .wrapping_add(y.wrapping_mul(1_013_904_223));
            image::Rgba([
                seed as u8,
                (seed >> 8) as u8,
                (seed >> 16) as u8,
                (seed >> 24) as u8,
            ])
        });

        assert_eq!(
            write_png_stream(&image, &mut sink, Some(&cancel)).unwrap_err(),
            "cancelled"
        );
        drop(sink);
        assert!(cancel.load(std::sync::atomic::Ordering::Acquire));
        assert!(bytes_written.load(std::sync::atomic::Ordering::Acquire) >= 1024);
        let partial_png = output.lock().unwrap();
        assert!(partial_png.starts_with(b"\x89PNG\r\n\x1a\n"));
        assert!(partial_png.windows(4).any(|chunk| chunk == b"IDAT"));
    }

    #[test]
    fn png_stream_cancellation_at_final_output_does_not_publish_completed_bytes() {
        let image = RgbaImage::from_fn(4, 4, |x, y| {
            image::Rgba([
                (x * 31 + y * 7) as u8,
                (y * 23 + x * 5) as u8,
                (x * 11 + y * 17) as u8,
                (x * 29 + y * 13) as u8,
            ])
        });
        let expected = encode_png_conversion(&image, None).unwrap();
        let cancel = std::sync::atomic::AtomicBool::new(false);
        let inner = CancellableVecWriter::new(Some(&cancel));
        let output = inner.output.clone();
        let bytes_written = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut sink = CancelAfterPngOutput {
            inner,
            cancel: &cancel,
            bytes_written: bytes_written.clone(),
            threshold: expected.len(),
        };

        assert_eq!(
            write_png_stream(&image, &mut sink, Some(&cancel)).unwrap_err(),
            "cancelled"
        );
        drop(sink);
        assert!(cancel.load(std::sync::atomic::Ordering::Acquire));
        assert_eq!(
            bytes_written.load(std::sync::atomic::Ordering::Acquire),
            expected.len()
        );
        assert_eq!(*output.lock().unwrap(), expected);
    }

    #[test]
    fn cancellation_aware_encoder_sink_interrupts_writes() {
        use std::io::Write;

        let writer = CancellableVecWriter::new(None);
        let output = writer.output.clone();
        let mut writer = writer;
        writer.write_all(b"partial").unwrap();
        drop(writer);
        assert_eq!(*output.lock().unwrap(), b"partial".to_vec());

        let cancel = std::sync::atomic::AtomicBool::new(true);
        let mut writer = CancellableVecWriter::new(Some(&cancel));
        assert_eq!(
            writer.write(b"discarded").unwrap_err().kind(),
            std::io::ErrorKind::Interrupted
        );
        drop(writer);
    }

    #[test]
    fn cancellation_aware_sink_checks_within_its_output_interval() {
        use std::io::Write;

        let cancel = std::sync::atomic::AtomicBool::new(true);
        let mut writer = CancellableVecWriter::new(Some(&cancel));
        writer.bytes_since_cancel_check = CONVERSION_CANCEL_CHECK_INTERVAL - 1;

        assert_eq!(writer.write(&[1, 2, 3, 4]).unwrap(), 1);
        assert_eq!(
            writer.write(&[5]).unwrap_err().kind(),
            std::io::ErrorKind::Interrupted
        );
    }

    #[test]
    fn conversion_accepts_current_result_and_applies_baked_overlays_once() {
        let mut state = state();
        state.active_mode = ImageToolMode::Convert;
        state.redaction_state.boxes.push(RedactionBox {
            normalized_rect: Rect::from_min_max(Pos2::ZERO, Pos2::new(0.5, 0.5)),
        });
        let input_stamp = state.input_stamp();
        state.convert_state.render_input_stamp = Some(input_stamp.clone());
        state.convert_state.is_encoding = true;
        let output = RgbaImage::from_pixel(4, 4, image::Rgba([7, 8, 9, 255]));
        let (tx, rx) = std::sync::mpsc::channel::<Result<ConvertWorkerMsg, JobError>>();
        state.convert_state.render_rx = Some(rx);
        tx.send(Ok(ConvertWorkerMsg {
            generation: state.convert_state.request_generation,
            input_stamp,
            result: Ok((vec![4, 5, 6], output.clone())),
        }))
        .unwrap();

        state.poll_conversion();
        assert!(state.convert_state.output_matches(&state.input_stamp()));
        assert_eq!(state.convert_state.encoded_bytes, vec![4, 5, 6]);

        state.apply_converted_to_canvas();
        assert_eq!(state.current_rgba.as_ref(), &output);
        assert_eq!(state.pixel_revision, 1);
        assert!(state.redaction_state.boxes.is_empty());
        assert!(!state.convert_state.output_matches(&state.input_stamp()));
        let scheduled_generation = state.convert_state.request_generation;
        assert!(state.convert_state.params_dirty_at.is_some());
        state.set_tool_mode(ImageToolMode::View);
        state.set_tool_mode(ImageToolMode::Convert);
        assert_eq!(state.convert_state.request_generation, scheduled_generation);
    }

    fn wait_for_optipng(state: &mut ImagePreviewState) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while state.convert_state.is_running_optipng && std::time::Instant::now() < deadline {
            state.poll_conversion();
            std::thread::yield_now();
        }
        assert!(!state.convert_state.is_running_optipng);
    }

    fn wait_for_optipng_on(state: &mut ImagePreviewState, jobs: &BackgroundJobs) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while state.convert_state.is_running_optipng && std::time::Instant::now() < deadline {
            state.poll_conversion_on(jobs);
            std::thread::yield_now();
        }
        assert!(!state.convert_state.is_running_optipng);
    }

    #[test]
    fn optipng_worker_publishes_current_success_and_preserves_bytes_on_failure() {
        let mut state = state();
        state.convert_state.target_format = ConvertTargetFormat::Png;
        state.update_conversion();
        let original = state.convert_state.encoded_bytes.clone();
        let original_len = original.len();
        let optimized = b"smaller encoded PNG".to_vec();
        let expected_optimized = optimized.clone();
        state.run_optipng_conversion_with(move |bytes, _control| {
            assert_eq!(bytes, original);
            Ok(optimized.clone())
        });
        assert!(state.convert_state.is_running_optipng);
        wait_for_optipng(&mut state);
        assert_eq!(state.convert_state.encoded_bytes, expected_optimized);
        assert_eq!(
            state.convert_state.optipng_savings,
            Some((original_len, expected_optimized.len()))
        );

        state.update_conversion();
        let original = state.convert_state.encoded_bytes.clone();
        state
            .run_optipng_conversion_with(|_, _control| Err("fixture optimizer failure".to_owned()));
        wait_for_optipng(&mut state);
        assert_eq!(state.convert_state.encoded_bytes, original);
        assert_eq!(state.convert_state.optipng_savings, None);
        assert!(state.status_toast.is_some());
    }

    #[test]
    fn optipng_worker_discards_stale_success_after_input_revision_changes() {
        let mut state = state();
        state.convert_state.target_format = ConvertTargetFormat::Png;
        state.update_conversion();
        let original = state.convert_state.encoded_bytes.clone();
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let release_rx = std::sync::Arc::new(std::sync::Mutex::new(release_rx));
        let worker_release_rx = release_rx.clone();
        state.run_optipng_conversion_with(move |_, _control| {
            started_tx.send(()).unwrap();
            worker_release_rx.lock().unwrap().recv().unwrap();
            Ok(b"stale optimized PNG".to_vec())
        });
        started_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("optimizer worker should start");
        state.schedule_conversion();
        state.poll_conversion();
        assert_eq!(state.convert_state.encoded_bytes, original);
        release_tx.send(()).unwrap();
        wait_for_optipng(&mut state);
        assert_eq!(state.convert_state.encoded_bytes, original);
        assert_eq!(state.convert_state.optipng_savings, None);
        assert!(state.convert_state.params_dirty_at.is_some());
    }

    #[test]
    fn optipng_queue_full_preserves_output_and_retries_after_delay() {
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
        let (_, queued_rx) = jobs.submit(std::time::Duration::MAX, || ());

        let mut state = state_with_png_conversion();
        let original = state.convert_state.encoded_bytes.clone();
        let original_len = original.len();
        state.convert_state.optipng_savings = Some((original_len + 4, original_len));
        let saved_before = state.convert_state.optipng_savings;
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let worker_calls = calls.clone();
        let optimizer: std::sync::Arc<OptipngOptimizer> = std::sync::Arc::new(move |_, _| {
            worker_calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(b"synthetic optimized png".to_vec())
        });
        state.run_optipng_conversion_with_on(optimizer, &jobs);
        state.poll_conversion_on(&jobs);

        assert!(!state.convert_state.is_running_optipng);
        assert!(state.convert_state.optipng_retry_at.is_some());
        assert!(
            state
                .convert_state
                .optipng_queue_status
                .as_deref()
                .is_some_and(|status| status.contains("busy"))
        );
        assert_eq!(state.convert_state.encoded_bytes, original);
        assert_eq!(state.convert_state.optipng_savings, saved_before);
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);
        let retry_deadline = state.convert_state.optipng_retry_at.unwrap();
        state.poll_conversion_on(&jobs);
        assert!(!state.convert_state.is_running_optipng);
        assert_eq!(state.convert_state.optipng_retry_at, Some(retry_deadline));

        release_tx.send(()).unwrap();
        blocker_rx.recv().unwrap().unwrap();
        queued_rx.recv().unwrap().unwrap();
        state.convert_state.optipng_retry_at = Some(std::time::Instant::now());
        state.poll_conversion_on(&jobs);
        assert!(state.convert_state.is_running_optipng);
        assert!(state.convert_state.optipng_retry_at.is_none());
        wait_for_optipng_on(&mut state, &jobs);
        state.poll_conversion_on(&jobs);
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(
            state.convert_state.encoded_bytes,
            b"synthetic optimized png"
        );
        assert_eq!(
            state.convert_state.optipng_savings,
            Some((original_len, b"synthetic optimized png".len()))
        );
        assert!(state.convert_state.optipng_queue_status.is_none());
    }

    #[test]
    fn optipng_stale_retry_is_cancelled_for_input_and_settings_changes() {
        for change_input in [false, true] {
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
            let (_, queued_rx) = jobs.submit(std::time::Duration::MAX, || ());
            let mut state = state_with_png_conversion();
            let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let worker_calls = calls.clone();
            let optimizer: std::sync::Arc<OptipngOptimizer> = std::sync::Arc::new(move |_, _| {
                worker_calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(vec![1, 2, 3])
            });
            state.run_optipng_conversion_with_on(optimizer, &jobs);
            state.poll_conversion_on(&jobs);
            assert!(state.convert_state.optipng_retry_at.is_some());

            if change_input {
                state.mark_pixels_changed();
            } else {
                state.convert_state.quality = state.convert_state.quality.saturating_sub(1);
                state.poll_conversion_on(&jobs);
            }
            assert!(state.convert_state.optipng_retry_at.is_none());
            assert!(state.convert_state.optipng_retry_work.is_none());
            assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);

            release_tx.send(()).unwrap();
            blocker_rx.recv().unwrap().unwrap();
            queued_rx.recv().unwrap().unwrap();
        }
    }

    #[test]
    fn dropping_viewer_cancels_queued_optipng_ticket_and_worker() {
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
        let (_, queued_rx) = jobs.submit(std::time::Duration::MAX, || ());
        let mut state = state_with_png_conversion();
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let worker_calls = calls.clone();
        let optimizer: std::sync::Arc<OptipngOptimizer> = std::sync::Arc::new(move |_, _| {
            worker_calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(vec![9, 8, 7])
        });
        state.run_optipng_conversion_with_on(optimizer, &jobs);
        let process_control = state
            .convert_state
            .optipng_process_control
            .as_ref()
            .unwrap()
            .clone();
        let worker_rx = state.convert_state.optipng_rx.take().unwrap();
        drop(state);
        assert!(
            process_control
                .cancel
                .load(std::sync::atomic::Ordering::SeqCst)
        );

        release_tx.send(()).unwrap();
        blocker_rx.recv().unwrap().unwrap();
        queued_rx.recv().unwrap().unwrap();
        assert!(matches!(
            worker_rx
                .recv_timeout(std::time::Duration::from_secs(1))
                .unwrap(),
            Err(JobError::Cancelled)
        ));
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);
    }
}
