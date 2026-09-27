//! Save As snapshots pixels on the bounded executor before allowing further edits.
use super::*;
use little_clip_core::jobs::{BackgroundJobs, JobError, JobTicket};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

const SAVE_TIMEOUT: Duration = Duration::from_secs(300);
const CANCELLABLE: u8 = 0;
const CANCELLED: u8 = 1;
const WRITING: u8 = 2;

type SaveCanvas = (RgbaImage, Vec<u8>);

enum SaveSnapshot {
    Pixels(RgbaImage),
    Converted(Vec<u8>),
}

pub(super) struct ImageSaveJob {
    ticket: JobTicket,
    phase: Arc<AtomicU8>,
    snapshot_ready: Arc<AtomicBool>,
    canvas: Arc<Mutex<Option<SaveCanvas>>>,
    receiver: mpsc::Receiver<Result<Result<SaveResult, String>, JobError>>,
    stamp: ImageInputStamp,
    restored: bool,
}

pub(super) struct ImageCopyJob {
    ticket: JobTicket,
    phase: Arc<AtomicU8>,
    snapshot_ready: Arc<AtomicBool>,
    canvas: Arc<Mutex<Option<RgbaImage>>>,
    receiver: mpsc::Receiver<Result<Result<String, String>, JobError>>,
    stamp: ImageInputStamp,
    restored: bool,
}

impl Drop for ImageCopyJob {
    fn drop(&mut self) {
        if self
            .phase
            .compare_exchange(CANCELLABLE, CANCELLED, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            self.ticket.cancel();
        }
    }
}

impl Drop for ImageSaveJob {
    fn drop(&mut self) {
        self.cancel();
    }
}

struct SaveResult {
    path: PathBuf,
    file_size: usize,
}

struct SaveOptions {
    path: PathBuf,
    stamp: ImageInputStamp,
    converted: bool,
    quality: u8,
    lossless: bool,
    conversion_format: Option<ConvertTargetFormat>,
}

impl ImageSaveJob {
    fn cancel(&self) -> bool {
        if self
            .phase
            .compare_exchange(CANCELLABLE, CANCELLED, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            self.ticket.cancel();
            true
        } else {
            false
        }
    }

    fn start(canvas: SaveCanvas, options: SaveOptions, jobs: &BackgroundJobs) -> Self {
        let stamp = options.stamp.clone();
        let canvas = Arc::new(Mutex::new(Some(canvas)));
        let worker_canvas = Arc::clone(&canvas);
        let snapshot_ready = Arc::new(AtomicBool::new(false));
        let worker_ready = Arc::clone(&snapshot_ready);
        let phase = Arc::new(AtomicU8::new(CANCELLABLE));
        let worker_phase = Arc::clone(&phase);
        // Once the write boundary is crossed, report its actual result regardless of
        // duration. The worker enforces the preparation timeout before that boundary.
        let (ticket, receiver) = jobs.submit(Duration::MAX, move || {
            // Retain the original buffers in the slot even on panic or queue rejection.
            // Only this initial copy requires exclusive ownership of the live canvas.
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let started = Instant::now();
                check_cancelled(&worker_phase, started)?;
                let snapshot = {
                    let slot = worker_canvas.lock().unwrap_or_else(|e| e.into_inner());
                    let original = slot.as_ref().ok_or("Save canvas unavailable")?;
                    if options.converted {
                        SaveSnapshot::Converted(original.1.clone())
                    } else {
                        SaveSnapshot::Pixels(original.0.clone())
                    }
                };
                worker_ready.store(true, Ordering::Release);
                write_save(snapshot, options, &worker_phase, started)
            }))
            .unwrap_or_else(|_| Err("Image save worker panicked".into()))
        });
        Self {
            ticket,
            phase,
            snapshot_ready,
            canvas,
            receiver,
            stamp,
            restored: false,
        }
    }
}

