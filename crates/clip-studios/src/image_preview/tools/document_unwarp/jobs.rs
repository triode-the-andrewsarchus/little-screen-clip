use super::*;

impl ImagePreviewState {
    pub(super) fn document_unwarp_stamp(&self) -> Stamp {
        Stamp {
            image: self.input_stamp(),
            method: self.deskew_state.method,
            illumination: self.deskew_state.method == DeskewMethod::SyntheticDoc
                && self.document_unwarp.illumination,
        }
    }

    pub(crate) fn start_document_unwarp(&mut self, jobs: &BackgroundJobs) {
        if self.document_unwarp.pending() || self.deskew_state.method == DeskewMethod::Manual {
            return;
        }
        self.cancel_document_unwarp();
        let snapshot = self.display_image_snapshot_request();
        let stamp = self.document_unwarp_stamp();
        self.document_unwarp.request = Some((snapshot, stamp));
        self.submit_document_unwarp(jobs);
    }

    fn submit_document_unwarp(&mut self, jobs: &BackgroundJobs) {
        let Some((snapshot, stamp)) = self.document_unwarp.request.clone() else {
            return;
        };
        let control = Arc::new(Mutex::new(None));
        self.document_unwarp.control = control.clone();
        self.document_unwarp.retry_at = None;
        self.document_unwarp.task = Some(Task::start_unbounded(jobs, move |cancelled| {
            let start = Instant::now();
            if cancelled.load(std::sync::atomic::Ordering::Acquire) {
                return Err("Document unwarp cancelled".into());
            }
            let original = Arc::new(snapshot.compose());
            let result = inference::run(
                &original,
                stamp.method,
                stamp.illumination,
                cancelled,
                &control,
            )?;
            Ok(Preview {
                original,
                result: Arc::new(result),
                stamp,
                duration: start.elapsed(),
            })
        }));
    }

    pub(crate) fn poll_document_unwarp(&mut self, ctx: &egui::Context, jobs: &BackgroundJobs) {
        let current = self.document_unwarp_stamp();
        let stale_request = self
            .document_unwarp
            .request
            .as_ref()
            .is_some_and(|(_, stamp)| stamp != &current);
        let stale_preview = self
            .document_unwarp
            .preview
            .as_ref()
            .is_some_and(|preview| preview.stamp != current);
        if stale_request || stale_preview || self.active_mode != ImageToolMode::Deskew {
            self.cancel_document_unwarp();
            return;
        }
        if let Some(at) = self.document_unwarp.retry_at {
            if Instant::now() >= at {
                self.submit_document_unwarp(jobs);
            } else {
                ctx.request_repaint_after(at.saturating_duration_since(Instant::now()));
            }
        }
        let result = self.document_unwarp.task.as_ref().and_then(Task::poll);
        if let Some(result) = result {
            self.document_unwarp.task = None;
            self.accept_document_unwarp(result, current);
        }
        if self.document_unwarp.pending() {
            ctx.request_repaint_after(std::time::Duration::from_millis(30));
        }
    }

    fn accept_document_unwarp(
        &mut self,
        result: Result<Result<Preview, String>, JobError>,
        current: Stamp,
    ) {
        match result {
            Ok(Ok(preview)) if preview.stamp == current => {
                self.document_unwarp.preview = Some(preview);
                self.document_unwarp.request = None;
                self.document_unwarp.texture = None;
            }
            Err(JobError::QueueFull) => {
                self.document_unwarp.retry_at =
                    Some(Instant::now() + std::time::Duration::from_millis(100));
            }
            Ok(Err(error)) => {
                self.document_unwarp.error = Some(error);
                self.document_unwarp.request = None;
            }
            Err(error) => {
                self.document_unwarp.error =
                    Some(format!("Document unwarp worker failed: {error}"));
                self.document_unwarp.request = None;
            }
            _ => {
                self.document_unwarp.request = None;
            }
        }
    }

    pub(crate) fn apply_document_unwarp(&mut self) {
        if self
            .document_unwarp
            .preview
            .as_ref()
            .is_none_or(|preview| preview.stamp != self.document_unwarp_stamp())
        {
            return;
        }
        let preview = self.document_unwarp.preview.take().unwrap();
        self.undo_stack.push(preview.original);
        self.redo_stack.clear();
        self.current_rgba = preview.result;
        self.redaction_state.boxes.clear();
        self.redaction_state.selected_idx = None;
        self.redaction_state.active_drag = None;
        self.redaction_state.inpaint_mask = None;
        self.redaction_state.last_brush_pos = None;
        self.markup_state.items.clear();
        self.markup_state.selected_idx = None;
        self.markup_state.editing_text_idx = None;
        self.markup_state.active_drag = None;
        self.mark_pixels_changed();
        self.dirty_texture = true;
        self.resize_state.sync_from_image(&self.current_rgba);
        self.palette_state.is_extracted = false;
        self.deskew_state.reset_quad();
        self.show_toast(format!(
            "Applied {} document unwarp",
            preview.stamp.method.label()
        ));
    }

    pub(crate) fn cancel_document_unwarp(&mut self) {
        self.document_unwarp.cancel();
        self.document_unwarp.clear_preview();
        self.document_unwarp.error = None;
    }
}
