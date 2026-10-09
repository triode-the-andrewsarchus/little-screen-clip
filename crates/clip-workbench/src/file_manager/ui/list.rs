//! Mixed List rows reuse the dedicated text, music, and video previews.
use super::super::{filesystem::Entry, previews::EntryPreviews, video_timeline};
use super::*;

struct ListPreviews<'a> {
    files: &'a mut EntryPreviews,
    music: &'a mut EntryPreviews,
    videos: &'a mut video_timeline::Previews,
    selection: &'a mut Option<music::MusicRange>,
    playback: &'a super::super::music::Music,
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
    fn row(
        &mut self,
        ui: &mut Ui,
        rect: Rect,
        entry: &'a Entry,
        marked: bool,
        name_width: f32,
        colors: WorkbenchColors,
        palette: &AppPalette,
        actions: &mut ListActions,
    ) -> (egui::Response, bool) {
        let is_audio = entry.regular_file && little_clip_core::audio::is_audio_path(&entry.path);
        let mut response = if is_audio || entry.is_video() {
            media_row(ui, rect, marked, colors)
        } else {
            let lines = (!entry.directory)
                .then(|| self.files.listing(entry))
                .flatten()
                .and_then(|ready| ready.data.document.as_ref())
                .map(|doc| doc.lines);
            file_row(ui, rect, entry, marked, name_width, lines, colors)
        };
        let mut engaged = false;
        if is_audio {
            self.music_paths.insert(entry.path.clone());
            let ready = self.music.music(ui.ctx(), entry);
            let (active, time) = music::preview_row(
                ui,
                rect,
                entry,
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
                self.files,
                self.videos,
                self.decimation,
                self.can_request,
                colors,
            );
            self.hovered_video |= time.is_some();
            actions.video_click(ui, &response, entry, time, self.video_start);
        } else if !entry.directory && self.paths.len() < 128 {
            self.paths.insert(entry.path.clone());
            let line = self.files.show_listing(ui, rect, entry, colors, palette);
            if response.double_clicked() {
                *self.text_start = line.map(|line| (entry.path.clone(), line));
            }
        }
        (response, engaged)
    }
}

impl FileManager {
    pub(super) fn list_ui(&mut self, ui: &mut Ui, colors: WorkbenchColors, palette: &AppPalette) {
        let width = ui.available_width();
        let name_width = (width - 245.0).max(width * 0.4);
        let (header, _) = ui.allocate_exact_size(vec2(width, ROW_HEIGHT), Sense::hover());
        paint_text(
            ui,
            header,
            8.0,
            name_width - 10.0,
            "Name",
            colors.text_secondary,
        );
        paint_text(ui, header, name_width, 88.0, "Size", colors.text_secondary);
        paint_text(
            ui,
            header,
            name_width + 90.0,
            width - name_width - 90.0,
            "Modified",
            colors.text_secondary,
        );
        ui.separator();
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
            ui.label(if directory.entries.is_empty() {
                "This folder is empty."
            } else {
                "No visible items. Enable hidden files to see this folder's contents."
            });
        }
        let selected = self
            .index
            .and_then(|index| self.history.get(index))
            .map(|location| &location.selection);
        let mut actions = ListActions::default();
        let can_edit = self.can_edit();
        let selection_count = self.selection_count();
        let background = actions.background(ui, can_edit, selection_count > 0);
        let mut targets = Vec::new();
        let preview_paths = std::collections::BTreeSet::new();
        let mut previews = ListPreviews {
            files: &mut self.previews,
            music: &mut self.music_previews,
            videos: &mut self.video_previews,
            selection: &mut self.music_selection,
            playback: &self.music,
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
        let output = media_rows(ui, scroll, &self.list_offsets, |ui, index, rect| {
            let entry = &directory.entries[self.visible[index]];
            let marked = selected.is_some_and(|paths| paths.contains(&entry.path));
            let (response, engaged) = previews.row(
                ui,
                rect,
                entry,
                marked,
                name_width,
                colors,
                palette,
                &mut actions,
            );
            if entry.directory && ui.is_enabled() {
                targets.push((response.rect.intersect(ui.clip_rect()), entry.path.clone()));
            }
            if engaged {
                actions.selection = Some((entry.path.clone(), false, false));
            } else {
                actions.row(ui, &response, entry, selected, can_edit, selection_count);
            }
        });
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
        if let Some((path, time)) = play {
            if self.music.current() != Some(&path) {
                self.music.play(&path);
            }
            self.music.seek(time);
        }
    }
}