impl ImageCopyJob {
    fn start(
        canvas: RgbaImage,
        stamp: ImageInputStamp,
        hand: HandSide,
        jobs: &BackgroundJobs,
    ) -> Self {
        let canvas = Arc::new(Mutex::new(Some(canvas)));
        let worker_canvas = Arc::clone(&canvas);
        let snapshot_ready = Arc::new(AtomicBool::new(false));
        let worker_ready = Arc::clone(&snapshot_ready);
        let phase = Arc::new(AtomicU8::new(CANCELLABLE));
        let worker_phase = Arc::clone(&phase);
        let worker_stamp = stamp.clone();
        let (ticket, receiver) = jobs.submit(Duration::MAX, move || {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let started = Instant::now();
                check_copy_cancelled(&worker_phase, started)?;
                let snapshot = {
                    let slot = worker_canvas.lock().unwrap_or_else(|e| e.into_inner());
                    slot.as_ref().ok_or("Copy image unavailable")?.clone()
                };
                worker_ready.store(true, Ordering::Release);
                check_copy_cancelled(&worker_phase, started)?;
                let composed = compose_display_image(snapshot, &worker_stamp);
                check_copy_cancelled(&worker_phase, started)?;
                let bytes = encode_png(&composed).map_err(|error| error.to_string())?;
                check_copy_cancelled(&worker_phase, started)?;
                phase_for_delivery(&worker_phase, started)?;
                deliver_image_copy(
                    &bytes,
                    hand,
                    || worker_phase.load(Ordering::Acquire) == CANCELLED,
                    |request| {
                        crate::send_ipc_request_with_timeout(request, Some(Duration::from_secs(2)))
                            .map_err(|error| error.to_string())
                            .and_then(|response| match response {
                                little_clip_core::ipc::IpcResponse::Success { .. } => Ok(()),
                                little_clip_core::ipc::IpcResponse::Error { error } => Err(error),
                                other => Err(format!("Unexpected clipboard response: {other:?}")),
                            })
                    },
                    |bytes, mime| {
                        little_clip_core::copy_bytes_to_system_clipboard(bytes, mime)
                            .map_err(|error| error.to_string())
                    },
                )
            }))
            .unwrap_or_else(|_| Err("Image clipboard worker panicked".into()))
        });
        Self {
            ticket,
            phase,
            snapshot_ready,
            canvas,
            receiver,
            stamp,
            restored: false,
        }
    }

    fn cancel(&self) -> bool {
        if self
            .phase
            .compare_exchange(CANCELLABLE, CANCELLED, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            self.ticket.cancel();
            true
        } else {
            false
        }
    }
}

fn phase_for_delivery(phase: &AtomicU8, started: Instant) -> Result<(), String> {
    check_copy_cancelled(phase, started)?;
    phase
        .compare_exchange(CANCELLABLE, WRITING, Ordering::AcqRel, Ordering::Acquire)
        .map(|_| ())
        .map_err(|_| "Image clipboard copy cancelled".into())
}

fn check_copy_cancelled(phase: &AtomicU8, started: Instant) -> Result<(), String> {
    if phase.load(Ordering::Acquire) == CANCELLED {
        Err("Image clipboard copy cancelled".into())
    } else if started.elapsed() > SAVE_TIMEOUT {
        Err("Image clipboard copy timed out".into())
    } else {
        Ok(())
    }
}

fn deliver_image_copy<I, C>(
    bytes: &[u8],
    hand: HandSide,
    cancelled: impl Fn() -> bool,
    mut push_to_daemon: I,
    mut copy_to_system_clipboard: C,
) -> Result<String, String>
where
    I: FnMut(&IpcRequest) -> Result<(), String>,
    C: FnMut(&[u8], &str) -> Result<(), String>,
{
    if cancelled() {
        return Err("Image clipboard copy cancelled".into());
    }
    let request = IpcRequest::PushImage {
        bytes: bytes.to_vec(),
        mime: "image/png".into(),
        hand,
    };
    let ipc_result = push_to_daemon(&request);
    if cancelled() {
        return Err("Image clipboard copy cancelled".into());
    }
    let clipboard_result = copy_to_system_clipboard(bytes, "image/png");
    match (ipc_result, clipboard_result) {
        (Ok(()), Ok(())) => Ok(format!(
            "📋 Copied image to {} Hand slot & OS Clipboard",
            hand_slot_label(hand)
        )),
        (Ok(()), Err(error)) => Ok(format!(
            "📋 Copied image to {} Hand slot; OS clipboard failed: {error}",
            hand_slot_label(hand)
        )),
        (Err(_), Ok(())) => Ok("📋 Copied image to OS Clipboard (daemon unavailable)".into()),
        (Err(ipc), Err(clipboard)) => Err(format!(
            "Could not copy image to clipboard: daemon: {ipc}; OS clipboard: {clipboard}"
        )),
    }
}

fn hand_slot_label(hand: HandSide) -> &'static str {
    match hand {
        HandSide::Left => "Left Hand (LH)",
        HandSide::Right => "Right Hand (RH)",
    }
}

fn check_cancelled(phase: &AtomicU8, started: Instant) -> Result<(), String> {
    if phase.load(Ordering::Acquire) == CANCELLED {
        Err("Image save cancelled".into())
    } else if started.elapsed() > SAVE_TIMEOUT {
        Err("Image save timed out".into())
    } else {
        Ok(())
    }
}

fn begin_write(phase: &AtomicU8, started: Instant) -> Result<(), String> {
    check_cancelled(phase, started)?;
    phase
        .compare_exchange(CANCELLABLE, WRITING, Ordering::AcqRel, Ordering::Acquire)
        .map(|_| ())
        .map_err(|_| "Image save cancelled".into())
}

