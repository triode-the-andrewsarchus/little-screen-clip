use super::*;

impl WorkbenchApp {
    pub(crate) fn start_text_file_load(&mut self, path: PathBuf) {
        self.prepare_preview_load();
        self.text_load_job = Some(TextPreviewLoadJob::from_file(path, self.config.clone()));
        self.clear_status_message();
    }

    pub(crate) fn start_text_preview_load(&mut self, content: String, title: Option<String>) {
        self.prepare_preview_load();
        self.text_load_job = Some(TextPreviewLoadJob::from_text(
            content,
            title,
            self.config.clone(),
        ));
        self.clear_status_message();
    }

    pub(crate) fn start_shared_text_preview_load(&mut self, content: Arc<String>, title: String) {
        self.prepare_preview_load();
        self.text_load_job = Some(TextPreviewLoadJob::from_shared_text(
            content,
            title,
            self.config.clone(),
        ));
        self.clear_status_message();
    }

    pub(crate) fn prepare_preview_load(&mut self) {
        if matches!(self.viewer, Some(ViewerSession::ImageLoading(_))) {
            self.replace_viewer(None);
        } else {
            self.cancel_preview_loads();
        }
    }

    pub(crate) fn cancel_preview_loads(&mut self) {
        self.image_load_job = None;
        self.text_load_job = None;
        if let Some((ticket, _, _)) = self.spreadsheet_load_job.take() {
            ticket.cancel();
        }
        if let Some((ticket, _)) = self.midi_load_job.take() {
            ticket.cancel();
        }
        if let Some((ticket, _)) = self.office_load_job.take() {
            ticket.cancel();
        }
    }

    pub(crate) fn poll_text_load(&mut self, ctx: &egui::Context) {
        let Some(job) = &self.text_load_job else {
            return;
        };
        let result = match job.receiver.try_recv() {
            Ok(result) => result,
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                ctx.request_repaint_after(Duration::from_millis(40));
                return;
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => Err(JobError::Cancelled),
        };
        self.text_load_job = None;
        match result {
            Ok(Ok(state)) => {
                self.replace_viewer(Some(state.into_viewer()));
                self.clear_status_message();
            }
            Ok(Err(error)) => self.set_status_message(format!("Text preview failed: {error}")),
            Err(error) => self.set_status_message(format!("Text preview failed: {error}")),
        }
        if self.viewer.is_none() && !self.startup_file_loads.is_empty() {
            self.advance_startup();
        }
        ctx.request_repaint();
    }

    pub(crate) fn start_spreadsheet_load(&mut self, source: SpreadsheetLoadSource) {
        self.prepare_preview_load();
        let fallback = match &source {
            SpreadsheetLoadSource::Text { content, title } => {
                Some((Arc::clone(content), title.clone()))
            }
            SpreadsheetLoadSource::File(_) => None,
        };
        let job = BackgroundJobs::global().submit(Duration::from_secs(30), move || match source {
            SpreadsheetLoadSource::File(path) => {
                load_spreadsheet(&path).map(|data| (data, Some(path)))
            }
            SpreadsheetLoadSource::Text { content, .. } => detect_csv_text(&content)
                .map(|data| (data, None))
                .ok_or_else(|| "Clipboard text is not a valid table".to_string()),
        });
        self.spreadsheet_load_job = Some((job.0, job.1, fallback));
        self.status_msg = None;
    }

    pub(crate) fn poll_spreadsheet_load(&mut self, ctx: &egui::Context) {
        let Some((_, receiver, _)) = &self.spreadsheet_load_job else {
            return;
        };
        let result = match receiver.try_recv() {
            Ok(result) => result,
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                ctx.request_repaint_after(Duration::from_millis(80));
                return;
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => Err(JobError::Cancelled),
        };
        let (_, _, fallback) = self.spreadsheet_load_job.take().unwrap();
        match result {
            Ok(Ok((data, path))) => {
                self.replace_viewer(Some(ViewerSession::Spreadsheet(
                    SpreadsheetViewerState::new(data, path),
                )));
                ctx.request_repaint();
            }
            failure => {
                if let Some((content, title)) = fallback {
                    self.start_shared_text_preview_load(content, title);
                } else {
                    self.status_msg = Some(match failure {
                        Ok(Err(error)) => format!("Spreadsheet: {error}"),
                        Err(error) => format!("Spreadsheet load failed: {error}"),
                        Ok(Ok(_)) => unreachable!(),
                    });
                    if self.viewer.is_none() && !self.startup_file_loads.is_empty() {
                        self.advance_startup();
                    }
                    ctx.request_repaint();
                }
            }
        }
    }

