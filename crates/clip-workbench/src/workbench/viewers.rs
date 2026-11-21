use super::*;
use clip_ui::buttons::ButtonRole;

#[derive(Clone)]
pub(super) enum MapTarget {
    File(PathBuf),
    Location(String),
}

fn geographic_path(path: &std::path::Path) -> bool {
    use little_clip_core::file_format::Format;
    Format::for_path(path) == Some(Format::Geography)
}

fn file_manager_command() -> std::process::Command {
    const NAME: &str = "clip-file-manager";
    let binary = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|parent| parent.join(NAME)))
        .filter(|path| path.is_file())
        .unwrap_or_else(|| PathBuf::from(NAME));
    std::process::Command::new(binary)
}

impl WorkbenchApp {
    pub(crate) fn retire_hidden_rolling_audio(&mut self) {
        let hidden = self.view != WorkbenchView::Palette
            || self.viewer.is_some()
            || self.in_app_prompt.is_some()
            || self.image_load_job.is_some()
            || self.text_load_job.is_some()
            || self.office_load_job.is_some();
        if hidden && let Some(strip) = &mut self.rolling_audio_strip {
            strip.stop();
        }
    }

    pub fn is_audio_region_active(&self) -> bool {
        self.view == WorkbenchView::Palette
            && self
                .rolling_audio_strip
                .as_ref()
                .is_some_and(|s| s.selected_region.is_some())
    }

    pub(crate) fn tick_viewer(&mut self, ctx: &egui::Context) {
        self.retire_hidden_rolling_audio();
        if let Some(strip) = &mut self.rolling_audio_strip {
            strip.tick(ctx);
        }
        match self.viewer.as_mut() {
            Some(ViewerSession::Diff(state)) => state.tick(ctx),
            Some(ViewerSession::BinaryDiff(state)) => state.tick(ctx),
            Some(ViewerSession::Audio(state)) => state.tick(ctx),
            Some(ViewerSession::Image(state)) => state.tick(ctx),
            Some(ViewerSession::Pdf(state)) => state.tick(ctx),
            Some(ViewerSession::Midi(state)) => state.tick(ctx),
            Some(ViewerSession::Model3d(state)) => state.tick(ctx),
            Some(ViewerSession::Splat(state)) => state.tick(ctx),
            Some(ViewerSession::TextEditor(state)) => state.tick(ctx),
            Some(ViewerSession::Text(state)) => state.tick(ctx),
            Some(
                ViewerSession::ImageLoading(_)
                | ViewerSession::Spreadsheet(_)
                | ViewerSession::ComponentGallery(_),
            )
            | None => {}
        }
        if let Some((_, state)) = self.retained_image_session.as_mut()
            && state.poll_cached_jobs()
        {
            ctx.request_repaint_after(Duration::from_millis(16));
        }
    }