fn write_save(
    snapshot: SaveSnapshot,
    options: SaveOptions,
    phase: &AtomicU8,
    started: Instant,
) -> Result<SaveResult, String> {
    check_cancelled(phase, started)?;
    let ext = options
        .path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("png")
        .to_lowercase();
    let bytes = match snapshot {
        SaveSnapshot::Converted(bytes) => bytes,
        SaveSnapshot::Pixels(canvas) => {
            let img = compose_display_image(canvas, &options.stamp);
            check_cancelled(phase, started)?;
            match ext.as_str() {
                "jpg" | "jpeg" => encode_jpeg_with_quality(
                    &img,
                    if options.conversion_format.is_some() {
                        options.quality
                    } else {
                        75
                    },
                )
                .map_err(|e| e.to_string())?,
                "webp" => encode_webp(&img, options.lossless, options.quality)?,
                _ => {
                    let format = image::ImageFormat::from_extension(&ext)
                        .ok_or_else(|| format!("Unsupported image extension: {ext}"))?;
                    let mut bytes = std::io::Cursor::new(Vec::new());
                    img.write_to(&mut bytes, format)
                        .map_err(|e| e.to_string())?;
                    bytes.into_inner()
                }
            }
        }
    };
    check_cancelled(phase, started)?;
    let parent = options
        .path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)
        .map_err(|e| format!("Failed to create destination directory: {e}"))?;
    // Preserve ordinary overwrite/create semantics, including symlinks, hardlinks,
    // permissions and the process umask. Cancellation is checked before the write;
    // an already-started filesystem write finishes normally.
    begin_write(phase, started)?;
    std::fs::write(&options.path, &bytes).map_err(|e| e.to_string())?;
    Ok(SaveResult {
        path: options.path,
        file_size: bytes.len(),
    })
}

impl ImagePreviewState {
    pub(super) fn start_copy_to_hand_slot(&mut self, hand: HandSide, jobs: &BackgroundJobs) {
        if let Some(job) = self.copy_job.as_ref() {
            if !job.cancel() {
                self.show_toast("Image clipboard delivery is already in progress");
            } else {
                self.show_toast("Cancelling previous image copy before starting another");
            }
            return;
        }
        if self.save_snapshot_pending() || self.current_rgba.is_empty() {
            self.show_toast("Image pixels are temporarily unavailable; try copying again shortly");
            return;
        }
        let stamp = self.input_stamp();
        let canvas = std::mem::take(&mut self.current_rgba);
        self.copy_job = Some(ImageCopyJob::start(canvas, stamp, hand, jobs));
        self.show_toast("Preparing image for clipboard…");
    }

    pub(super) fn copy_snapshot_pending(&self) -> bool {
        self.copy_job.as_ref().is_some_and(|job| !job.restored)
    }

    pub(super) fn poll_copy_job(&mut self) {
        let Some(job) = self.copy_job.as_mut() else {
            return;
        };
        let completion = match job.receiver.try_recv() {
            Ok(result) => Some(
                result
                    .map_err(|error| error.to_string())
                    .and_then(|result| result),
            ),
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => {
                Some(Err("Image clipboard worker disconnected".into()))
            }
        };
        if !job.restored && (job.snapshot_ready.load(Ordering::Acquire) || completion.is_some()) {
            let mut slot = match job.canvas.try_lock() {
                Ok(slot) => slot,
                Err(std::sync::TryLockError::Poisoned(error)) => error.into_inner(),
                Err(std::sync::TryLockError::WouldBlock) => return,
            };
            if let Some(canvas) = slot.take() {
                self.current_rgba = canvas;
                job.restored = true;
            }
        }
        if let Some(completion) = completion {
            self.copy_job = None;
            match completion {
                Ok(message) => self.show_toast(message),
                Err(error)
                    if error == "job was cancelled"
                        || error == "Image clipboard copy cancelled" =>
                {
                    self.show_toast("Image clipboard copy cancelled")
                }
                Err(error) => self.show_toast(format!("❌ Failed to copy image: {error}")),
            }
        }
    }

