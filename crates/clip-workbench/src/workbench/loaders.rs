use super::*;

impl WorkbenchApp {
    pub(crate) fn start_text_file_load(&mut self, path: PathBuf) {
        self.prepare_text_load();
        self.text_load_job = Some(TextPreviewLoadJob::from_file(path, self.config.clone()));
        self.clear_status_message();
    }

    pub(crate) fn start_text_preview_load(&mut self, content: String, title: Option<String>) {
        self.prepare_text_load();
        self.text_load_job = Some(TextPreviewLoadJob::from_text(
            content,
            title,
            self.config.clone(),
        ));
        self.clear_status_message();
    }

    pub(crate) fn start_shared_text_preview_load(&mut self, content: Arc<String>, title: String) {
        self.prepare_text_load();
        self.text_load_job = Some(TextPreviewLoadJob::from_shared_text(
            content,
            title,
            self.config.clone(),
        ));
        self.clear_status_message();
    }

    pub(crate) fn prepare_text_load(&mut self) {
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
                self.replace_viewer(Some(ViewerSession::Text(state)));
                self.clear_status_message();
            }
            Ok(Err(error)) => self.set_status_message(format!("Text preview failed: {error}")),
            Err(error) => self.set_status_message(format!("Text preview failed: {error}")),
        }
        ctx.request_repaint();
    }

    pub(crate) fn start_spreadsheet_load(&mut self, source: SpreadsheetLoadSource) {
        self.image_load_job = None;
        self.text_load_job = None;
        if let Some((ticket, _)) = self.midi_load_job.take() {
            ticket.cancel();
        }
        if let Some((ticket, _, _)) = self.spreadsheet_load_job.take() {
            ticket.cancel();
        }
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
                }
            }
        }
    }

    pub(crate) fn start_midi_load(&mut self, path: PathBuf) {
        self.image_load_job = None;
        self.text_load_job = None;
        if let Some((ticket, _, _)) = self.spreadsheet_load_job.take() {
            ticket.cancel();
        }
        if let Some((ticket, _)) = self.midi_load_job.take() {
            ticket.cancel();
        }
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
        self.prepare_text_load();
        if let Some((ticket, _)) = self.office_load_job.take() {
            ticket.cancel();
        }
        let (ticket, receiver) =
            BackgroundJobs::global().submit(Duration::from_secs(60), move || {
                let store =
                    little_clip_core::staging::ArtifactStore::new().map_err(|e| e.to_string())?;
                let workspace = store
                    .create_workspace("office_preview", "pdf_render")
                    .map_err(|e| e.to_string())?;
                let stem = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("document");
                let pdf_path = workspace.path().join(format!("{stem}.pdf"));
                let title = path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| "document".to_string());
                if little_clip_core::document::render_to_pdf(&path, &pdf_path).is_ok()
                    && std::fs::metadata(&pdf_path)
                        .map(|m| m.len() > 0)
                        .unwrap_or(false)
                {
                    let workspace_dir = workspace.retain();
                    Ok(OfficeLoadResult::Pdf {
                        pdf_path,
                        workspace_dir,
                        title,
                    })
                } else {
                    let md = little_clip_core::document::render_to_markdown(&path)?;
                    Ok(OfficeLoadResult::Markdown {
                        content: md,
                        title,
                        path,
                    })
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
                workspace_dir,
                title,
            })) => {
                let state = PdfEditorState::new(pdf_path)
                    .with_title(title)
                    .with_cleanup_dir(workspace_dir)
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