    pub(crate) fn render_modal_viewers(&mut self, root: &mut egui::Ui) -> bool {
        let ctx = &root.ctx().clone();
        if self.render_prompt_modal(ctx) {
            return true;
        }

        let Some(viewer) = self.viewer.as_mut() else {
            if self.image_load_job.is_some()
                || self.text_load_job.is_some()
                || self.office_load_job.is_some()
                || self.spreadsheet_load_job.is_some()
                || self.midi_load_job.is_some()
            {
                let mut cancel = false;
                show_viewer_panel(root, |ui| {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(if self.image_load_job.is_some() {
                            "Loading image…"
                        } else if self.office_load_job.is_some() {
                            "Converting document…"
                        } else if self.spreadsheet_load_job.is_some() {
                            "Loading spreadsheet…"
                        } else if self.midi_load_job.is_some() {
                            "Loading MIDI…"
                        } else {
                            "Loading text…"
                        });
                        cancel = ui
                            .add(ButtonRole::Secondary.button("Cancel [Esc]", &self.ui_palette))
                            .clicked();
                    });
                });
                if cancel || ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
                    self.cancel_preview_loads();
                    self.advance_startup();
                }
                ctx.request_repaint_after(Duration::from_millis(40));
                return true;
            }
            return false;
        };
        let mut close = self.pending_image_close;
        let mut pdf_dirty = false;
        let mut gallery_open_request = None;
        match viewer {
            ViewerSession::Diff(state) => {
                show_viewer_panel(root, |ui| {
                    state.render_ui(ui, ctx, &self.ui_palette, &mut close);
                });
            }
            ViewerSession::BinaryDiff(state) => {
                show_viewer_panel(root, |ui| {
                    state.render_ui(ui, ctx, &self.ui_palette, &mut close);
                });
            }
            ViewerSession::Audio(state) => {
                show_viewer_panel(root, |ui| {
                    state.render_ui(ui, ctx, &self.ui_palette, &mut close);
                });
                if close {
                    state.stop();
                }
            }
            ViewerSession::ImageLoading(_) => {
                show_viewer_panel(root, |ui| {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Loading image…");
                        close = ui
                            .add(ButtonRole::Secondary.button("Cancel [Esc]", &self.ui_palette))
                            .clicked();
                    });
                });
                close |= ctx.input(|input| input.key_pressed(egui::Key::Escape));
                ctx.request_repaint_after(Duration::from_millis(40));
            }
            ViewerSession::Image(state) => {
                show_viewer_panel(root, |ui| {
                    state.render_ui(ui, ctx, &self.ui_palette, &mut close);
                });
            }
            ViewerSession::Spreadsheet(state) => {
                show_viewer_panel(root, |ui| {
                    state.render_ui(ui, ctx, &self.ui_palette, &mut close);
                });
            }
            ViewerSession::Pdf(state) => {
                show_viewer_panel(root, |ui| {
                    state.render_ui(ui, ctx, &self.ui_palette, &mut close);
                });
                pdf_dirty = close && state.dirty;
            }
            ViewerSession::Midi(state) => {
                show_viewer_panel(root, |ui| {
                    state.render_ui(ui, &self.ui_palette, &mut close);
                });
                if close {
                    state.stop();
                }
            }
            ViewerSession::Model3d(state) => {
                show_viewer_panel(root, |ui| {
                    state.render_ui(ui, &self.ui_palette, &mut close);
                });
            }
            ViewerSession::Splat(state) => {
                show_viewer_panel(root, |ui| state.render_ui(ui, &self.ui_palette, &mut close));
            }
            ViewerSession::TextEditor(state) => {
                show_viewer_panel(root, |ui| state.ui(ui, &self.ui_palette, &mut close));
            }
            ViewerSession::Text(state) => {
                show_viewer_panel(root, |ui| {
                    state.render_ui(ui, ctx, &self.ui_palette, &mut close);
                });
            }
            ViewerSession::ComponentGallery(state) => {
                if ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
                    close = true;
                } else {
                    show_viewer_panel(root, |ui| {
                        state.render_ui(ui, &self.ui_palette, &mut close);
                    });
                    gallery_open_request = state.take_pending_open();
                }
            }
        }

        if !close && let Some(request) = gallery_open_request {
            self.viewer = Some(gallery_request_to_viewer_session(request, &self.config));
            self.active_image_key = None;
            ctx.request_repaint();
            return true;
        }

        let image_loading = self.image_load_job.is_some();
        let text_loading = self.text_load_job.is_some();
        if image_loading
            || text_loading
            || self.status_msg.as_deref().is_some_and(|message| {
                message.starts_with("Image preview failed:")
                    || message.starts_with("Text preview failed:")
            })
        {
            egui::Area::new(egui::Id::unique("image_load_status"))
                .order(egui::Order::Foreground)
                .anchor(egui::Align2::RIGHT_TOP, [-12.0, 12.0])
                .show(ctx, |ui| {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        ui.horizontal(|ui| {
                            if image_loading || text_loading {
                                ui.spinner();
                                ui.label(if image_loading {
                                    "Loading image…"
                                } else {
                                    "Loading text…"
                                });
                                if ui
                                    .add(ButtonRole::Secondary.button("Cancel", &self.ui_palette))
                                    .clicked()
                                {
                                    self.cancel_preview_loads();
                                }
                            } else if let Some(message) = &self.status_msg {
                                ui.label(message);
                                if ui
                                    .add(ButtonRole::Secondary.button("Dismiss", &self.ui_palette))
                                    .clicked()
                                {
                                    self.status_msg = None;
                                }
                            }
                        });
                    });
                });
            if image_loading || text_loading {
                ctx.request_repaint_after(Duration::from_millis(40));
            }
        }
        if close {
            self.cancel_preview_loads();
            if !self.retain_current_image_session() {
                self.pending_image_close = true;
                ctx.request_repaint_after(Duration::from_millis(16));
                return true;
            }
            self.pending_image_close = false;
            self.active_image_key = None;
            self.advance_startup();
            self.dirty_mirror |= pdf_dirty;
            ctx.request_repaint();
        }
        true
    }

    pub(crate) fn render_prompt_modal(&mut self, ctx: &egui::Context) -> bool {
        self.render_prompt_modal_dispatch(ctx, |app, request| {
            app.submit_daemon_mutation(request, DaemonMutationKind::Action);
        })
    }

    pub(crate) fn render_prompt_modal_dispatch(
        &mut self,
        ctx: &egui::Context,
        dispatch: impl FnOnce(&mut Self, IpcRequest),
    ) -> bool {
        let pending = self.daemon_mutation.is_some();
        if let Some(prompt) = &mut self.in_app_prompt {
            let mut close_prompt = false;
            let mut submit_request: Option<IpcRequest> = None;

            let modal_bg = hex_to_c32(&self.ui_palette.colors.surface_recessed);
            let modal_stroke = hex_to_c32(&self.ui_palette.harmonies.base);
            egui::Window::new(format!("Input Required — {}", prompt.recipe_name))
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .fixed_size([500.0, 180.0])
                .frame(
                    egui::Frame::window(&ctx.global_style())
                        .fill(modal_bg)
                        .stroke(egui::Stroke::new(1.5_f32, modal_stroke))
                        .corner_radius(crate::ui_ext::CARD_CORNER_RADIUS)
                        .inner_margin(egui::Margin::symmetric(22, 18)),
                )
                .show(ctx, |ui| {
                    if pending {
                        ui.disable();
                    }
                    ui.horizontal(|ui| {
                        Icon::Sparkles.ui(
                            ui,
                            18.0,
                            hex_to_c32(&self.ui_palette.harmonies.triadic_1),
                        );
                        ui.heading(
                            egui::RichText::new(&prompt.recipe_name)
                                .color(hex_to_c32(&self.ui_palette.colors.text_primary))
                                .strong(),
                        );
                    });

                    ui.add_space(crate::ui_ext::SPACE_SM);
                    ui.label(
                        egui::RichText::new(&prompt.message)
                            .color(hex_to_c32(&self.ui_palette.colors.text_secondary)),
                    );

                    ui.add_space(10.0);
                    let edit_resp = ui.add_sized(
                        [ui.available_width(), ui_ext::CONTROL_HEIGHT],
                        egui::TextEdit::singleline(&mut prompt.input_text)
                            .hint_text("Type input text here..."),
                    );

                    if prompt.first_frame {
                        edit_resp.request_focus();
                        prompt.first_frame = false;
                    }

                    let enter =
                        edit_resp.lost_focus() && ctx.input(|i| i.key_pressed(egui::Key::Enter));

                    ui.add_space(14.0);
                    ui.horizontal(|ui| {
                        let submit_btn = ui.add(
                            ButtonRole::Primary.button("Execute & Run [Enter]", &self.ui_palette),
                        );
                        let cancel_btn =
                            ui.add(ButtonRole::Secondary.button("Cancel [Esc]", &self.ui_palette));

                        if !pending && (submit_btn.clicked() || enter) {
                            submit_request = Some(IpcRequest::ExecuteRecipeWithPrompt {
                                recipe_name: prompt.recipe_name.clone(),
                                prompt_text: Some(prompt.input_text.clone()),
                            });
                        }

                        if cancel_btn.clicked() || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                            close_prompt = true;
                        }
                    });
                });

            if let Some(request) = submit_request {
                dispatch(self, request);
                ctx.request_repaint();
            } else if close_prompt && !pending {
                self.in_app_prompt = None;
            }
            return true;
        }
        false
    }

    pub(crate) fn replace_viewer(&mut self, viewer: Option<ViewerSession>) {
        if !self.retain_current_image_session() {
            return;
        }
        self.cancel_preview_loads();
        self.active_image_key = None;
        self.pending_image_close = false;
        self.viewer = viewer;
    }

    pub(crate) fn open_slot_file_preview(&mut self, fp: &std::path::Path, is_image: bool) {
        if geographic_path(fp) {
            self.open_geography_file(fp);
        } else if fp
            .extension()
            .and_then(|s| s.to_str())
            .map(|e| e.eq_ignore_ascii_case("mid") || e.eq_ignore_ascii_case("midi"))
            .unwrap_or(false)
        {
            self.start_midi_load(fp.to_path_buf());
        } else if little_clip_scene::is_splat_file(fp) || is_3d_model_file(fp) {
            self.replace_viewer(Some(scene_file_viewer(fp.to_path_buf(), &self.config)));
        } else if is_image && little_clip_core::notebook::is_candidate_path(fp) {
            self.replace_viewer(Some(ViewerSession::Pdf(
                PdfEditorState::new_notebook_checked(
                    fp.to_path_buf(),
                    crate::file_source::ReadCheck::default(),
                ),
            )));
        } else if little_clip_core::pdf::is_pdf_path(fp) {
            self.replace_viewer(Some(ViewerSession::Pdf(PdfEditorState::new(
                fp.to_path_buf(),
            ))));
        } else if little_clip_core::document::is_office_document(fp) {
            self.start_office_document_load(fp.to_path_buf());
        } else if is_spreadsheet_file(fp) {
            self.start_spreadsheet_load(SpreadsheetLoadSource::File(fp.to_path_buf()));
        } else if is_image || is_image_path(fp) {
            self.start_image_load(ImageLoadSource::File(fp.to_path_buf()));
        } else if little_clip_core::geography::trajectory::Trajectory::is_candidate(fp) {
            self.start_file_map_or_text_load(fp.to_path_buf());
        } else if little_clip_ops::is_text_file(fp) {
            self.start_text_file_load(fp.to_path_buf());
        }
    }

    pub(super) fn geography_action(&mut self, ui: &mut egui::Ui) {
        if let Some(target) = self.map_target(ui.ctx())
            && ui
                .add(contextual_palette_action_button(
                    Some(Icon::Map),
                    "Open in map",
                    &self.ui_palette,
                ))
                .clicked()
        {
            if self.open_map(target) {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
    }

    fn map_target(&self, ctx: &egui::Context) -> Option<MapTarget> {
        self.selected_slots.iter().find_map(|slot| {
            let item = self.state.get(*slot)?;
            if let Some(path) = bounded_item_file_path(item) {
                return (geographic_path(&path)
                    || self
                        .renderer
                        .as_ref()
                        .is_some_and(|renderer| renderer.cached_map_for_file(&path)))
                .then_some(MapTarget::File(path));
            }
            let ClipPayload::Text(text) = &item.payload else {
                return None;
            };
            if text.trim().len() > 128 {
                return None;
            }
            let key = egui::Id::unique("workbench-map-location");
            if let Some((id, target)) =
                ctx.data(|d| d.get_temp::<(uuid::Uuid, Option<MapTarget>)>(key))
                && id == item.id
            {
                return target;
            }
            let target = little_clip_core::geography::places::resolve(text)
                .map(|_| MapTarget::Location(text.trim().to_owned()));
            ctx.data_mut(|d| d.insert_temp(key, (item.id, target.clone())));
            target
        })
    }

    pub(super) fn open_geography_file(&mut self, path: &std::path::Path) {
        self.open_map(MapTarget::File(path.to_owned()));
    }

    fn open_map(&mut self, target: MapTarget) -> bool {
        let mut command = file_manager_command();
        match target {
            MapTarget::File(path) => {
                let absolute = std::path::absolute(&path).unwrap_or(path);
                command.arg(absolute);
            }
            MapTarget::Location(text) => {
                command.arg("--map-location").arg(text);
            }
        }
        match command.spawn() {
            Ok(mut child) => {
                std::thread::spawn(move || {
                    let _ = child.wait();
                });
                self.clear_status_message();
                true
            }
            Err(error) => {
                self.set_status_message(format!("Could not open the map in Files: {error}"));
                false
            }
        }
    }

    pub(crate) fn open_slot_text_preview(&mut self, text: String, title: &str) {
        // A distinctive diagram header takes precedence over delimiter-based
        // table inference, but all actual parsing/layout stays on the workers.
        if little_clip_core::diagram::Format::from_snippet(&text).is_some() {
            self.start_text_preview_load(text, Some(title.into()));
            return;
        }
        if text.len() <= MAX_FILE_PATH_TEXT_BYTES {
            let p = std::path::Path::new(text.trim());
            if p.is_file() {
                self.open_slot_file_preview(p, false);
                return;
            }
        }
        if little_clip_core::geography::places::resolve(&text).is_some() {
            self.open_map(MapTarget::Location(text.trim().to_owned()));
            return;
        }
        self.start_spreadsheet_load(SpreadsheetLoadSource::Text {
            content: Arc::new(text),
            title: title.to_string(),
        });
    }

    pub(crate) fn open_slot_preview_modal(&mut self, item: &ClipItem) {
        if is_large_text_payload(item) {
            if let Some(text) = item.to_clipboard_text() {
                self.open_slot_text_preview(text, &item.title);
            }
            return;
        }
        if is_pdf_clip_item(item) {
            if let Some(fp) = item.file_path() {
                self.replace_viewer(Some(ViewerSession::Pdf(PdfEditorState::new(fp))));
            } else {
                self.status_msg = Some("PDF preview requires a local file path".to_owned());
            }
            return;
        }
        if crate::is_office_document_clip_item(item) {
            if let Some(fp) = item.file_path() {
                self.start_office_document_load(fp);
            } else {
                self.status_msg =
                    Some("Office document preview requires a local file path".to_owned());
            }
            return;
        }
        if let Some(audio_path) = item.audio_path() {
            self.replace_viewer(Some(ViewerSession::Audio(Box::new(
                AudioPreviewState::new(audio_path, item.title.clone())
                    .with_editor(self.config.audio.editor.clone()),
            ))));
            return;
        }
        if let Some(fp) = bounded_item_file_path(item) {
            if item.is_image() || is_image_path(&fp) {
                self.start_image_item_load(item);
            } else {
                self.open_slot_file_preview(&fp, false);
            }
        } else if item.is_image() {
            self.start_image_item_load(item);
        } else if let Some(text) = item.to_clipboard_text() {
            self.open_slot_text_preview(text, &item.title);
        }
    }

    pub(crate) fn check_inspector_text_preview(
        &mut self,
    ) -> Option<(SlotId, Option<PathBuf>, bool)> {
        let selected_item_ids = self
            .selected_slots
            .iter()
            .map(|slot| self.state.get(*slot).map(|item| item.id))
            .collect::<Vec<_>>();
        if let Some(cache) = &self.inspector_text_preview_cache
            && cache.selected_slots == self.selected_slots
            && cache.selected_item_ids == selected_item_ids
        {
            return cache.text_preview.clone();
        }

        let spreadsheet_slot = self.selected_slots.iter().copied().find(|slot| {
            let Some(item) = self.state.get(*slot) else {
                return false;
            };
            if bounded_item_file_path(item).is_some_and(|path| is_spreadsheet_file(&path)) {
                return true;
            }
            match &item.payload {
                ClipPayload::Text(text) | ClipPayload::Markdown(text) => {
                    text.len() > MAX_INLINE_TEXT_ROUTING_BYTES || looks_like_csv_text(text)
                }
                _ => false,
            }
        });
        let text_preview = self.selected_slots.iter().find_map(|slot| {
            let item = self.state.get(*slot)?;
            let file_path = if let Some(path) =
                bounded_item_file_path(item).filter(|p| little_clip_ops::is_text_file(p))
            {
                Some(path)
            } else {
                let text = match &item.payload {
                    // Borrow text and Markdown payloads so a large clipboard value is
                    // not copied just to decide which inspector button to show.
                    ClipPayload::Text(text) | ClipPayload::Markdown(text) => {
                        Some(std::borrow::Cow::Borrowed(text.as_str()))
                    }
                    _ => item.to_clipboard_text().map(std::borrow::Cow::Owned),
                };
                text.as_deref().and_then(|text| {
                    if text.len() > MAX_FILE_PATH_TEXT_BYTES {
                        return None;
                    }
                    let path = std::path::Path::new(text.trim());
                    (path.is_file() && little_clip_ops::is_text_file(path))
                        .then(|| path.to_path_buf())
                })
            };
            let (file_path, is_markdown) = if let Some(path) = file_path {
                let is_markdown = path.extension().is_some_and(|ext| {
                    ext.eq_ignore_ascii_case("md") || ext.eq_ignore_ascii_case("markdown")
                });
                (Some(path), is_markdown)
            } else {
                let is_markdown = match &item.payload {
                    ClipPayload::Markdown(_) => true,
                    ClipPayload::Text(text)
                        if text.len() <= MAX_INLINE_MARKDOWN_CLASSIFICATION_BYTES =>
                    {
                        text_preview::detect_markdown_text(text)
                    }
                    // Preview construction performs full classification on its
                    // worker. This value only chooses the inspector's button label.
                    ClipPayload::Text(_) => false,
                    _ => item
                        .to_clipboard_text()
                        .is_some_and(|text| text_preview::detect_markdown_text(&text)),
                };
                (None, is_markdown)
            };
            Some((*slot, file_path, is_markdown))
        });

        self.inspector_text_preview_cache = Some(InspectorTextPreviewCache {
            selected_slots: self.selected_slots.clone(),
            selected_item_ids,
            text_preview: text_preview.clone(),
            spreadsheet_slot,
        });
        text_preview
    }
}