    pub(super) fn render_copy_progress(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        close: &mut bool,
    ) {
        let Some(job) = self.copy_job.as_ref() else {
            return;
        };
        let blocked = !job.restored;
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(if blocked {
                "Preparing clipboard image snapshot…"
            } else {
                "Copying image to clipboard…"
            });
            if ui
                .add_enabled(!job.restored, egui::Button::new("Cancel copy"))
                .clicked()
            {
                job.cancel();
            }
            if blocked && ui.button("Close (Esc)").clicked() {
                *close = true;
            }
        });
        if blocked {
            if ui.input(|input| input.key_pressed(egui::Key::Escape)) {
                *close = true;
            }
            if let Some(texture) = &self.texture {
                let size = texture.size_vec2();
                let available = ui.available_size();
                let scale = (available.x / size.x).min(available.y / size.y).min(1.0);
                ui.centered_and_justified(|ui| {
                    let response = ui.image((texture.id(), size * scale));
                    let image_rect = Rect::from_center_size(response.rect.center(), size * scale);
                    for redaction in &job.stamp.redactions {
                        let norm = redaction.normalized_rect;
                        let to_screen = |point: Pos2| {
                            image_rect.min
                                + Vec2::new(
                                    point.x * image_rect.width(),
                                    point.y * image_rect.height(),
                                )
                        };
                        ui.painter().rect_filled(
                            Rect::from_min_max(to_screen(norm.min), to_screen(norm.max)),
                            0.0,
                            Color32::BLACK,
                        );
                    }
                });
            }
        }
        ctx.request_repaint_after(Duration::from_millis(16));
    }

    pub(super) fn save_snapshot_pending(&self) -> bool {
        self.save_job.as_ref().is_some_and(|job| !job.restored)
    }

    pub fn save_image_to_path(&mut self, path: &Path) {
        self.start_save(path, BackgroundJobs::global());
    }

    fn start_save(&mut self, path: &Path, jobs: &BackgroundJobs) {
        if self.save_job.is_some() {
            self.show_toast("An image save is already in progress");
            return;
        }
        let mut path = path.to_path_buf();
        if path.extension().is_none() {
            path.set_extension("png");
        }
        let stamp = self.input_stamp();
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("png");
        let conversion_format = (self.active_mode == ImageToolMode::Convert
            && (ext.eq_ignore_ascii_case(self.convert_state.target_format.extension())
                || (self.convert_state.target_format == ConvertTargetFormat::Jpeg
                    && ext.eq_ignore_ascii_case("jpeg"))))
        .then_some(self.convert_state.target_format);
        let converted = conversion_format.is_some()
            && !self.convert_state.encoded_bytes.is_empty()
            && self.convert_state.output_matches(&stamp);
        let encoded_bytes = if converted {
            std::mem::take(&mut self.convert_state.encoded_bytes)
        } else {
            Vec::new()
        };
        let canvas = std::mem::take(&mut self.current_rgba);
        self.save_job = Some(ImageSaveJob::start(
            (canvas, encoded_bytes),
            SaveOptions {
                path,
                stamp,
                converted,
                quality: self.convert_state.quality,
                lossless: self.convert_state.lossless,
                conversion_format,
            },
            jobs,
        ));
    }

    pub(super) fn poll_save_job(&mut self) {
        let Some(job) = self.save_job.as_mut() else {
            return;
        };
        let completion = match job.receiver.try_recv() {
            Ok(result) => Some(result.map_err(|e| e.to_string()).and_then(|result| result)),
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => {
                Some(Err("Image save worker disconnected".into()))
            }
        };
        if !job.restored && (job.snapshot_ready.load(Ordering::Acquire) || completion.is_some()) {
            // Never wait on the UI thread; a poisoned slot still contains the original.
            let mut slot = match job.canvas.try_lock() {
                Ok(slot) => slot,
                Err(std::sync::TryLockError::Poisoned(e)) => e.into_inner(),
                Err(std::sync::TryLockError::WouldBlock) => return,
            };
            if let Some((canvas, encoded_bytes)) = slot.take() {
                self.current_rgba = canvas;
                if !encoded_bytes.is_empty() {
                    self.convert_state.encoded_bytes = encoded_bytes;
                }
                job.restored = true;
            }
        }
        if let Some(completion) = completion {
            let stamp = job.stamp.clone();
            self.save_job = None;
            match completion {
                Ok(result) => {
                    let name = result
                        .path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string();
                    if self.input_stamp() == stamp {
                        self.path = Some(result.path.clone());
                        self.title = name.clone();
                        self.file_size_bytes = result.file_size;
                        self.mime = little_clip_core::item::guess_image_mime(&result.path);
                        self.show_toast(format!("💾 Saved image to {name}"));
                    } else {
                        self.show_toast(format!(
                            "💾 Saved earlier image snapshot to {name}; current edits are unsaved"
                        ));
                    }
                }
                Err(error) if error == "job was cancelled" || error == "Image save cancelled" => {
                    self.show_toast("Image save cancelled");
                }
                Err(error) => self.show_toast(format!("❌ Failed to save: {error}")),
            }
        }
    }

    /// Returns true while live pixels are owned exclusively by the snapshot worker.
    pub(super) fn render_save_progress(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        close: &mut bool,
    ) -> bool {
        let Some(job) = self.save_job.as_ref() else {
            return false;
        };
        let blocked = !job.restored;
        let phase = job.phase.load(Ordering::Acquire);
        let cancelled = phase == CANCELLED;
        let writing = phase == WRITING;
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(if writing {
                "Finishing save…"
            } else if cancelled {
                "Cancelling image save…"
            } else if blocked {
                "Preparing image snapshot…"
            } else {
                "Saving image…"
            });
            if ui
                .add_enabled(!cancelled && !writing, egui::Button::new("Cancel save"))
                .clicked()
            {
                job.cancel();
            }
            if blocked && ui.button("Close (Esc)").clicked() {
                *close = true;
            }
        });
        if blocked {
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                *close = true;
            }
            if let Some(texture) = &self.texture {
                let size = texture.size_vec2();
                let available = ui.available_size();
                let scale = (available.x / size.x).min(available.y / size.y).min(1.0);
                ui.centered_and_justified(|ui| {
                    let response = ui.image((texture.id(), size * scale));
                    // Justified layout expands Response::rect to the container; the
                    // image itself remains centered at its requested texture size.
                    let image_rect = Rect::from_center_size(response.rect.center(), size * scale);
                    // The retained texture contains raw pixels; keep pending redactions
                    // visible while the normal interactive overlay is suspended.
                    for redaction in &job.stamp.redactions {
                        let norm = redaction.normalized_rect;
                        let to_screen = |p: Pos2| {
                            image_rect.min
                                + Vec2::new(p.x * image_rect.width(), p.y * image_rect.height())
                        };
                        ui.painter().rect_filled(
                            Rect::from_min_max(to_screen(norm.min), to_screen(norm.max)),
                            0.0,
                            Color32::BLACK,
                        );
                    }
                });
            }
        }
        ctx.request_repaint_after(Duration::from_millis(16));
        blocked
    }
}

