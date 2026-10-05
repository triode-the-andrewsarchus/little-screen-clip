use super::*;

impl PdfEditorState {
    /// Place generated documents beside the source.  Edits intentionally never
    /// overwrite the source: complex PDFs are too easy to damage during rewrite.
    pub(super) fn sibling_output_path(&self, operation: &str) -> PathBuf {
        let stem = self
            .path
            .file_stem()
            .and_then(|name| name.to_str())
            .unwrap_or("document");
        self.path
            .with_file_name(format!("{stem}.{operation}.edited.pdf"))
    }

    pub fn delete_current_page(&mut self, now: f64) {
        if self.info_job.is_some() {
            return;
        }
        let Some(cur) = self.selected_page else {
            return;
        };
        if self.page_count <= 1 {
            self.status_toast
                .set("Cannot delete only page from PDF", now);
            return;
        }

        self.start_edit("delete", cur, 0, now);
    }

    pub fn rotate_current_page(&mut self, degrees: u32, now: f64) {
        if self.info_job.is_some() {
            return;
        }
        let Some(cur) = self.selected_page else {
            return;
        };
        self.start_edit("rotate", cur, degrees, now);
    }

    pub(super) fn start_edit(
        &mut self,
        operation: &'static str,
        page_num: usize,
        degrees: u32,
        now: f64,
    ) {
        self.start_edit_on(
            BackgroundJobs::global(),
            operation,
            page_num,
            now,
            move |input, staged_output| match operation {
                "delete" => delete_pdf_pages(input, &[page_num], staged_output),
                "rotate" => rotate_pdf_page(input, page_num, degrees, staged_output),
                _ => Err("Unknown PDF edit operation".to_string()),
            },
        );
    }

    pub(super) fn start_edit_on<F>(
        &mut self,
        jobs: &BackgroundJobs,
        operation: &'static str,
        page_num: usize,
        now: f64,
        perform_edit: F,
    ) where
        F: FnOnce(&Path, &Path) -> Result<(), String> + Send + 'static,
    {
        if self.edit_in_progress {
            self.status_toast
                .set("A PDF edit is already in progress", now);
            return;
        }
        let input = self.path.clone();
        let output = self.sibling_output_path(operation);
        let output_dir = output
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let staging_dir = match tempfile::tempdir_in(output_dir) {
            Ok(dir) => dir,
            Err(error) => {
                self.status_toast
                    .set(format!("Could not stage PDF edit: {error}"), now);
                return;
            }
        };
        let expected_page_count = self
            .page_count
            .saturating_sub(usize::from(operation == "delete"));
        let protect_outputs = self.protect_outputs;
        let (ticket, rx) = jobs.submit(Duration::MAX, move || {
            let staged_output = staging_dir.path().join(STAGED_PDF_FILENAME);
            let source = if protect_outputs {
                crate::file_source::FileSource::capture(&input).map(Some)
            } else {
                Ok(None)
            };
            let result = source
                .and_then(|source| {
                    perform_edit(&input, &staged_output)?;
                    if let Some(source) = source {
                        source.check()?;
                    }
                    Ok(())
                })
                .and_then(|()| {
                    if !staged_output.is_file() {
                        return Err("PDF operation produced no output file".to_string());
                    }
                    let info = get_pdf_info(&staged_output)?;
                    if info.page_count != expected_page_count {
                        return Err(format!(
                            "Edited PDF has {} pages; expected {}",
                            info.page_count, expected_page_count
                        ));
                    }
                    render_pdf_page_preview(&staged_output, 1, 64).map(|_| ())
                });
            PdfEditMsg {
                operation,
                page_num,
                output,
                staging_dir,
                result,
            }
        });
        self.edit_in_progress = true;
        self.edit_ticket = Some(ticket);
        self.edit_rx = Some(rx);
        self.status_toast.set(
            format!(
                "{} page {}…",
                if operation == "delete" {
                    "Deleting"
                } else {
                    "Rotating"
                },
                page_num
            ),
            now,
        );
    }

    pub(super) fn cancel_edit_worker(&mut self) {
        if let Some(ticket) = self.edit_ticket.take() {
            ticket.cancel();
        }
        self.edit_rx = None;
        self.edit_in_progress = false;
    }

    pub(super) fn poll_edit_worker(&mut self, ctx: &egui::Context, now: f64) {
        let message = match self.edit_rx.as_ref().map(|rx| rx.try_recv()) {
            Some(Ok(Ok(message))) => Some(message),
            Some(Ok(Err(error))) => {
                self.edit_rx = None;
                self.edit_ticket = None;
                self.edit_in_progress = false;
                self.status_toast
                    .set(format!("PDF edit unavailable: {error}"), now);
                return;
            }
            Some(Err(TryRecvError::Disconnected)) => {
                self.edit_rx = None;
                self.edit_ticket = None;
                self.edit_in_progress = false;
                self.status_toast.set("PDF edit worker disconnected", now);
                return;
            }
            _ => None,
        };
        if let Some(message) = message {
            self.edit_rx = None;
            self.edit_ticket = None;
            self.edit_in_progress = false;
            let result = message.result.and_then(|()| {
                self.publish_output(
                    &message.staging_dir.path().join(STAGED_PDF_FILENAME),
                    &message.output,
                )
            });
            match result {
                Ok(()) => {
                    self.reset_navigation();
                    self.document_tools.invalidate_layout();
                    self.path = message.output;
                    self.title = self
                        .path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("edited.pdf")
                        .to_string();
                    self.reload_pages(ctx);
                    self.dirty = true;
                    self.status_toast.set(
                        format!(
                            "{} page {}",
                            if message.operation == "delete" {
                                "Deleted"
                            } else {
                                "Rotated"
                            },
                            message.page_num
                        ),
                        now,
                    );
                }
                Err(error) => self.status_toast.set(
                    format!("Failed to {} page: {}", message.operation, error),
                    now,
                ),
            }
        } else if self.edit_in_progress {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
    }
}
