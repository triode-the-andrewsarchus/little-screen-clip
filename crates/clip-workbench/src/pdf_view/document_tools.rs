use super::*;
use crate::image_preview::{ImageToolProcessControl, ocr_command, run_image_tool_status_command};
use little_clip_core::pdf::PdfPageLayout;
use std::process::Stdio;

struct ToolJob<T> {
    ticket: JobTicket,
    receiver: Receiver<Result<Result<T, String>, JobError>>,
    control: Arc<ImageToolProcessControl>,
}

impl<T: Send + 'static> ToolJob<T> {
    fn start(
        run: impl FnOnce(Arc<ImageToolProcessControl>) -> Result<T, String> + Send + 'static,
    ) -> Self {
        let control = Arc::new(ImageToolProcessControl::new());
        let worker_control = control.clone();
        let (ticket, receiver) =
            BackgroundJobs::global().submit(Duration::MAX, move || run(worker_control));
        Self {
            ticket,
            receiver,
            control,
        }
    }

    fn poll(&self, ctx: &egui::Context) -> Option<Result<T, String>> {
        match self.receiver.try_recv() {
            Ok(result) => Some(
                result
                    .map_err(|error| error.to_string())
                    .and_then(|result| result),
            ),
            Err(TryRecvError::Disconnected) => Some(Err("PDF worker stopped".into())),
            Err(TryRecvError::Empty) => {
                ctx.request_repaint_after(Duration::from_millis(40));
                None
            }
        }
    }
}

impl<T> Drop for ToolJob<T> {
    fn drop(&mut self) {
        self.control.cancel();
        self.ticket.cancel();
    }
}

#[derive(Default)]
pub(super) struct PdfDocumentTools {
    pub layout: Option<Arc<PdfPageLayout>>,
    pub layout_error: Option<String>,
    page: Option<usize>,
    layout_job: Option<ToolJob<PdfPageLayout>>,
    ocr_job: Option<ToolJob<super::output::PdfOutput>>,
    copy_job: Option<ToolJob<()>>,
    reuse_job: Option<ToolJob<Option<PathBuf>>>,
    pub ocr_language: String,
    pub ocr_status: String,
    pub ocr_complete: bool,
    pub interaction: super::interaction::PdfInteraction,
}

/// Reuse image tools' process group cancellation and the existing Python environment.
fn run_pdf_tool(
    operation: &str,
    source: &Path,
    output: &Path,
    arguments: &[String],
    extra_input: Option<&Path>,
    control: &ImageToolProcessControl,
) -> Result<(), String> {
    use std::io::Read;
    if !source.is_file() {
        return Err("PDF source is unavailable".into());
    }
    let (python, script) = ocr_command();
    let script = Path::new(&script).with_file_name("pdf_document.py");
    let log = tempfile::NamedTempFile::new().map_err(|error| error.to_string())?;
    let mut request = little_clip_core::process::CommandRequest::new(python)
        .arg(script)
        .arg(operation)
        .arg_content_file(source)
        .map_err(|error| error.to_string())?
        .arg_content_output(output)
        .map_err(|error| error.to_string())?;
    if let Some(input) = extra_input {
        request = request
            .arg("--candidate")
            .arg_content_file(input)
            .map_err(|error| error.to_string())?;
    }
    let mut command = request.args(arguments).build_command();
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(
            log.reopen().map_err(|error| error.to_string())?,
        ));
    let status = run_image_tool_status_command(command, control)?;
    if !status.success() {
        let mut diagnostic = String::new();
        log.reopen()
            .map_err(|error| error.to_string())?
            .take(16 * 1024)
            .read_to_string(&mut diagnostic)
            .map_err(|error| error.to_string())?;
        return Err(format!("PDF {operation} failed: {diagnostic}"));
    }
    if control.cancel.load(Ordering::SeqCst) {
        return Err("PDF operation canceled".into());
    }
    Ok(())
}

impl PdfDocumentTools {
    pub fn layout_for(&self, page: usize) -> Option<Arc<PdfPageLayout>> {
        (self.page == Some(page))
            .then(|| self.layout.clone())
            .flatten()
    }

    pub fn ocr_busy(&self) -> bool {
        self.ocr_job.is_some()
    }

    pub fn cancel_ocr(&mut self) {
        self.ocr_job = None;
        self.ocr_status = "OCR canceled; source PDF unchanged".into();
    }

    pub fn invalidate_layout(&mut self) {
        self.layout_job = None;
        self.layout = None;
        self.page = None;
        self.layout_error = None;
        self.interaction = Default::default();
    }