    pub(crate) fn start_midi_load(&mut self, path: PathBuf) {
        self.prepare_preview_load();
        let (ticket, receiver) = BackgroundJobs::global()
            .submit(Duration::from_secs(30), move || {
                MidiData::parse_file(&path).map(|data| (data, path))
            });
        self.midi_load_job = Some((ticket, receiver));
        self.status_msg = None;
    }

    pub(crate) fn poll_midi_load(&mut self, ctx: &egui::Context) {
        let Some((_, receiver)) = &self.midi_load_job else {
            return;
        };
        let result = match receiver.try_recv() {
            Ok(result) => result,
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                ctx.request_repaint_after(Duration::from_millis(80));
                return;
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => Err(JobError::Cancelled),
        };
        self.midi_load_job = None;
        match result {
            Ok(Ok((data, path))) => {
                self.replace_viewer(Some(ViewerSession::Midi(
                    MidiViewerState::from_parsed_file(path, data),
                )));
                ctx.request_repaint();
            }
            Ok(Err(error)) => self.status_msg = Some(format!("MIDI load failed: {error}")),
            Err(error) => self.status_msg = Some(format!("MIDI load failed: {error}")),
        }
    }

    pub(crate) fn open_spreadsheet_item(&mut self, slot: SlotId) {
        let source = self.state.get(slot).and_then(|item| {
            if let Some(path) = bounded_item_file_path(item).filter(|p| is_spreadsheet_file(p)) {
                Some(SpreadsheetLoadSource::File(path))
            } else {
                match &item.payload {
                    ClipPayload::Text(text) | ClipPayload::Markdown(text) => {
                        Some(SpreadsheetLoadSource::Text {
                            content: Arc::new(text.clone()),
                            title: item.title.clone(),
                        })
                    }
                    _ => None,
                }
            }
        });
        if let Some(source) = source {
            self.start_spreadsheet_load(source);
        }
    }

    pub(crate) fn start_office_document_load(&mut self, path: PathBuf) {
        self.prepare_preview_load();
        let (ticket, receiver) =
            BackgroundJobs::global().submit(Duration::from_secs(60), move || {
                let title = path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| "document".to_string());
                match little_clip_core::document::prepare_office_preview(&path)? {
                    little_clip_core::document::PreparedOfficePreview::Pdf {
                        pdf_path,
                        workspace,
                    } => Ok(OfficeLoadResult::Pdf {
                        pdf_path,
                        workspace,
                        title,
                    }),
                    little_clip_core::document::PreparedOfficePreview::Markdown(content) => {
                        Ok(OfficeLoadResult::Markdown {
                            content,
                            title,
                            path,
                        })
                    }
                }
            });
        self.office_load_job = Some((ticket, receiver));
        self.clear_status_message();
    }

    pub(crate) fn poll_office_document_load(&mut self, ctx: &egui::Context) {
        let Some((_, receiver)) = &self.office_load_job else {
            return;
        };
        let result = match receiver.try_recv() {
            Ok(result) => result,
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                ctx.request_repaint_after(Duration::from_millis(80));
                return;
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => Err(JobError::Cancelled),
        };
        self.office_load_job = None;
        match result {
            Ok(Ok(OfficeLoadResult::Pdf {
                pdf_path,
                workspace,
                title,
            })) => {
                let state = PdfEditorState::new(pdf_path)
                    .with_title(title)
                    .with_office_workspace(workspace)
                    .with_protected_outputs();
                self.replace_viewer(Some(ViewerSession::Pdf(state)));
                ctx.request_repaint();
            }
            Ok(Ok(OfficeLoadResult::Markdown {
                content,
                title,
                path,
            })) => {
                let mut state = TextPreviewState::from_text(content, Some(title), &self.config);
                state.path = Some(path);
                state.view_mode = crate::text_preview::TextViewMode::RenderedMarkdown;
                self.replace_viewer(Some(ViewerSession::Text(state)));
                ctx.request_repaint();
            }
            Ok(Err(err)) => {
                self.status_msg = Some(format!("Document preview: {err}"));
            }
            Err(err) => {
                self.status_msg = Some(format!("Document preview failed: {err}"));
            }
        }
    }
}
