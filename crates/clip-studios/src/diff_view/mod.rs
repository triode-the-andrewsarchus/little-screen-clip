//! Text and image differential inspection workbench view.
//!
//! Provides side-by-side, unified, and inline word diffs for text, and 3-up/overlay/mask
//! pixel diffs for bitmap images.

mod handoff;
mod image_view;
mod studio;
mod text;

use crate::ansi::ansi_to_layout_job;
use crate::icons::Icon;
use crate::shell::{EscapeHatch, StudioShell};
use crate::ui_ext::{BODY_FONT_SIZE, CONTROL_HEIGHT};
use egui::{self, Color32, Rounding, Stroke, Vec2};
use little_clip_core::HandSide;
use little_clip_core::ipc::IpcRequest;
use little_clip_core::jobs::task::Task;
use little_clip_core::{BackgroundJobs, JobTicket};
use little_clip_ops::diff::{DiffDisplayMode, DiffResult, DiffTarget, compute_diff};
use little_clip_ops::image_diff::{ImageDiffOptions, ImageDiffResult};

enum DiffHandoffTarget {
    Existing(std::path::PathBuf),
    Shared(std::sync::Arc<DiffTarget>, String),
}

#[derive(Debug)]
struct StagedHandoffPath {
    path: std::path::PathBuf,
    remove_on_drop: bool,
}

impl StagedHandoffPath {
    fn preserve_for_external_tool(&mut self) {
        self.remove_on_drop = false;
    }
}