    fn service_layout(
        &mut self,
        path: &Path,
        page: Option<usize>,
        ctx: &egui::Context,
        lease: Option<Arc<tempfile::TempDir>>,
        check: crate::file_source::ReadCheck,
    ) {
        if self.page != page {
            self.invalidate_layout();
            self.page = page;
            if let Some(page) = page {
                let source = path.to_owned();
                self.layout_job = Some(ToolJob::start(move |control| {
                    let _lease = lease;
                    check.read(&control.cancel, || {
                        let directory = tempfile::tempdir().map_err(|error| error.to_string())?;
                        let output = directory.path().join("layout.json");
                        run_pdf_tool(
                            "layout",
                            &source,
                            &output,
                            &["--page".into(), page.to_string()],
                            None,
                            &control,
                        )?;
                        let file =
                            std::fs::File::open(output).map_err(|error| error.to_string())?;
                        serde_json::from_reader(file).map_err(|error| error.to_string())
                    })
                }));
            }
        }
        if let Some(result) = self.layout_job.as_ref().and_then(|job| job.poll(ctx)) {
            self.layout_job = None;
            match result {
                Ok(layout) => {
                    self.ocr_complete |= layout.ocr_complete;
                    self.layout = Some(Arc::new(layout));
                }
                Err(error) => self.layout_error = Some(error),
            }
        }
    }

    fn start_ocr(
        &mut self,
        source: PathBuf,
        output: PathBuf,
        count: usize,
        lease: Option<Arc<tempfile::TempDir>>,
        check: crate::file_source::ReadCheck,
    ) {
        if self.ocr_busy() || self.ocr_complete {
            return;
        }
        let language = if self.ocr_language.trim().is_empty() {
            "eng".to_owned()
        } else {
            self.ocr_language.trim().to_owned()
        };
        self.ocr_status = format!("Adding searchable text to {count} pages…");
        self.ocr_job = Some(ToolJob::start(move |control| {
            let _lease = lease;
            super::output::PdfOutput::generate(&source, &output, count, true, |source, staged| {
                check.read(&control.cancel, || {
                    run_pdf_tool(
                        "ocr",
                        source,
                        staged,
                        &["--language".into(), language],
                        None,
                        &control,
                    )
                })
            })
        }));
    }

    fn copy_image(
        &mut self,
        source: PathBuf,
        page: usize,
        image: usize,
        lease: Option<Arc<tempfile::TempDir>>,
        check: crate::file_source::ReadCheck,
    ) {
        if self.copy_job.is_some() {
            return;
        }
        self.copy_job = Some(ToolJob::start(move |control| {
            let _lease = lease;
            let bytes = check.read(&control.cancel, || {
                let directory = tempfile::tempdir().map_err(|error| error.to_string())?;
                let output = directory.path().join("image.png");
                run_pdf_tool(
                    "image",
                    &source,
                    &output,
                    &[
                        "--page".into(),
                        page.to_string(),
                        "--image".into(),
                        image.to_string(),
                    ],
                    None,
                    &control,
                )?;
                std::fs::read(output).map_err(|error| error.to_string())
            })?;
            little_clip_core::copy_bytes_to_system_clipboard(&bytes, "image/png")
                .map_err(|error| error.to_string())
        }));
    }
}

impl PdfEditorState {
    pub(super) fn start_existing_ocr_check(&mut self) -> bool {
        let candidate = self.sibling_output_path("ocr");
        if !candidate.is_file() {
            return false;
        }
        let source = self.path.clone();
        let lease = self.source_directory.clone();
        self.document_tools.reuse_job = Some(ToolJob::start(move |control| {
            let _lease = lease;
            let source_revision = crate::file_source::FileSource::capture(&source)?;
            let candidate_revision = crate::file_source::FileSource::capture(&candidate)?;
            let directory = tempfile::tempdir().map_err(|error| error.to_string())?;
            let output = directory.path().join("matches.json");
            run_pdf_tool("compare", &source, &output, &[], Some(&candidate), &control)?;
            let matches: bool = serde_json::from_reader(
                std::fs::File::open(output).map_err(|error| error.to_string())?,
            )
            .map_err(|error| error.to_string())?;
            source_revision.check()?;
            candidate_revision.check()?;
            Ok(matches.then_some(candidate))
        }));
        true
    }

    pub(super) fn poll_existing_ocr(&mut self, ctx: &egui::Context, now: f64) {
        let Some(result) = self
            .document_tools
            .reuse_job
            .as_ref()
            .and_then(|job| job.poll(ctx))
        else {
            return;
        };
        self.document_tools.reuse_job = None;
        if let Ok(Some(path)) = result {
            self.document_tools.ocr_complete = true;
            self.path = path;
            self.title = self
                .path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            self.document_tools.ocr_status =
                format!("Using existing searchable PDF: {}", self.title);
            self.status_toast
                .set(self.document_tools.ocr_status.as_str(), now);
        }
        self.start_info_worker();
    }