#[cfg(test)]
pub(super) fn wait_for_save(state: &mut ImagePreviewState) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while state.save_job.is_some() {
        state.poll_save_job();
        assert!(Instant::now() < deadline, "image save did not finish");
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[cfg(test)]
fn wait_for_copy(state: &mut ImagePreviewState) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while state.copy_job.is_some() {
        state.poll_copy_job();
        assert!(
            Instant::now() < deadline,
            "image clipboard copy did not finish"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::GenericImageView;

    #[test]
    fn clipboard_delivery_preserves_png_mime_bytes_and_right_slot() {
        let image = RgbaImage::from_pixel(7, 5, image::Rgba([12, 34, 56, 78]));
        let bytes = encode_png(&image).unwrap();
        let mut pushed = None;
        let mut clipboard = None;
        let status = deliver_image_copy(
            &bytes,
            HandSide::Right,
            || false,
            |request| {
                pushed = Some(request.clone());
                Ok(())
            },
            |payload, mime| {
                clipboard = Some((payload.to_vec(), mime.to_owned()));
                Ok(())
            },
        )
        .unwrap();
        match pushed.unwrap() {
            IpcRequest::PushImage {
                bytes: pushed,
                mime,
                hand,
            } => {
                assert_eq!(pushed, bytes);
                assert_eq!(mime, "image/png");
                assert_eq!(hand, HandSide::Right);
                assert_eq!(
                    image::load_from_memory(&pushed).unwrap().dimensions(),
                    (7, 5)
                );
            }
            other => panic!("unexpected request: {other:?}"),
        }
        assert_eq!(clipboard, Some((bytes, "image/png".into())));
        assert!(status.contains("Right Hand (RH)"));
    }

    #[test]
    fn clipboard_delivery_falls_back_to_os_when_daemon_fails() {
        let image = RgbaImage::from_pixel(2, 3, image::Rgba([1, 2, 3, 255]));
        let bytes = encode_png(&image).unwrap();
        let mut copied = false;
        let status = deliver_image_copy(
            &bytes,
            HandSide::Left,
            || false,
            |_| Err("daemon unavailable".into()),
            |payload, mime| {
                copied = payload == bytes && mime == "image/png";
                Ok(())
            },
        )
        .unwrap();
        assert!(copied);
        assert!(status.contains("OS Clipboard"));
    }

    #[test]
    fn stale_clipboard_delivery_does_not_publish() {
        let mut pushed = false;
        let mut copied = false;
        let result = deliver_image_copy(
            b"png",
            HandSide::Left,
            || true,
            |_| {
                pushed = true;
                Ok(())
            },
            |_, _| {
                copied = true;
                Ok(())
            },
        );
        assert!(result.unwrap_err().contains("cancelled"));
        assert!(!pushed && !copied);
    }

    #[test]
    fn cancelled_queued_copy_restores_canvas_and_reports_cancellation() {
        let jobs = BackgroundJobs::new(1, 1);
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (_ticket, _rx) = jobs.submit(SAVE_TIMEOUT, move || {
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        });
        started_rx.recv().unwrap();

        let mut state = state();
        let original = state.current_rgba.clone();
        state.start_copy_to_hand_slot(HandSide::Right, &jobs);
        assert!(state.copy_job.as_ref().unwrap().cancel());
        release_tx.send(()).unwrap();
        wait_for_copy(&mut state);
        assert_eq!(state.current_rgba, original);
        assert!(state.status_toast.as_ref().unwrap().0.contains("cancelled"));
    }

    fn state() -> ImagePreviewState {
        let image = RgbaImage::from_pixel(24, 16, image::Rgba([120, 180, 240, 200]));
        ImagePreviewState::from_bytes_with_meta(
            encode_png(&image).unwrap(),
            "image/png".into(),
            None,
            "original".into(),
            &AppConfig::default(),
        )
        .unwrap()
    }

    #[test]
    fn pending_redaction_and_crop_are_burned_into_export_without_changing_canvas() {
        let mut state = state();
        state.redaction_state.boxes.push(RedactionBox {
            normalized_rect: Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(0.5, 1.0)),
        });
        state.active_mode = ImageToolMode::Crop;
        state.crop_state.normalized_rect =
            Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(0.75, 1.0));
        let original = state.current_rgba.clone();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/export");
        state.save_image_to_path(&path);
        wait_for_save(&mut state);
        let saved = image::open(path.with_extension("png")).unwrap().to_rgba8();
        assert_eq!(saved.dimensions(), (18, 16));
        assert_eq!(saved.get_pixel(0, 0), &image::Rgba([0, 0, 0, 255]));
        assert_eq!(saved.get_pixel(17, 0), original.get_pixel(17, 0));
        assert_eq!(state.current_rgba, original);
    }

    #[test]
    fn queue_rejection_restores_exact_canvas_without_copying_or_losing_edits() {
        let jobs = BackgroundJobs::new(1, 1);
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (_ticket, _rx) = jobs.submit(SAVE_TIMEOUT, move || {
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        });
        started_rx.recv().unwrap();
        let (_ticket, _rx) = jobs.submit(SAVE_TIMEOUT, || ());
        let mut state = state();
        let pointer = state.current_rgba.as_raw().as_ptr();
        let dir = tempfile::tempdir().unwrap();
        state.start_save(&dir.path().join("rejected.png"), &jobs);
        state.poll_save_job();
        assert!(state.save_job.is_none());
        assert_eq!(state.current_rgba.as_raw().as_ptr(), pointer);
        assert!(
            state
                .status_toast
                .as_ref()
                .unwrap()
                .0
                .contains("queue is full")
        );
        release_tx.send(()).unwrap();
    }

    #[test]
    fn cancelled_queued_save_restores_canvas_and_preserves_destination() {
        let jobs = BackgroundJobs::new(1, 2);
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (_ticket, _rx) = jobs.submit(SAVE_TIMEOUT, move || {
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        });
        started_rx.recv().unwrap();
        let mut state = state();
        let pointer = state.current_rgba.as_raw().as_ptr();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("existing.png");
        std::fs::write(&path, b"existing data").unwrap();
        state.start_save(&path, &jobs);
        let job = state.save_job.as_ref().unwrap();
        assert!(job.cancel());
        release_tx.send(()).unwrap();
        wait_for_save(&mut state);
        assert_eq!(state.current_rgba.as_raw().as_ptr(), pointer);
        assert_eq!(std::fs::read(path).unwrap(), b"existing data");
    }

    #[test]
    fn failed_write_restores_canvas_and_leaves_current_identity_unchanged() {
        let mut state = state();
        let original = state.current_rgba.clone();
        let dir = tempfile::tempdir().unwrap();
        let invalid_parent = dir.path().join("file");
        std::fs::write(&invalid_parent, b"not a directory").unwrap();
        state.save_image_to_path(&invalid_parent.join("export.png"));
        wait_for_save(&mut state);
        assert_eq!(state.current_rgba, original);
        assert_eq!(state.title, "original");
        assert!(state.path.is_none());
        assert!(
            state
                .status_toast
                .as_ref()
                .unwrap()
                .0
                .contains("Failed to save")
        );
    }

    #[test]
    fn stale_conversion_bytes_do_not_bypass_new_redactions() {
        let mut state = state();
        state.active_mode = ImageToolMode::Convert;
        state.convert_state.target_format = ConvertTargetFormat::Png;
        state.update_conversion();
        state.redaction_state.boxes.push(RedactionBox {
            normalized_rect: Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
        });
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("redacted.png");
        state.save_image_to_path(&path);
        wait_for_save(&mut state);
        let saved = image::open(path).unwrap().to_rgba8();
        assert!(saved.pixels().all(|p| *p == image::Rgba([0, 0, 0, 255])));
    }

    #[test]
    fn snapshot_ready_restores_editing_before_completion_and_stale_result_keeps_identity() {
        let mut state = state();
        let stamp = state.input_stamp();
        let jobs = BackgroundJobs::new(1, 1);
        let (ticket, ignored_rx) = jobs.submit(SAVE_TIMEOUT, || ());
        drop(ignored_rx);
        let (tx, rx) = mpsc::channel();
        state.save_job = Some(ImageSaveJob {
            ticket,
            phase: Arc::new(AtomicU8::new(CANCELLABLE)),
            snapshot_ready: Arc::new(AtomicBool::new(true)),
            canvas: Arc::new(Mutex::new(Some((
                std::mem::take(&mut state.current_rgba),
                Vec::new(),
            )))),
            receiver: rx,
            stamp,
            restored: false,
        });
        state.poll_save_job();
        assert!(!state.save_snapshot_pending());
        assert!(state.save_job.is_some());
        state
            .current_rgba
            .put_pixel(0, 0, image::Rgba([1, 2, 3, 255]));
        state.mark_pixels_changed();
        tx.send(Ok(Ok(SaveResult {
            path: PathBuf::from("old_snapshot.png"),
            file_size: 10,
        })))
        .unwrap();
        state.poll_save_job();
        assert!(state.path.is_none());
        assert_eq!(state.title, "original");
        assert_eq!(
            state.current_rgba.get_pixel(0, 0),
            &image::Rgba([1, 2, 3, 255])
        );
        assert!(
            state
                .status_toast
                .as_ref()
                .unwrap()
                .0
                .contains("current edits are unsaved")
        );
    }
    #[test]
    fn current_conversion_bytes_and_live_buffers_survive_save() {
        let mut state = state();
        state.active_mode = ImageToolMode::Convert;
        state.convert_state.target_format = ConvertTargetFormat::Webp;
        state.convert_state.quality = 37;
        state.update_conversion();
        let expected = state.convert_state.encoded_bytes.clone();
        let canvas_pointer = state.current_rgba.as_raw().as_ptr();
        let bytes_pointer = state.convert_state.encoded_bytes.as_ptr();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("converted.webp");
        state.save_image_to_path(&path);
        wait_for_save(&mut state);
        assert_eq!(std::fs::read(path).unwrap(), expected);
        assert_eq!(state.current_rgba.as_raw().as_ptr(), canvas_pointer);
        assert_eq!(state.convert_state.encoded_bytes.as_ptr(), bytes_pointer);
    }

    #[test]
    fn stale_jpeg_conversion_uses_selected_quality_and_current_composition() {
        let mut state = state();
        state.active_mode = ImageToolMode::Convert;
        state.convert_state.target_format = ConvertTargetFormat::Jpeg;
        state.convert_state.quality = 21;
        state.update_conversion();
        state.markup_state.items.push(MarkupShape::Rectangle {
            rect: Rect::from_min_max(Pos2::ZERO, Pos2::new(0.5, 1.0)),
            color: Color32::RED,
            width: 1.0,
            filled: true,
            rounding: 0.0,
        });
        let expected = encode_jpeg_with_quality(&state.current_display_image(), 21).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("current.jpeg");
        state.save_image_to_path(&path);
        wait_for_save(&mut state);
        assert_eq!(std::fs::read(path).unwrap(), expected);
    }

    #[cfg(unix)]
    #[test]
    fn saving_through_existing_symlink_preserves_link_and_updates_target() {
        let mut state = state();
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target.png");
        std::fs::write(&target, b"old").unwrap();
        let link = dir.path().join("link.png");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        state.save_image_to_path(&link);
        wait_for_save(&mut state);
        assert!(
            std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            image::open(target).unwrap().to_rgba8().dimensions(),
            (24, 16)
        );
    }
    #[cfg(unix)]
    #[test]
    fn saving_preserves_hardlink_identity_and_normal_create_permissions() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let mut state = state();
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target.png");
        let alias = dir.path().join("alias.png");
        std::fs::write(&target, b"old").unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o640)).unwrap();
        std::fs::hard_link(&target, &alias).unwrap();
        let inode = std::fs::metadata(&target).unwrap().ino();
        state.save_image_to_path(&target);
        wait_for_save(&mut state);
        assert_eq!(std::fs::metadata(&target).unwrap().ino(), inode);
        assert_eq!(
            std::fs::metadata(&target).unwrap().permissions().mode() & 0o777,
            0o640
        );
        assert_eq!(
            std::fs::read(&alias).unwrap(),
            std::fs::read(&target).unwrap()
        );
        let control = dir.path().join("ordinary.png");
        let new_file = dir.path().join("new.png");
        std::fs::write(&control, b"ordinary").unwrap();
        state.save_image_to_path(&new_file);
        wait_for_save(&mut state);
        assert_eq!(
            std::fs::metadata(control).unwrap().permissions().mode() & 0o777,
            std::fs::metadata(new_file).unwrap().permissions().mode() & 0o777
        );
    }
    #[test]
    fn write_boundary_cannot_cancel_a_successful_result() {
        let jobs = BackgroundJobs::new(1, 1);
        let phase = Arc::new(AtomicU8::new(CANCELLABLE));
        let worker_phase = Arc::clone(&phase);
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (ticket, receiver) = jobs.submit(Duration::MAX, move || {
            begin_write(&worker_phase, Instant::now()).unwrap();
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            Ok(SaveResult {
                path: PathBuf::from("written.png"),
                file_size: 42,
            })
        });
        let job = ImageSaveJob {
            ticket,
            phase,
            snapshot_ready: Arc::new(AtomicBool::new(true)),
            canvas: Arc::new(Mutex::new(None)),
            receiver,
            stamp: state().input_stamp(),
            restored: true,
        };
        started_rx.recv().unwrap();
        assert!(!job.cancel(), "writing must win over late cancellation");
        assert_eq!(job.phase.load(Ordering::Acquire), WRITING);
        release_tx.send(()).unwrap();
        assert!(matches!(
            job.receiver.recv().unwrap(),
            Ok(Ok(SaveResult { file_size: 42, .. }))
        ));
    }

    #[test]
    fn cancellation_and_write_boundary_have_exactly_one_winner() {
        for _ in 0..32 {
            let phase = Arc::new(AtomicU8::new(CANCELLABLE));
            let worker_phase = Arc::clone(&phase);
            let barrier = Arc::new(std::sync::Barrier::new(2));
            let worker_barrier = Arc::clone(&barrier);
            let worker = std::thread::spawn(move || {
                worker_barrier.wait();
                begin_write(&worker_phase, Instant::now()).is_ok()
            });
            barrier.wait();
            let cancelled = phase
                .compare_exchange(CANCELLABLE, CANCELLED, Ordering::AcqRel, Ordering::Acquire)
                .is_ok();
            let writing = worker.join().unwrap();
            assert_ne!(cancelled, writing);
            assert_eq!(
                phase.load(Ordering::Acquire),
                if writing { WRITING } else { CANCELLED }
            );
        }
    }

    #[test]
    fn preparation_timeout_prevents_crossing_write_boundary() {
        let phase = AtomicU8::new(CANCELLABLE);
        let expired = Instant::now() - SAVE_TIMEOUT - Duration::from_secs(1);
        assert_eq!(
            begin_write(&phase, expired).unwrap_err(),
            "Image save timed out"
        );
        assert_eq!(phase.load(Ordering::Acquire), CANCELLABLE);
    }

    #[test]
    fn snapshot_pending_preview_paints_opaque_redactions_over_texture() {
        let mut state = state();
        state.redaction_state.boxes.push(RedactionBox {
            normalized_rect: Rect::from_min_max(Pos2::new(0.6, 0.0), Pos2::new(1.0, 1.0)),
        });
        let ctx = egui::Context::default();
        state.texture = Some(ctx.load_texture(
            "pending-preview",
            egui::ColorImage::new([24, 16], Color32::WHITE),
            egui::TextureOptions::LINEAR,
        ));
        let texture_id = state.texture.as_ref().unwrap().id();
        let jobs = BackgroundJobs::new(1, 1);
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (_ticket, _rx) = jobs.submit(SAVE_TIMEOUT, move || {
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        });
        started_rx.recv().unwrap();
        let dir = tempfile::tempdir().unwrap();
        state.start_save(&dir.path().join("pending.png"), &jobs);
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(500.0, 400.0))),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    assert!(state.render_save_progress(ui, ctx, &mut false));
                });
            },
        );
        let (texture_index, image_rect) = output
            .shapes
            .iter()
            .enumerate()
            .find_map(|(i, shape)| match &shape.shape {
                egui::Shape::Rect(rect) if rect.fill_texture_id == texture_id => {
                    Some((i, rect.rect))
                }
                egui::Shape::Mesh(mesh) if mesh.texture_id == texture_id => Some((
                    i,
                    mesh.vertices
                        .iter()
                        .fold(Rect::NOTHING, |mut bounds, vertex| {
                            bounds.extend_with(vertex.pos);
                            bounds
                        }),
                )),
                _ => None,
            })
            .expect("retained preview texture was not painted");
        let expected = Rect::from_min_max(
            Pos2::new(
                image_rect.min.x + image_rect.width() * 0.6,
                image_rect.min.y,
            ),
            image_rect.max,
        );
        assert!(output.shapes.iter().skip(texture_index + 1).any(|shape| matches!(&shape.shape, egui::Shape::Rect(rect) if rect.fill == Color32::BLACK && rect.rect == expected)), "opaque redaction must cover the corresponding retained pixels");
        assert!(state.save_job.as_ref().unwrap().cancel());
        release_tx.send(()).unwrap();
        wait_for_save(&mut state);
    }
}