impl Drop for StagedHandoffPath {
    fn drop(&mut self) {
        if self.remove_on_drop {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

struct HandoffCancelGuard(std::sync::Arc<std::sync::atomic::AtomicBool>);

impl Drop for HandoffCancelGuard {
    fn drop(&mut self) {
        self.0.store(true, std::sync::atomic::Ordering::Release);
    }
}

impl DiffHandoffTarget {
    fn from_target(target: &std::sync::Arc<DiffTarget>, side: &str) -> Self {
        match target.as_ref() {
            DiffTarget::File { path, .. }
            | DiffTarget::Image {
                path: Some(path), ..
            } => Self::Existing(path.clone()),
            _ => Self::Shared(target.clone(), side.into()),
        }
    }

    fn stage(self) -> Result<StagedHandoffPath, String> {
        match self {
            Self::Existing(path) => Ok(StagedHandoffPath {
                path,
                remove_on_drop: false,
            }),
            Self::Shared(target, side) => {
                let (bytes, ext): (&[u8], &str) = match target.as_ref() {
                    DiffTarget::ClipboardText {
                        content, ext_hint, ..
                    } => (content.as_bytes(), ext_hint.as_deref().unwrap_or("txt")),
                    DiffTarget::Image { bytes, mime, .. } => {
                        (bytes, if mime.contains("png") { "png" } else { "jpg" })
                    }
                    _ => unreachable!(),
                };
                use std::io::Write;
                let stage = || -> std::io::Result<std::path::PathBuf> {
                    let mut file = tempfile::Builder::new()
                        .prefix(&format!("clip_diff_{side}_"))
                        .suffix(&format!(".{ext}"))
                        .tempfile()?;
                    file.write_all(bytes)?;
                    file.into_temp_path().keep().map_err(|error| error.error)
                };
                let path = stage()
                    .map_err(|error| format!("Failed to stage diff {side} file: {error}"))?;
                Ok(StagedHandoffPath {
                    path,
                    remove_on_drop: true,
                })
            }
        }
    }
}

/// Layout visualization mode for comparing bitmap images.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageDiffViewMode {
    /// 3-Up view: `[Base] [Comparison] [Diff Mask]`.
    ThreeUp, // [ base ] [ comparison ] [ diff ] (matching ODiff demo table)
    /// Dimmed base image overlayed with glowing diff pixels.
    Overlay, // [ dimmed base + glowing diff pixels ]
    /// High-contrast black mask highlighting pixel differences.
    MaskOnly, // [ black mask + glowing diff pixels ]
    /// Split view: `[Base] [Diff Mask]`.
    Split, // [ base ] [ diff ]
}

impl ImageDiffViewMode {
    /// Human-readable display label.
    pub fn label(&self) -> &'static str {
        match self {
            ImageDiffViewMode::ThreeUp => "3-Up (Base | Comp | Diff)",
            ImageDiffViewMode::Overlay => "Overlay",
            ImageDiffViewMode::MaskOnly => "Diff Mask",
            ImageDiffViewMode::Split => "Split (Base | Diff)",
        }
    }

    /// SVG icon for mode tab.
    pub fn icon(&self) -> Icon {
        match self {
            ImageDiffViewMode::ThreeUp => Icon::Grid,
            ImageDiffViewMode::Overlay => Icon::Layers,
            ImageDiffViewMode::MaskOnly => Icon::Eye,
            ImageDiffViewMode::Split => Icon::Diff,
        }
    }
}

/// Interactive state for the text and visual image diff viewer.
pub struct DiffViewerState {
    pub lhs: std::sync::Arc<DiffTarget>,
    pub rhs: std::sync::Arc<DiffTarget>,
    pub mode: DiffDisplayMode,
    // Text diff state
    pub result: DiffResult,
    pub layout_job: egui::text::LayoutJob,
    // Image diff state
    pub image_result: Option<std::sync::Arc<ImageDiffResult>>,
    copy_job: Option<crate::desktop_ipc::PendingRequest>,
    pub image_options: ImageDiffOptions,
    pub image_mode: ImageDiffViewMode,
    pub base_texture: Option<egui::TextureHandle>,
    pub comp_texture: Option<egui::TextureHandle>,
    pub mask_texture: Option<egui::TextureHandle>,
    pub overlay_texture: Option<egui::TextureHandle>,
    pub textures_loaded: bool,
    pub decode_error: Option<String>,
    pub status_toast: Option<(crate::ui_ext::StatusMessage, f64)>, // (text, timestamp)
    compute_job: Option<Task<Result<DiffComputation, String>>>,
    pub compute_start_time: std::time::Instant,
    handoff: Option<(
        JobTicket,
        little_clip_core::jobs::JobReceiver<Result<String, String>>,
    )>,
    handoff_cancel: Option<HandoffCancelGuard>,
}

impl Drop for DiffViewerState {
    fn drop(&mut self) {
        self.cancel_handoff();
    }
}

enum DiffComputation {
    Image(ImageDiffResult),
    Text(DiffResult, egui::text::LayoutJob),
}

fn spawn_image_diff_worker(
    lhs: &std::sync::Arc<DiffTarget>,
    rhs: &std::sync::Arc<DiffTarget>,
    options: ImageDiffOptions,
    jobs: &BackgroundJobs,
) -> Result<Task<Result<DiffComputation, String>>, String> {
    if let (Some(b1), Some(b2)) = (lhs.image_bytes(), rhs.image_bytes()) {
        if b1.is_empty() || b2.is_empty() {
            let mut missing = Vec::new();
            if b1.is_empty() {
                missing.push(format!("left target '{}'", lhs.label()));
            }
            if b2.is_empty() {
                missing.push(format!("right target '{}'", rhs.label()));
            }
            let err = format!(
                "Image data could not be loaded for {}. Please check that the file exists.",
                missing.join(" and ")
            );
            return Err(err);
        }

        let lhs = std::sync::Arc::clone(lhs);
        let rhs = std::sync::Arc::clone(rhs);
        let job = Task::start_unbounded(jobs, move |_| {
            let (Some(b1), Some(b2)) = (lhs.image_bytes(), rhs.image_bytes()) else {
                return Err("Failed to obtain image bytes from comparison targets".to_string());
            };
            little_clip_ops::image_diff::compare_image_bytes_with_mime(
                b1,
                lhs.image_mime(),
                b2,
                rhs.image_mime(),
                &options,
            )
            .map(DiffComputation::Image)
            .map_err(|e| e.to_string())
        });

        Ok(job)
    } else {
        Err("Failed to obtain image bytes from comparison targets".to_string())
    }
}

fn spawn_text_diff_worker(
    lhs: &std::sync::Arc<DiffTarget>,
    rhs: &std::sync::Arc<DiffTarget>,
    mode: DiffDisplayMode,
    width: usize,
    jobs: &BackgroundJobs,
) -> Task<Result<DiffComputation, String>> {
    let lhs = std::sync::Arc::clone(lhs);
    let rhs = std::sync::Arc::clone(rhs);
    Task::start_unbounded(jobs, move |_| {
        let r = compute_diff(&lhs, &rhs, mode, width);
        let job = ansi_to_layout_job(&r.raw_ansi, crate::ui_ext::MONO_FONT_SIZE);
        Ok(DiffComputation::Text(r, job))
    })
}

impl DiffViewerState {
    /// Initializes text or image diff calculation between two comparison targets.
    pub fn new(lhs: DiffTarget, rhs: DiffTarget, mode: DiffDisplayMode) -> Self {
        Self::new_with_jobs(lhs, rhs, mode, BackgroundJobs::global())
    }

