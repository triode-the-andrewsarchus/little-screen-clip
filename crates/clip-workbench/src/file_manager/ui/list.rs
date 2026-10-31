//! Mixed List rows share image, text, music, video, MIDI, and PDF previews.
use super::super::{filesystem::Entry, previews::EntryPreviews, video_timeline};
use super::*;

struct ListPreviews<'a> {
    columns: ListColumns,
    metadata_only: bool,
    files: &'a mut EntryPreviews,
    music: &'a mut EntryPreviews,
    videos: &'a mut video_timeline::Previews,
    midi: &'a mut super::super::midi_strip::Preview,
    selection: &'a mut Option<music::MusicRange>,
    playback: &'a super::super::music::Music,
    pdf_start: &'a mut Option<(PathBuf, usize)>,
    text_start: &'a mut Option<(PathBuf, usize)>,
    video_start: &'a mut Option<(PathBuf, f64)>,
    decimation: u8,
    can_request: bool,
    paths: std::collections::BTreeSet<PathBuf>,
    music_paths: std::collections::BTreeSet<PathBuf>,
    visible_videos: Vec<&'a Entry>,
    hovered_video: bool,
    play: Option<(PathBuf, f32)>,
}
impl<'a> ListPreviews<'a> {
    fn response(
        &mut self,
        ui: &mut Ui,
        rect: Rect,
        entry: &Entry,
        marked: bool,
        colors: WorkbenchColors,
    ) -> egui::Response {
        let response = if entry.system.is_some()
            || entry.is_playable_media()
            || entry.is_scene()
            || entry.is_blend()
            || entry.is_photo()
            || entry.is_material()
            || (entry.regular_file && super::super::font_strip::known(&entry.path))
            || self.files.is_binary(entry)
        {
            media_row(ui, rect, marked, colors)
        } else {
            let lines = (!entry.directory)
                .then(|| self.files.listing(entry))
                .flatten()
                .and_then(|ready| {
                    ready
                        .data
                        .document
                        .as_ref()
                        .map(|doc| format!("{} lines", doc.lines))
                        .or_else(|| {
                            ready
                                .data
                                .pdf_info
                                .as_ref()
                                .map(|info| format!("{} pages", info.page_count))
                        })
                });
            file_row(ui, rect, entry, marked, self.columns, lines, colors)
        };
        response
    }
    fn row(
        &mut self,
        ui: &mut Ui,
        rect: Rect,
        entry: &'a Entry,
        marked: bool,
        colors: WorkbenchColors,
        palette: &AppPalette,
        actions: &mut ListActions,
    ) -> (egui::Response, bool) {
        if self.metadata_only {
            return (
                file_row(ui, rect, entry, marked, self.columns, None, colors),
                false,
            );
        }
        let is_audio = entry.regular_file && little_clip_core::audio::is_audio_path(&entry.path);
        let mut response = self.response(ui, rect, entry, marked, colors);
        let mut engaged = false;
        if entry.regular_file && super::super::midi_strip::is_midi(&entry.path) {
            self.paths.insert(entry.path.clone());
            engaged = super::super::midi_strip::show(
                ui,
                rect,
                entry,
                Some(self.columns),
                self.files,
                self.midi,
                &mut response,
                colors,
                palette,
            );
        } else if is_audio {
            self.music_paths.insert(entry.path.clone());
            let ready = self.music.music(ui.ctx(), entry);
            let (active, time) = music::preview_row(
                ui,
                rect,
                entry,
                Some(self.columns),
                ready,
                &mut response,
                self.selection,
                self.playback,
                colors,
            );
            engaged = active;
            if let Some(time) = time {
                self.play = Some((entry.path.clone(), time));
            }
        } else if entry.is_video() {
            self.visible_videos.push(entry);
            self.paths.insert(entry.path.clone());
            let time = video::preview_row(
                ui,
                rect,
                entry,
                Some(self.columns),
                self.files,
                self.videos,
                self.decimation,
                self.can_request,
                colors,
            );
            self.hovered_video |= time.is_some();
            actions.video_click(ui, &response, entry, time, self.video_start);
        } else {
            self.passive_row(ui, rect, entry, &response, colors, palette);
        }
        (response, engaged)
    }
    fn passive_row(
        &mut self,
        ui: &mut Ui,
        rect: Rect,
        entry: &Entry,
        response: &egui::Response,
        colors: WorkbenchColors,
        palette: &AppPalette,
    ) {
        if entry.system.is_some() {
            self.paths.insert(entry.path.clone());
            self.files
                .show_visual(ui, rect, entry, Some(self.columns), colors, palette);
        } else if entry.is_scene() {
            self.paths.insert(entry.path.clone());
            self.files
                .show_scene(ui, rect, entry, Some(self.columns), colors, palette);
        } else if entry.is_photo()
            || entry.is_material()
            || entry.is_blend()
            || (entry.regular_file && super::super::font_strip::known(&entry.path))
        {
            self.paths.insert(entry.path.clone());
            self.files
                .show_visual(ui, rect, entry, Some(self.columns), colors, palette);
        } else if entry.regular_file
            && (little_clip_core::pdf::is_pdf_path(&entry.path)
                || little_clip_core::document::is_office_document(&entry.path))
        {
            self.paths.insert(entry.path.clone());
            let page = self.files.show_pdf(ui, rect, entry, colors);
            if response.double_clicked() {
                *self.pdf_start = Some((entry.path.clone(), page.unwrap_or(1)));
            }
        } else if !entry.directory && self.paths.len() < 128 {
            self.paths.insert(entry.path.clone());
            let line =
                self.files
                    .show_listing(ui, rect, entry, Some(self.columns), colors, palette);
            if response.double_clicked() {
                *self.text_start = line.map(|line| (entry.path.clone(), line));
            }
        }
    }
}

