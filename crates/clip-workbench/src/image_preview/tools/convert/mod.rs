mod encoding;
mod optipng;
mod preview_jobs;
mod studio;

use super::super::*;
use encoding::*;
use little_clip_core::{BackgroundJobs, JobError, JobTicket};
use optipng::run_optipng_optimization;

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

#[cfg(test)]
mod input_stamp_tests;
