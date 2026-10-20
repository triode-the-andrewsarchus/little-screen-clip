use super::*;

impl WorkbenchApp {
    pub(crate) fn retain_current_image_session(&mut self) -> bool {
        if let Some(ViewerSession::Image(state)) = self.viewer.as_mut() {
            if !state.poll_session_snapshots() {
                return false;
            }
        }
        if let Some(ViewerSession::Image(mut state)) = self.viewer.take() {
            let key = self.active_image_key.take();
            let edited = !state.undo_stack.is_empty()
                || !state.redo_stack.is_empty()
                || state.current_rgba.as_ref() != state.original_rgba.as_ref();
            state.suspend_pending_work();
            if let Some(key) = key.filter(|_| edited) {
                self.retained_image_session = Some((key, state));
            }
        }
        true
    }

    pub(crate) fn start_image_item_load(&mut self, item: &ClipItem) {
        let key = ImageSessionKey::ClipboardItem(item.id);
        if self.restore_image_session(&key) {
            return;
        }
        let source = ImageLoadSource::from_item(item);
        match source {
            Ok(source) => self.start_image_load_with_key(source, Some(key)),
            Err(error) => {
                self.image_load_job = None;
                self.text_load_job = None;
                self.active_image_key = None;
                self.set_status_message(format!("Image preview failed: {error}"));
            }
        }
    }

    pub(crate) fn start_image_load(&mut self, source: ImageLoadSource) {
        let key = source.session_key();
        if key
            .as_ref()
            .is_some_and(|key| self.restore_image_session(key))
        {
            return;
        }
        self.start_image_load_with_key(source, key);
    }

    pub(crate) fn start_image_load_with_key(
        &mut self,
        source: ImageLoadSource,
        key: Option<ImageSessionKey>,
    ) {
        self.retained_image_session = None;
        self.image_load_job = None;
        self.text_load_job = None;
        if matches!(self.viewer, Some(ViewerSession::ImageLoading(_))) {
            self.replace_viewer(None);
        }
        if let Some((ticket, _, _)) = self.spreadsheet_load_job.take() {
            ticket.cancel();
        }
        if let Some((ticket, _)) = self.midi_load_job.take() {
            ticket.cancel();
        }
        if let Some((ticket, _)) = self.office_load_job.take() {
            ticket.cancel();
        }
        self.image_load_job = Some(ImageLoadJob::new_with_key(source, key));
        self.clear_status_message();
    }

    pub(crate) fn restore_image_session(&mut self, key: &ImageSessionKey) -> bool {
        if self.active_image_key.as_ref() == Some(key)
            && matches!(self.viewer, Some(ViewerSession::Image(_)))
        {
            return true;
        }
        let Some((cached_key, state)) = self.retained_image_session.take() else {
            return false;
        };
        if cached_key == *key {
            self.image_load_job = None;
            self.text_load_job = None;
            self.viewer = Some(ViewerSession::Image(state));
            self.active_image_key = Some(key.clone());
            self.clear_status_message();
            true
        } else {
            drop(state);
            false
        }
    }

    pub(crate) fn poll_image_load(&mut self, ctx: &egui::Context) {
        // A startup image occupies its FIFO position; jobs behind the active
        // viewer remain queued and cannot publish into the foreground.
        let startup = self.image_load_job.is_none();
        let job = if let Some(job) = &self.image_load_job {
            job
        } else if let Some(ViewerSession::ImageLoading(job)) = &self.viewer {
            job
        } else {
            return;
        };
        let key = job.key.clone();
        let result = match job.receiver.try_recv() {
            Ok(result) => result,
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                ctx.request_repaint_after(Duration::from_millis(40));
                return;
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => Err(JobError::Cancelled),
        };
        self.image_load_job = None;
        match result {
            Ok(Ok(loaded)) => {
                self.viewer = None;
                self.active_image_key = None;
                self.retained_image_session = None;
                self.viewer = Some(ViewerSession::Image(ImagePreviewState::from_decoded(
                    loaded.data,
                    loaded.path,
                    loaded.title,
                    &self.config,
                )));
                self.active_image_key = key;
                self.status_msg = None;
            }
            failure => {
                self.status_msg = Some(match failure {
                    Ok(Err(error)) => format!("Image preview failed: {error}"),
                    Err(error) => format!("Image preview failed: {error}"),
                    Ok(Ok(_)) => unreachable!(),
                });
                if startup {
                    self.advance_startup();
                }
            }
        }
        ctx.request_repaint();
    }
}