    fn new_with_jobs(
        lhs: DiffTarget,
        rhs: DiffTarget,
        mode: DiffDisplayMode,
        jobs: &BackgroundJobs,
    ) -> Self {
        let lhs = std::sync::Arc::new(lhs);
        let rhs = std::sync::Arc::new(rhs);
        let is_img = lhs.is_image() && rhs.is_image();
        let image_options = ImageDiffOptions::default();

        let (compute_job, decode_error) = if is_img {
            match spawn_image_diff_worker(&lhs, &rhs, image_options.clone(), jobs) {
                Ok(job) => (Some(job), None),
                Err(error) => (None, Some(error)),
            }
        } else {
            let width = if mode == DiffDisplayMode::SideBySide {
                150
            } else {
                120
            };
            (
                Some(spawn_text_diff_worker(&lhs, &rhs, mode, width, jobs)),
                None,
            )
        };

        let result = DiffResult {
            engine_name: "Computing...".to_string(),
            raw_ansi: String::new(),
            is_identical: false,
            mode,
        };
        let layout_job = ansi_to_layout_job("", crate::ui_ext::MONO_FONT_SIZE);

        Self {
            lhs,
            rhs,
            mode,
            result,
            layout_job,
            image_result: None,
            copy_job: None,
            image_options,
            image_mode: ImageDiffViewMode::ThreeUp,
            base_texture: None,
            comp_texture: None,
            mask_texture: None,
            overlay_texture: None,
            textures_loaded: false,
            decode_error,
            status_toast: None,
            compute_job,
            compute_start_time: std::time::Instant::now(),
            handoff: None,
            handoff_cancel: None,
        }
    }

    #[cfg(test)]
    pub fn wait_for_completion(&mut self) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while self.is_computing() {
            self.poll_computation();
            assert!(
                std::time::Instant::now() < deadline,
                "diff computation timed out"
            );
            std::thread::yield_now();
        }
    }

    pub fn is_image(&self) -> bool {
        self.lhs.is_image() && self.rhs.is_image()
    }

    pub fn is_computing(&self) -> bool {
        self.compute_job.is_some()
    }

    fn poll_computation(&mut self) {
        let Some(outcome) = self.compute_job.as_ref().and_then(Task::poll) else {
            return;
        };
        self.compute_job = None;
        let outcome = outcome
            .map_err(|error| format!("Diff computation failed: {error}"))
            .and_then(|result| result);
        match outcome {
            Ok(DiffComputation::Image(result)) => {
                self.image_result = Some(std::sync::Arc::new(result));
                self.textures_loaded = false;
                self.decode_error = None;
            }
            Ok(DiffComputation::Text(result, layout_job)) => {
                self.result = result;
                self.layout_job = layout_job;
                self.decode_error = None;
            }
            Err(error) => {
                self.decode_error = Some(error);
                if self.is_image() {
                    self.image_result = None;
                }
            }
        }
    }

    pub fn recompute(&mut self) {
        // Retire the old read before starting its replacement. Task owns cancellation.
        self.compute_job = None;
        if self.is_image() {
            match spawn_image_diff_worker(
                &self.lhs,
                &self.rhs,
                self.image_options.clone(),
                BackgroundJobs::global(),
            ) {
                Ok(job) => {
                    self.compute_job = Some(job);
                    self.decode_error = None;
                }
                Err(error) => self.decode_error = Some(error),
            }
            self.textures_loaded = false;
        } else {
            let width = if self.mode == DiffDisplayMode::SideBySide {
                150
            } else {
                120
            };
            self.compute_job = Some(spawn_text_diff_worker(
                &self.lhs,
                &self.rhs,
                self.mode,
                width,
                BackgroundJobs::global(),
            ));
        }
        self.compute_start_time = std::time::Instant::now();
    }

    pub fn toggle_mode(&mut self) {
        if self.is_image() {
            self.image_mode = match self.image_mode {
                ImageDiffViewMode::ThreeUp => ImageDiffViewMode::Overlay,
                ImageDiffViewMode::Overlay => ImageDiffViewMode::MaskOnly,
                ImageDiffViewMode::MaskOnly => ImageDiffViewMode::Split,
                ImageDiffViewMode::Split => ImageDiffViewMode::ThreeUp,
            };
        } else {
            self.mode = self.mode.toggled();
            self.recompute();
        }
    }

    pub fn swap_operands(&mut self) {
        std::mem::swap(&mut self.lhs, &mut self.rhs);
        self.recompute();
    }
}

fn image_mask_push_request(result: &ImageDiffResult) -> Result<IpcRequest, image::ImageError> {
    Ok(IpcRequest::PushImage {
        bytes: result.encode_mask_png()?,
        mime: "image/png".to_string(),
        hand: HandSide::Left,
    })
}

fn strip_ansi_codes(text: &str) -> String {
    let mut clean = String::with_capacity(text.len());
    let mut in_escape = false;

    for c in text.chars() {
        if c == '\x1b' {
            in_escape = true;
        } else if in_escape {
            if c == 'm' {
                in_escape = false;
            }
        } else {
            clean.push(c);
        }
    }

    clean
}

#[cfg(test)]
mod tests;
