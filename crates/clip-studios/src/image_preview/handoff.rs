use super::*;
use little_clip_core::staging::{ArtifactLease, ArtifactStore};
use std::sync::mpsc::Receiver;
use std::time::Duration;

pub(super) struct ImageHandoffJob {
    app: String,
    stamp: ImageInputStamp,
    source: Option<PathBuf>,
    encoded: Arc<Vec<u8>>,
    ticket: JobTicket,
    receiver: Receiver<Result<Result<HandoffFile, String>, JobError>>,
}

impl Drop for ImageHandoffJob {
    fn drop(&mut self) {
        self.ticket.cancel();
    }
}

enum HandoffFile {
    Original(PathBuf),
    Staged(ArtifactLease),
}

impl HandoffFile {
    fn path(&self) -> &std::path::Path {
        match self {
            Self::Original(path) => path,
            Self::Staged(lease) => lease.path(),
        }
    }
}

impl ImagePreviewState {
    pub fn launch_external_app(&mut self, app: &str, ctx: &egui::Context, _close: &mut bool) {
        self.start_image_handoff(app, BackgroundJobs::global());
        ctx.request_repaint();
    }

    pub(super) fn start_image_handoff(&mut self, app: &str, jobs: &BackgroundJobs) {
        self.image_handoff_job = None;
        let snapshot = self.display_image_snapshot_request();
        let stamp = snapshot.input_stamp.clone();
        let source = self.path.clone();
        let worker_source = source.clone();
        let encoded = Arc::clone(&self.raw_bytes);
        let original_encoded = Arc::clone(&encoded);
        let edited = !self.undo_stack.is_empty()
            || !Arc::ptr_eq(&self.current_rgba, &self.original_rgba)
            || !stamp.redactions.is_empty()
            || !stamp.markup.is_empty()
            || stamp.crop.is_some();
        let extension = if self.is_svg {
            "svg"
        } else if self.mime.contains("jpeg") || self.mime.contains("jpg") {
            "jpg"
        } else if self.mime.contains("webp") {
            "webp"
        } else {
            "png"
        };

        let (ticket, receiver) = jobs.submit(Duration::from_secs(60), move || {
            if !edited
                && let Some(path) = worker_source
                && little_clip_core::kernel_interface::open_content(&path, true).is_ok()
            {
                return Ok(HandoffFile::Original(path));
            }
            let store = ArtifactStore::new().map_err(|error| error.to_string())?;
            let (bytes, extension) = if edited || encoded.is_empty() {
                (
                    std::borrow::Cow::Owned(
                        snapshot.compose_png().map_err(|error| error.to_string())?,
                    ),
                    "png",
                )
            } else {
                (std::borrow::Cow::Borrowed(encoded.as_slice()), extension)
            };
            let lease = store
                .create_file("image_handoff", "image", extension)
                .map_err(|error| error.to_string())?;
            lease
                .write_atomic(&bytes)
                .map_err(|error| error.to_string())?;
            Ok(HandoffFile::Staged(lease))
        });
        self.image_handoff_job = Some(ImageHandoffJob {
            app: app.to_owned(),
            stamp,
            source,
            encoded: original_encoded,
            ticket,
            receiver,
        });
        self.show_toast("Preparing image for external app…");
    }

    pub(super) fn poll_image_handoff_with(
        &mut self,
        ctx: &egui::Context,
        launch: impl FnOnce(&str, &str, &std::path::Path) -> Result<String, String>,
    ) {
        let Some(job) = &self.image_handoff_job else {
            return;
        };
        let result = match job.receiver.try_recv() {
            Ok(result) => result
                .map_err(|error| error.to_string())
                .and_then(|result| result),
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                ctx.request_repaint_after(Duration::from_millis(40));
                return;
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                Err("Image handoff worker stopped".into())
            }
        };
        let job = self.image_handoff_job.take().unwrap();
        if self.input_stamp() != job.stamp
            || self.path != job.source
            || !Arc::ptr_eq(&self.raw_bytes, &job.encoded)
        {
            self.show_toast(
                "Image changed while preparing it. Open it again to use the latest edits.",
            );
            return;
        }
        match result.and_then(|file| {
            let message = launch(&job.app, &job.app, file.path())?;
            if let HandoffFile::Staged(lease) = file {
                lease.retain();
            }
            Ok(message)
        }) {
            Ok(message) => self.show_toast(crate::ui_ext::StatusMessage::new(
                crate::icons::Icon::ArrowRight,
                message,
            )),
            Err(error) => self.show_toast(crate::ui_ext::StatusMessage::new(
                crate::icons::Icon::Alert,
                error,
            )),
        }
    }
}
