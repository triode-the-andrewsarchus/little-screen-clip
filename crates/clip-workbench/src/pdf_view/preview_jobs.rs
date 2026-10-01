use super::*;

impl PdfEditorState {
    pub(super) fn start_info_worker(&mut self) {
        if let Some((ticket, _)) = self.info_job.take() {
            ticket.cancel();
        }
        self.cancel_edit_worker();
        self.cancel_preview_worker();
        self.raster_cache = None;
        if let Some(job) = &mut self.decode_job {
            job.cancel();
        }
        self.decode_requests.clear();
        self.viewport = PdfPageViewport::default();
        self.pages.clear();
        self.page_count = 0;
        let path = self.path.clone();
        self.info_job = Some(
            BackgroundJobs::global().submit(Duration::from_secs(30), move || get_pdf_info(&path)),
        );
    }

    pub(super) fn poll_info_worker(&mut self, ctx: &egui::Context, now: f64) {
        let Some((_, rx)) = &self.info_job else {
            return;
        };
        let result = match rx.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => {
                ctx.request_repaint_after(Duration::from_millis(50));
                return;
            }
            Err(TryRecvError::Disconnected) => Err(JobError::Cancelled),
        };
        self.info_job = None;
        let info = match result {
            Ok(Ok(info)) => info,
            failure => {
                let reason = match failure {
                    Ok(Err(error)) => error,
                    Err(error) => error.to_string(),
                    Ok(Ok(_)) => unreachable!(),
                };
                self.status_toast
                    .set(format!("PDF metadata unavailable: {reason}"), now);
                PdfInfo {
                    page_count: 1,
                    title: None,
                    author: None,
                    page_size: None,
                }
            }
        };
        self.page_count = info.page_count;
        self.pages = (1..=self.page_count)
            .map(|page_num| PdfPageItem {
                page_num,
                raster: None,
                texture: None,
                texture_last_used: 0,
                error: None,
            })
            .collect();
        self.selected_page =
            (self.page_count > 0).then(|| self.selected_page.unwrap_or(1).min(self.page_count));
        self.start_preview_worker();
        ctx.request_repaint();
    }

    pub(super) fn start_preview_worker(&mut self) {
        self.start_preview_worker_on(BackgroundJobs::global(), render_pdf_page_preview);
    }

    pub(super) fn start_preview_worker_on<F>(&mut self, jobs: &BackgroundJobs, render_page: F)
    where
        F: Fn(&Path, usize, u32) -> Result<(Vec<u8>, u32, u32), String> + Send + 'static,
    {
        self.cancel_preview_worker();
        let raster_cache = match tempfile::tempdir() {
            Ok(cache) => std::sync::Arc::new(cache),
            Err(error) => {
                self.previews_loading = false;
                for page in &mut self.pages {
                    page.error = Some(format!("PDF preview cache unavailable: {error}"));
                }
                return;
            }
        };
        self.raster_cache = Some(std::sync::Arc::clone(&raster_cache));
        let path = self.path.clone();
        let page_count = self.page_count;
        let selected_page = self.selected_page.unwrap_or(1).min(page_count.max(1));
        // Bound pending raster data if rendering outruns the UI. Cancellation
        // drops this receiver to unblock a producer waiting to publish a page.
        let (tx, rx) = sync_channel(PREVIEW_QUEUE_CAPACITY);
        let cancelled = Arc::new(AtomicBool::new(false));
        let (ticket, job_rx) = spawn_preview_worker(
            jobs,
            path,
            page_count,
            selected_page,
            raster_cache,
            tx,
            Arc::clone(&cancelled),
            render_page,
        );
        self.preview_rx = Some(rx);
        self.preview_job = Some((ticket, job_rx));
        self.preview_cancel = Some(cancelled);
        self.previews_loading = true;
    }

    pub(super) fn cancel_preview_worker(&mut self) {
        if let Some(cancelled) = self.preview_cancel.take() {
            cancelled.store(true, Ordering::Release);
        }
        if let Some((ticket, _)) = self.preview_job.take() {
            ticket.cancel();
        }
        self.preview_rx = None;
        self.previews_loading = false;
    }

    pub(super) fn poll_preview_worker(&mut self, ctx: &egui::Context) {
        let mut finished = false;
        if let Some(rx) = &self.preview_rx {
            loop {
                match rx.try_recv() {
                    Ok(message) => {
                        if let Some(job) = &mut self.decode_job {
                            if job.raster.page_num() == message.page_num {
                                job.cancel();
                            }
                        }
                        if let Some(page) = self.pages.get_mut(message.page_num.saturating_sub(1)) {
                            match message.result {
                                Ok(raster) => {
                                    page.texture = None;
                                    page.raster = Some(raster);
                                    page.error = None;
                                }
                                Err(error) => page.error = Some(error),
                            }
                        }
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        finished = true;
                        break;
                    }
                }
            }
        }
        if finished {
            self.preview_rx = None;
        }
        let completion = self
            .preview_job
            .as_ref()
            .and_then(|(_, rx)| match rx.try_recv() {
                Ok(Ok(())) => Some(Ok(())),
                Ok(Err(error)) => Some(Err(error.to_string())),
                Err(TryRecvError::Disconnected) => Some(Err(
                    "PDF preview worker disconnected unexpectedly".to_owned(),
                )),
                Err(TryRecvError::Empty) => None,
            });
        if let Some(result) = completion {
            self.preview_job = None;
            self.preview_cancel = None;
            self.preview_rx = None;
            self.previews_loading = false;
            if let Err(error) = result {
                let message = format!("PDF previews unavailable: {error}");
                for page in &mut self.pages {
                    if page.raster.is_none() && page.error.is_none() {
                        page.error = Some(message.clone());
                    }
                }
                self.status_toast
                    .set(message, ctx.input(|input| input.time));
            }
        } else if finished && self.preview_job.is_none() {
            self.previews_loading = false;
        } else if self.previews_loading {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
    }

    pub fn reload_pages(&mut self, ctx: &egui::Context) {
        self.start_info_worker();
        ctx.request_repaint();
    }
}
pub(super) fn spawn_preview_worker<F>(
    jobs: &BackgroundJobs,
    path: PathBuf,
    page_count: usize,
    selected_page: usize,
    raster_cache: std::sync::Arc<tempfile::TempDir>,
    tx: std::sync::mpsc::SyncSender<PdfPreviewMsg>,
    cancelled: Arc<AtomicBool>,
    render_page: F,
) -> (JobTicket, Receiver<Result<(), JobError>>)
where
    F: Fn(&Path, usize, u32) -> Result<(Vec<u8>, u32, u32), String> + Send + 'static,
{
    jobs.submit(Duration::MAX, move || {
        // A large selected-page raster arrives first; thumbnails then fill in
        // incrementally. An in-flight renderer may complete after cancellation,
        // but its result must not be written or published.
        let mut order = Vec::with_capacity(page_count);
        if page_count > 0 {
            order.push(selected_page);
        }
        order.extend((1..=page_count).filter(|page| *page != selected_page));
        for page_num in order {
            if cancelled.load(Ordering::Acquire) {
                break;
            }
            let max_dim = if page_num == selected_page {
                PAGE_RASTER_MAX_DIM
            } else {
                360
            };
            let rendered = render_page(&path, page_num, max_dim);
            if cancelled.load(Ordering::Acquire) {
                break;
            }
            let result = rendered.and_then(|(png_bytes, width, height)| {
                let raster_path = raster_cache.path().join(format!("page-{page_num}.png"));
                std::fs::write(&raster_path, png_bytes)
                    .map(|()| {
                        Arc::new(PdfPageRasterResource {
                            page_num,
                            path: raster_path,
                            width,
                            height,
                            render_width: max_dim,
                            _raster_directory: Arc::clone(&raster_cache),
                        })
                    })
                    .map_err(|error| format!("Could not cache PDF page {page_num}: {error}"))
            });
            if cancelled.load(Ordering::Acquire) {
                break;
            }
            if tx.send(PdfPreviewMsg { page_num, result }).is_err() {
                break;
            }
        }
    })
}
