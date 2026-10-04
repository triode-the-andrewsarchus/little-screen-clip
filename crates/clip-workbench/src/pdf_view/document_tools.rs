use super::*;
use crate::image_preview::{ImageToolProcessControl, ocr_command, run_image_tool_status_command};
use little_clip_core::pdf::PdfPageLayout;
use std::process::{Command, Stdio};

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

struct OcrOutput {
    directory: tempfile::TempDir,
    output: PathBuf,
}

#[derive(Default)]
pub(super) struct PdfDocumentTools {
    pub layout: Option<Arc<PdfPageLayout>>,
    pub layout_error: Option<String>,
    page: Option<usize>,
    layout_job: Option<ToolJob<PdfPageLayout>>,
    ocr_job: Option<ToolJob<OcrOutput>>,
    copy_job: Option<ToolJob<()>>,
    pub ocr_language: String,
    pub ocr_status: String,
    pub interaction: super::interaction::PdfInteraction,
}

/// Reuse image tools' process group cancellation and the existing Python environment.
fn run_pdf_tool(
    operation: &str,
    source: &Path,
    output: &Path,
    arguments: &[String],
    control: &ImageToolProcessControl,
) -> Result<(), String> {
    use std::io::Read;
    if !source.is_file() {
        return Err("PDF source is unavailable".into());
    }
    let (python, script) = ocr_command();
    let script = Path::new(&script).with_file_name("pdf_document.py");
    let log = tempfile::NamedTempFile::new().map_err(|error| error.to_string())?;
    let mut command = Command::new(python);
    command
        .arg(script)
        .arg(operation)
        .arg(source)
        .arg(output)
        .args(arguments)
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
    ) {
        if self.page != page {
            self.invalidate_layout();
            self.page = page;
            if let Some(page) = page {
                let source = path.to_owned();
                self.layout_job = Some(ToolJob::start(move |control| {
                    let _lease = lease;
                    let directory = tempfile::tempdir().map_err(|error| error.to_string())?;
                    let output = directory.path().join("layout.json");
                    run_pdf_tool(
                        "layout",
                        &source,
                        &output,
                        &["--page".into(), page.to_string()],
                        &control,
                    )?;
                    let file = std::fs::File::open(output).map_err(|error| error.to_string())?;
                    serde_json::from_reader(file).map_err(|error| error.to_string())
                }));
            }
        }
        if let Some(result) = self.layout_job.as_ref().and_then(|job| job.poll(ctx)) {
            self.layout_job = None;
            match result {
                Ok(layout) => self.layout = Some(Arc::new(layout)),
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
    ) {
        if self.ocr_busy() {
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
            let parent = output
                .parent()
                .filter(|path| !path.as_os_str().is_empty())
                .unwrap_or(Path::new("."));
            let directory = tempfile::tempdir_in(parent).map_err(|error| error.to_string())?;
            let staged = directory.path().join(STAGED_PDF_FILENAME);
            run_pdf_tool(
                "ocr",
                &source,
                &staged,
                &["--language".into(), language],
                &control,
            )?;
            let info = get_pdf_info(&staged)?;
            if info.page_count != count {
                return Err("OCR changed the PDF page count".into());
            }
            Ok(OcrOutput { directory, output })
        }));
    }

    fn copy_image(
        &mut self,
        source: PathBuf,
        page: usize,
        image: usize,
        lease: Option<Arc<tempfile::TempDir>>,
    ) {
        if self.copy_job.is_some() {
            return;
        }
        self.copy_job = Some(ToolJob::start(move |control| {
            let _lease = lease;
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
                &control,
            )?;
            let bytes = std::fs::read(output).map_err(|error| error.to_string())?;
            little_clip_core::copy_bytes_to_system_clipboard(&bytes, "image/png")
                .map_err(|error| error.to_string())
        }));
    }
}

impl PdfEditorState {
    pub(super) fn poll_document_tools(&mut self, ctx: &egui::Context, now: f64) {
        let page = self.selected_page.filter(|_| self.info_job.is_none());
        self.document_tools
            .service_layout(&self.path, page, ctx, self.source_directory.clone());
        if let Some(result) = self
            .document_tools
            .ocr_job
            .as_ref()
            .and_then(|job| job.poll(ctx))
        {
            self.document_tools.ocr_job = None;
            match result.and_then(|output| {
                std::fs::rename(
                    output.directory.path().join(STAGED_PDF_FILENAME),
                    &output.output,
                )
                .map_err(|error| error.to_string())?;
                Ok(output.output)
            }) {
                Ok(path) => {
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
        if !self.edit_in_progress && self.info_job.is_none() && self.page_count > 0 {
            self.document_tools.start_ocr(
                self.path.clone(),
                self.sibling_output_path("ocr"),
                self.page_count,
                self.source_directory.clone(),
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
            );
        }
    }
}
