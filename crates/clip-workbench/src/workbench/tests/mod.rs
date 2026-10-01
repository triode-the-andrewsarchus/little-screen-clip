use super::*;

mod actions;
mod gallery;
mod image_sessions;
mod model3d;
mod shortcuts;
mod startup;
mod text_loading;
mod viewer_lifecycle;

pub(super) fn test_workbench_app(
    viewer: Option<ViewerSession>,
    startup_viewers: VecDeque<ViewerSession>,
) -> WorkbenchApp {
    let config = AppConfig::default();
    let palette = ui_appearance::palette(config.workbench.appearance);
    let (state_tx, state_rx) = std::sync::mpsc::channel();
    drop(state_tx);
    WorkbenchApp {
        state: SlotManager::default(),
        state_rx,
        view: WorkbenchView::Palette,
        canvas_state: NodeCanvasState::new(),
        recipes: Vec::new(),
        search_query: String::new(),
        palette_search_needs_focus: false,
        selected_action_index: 0,
        selected_slots: vec![SlotId::LeftHand],
        inspector_text_preview_cache: None,
        status_msg: None,
        appearance_sync: AppearanceSync::default(),
        renderer: ScreenRenderer::fallback(config.clone()),
        ui_palette: palette,
        mirror_texture: None,
        dirty_mirror: false,
        in_app_prompt: None,
        viewer,
        active_image_key: None,
        retained_image_session: None,
        pending_image_close: false,
        startup_viewers,
        startup_midi: None,
        image_load_job: None,
        text_load_job: None,
        spreadsheet_load_job: None,
        midi_load_job: None,
        rolling_editor_job: None,
        rolling_audio_strip: None,
        dragging_slot: None,
        config,
        appearance_applied: true,
    }
}

pub(super) fn queued_job_behind_blocker<T: Send + 'static>(
    operation: impl FnOnce() -> T + Send + 'static,
) -> (
    BackgroundJobs,
    std::sync::mpsc::Sender<()>,
    JobTicket,
    Receiver<Result<T, JobError>>,
) {
    let jobs = BackgroundJobs::new(1, 1);
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let (_blocker_ticket, _blocker_result) = jobs.submit(Duration::MAX, move || {
        let _ = started_tx.send(());
        let _ = release_rx.recv();
    });
    started_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("executor blocker should start");
    let (ticket, receiver) = jobs.submit(Duration::MAX, operation);
    (jobs, release_tx, ticket, receiver)
}

pub(super) fn release_blocker_and_wait_for_queue_to_drain(
    jobs: &BackgroundJobs,
    release: std::sync::mpsc::Sender<()>,
) {
    release
        .send(())
        .expect("executor blocker should be waiting");
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    loop {
        assert!(
            std::time::Instant::now() < deadline,
            "executor queue did not drain before deadline"
        );
        let (_ticket, receiver) = jobs.submit(Duration::MAX, || ());
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        match receiver.recv_timeout(remaining) {
            Ok(Ok(())) => return,
            Ok(Err(JobError::QueueFull)) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                panic!("admitted executor sentinel did not complete before deadline")
            }
            result => panic!("unexpected executor sentinel result: {result:?}"),
        }
    }
}

pub(super) fn isolated_job_ticket() -> (BackgroundJobs, JobTicket) {
    let jobs = BackgroundJobs::new(1, 1);
    let (ticket, _receiver) = jobs.submit(Duration::MAX, || ());
    (jobs, ticket)
}

pub(super) fn synthetic_job_receiver<T>(
    result: Result<T, JobError>,
) -> Receiver<Result<T, JobError>> {
    let (sender, receiver) = std::sync::mpsc::channel();
    sender.send(result).unwrap();
    receiver
}

pub(super) fn synthetic_image_bytes() -> Vec<u8> {
    let mut bytes = Vec::new();
    image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
        1,
        1,
        image::Rgba([12, 34, 56, 255]),
    ))
    .write_to(
        &mut std::io::Cursor::new(&mut bytes),
        image::ImageFormat::Png,
    )
    .unwrap();
    bytes
}

pub(super) fn click_text_smart_copy_action(source_text: &str, button_label: &str) -> String {
    fn frame(
        ctx: &egui::Context,
        app: &mut WorkbenchApp,
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(900.0, 700.0),
                )),
                time: Some(1.0),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    app.render_text_smart_preview_actions(ui);
                });
            },
        )
    }

    let mut app = test_workbench_app(None, VecDeque::new());
    *app.state.get_mut(SlotId::LeftHand).expect("left slot") =
        Some(ClipItem::from_text(source_text));
    let ctx = egui::Context::default();
    let rendered = frame(&ctx, &mut app, Vec::new());
    let button_rect = rendered
        .shapes
        .iter()
        .find_map(|shape| {
            let egui::epaint::Shape::Text(text) = &shape.shape else {
                return None;
            };
            (text.galley.job.text == button_label)
                .then(|| text.galley.rect.translate(text.pos.to_vec2()))
        })
        .unwrap_or_else(|| panic!("smart preview button {button_label:?} did not render"));
    let pos = button_rect.center();
    let pointer_button = |pressed| egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    let _ = frame(
        &ctx,
        &mut app,
        vec![egui::Event::PointerMoved(pos), pointer_button(true)],
    );
    let clicked = frame(
        &ctx,
        &mut app,
        vec![egui::Event::PointerMoved(pos), pointer_button(false)],
    );
    clicked.platform_output.copied_text
}

pub(super) fn viewport_close_command_count(output: &egui::FullOutput) -> usize {
    output
        .viewport_output
        .get(&egui::ViewportId::ROOT)
        .map(|output| {
            output
                .commands
                .iter()
                .filter(|command| matches!(command, egui::ViewportCommand::Close))
                .count()
        })
        .unwrap_or_default()
}

pub(super) fn space_key_event() -> egui::Event {
    key_press_event(egui::Key::Space)
}

pub(super) fn key_press_event(key: egui::Key) -> egui::Event {
    egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    }
}

pub(super) fn wait_for_image_load(app: &mut WorkbenchApp) {
    let ctx = egui::Context::default();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while app.image_load_job.is_some() || matches!(app.viewer, Some(ViewerSession::ImageLoading(_)))
    {
        app.poll_image_load(&ctx);
        assert!(
            std::time::Instant::now() < deadline,
            "image load did not finish"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}

pub(super) fn wait_for_text_load(app: &mut WorkbenchApp) {
    let ctx = egui::Context::default();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while app.text_load_job.is_some() || app.spreadsheet_load_job.is_some() {
        app.poll_spreadsheet_load(&ctx);
        app.poll_text_load(&ctx);
        assert!(
            std::time::Instant::now() < deadline,
            "text load did not finish"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}

pub(super) fn write_test_image(path: &std::path::Path, width: u32) {
    image::RgbImage::from_pixel(width, 2, image::Rgb([24, 120, 180]))
        .save(path)
        .unwrap();
}

pub(super) fn gallery_viewer() -> ViewerSession {
    ViewerSession::ComponentGallery(component_gallery::ComponentGalleryState::default())
}

pub(super) fn test_image_preview_state() -> ImagePreviewState {
    let image = image::RgbaImage::from_pixel(2, 2, image::Rgba([12, 34, 56, 255]));
    let mut encoded = std::io::Cursor::new(Vec::new());
    image
        .write_to(&mut encoded, image::ImageFormat::Png)
        .unwrap();
    ImagePreviewState::from_bytes_with_meta(
        encoded.into_inner(),
        "image/png".to_owned(),
        None,
        "session fixture".to_owned(),
        &AppConfig::default(),
    )
    .unwrap()
}
