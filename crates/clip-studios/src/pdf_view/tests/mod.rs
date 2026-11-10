use super::preview_jobs::spawn_preview_worker;
use super::*;
use std::sync::mpsc::channel;

mod cache;
mod editing;
mod raster;
mod studio;
pub(super) fn action_guard_state(
    path: PathBuf,
    page_count: usize,
    selected_page: Option<usize>,
) -> PdfEditorState {
    let mut state = raster_state(page_count, 1);
    state.path = path;
    state.title = "fixture.pdf".to_owned();
    state.selected_page = selected_page;
    state
}

pub(super) fn write_two_page_pdf(path: &Path) {
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Kids [3 0 R 5 0 R] /Count 2 >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 300] /Resources << >> /Contents 4 0 R >>",
        "<< /Length 0 >>\nstream\n\nendstream",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 100] /Resources << >> /Contents 6 0 R >>",
        "<< /Length 0 >>\nstream\n\nendstream",
    ];
    let mut bytes = b"%PDF-1.4\n".to_vec();
    let mut offsets = vec![0usize];
    for (index, object) in objects.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{} 0 obj\n{}\nendobj\n", index + 1, object).as_bytes());
    }
    let xref_offset = bytes.len();
    bytes.extend_from_slice(format!("xref\n0 {}\n", objects.len() + 1).as_bytes());
    bytes.extend_from_slice(b"0000000000 65535 f \n");
    for offset in offsets.into_iter().skip(1) {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref_offset}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    std::fs::write(path, bytes).unwrap();
}

pub(super) fn raster_state(page_count: usize, dimension: u32) -> PdfEditorState {
    raster_state_with_dimensions(page_count, (dimension, dimension))
}

pub(super) fn raster_state_with_dimensions(
    page_count: usize,
    dimensions: (u32, u32),
) -> PdfEditorState {
    let (width, height) = dimensions;
    let image = image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
        width,
        height,
        image::Rgba([23, 45, 67, 255]),
    ));
    let mut png = std::io::Cursor::new(Vec::new());
    image.write_to(&mut png, image::ImageFormat::Png).unwrap();
    let raster_cache = std::sync::Arc::new(tempfile::tempdir().unwrap());
    let mut state = PdfEditorState::without_jobs(PathBuf::from("fixture.pdf"));
    state.page_count = page_count;
    state.pages = (1..=page_count)
        .map(|page_num| PdfPageItem {
            page_num,
            raster: {
                let path = raster_cache.path().join(format!("page-{page_num}.png"));
                std::fs::write(&path, png.get_ref()).unwrap();
                Some(test_page_raster(
                    page_num,
                    path,
                    width,
                    height,
                    Arc::clone(&raster_cache),
                ))
            },
            texture: None,
            texture_last_used: 0,
            error: None,
        })
        .collect();
    state.selected_page = (page_count > 0).then_some(1);
    state.raster_cache = Some(raster_cache);
    state
}

pub(super) fn wait_for_decode(state: &mut PdfEditorState, page_num: usize, ctx: &egui::Context) {
    state.decode_requests.clear();
    state.decode_requests.push(page_num);
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        state.poll_decode_worker(ctx);
        state.schedule_page_decode(ctx);
        if state
            .decode_job
            .as_ref()
            .is_some_and(|job| job.raster.page_num() == page_num && job.ready.is_some())
            || state.pages[page_num - 1].error.is_some()
        {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "decode did not complete"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

pub(super) fn show_inline_frame(ctx: &egui::Context, preview: &mut InlinePdfPagePreview) {
    let _ = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            preview.show(ui, ctx, Vec2::new(64.0, 64.0));
        });
    });
}

pub(super) fn wait_for_inline_texture(ctx: &egui::Context, preview: &mut InlinePdfPagePreview) {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        show_inline_frame(ctx, preview);
        if preview.texture.is_some() || preview.error.is_some() {
            assert!(
                preview.texture.is_some(),
                "inline PDF decode failed: {:?}",
                preview.error
            );
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "inline PDF decode did not complete"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

pub(super) fn upload_page(
    state: &mut PdfEditorState,
    page_num: usize,
    ctx: &egui::Context,
) -> egui::TextureId {
    wait_for_decode(state, page_num, ctx);
    state.ensure_page_texture(page_num, ctx).unwrap()
}

pub(super) fn resident_textures(state: &PdfEditorState) -> (usize, usize) {
    state
        .pages
        .iter()
        .filter_map(|page| page.texture.as_ref())
        .fold((0, 0), |(count, bytes), texture| {
            (count + 1, bytes + texture.byte_size())
        })
}

pub(super) fn test_page_raster(
    page_num: usize,
    path: PathBuf,
    width: u32,
    height: u32,
    raster_directory: Arc<tempfile::TempDir>,
) -> Arc<PdfPageRasterResource> {
    Arc::new(PdfPageRasterResource {
        page_num,
        path,
        width,
        height,
        render_width: PAGE_RASTER_MAX_DIM,
        _raster_directory: raster_directory,
    })
}

// Block a running job so stale publication and admission can be inspected
// deterministically, without depending on image decoder timing.
pub(super) fn blocked_decode(
    state: &mut PdfEditorState,
) -> (std::sync::mpsc::Sender<()>, JobTicket) {
    let jobs = BackgroundJobs::new(1, 1);
    let (release, wait) = channel();
    let (started, start) = channel();
    let (ticket, rx) = jobs.submit(Duration::from_secs(10), move || {
        started.send(()).unwrap();
        wait.recv().unwrap();
        Err("obsolete decode error".to_owned())
    });
    start.recv_timeout(Duration::from_secs(2)).unwrap();
    let raster = Arc::clone(state.pages[0].raster.as_ref().unwrap());
    state.decode_job = Some(PdfPageDecodeSlot {
        raster,
        ticket: ticket.clone(),
        rx,
        ready: None,
        cancelled: false,
    });
    (release, ticket)
}
