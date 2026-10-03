use super::*;

impl WorkbenchApp {
    pub(crate) fn set_status_message(&mut self, message: impl Into<String>) {
        self.status_msg = Some(message.into());
    }

    pub(crate) fn clear_status_message(&mut self) {
        self.status_msg = None;
    }

    pub(crate) fn new(
        ctx: egui::Context,
        start_with_diff: bool,
        start_with_binary_diff: bool,
        initial_audio: Option<PathBuf>,
        initial_image: Option<PathBuf>,
        initial_pdf: Option<PathBuf>,
        initial_midi: Option<PathBuf>,
        initial_scene: Option<PathBuf>,
        open_component_gallery: bool,
    ) -> Self {
        let mut config = AppConfig::load();
        // Desktop monitor mirror is always upright (no 180 flip)
        config.display.rotate_180 = false;
        let ui_palette = ui_appearance::palette(config.workbench.appearance);
        let state = Arc::new(SlotManager::default());
        let state_rx = state_sync::spawn_state_poller(ctx);

        let canvas_state = NodeCanvasState::new();
        let recipes = load_all_recipes();

        let audio_preview = initial_audio.map(|p| {
            let title = p
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "Audio file".to_string());
            AudioPreviewState::new(p, title).with_editor(config.audio.editor.clone())
        });

        let rolling_audio_strip = Some(RollingAudioStripState::new());
        let mut startup_viewers = VecDeque::new();
        if let Some(audio) = audio_preview {
            startup_viewers.push_back(ViewerSession::Audio(audio));
        }
        if let Some(path) = initial_image {
            startup_viewers.push_back(ViewerSession::ImageLoading(ImageLoadJob::new(
                ImageLoadSource::File(path),
            )));
        }
        if let Some(path) = initial_pdf {
            startup_viewers.push_back(ViewerSession::Pdf(PdfEditorState::new(path)));
        }
        if let Some(path) = initial_scene {
            startup_viewers.push_back(scene_file_viewer(path, &config));
        }

        let mut app = Self {
            state,
            state_rx,
            state_loaded: false,
            startup_comparison: (!open_component_gallery
                && (start_with_diff || start_with_binary_diff))
                .then_some(start_with_binary_diff),
            view: WorkbenchView::Palette,
            canvas_state,
            recipes,
            search_query: String::new(),
            palette_search_needs_focus: true,
            selected_action_index: 0,
            selected_slots: vec![SlotId::LeftHand],
            inspector_text_preview_cache: None,
            status_msg: None,
            appearance_sync: AppearanceSync::default(),
            renderer: None,
            mirror_job: None,
            mirror_error: None,
            mirror_animated: false,
            palette_ops: None,
            palette_ops_job: None,
            ui_palette,
            mirror_texture: None,
            dirty_mirror: true,
            in_app_prompt: None,
            viewer: None,
            active_image_key: None,
            retained_image_session: None,
            pending_image_close: false,
            startup_viewers,
            startup_midi: initial_midi,
            image_load_job: None,
            text_load_job: None,
            spreadsheet_load_job: None,
            midi_load_job: None,
            rolling_editor_job: None,
            rolling_audio_strip,
            dragging_slot: None,
            config,
            appearance_applied: false,
        };

        if open_component_gallery {
            app.viewer = Some(ViewerSession::ComponentGallery(
                component_gallery::ComponentGalleryState::default(),
            ));
        }

        if app.viewer.is_none() && app.startup_comparison.is_none() {
            app.advance_startup();
        }

        app
    }

    pub(crate) fn advance_startup(&mut self) {
        if !self.retain_current_image_session() {
            self.pending_image_close = true;
            return;
        }
        self.viewer = self.startup_viewers.pop_front();
        self.active_image_key = None;
        if self.viewer.is_none() {
            if let Some(path) = self.startup_midi.take() {
                self.start_midi_load(path);
            }
        }
    }
}