impl FileManager {
    pub(super) fn list_ui(&mut self, ui: &mut Ui, colors: WorkbenchColors, palette: &AppPalette) {
        let count = self.search.answers.count();
        if count == 0 {
            self.list_contents(ui, colors, palette);
            return;
        }
        egui::ScrollArea::horizontal()
            .id_source("answer_columns_horizontal")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let extras = count + usize::from(self.search.images.active());
                ui.set_min_width((600.0 + 145.0 * extras as f32).max(ui.available_width()));
                self.list_contents(ui, colors, palette);
            });
    }
    fn list_header(&mut self, ui: &mut Ui, colors: WorkbenchColors) -> ListColumns {
        let width = ui.available_width();
        let (header, _) = ui.allocate_exact_size(vec2(width, ROW_HEIGHT), Sense::hover());
        let columns = self.search_columns(ui, header, colors);
        let sort = (self
            .search
            .answers
            .current()
            .is_none_or(|folder| folder.sort.is_none())
            && !self
                .search
                .applied
                .as_ref()
                .is_some_and(|query| query.browser_sort().is_none()))
        .then_some(self.sort);
        if let Some(column) = columns.sort_header(ui, header, sort, colors) {
            self.sort_by(column);
            self.focus_list = true;
        }
        ui.separator();
        columns
    }
    fn list_contents(&mut self, ui: &mut Ui, colors: WorkbenchColors, palette: &AppPalette) {
        let columns = self.list_header(ui, colors);
        if self.previews.poll(ui.ctx()) {
            self.rebuild_list_offsets();
        }
        self.music_previews.poll(ui.ctx());
        self.video_previews.poll(ui.ctx());
        ui.interact(
            ui.available_rect_before_wrap(),
            music::waveform_id(),
            Sense::focusable_noninteractive(),
        );
        ui.interact(
            ui.available_rect_before_wrap(),
            super::super::midi_strip::focus_id(),
            Sense::focusable_noninteractive(),
        );
        if let Some(message) = self.midi.notice.message(ui.input(|input| input.time)) {
            ui.label(message);
        }
        let scroll = self.list_scroll(ROW_HEIGHT);
        let Some(directory) = &self.directory else {
            ui.label(if self.pending.is_some() {
                "Loading folder…"
            } else {
                "Choose a folder or enter a path."
            });
            return;
        };
        if self.visible.is_empty() {
            ui.label(if self.search.applied.is_some() {
                "No matching files."
            } else if directory.entries.is_empty() {
                "This folder is empty."
            } else {
                "No visible items. Enable hidden files to see this folder's contents."
            });
        }
        let selected = self
            .index
            .and_then(|index| self.history.get(index))
            .map(|location| &location.selection);
        let mut actions = ListActions {
            search_results: self.search.has_result_sources(),
            metadata_only: self.search.has_offline_results(),
            ..Default::default()
        };
        let can_edit = self.can_edit();
        let selection_count = self.selection_count();
        let background = actions.background(ui, can_edit, selection_count > 0);
        let mut targets = Vec::new();
        let preview_paths = std::collections::BTreeSet::new();
        let mut previews = ListPreviews {
            columns,
            midi: &mut self.midi,
            metadata_only: self.search.has_offline_results(),
            files: &mut self.previews,
            music: &mut self.music_previews,
            videos: &mut self.video_previews,
            selection: &mut self.music_selection,
            playback: &self.music,
            pdf_start: &mut self.pdf_start,
            text_start: &mut self.text_start,
            video_start: &mut self.video_start,
            decimation: self.video_timeline_decimation,
            can_request: self.pending.is_none(),
            paths: preview_paths,
            music_paths: Default::default(),
            visible_videos: Vec::new(),
            hovered_video: false,
            play: None,
        };
        let mut hash_priority = Vec::new();
        let search = &self.search;
        let images = &self.search.images;
        let answers = &self.search.answers;
        let mut answer_priority = Vec::new();
        let output = media_rows(ui, scroll, &self.list_offsets, |ui, index, rect| {
            let entry = &directory.entries[self.visible[index]];
            let marked = selected.is_some_and(|paths| paths.contains(&entry.path));
            let (rect, detail) = search.result_row(ui, rect, entry, colors);
            let (mut response, engaged) =
                previews.row(ui, rect, entry, marked, colors, palette, &mut actions);
            if let Some(detail) = detail {
                response = response.union(detail);
            }
            hash_priority.push(entry.path.clone());
            images.paint_cell(ui, columns, rect, entry, colors);
            answers.paint_cells(
                ui,
                columns,
                rect,
                entry,
                usize::from(images.active()),
                colors,
            );
            answer_priority.push(self.visible[index]);
            if entry.directory && ui.is_enabled() {
                targets.push((response.rect.intersect(ui.clip_rect()), entry.path.clone()));
            }
            if engaged {
                actions.selection = Some((entry.path.clone(), false, false));
            } else if search.is_offline(&entry.path) {
                actions.unavailable_row(ui, &response, entry, selected);
            } else {
                actions.row(ui, &response, entry, selected, can_edit, selection_count);
            }
        });
        self.search.images.prioritize(hash_priority);
        self.search.answers.priority = answer_priority;
        if previews.paths.is_empty() {
            previews.files.retire();
        } else {
            previews.files.retain(&previews.paths);
        }
        previews.music.retain(&previews.music_paths);
        previews.videos.retain(&previews.visible_videos);
        if !previews.hovered_video {
            previews.videos.clear_hover();
        }
        let play = previews.play.take();
        drop(previews);
        self.drop_targets.extend(targets);
        self.finish_listing(ui, actions, output.state.offset.y, &background);
        if let Some((path, revision, document, time)) = self.midi.queued.take() {
            self.music.stop();
            self.midi.play(path, revision, document, time, None);
        }
        if let Some((path, time)) = play {
            self.midi.stop();
            let guard = self.search_source_guard(&path);
            self.music.play(&path, guard);
            self.music.seek(time);
        }
    }
}