    pub(super) fn poll_document_tools(&mut self, ctx: &egui::Context, now: f64) {
        let page = self.selected_page.filter(|_| self.info_job.is_none());
        self.document_tools.service_layout(
            &self.path,
            page,
            ctx,
            self.source_directory.clone(),
            self.read_check.clone(),
        );
        if let Some(result) = self
            .document_tools
            .ocr_job
            .as_ref()
            .and_then(|job| job.poll(ctx))
        {
            self.document_tools.ocr_job = None;
            match result.and_then(|output| output.publish(self.protect_outputs)) {
                Ok(path) => {
                    self.read_check = Default::default();
                    self.document_tools.ocr_complete = true;
                    self.path = path;
                    self.title = self
                        .path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned();
                    self.reset_navigation();
                    self.document_tools.invalidate_layout();
                    self.dirty = true;
                    self.reload_pages(ctx);
                    self.document_tools.ocr_status =
                        format!("Searchable PDF saved: {}", self.title);
                    self.status_toast
                        .set(self.document_tools.ocr_status.as_str(), now);
                }
                Err(error) => self.document_tools.ocr_status = error,
            }
        }
        if let Some(result) = self
            .document_tools
            .copy_job
            .as_ref()
            .and_then(|job| job.poll(ctx))
        {
            self.document_tools.copy_job = None;
            self.status_toast.set(
                match result {
                    Ok(()) => "Image copied".to_owned(),
                    Err(error) => format!("Image copy failed: {error}"),
                },
                now,
            );
        }
    }

    pub(super) fn start_pdf_ocr(&mut self) {
        if !self.edit_in_progress
            && self.info_job.is_none()
            && self.page_count > 0
            && self.document_tools.layout.is_some()
        {
            self.document_tools.start_ocr(
                self.path.clone(),
                self.sibling_output_path("ocr"),
                self.page_count,
                self.source_directory.clone(),
                self.read_check.clone(),
            );
        }
    }

    pub(super) fn copy_pdf_image(&mut self, image: usize) {
        if let Some(page) = self.selected_page {
            self.document_tools.copy_image(
                self.path.clone(),
                page,
                image,
                self.source_directory.clone(),
                self.read_check.clone(),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extra_pdf_inputs_are_rejected_before_launch_when_they_are_kernel_aliases() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source.pdf");
        std::fs::write(&source, b"ordinary source; no parsing should occur").unwrap();
        let candidate = root.path().join("candidate.pdf");
        std::os::unix::fs::symlink("/proc/uptime", &candidate).unwrap();
        let output = root.path().join("matches.json");
        let error = run_pdf_tool(
            "compare",
            &source,
            &output,
            &[],
            Some(&candidate),
            &ImageToolProcessControl::new(),
        )
        .unwrap_err();
        assert!(error.contains("inspection-only"), "{error}");
        assert!(!output.exists());
    }

    #[test]
    fn opening_adopts_only_a_verified_ocr_copy_before_loading_pages() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source.pdf");
        let candidate = directory.path().join("source.ocr.edited.pdf");
        super::super::tests::write_two_page_pdf(&source);
        std::fs::copy(&source, &candidate).unwrap();
        let source_bytes = std::fs::read(&source).unwrap();
        for result in [
            Ok(Some(candidate.clone())),
            Ok(None),
            Err("unreadable candidate".into()),
        ] {
            let expected = result
                .as_ref()
                .ok()
                .and_then(|path| path.clone())
                .unwrap_or(source.clone());
            let mut state = PdfEditorState::without_jobs(source.clone());
            state.document_tools.reuse_job = Some(ToolJob::start(move |_| result));
            assert!(state.info_job.is_none());
            let ctx = egui::Context::default();
            let deadline = std::time::Instant::now() + Duration::from_secs(3);
            while state.document_tools.reuse_job.is_some() && std::time::Instant::now() < deadline {
                state.poll_existing_ocr(&ctx, 0.0);
                std::thread::sleep(Duration::from_millis(5));
            }
            assert!(state.document_tools.reuse_job.is_none());
            assert_eq!(state.path, expected);
            assert!(state.info_job.is_some());
            assert!(!state.dirty);
            assert_eq!(state.document_tools.ocr_complete, expected == candidate);
            if state.document_tools.ocr_complete {
                state.info_job.take().unwrap().0.cancel();
                state.page_count = 2;
                state.document_tools.layout = Some(Arc::new(PdfPageLayout::default()));
                state.start_pdf_ocr();
                assert!(state.document_tools.ocr_job.is_none());
            }
            assert_eq!(std::fs::read(&source).unwrap(), source_bytes);
        }
    }

    #[test]
    fn newly_completed_ocr_cannot_start_a_second_job() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source.pdf");
        super::super::tests::write_two_page_pdf(&source);
        let mut state = PdfEditorState::without_jobs(source.clone());
        let output = state.sibling_output_path("ocr");
        let staging = super::super::output::PdfOutput::new(&output).unwrap();
        std::fs::copy(&source, staging.staged_path()).unwrap();
        let expected = output.clone();
        state.document_tools.ocr_job = Some(ToolJob::start(move |_| Ok(staging)));
        let ctx = egui::Context::default();
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while state.document_tools.ocr_busy() && std::time::Instant::now() < deadline {
            state.poll_document_tools(&ctx, 0.0);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(!state.document_tools.ocr_busy());
        assert!(state.document_tools.ocr_complete);
        assert_eq!(state.path, expected);
        state.info_job.take().unwrap().0.cancel();
        state.page_count = 2;
        state.document_tools.layout = Some(Arc::new(PdfPageLayout::default()));
        state.start_pdf_ocr();
        assert!(!state.document_tools.ocr_busy());
        assert!(!state.sibling_output_path("ocr").exists());
    }
}
