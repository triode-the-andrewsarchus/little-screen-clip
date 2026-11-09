//! Files owns navigation and naming; studios keep their editing and playback state.
use super::*;
use crate::shell::file_editor::FileEditor;

impl Viewers {
    pub(in crate::file_manager) fn menu_controls(&mut self, ui: &mut Ui) {
        let ready = !self.confirm_close
            && !self.audio_return_saving
            && !self.collection_pending()
            && !self.collection_modal()
            && !self.rename.open
            && !self.rename.busy()
            && !self.writes_pending();
        // The menu is laid out from the right edge inward.
        if let Some(collection) = &mut self.gallery {
            let active = match &self.session {
                Some(ViewerSession::Image(state)) => Some(state),
                _ => None,
            };
            collection.export_controls(ui, active, ready && self.loading.is_none());
        }
        if ui
            .add_enabled(ready, egui::Button::new("Back to folder"))
            .clicked()
        {
            self.close_requested = true;
        }
    }
    pub(in crate::file_manager) fn take_file_renames(&mut self) -> Vec<(PathBuf, PathBuf)> {
        std::mem::take(&mut self.renamed)
    }
    pub(super) fn writes_pending(&self) -> bool {
        match &self.session {
            Some(ViewerSession::Image(state)) => state.file_write_pending(),
            Some(ViewerSession::Audio(state)) => state.file_write_pending(),
            Some(ViewerSession::Text(state)) => state.busy(),
            Some(ViewerSession::Pdf(state)) => state.file_write_pending(),
            Some(ViewerSession::Midi(state)) => state.export_pending(),
            _ => false,
        }
    }
    pub(super) fn session_busy(&self) -> bool {
        self.writes_pending()
            || match &self.session {
                Some(ViewerSession::Audio(state)) => state.is_loading_analysis,
                Some(ViewerSession::Model(state)) => state.rx.is_some(),
                Some(ViewerSession::Scene(state)) => state.file_load_pending(),
                _ => false,
            }
    }
    pub(super) fn session_dirty(&self) -> bool {
        match &self.session {
            Some(ViewerSession::Image(state)) => state.has_unsaved_changes(),
            Some(ViewerSession::Audio(state)) => self.pad_audio || state.has_session_edits(),
            Some(ViewerSession::Text(state)) => state.dirty(),
            _ => false,
        }
    }
    fn editor_source(&self) -> Option<FileSource> {
        match &self.session {
            Some(ViewerSession::Image(state)) => state.file_source(),
            Some(ViewerSession::Audio(state)) => state.file_source(),
            Some(ViewerSession::Text(state)) => state.file_source(),
            Some(ViewerSession::Video(state)) => Some(state.file_source()),
            _ => None,
        }
    }
    pub(super) fn poll_file_source(&mut self, ctx: &Context) {
        if let Some((path, receiver)) = &self.source_request {
            match receiver.try_recv() {
                Err(mpsc::TryRecvError::Empty) => {
                    ctx.request_repaint_after(Duration::from_millis(40));
                    return;
                }
                result => {
                    if self.path.as_ref() == Some(path) {
                        match result
                            .map_err(|e| e.to_string())
                            .and_then(|r| r.map_err(|e| e.to_string()))
                            .and_then(|r| r)
                        {
                            Ok(source) => {
                                self.source = Some(source);
                                self.source_path = Some(path.clone());
                            }
                            Err(error) => {
                                self.error = Some(format!("File identity unavailable: {error}"))
                            }
                        }
                    }
                    self.source_request = None;
                }
            }
        }
        if self.editor_source().is_some()
            || self.office_preview
            || matches!(self.session, Some(ViewerSession::System(_)))
        {
            return;
        }
        let Some(path) = self.path.as_ref() else {
            return;
        };
        if self
            .source_path
            .as_ref()
            .is_some_and(|source_path| source_path != path)
        {
            let target = path.clone();
            self.source = None;
            let (_, receiver) = BackgroundJobs::global()
                .submit(Duration::from_secs(30), move || {
                    FileSource::capture(&target)
                });
            self.source_request = Some((path.clone(), receiver));
            ctx.request_repaint_after(Duration::from_millis(40));
        }
    }
    pub(super) fn render_file_session(
        &mut self,
        ui: &mut Ui,
        palette: &AppPalette,
        close: &mut bool,
    ) {
        let source = self.editor_source().or_else(|| self.source.clone());
        let can_rename = source.is_some()
            && !self.session_busy()
            && !self.rename.open
            && !self.rename.busy()
            && !self.confirm_close
            && !self.collection_modal()
            && !self.collection_pending();
        let chrome = FileEditor {
            path: self.path.clone().unwrap_or_default(),
            rename_enabled: can_rename,
            rename_clicked: false,
            suspend_keyboard: !ui.is_enabled(),
            photo: self
                .gallery
                .as_ref()
                .map(|collection| collection.footer(can_rename)),
        };
        let ctx = ui.ctx().clone();
        let (_, chrome) = chrome.scope(&ctx, || self.render_session(ui, palette, close));
        if let Some(photo) = chrome.photo.filter(|photo| photo.changed)
            && let Some(collection) = &mut self.gallery
        {
            collection.set_rating(photo.rating);
        }
        if chrome.rename_clicked
            && let (Some(path), Some(source)) = (
                self.path.clone(),
                self.editor_source().or_else(|| self.source.clone()),
            )
        {
            self.rename.begin(path, source);
        }
    }
    pub(super) fn rename_ui(&mut self, ui: &mut Ui) {
        if let Some((old, result)) = self.rename.ui(ui) {
            ui.ctx().request_repaint();
            match result {
                Ok((path, source)) => {
                    if let Some(viewer) = &mut self.session {
                        relocate(viewer, &old, &path, source.clone());
                    }
                    if let Some(collection) = &mut self.gallery {
                        collection.relocate(&old, &path);
                    }
                    self.path = Some(path.clone());
                    self.source_path = Some(path.clone());
                    self.source = Some(source);
                    self.renamed.push((old, path));
                    self.error = None;
                }
                Err(error) => self.error = Some(format!("Rename: {error}")),
            }
        }
    }
}
fn relocate(viewer: &mut ViewerSession, old: &Path, path: &Path, source: FileSource) {
    let name = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    match viewer {
        ViewerSession::Image(state) => state.relocate_file(path.into(), source),
        ViewerSession::Audio(state) => state.relocate_file(path.into(), source),
        ViewerSession::Text(state) => state.relocate_file(path.into(), source),
        ViewerSession::Pdf(state) => {
            if state.path == old {
                state.relocate_file(path.into());
            }
            state.title = name;
        }
        ViewerSession::Sheet(state) => {
            state.path = Some(path.into());
            state.title = name;
        }
        ViewerSession::Midi(state) => {
            state.path = Some(path.into());
            state.title = name;
        }
        ViewerSession::Model(state) => {
            state.path = Some(path.into());
            state.title = name;
        }
        ViewerSession::Scene(state) => state.relocate_file(path.into()),
        ViewerSession::Video(state) => state.relocate_file(path.into(), source),
        ViewerSession::System(_)
        | ViewerSession::Font(_)
        | ViewerSession::Notebook(_)
        | ViewerSession::Mail(_) => {}
        ViewerSession::Book(state) => state.relocate_file(source),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn renaming_text_keeps_the_preview_and_updates_its_save_guard() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("draft.txt");
        std::fs::write(&path, "original").unwrap();
        let config = AppConfig::default();
        let mut viewer = session(
            load(&path, &config).unwrap(),
            path.clone(),
            &config,
            0.0,
            crate::file_source::ReadCheck::default(),
        );
        let source = FileSource::capture(&path).unwrap();
        let (destination, source) =
            super::super::super::edits::rename_file(&path, "renamed.txt", &source).unwrap();
        relocate(&mut viewer, &path, &destination, source.clone());
        let ViewerSession::Text(state) = viewer else {
            panic!()
        };
        assert_eq!(state.path(), destination);
        assert_eq!(state.file_source(), Some(source));
        assert_eq!(state.preview.content.as_str(), "original");
    }
    #[test]
    fn rename_preserves_audio_session_settings_and_rebinds_source() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audio.wav");
        std::fs::write(&path, "fixture").unwrap();
        let source = FileSource::capture(&path).unwrap();
        let mut state = AudioPreviewState::new(path.clone(), "audio.wav".into())
            .with_file_source(source.clone());
        state.selection = Some((0.2, 0.8));
        let mut viewer = ViewerSession::Audio(state);
        let (destination, source) =
            super::super::super::edits::rename_file(&path, "renamed.wav", &source).unwrap();
        relocate(&mut viewer, &path, &destination, source.clone());
        let ViewerSession::Audio(state) = viewer else {
            panic!()
        };
        assert_eq!(state.path, destination);
        assert_eq!(state.selection, Some((0.2, 0.8)));
        assert_eq!(state.file_source(), Some(source));
    }
}
