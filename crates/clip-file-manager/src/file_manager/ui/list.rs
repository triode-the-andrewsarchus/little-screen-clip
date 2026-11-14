//! Mixed List rows share image, text, music, video, MIDI, and PDF previews.
use super::super::{filesystem::Entry, previews::EntryPreviews, video_timeline};
use super::*;

struct ListPreviews<'a> {
    columns: ListColumns,
    selected: Option<&'a std::collections::BTreeSet<PathBuf>>,
    can_edit: bool,
    selection_count: usize,
    child_targets: Vec<(Rect, PathBuf)>,
    containers: &'a mut super::super::container_preview::Previews,
    container_paths: std::collections::BTreeSet<PathBuf>,
    hidden: bool,
    metadata_only: bool,
    files: &'a mut EntryPreviews,
    music: &'a mut EntryPreviews,
    videos: &'a mut video_timeline::Previews,
    midi: &'a mut super::super::midi_strip::Preview,
    selection: &'a mut Option<music::MusicRange>,
    playback: &'a super::super::music::Music,
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
        if entry.system.is_some()
            || entry.has_content_strip()
            || self.files.is_binary(entry)
            || self.files.is_multisample(entry)
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
        }
    }
    #[expect(
        clippy::too_many_arguments,
        reason = "Drawing keeps target, geometry, content and palette explicit at the rendering boundary"
    )]
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
        if !actions.context.active() && super::super::container_preview::candidate(entry) {
            return (
                self.container_row(ui, rect, entry, marked, colors, palette, actions),
                false,
            );
        }
        if self.files.is_multisample(entry)
            && preview_hovered(ui, rect)
            && ui.input(|i| i.pointer.any_pressed())
        {
            self.files.multisample.focus(entry);
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
            let position = self.passive_row(ui, rect, entry, colors, palette);
            if response.double_clicked() {
                actions.open_position = position;
            }
        }
        (response, engaged)
    }
    #[expect(
        clippy::too_many_arguments,
        reason = "Shared row painting boundary keeps selection and palette explicit"
    )]
    fn container_row(
        &mut self,
        ui: &mut Ui,
        rect: Rect,
        entry: &Entry,
        marked: bool,
        colors: WorkbenchColors,
        palette: &AppPalette,
        actions: &mut ListActions,
    ) -> egui::Response {
        self.container_paths.insert(entry.path.clone());
        if self.can_request {
            self.containers.request(entry);
        }
        let header = Rect::from_min_max(rect.min, pos2(rect.right(), rect.top() + 28.0));
        let response = file_row(ui, header, entry, marked, self.columns, None, colors);
        self.containers.show(
            ui,
            rect,
            entry,
            self.files,
            &mut self.paths,
            self.hidden,
            colors,
            palette,
            self.selected,
            self.can_request,
            |ui, child, target| {
                let response = match target {
                    super::super::container_preview::Target::Member(response, ordinal) => {
                        if response.clicked() {
                            actions.selection = Some((child.path.clone(), false, false));
                            actions.open = Some(child.path.clone());
                            actions.open_position =
                                Some(super::super::DocumentPosition::ArchiveMember(
                                    ordinal,
                                    child.revision,
                                ));
                        }
                        return;
                    }
                    super::super::container_preview::Target::Enter(response) => {
                        if child.directory && ui.is_enabled() {
                            self.child_targets.push((
                                response.rect.intersect(ui.clip_rect()),
                                child.path.clone(),
                            ));
                        }
                        actions.row(
                            ui,
                            response,
                            child,
                            self.selected,
                            self.can_edit,
                            self.selection_count,
                        );
                        if response.clicked() {
                            actions.open = Some(child.path.clone());
                        }
                        return;
                    }
                    super::super::container_preview::Target::Card(response) => response.clone(),
                };
                if child.directory && ui.is_enabled() {
                    self.child_targets
                        .push((response.rect.intersect(ui.clip_rect()), child.path.clone()));
                }
                actions.row(
                    ui,
                    &response,
                    child,
                    self.selected,
                    self.can_edit,
                    self.selection_count,
                );
            },
        );
        response
    }
    fn passive_row(
        &mut self,
        ui: &mut Ui,
        rect: Rect,
        entry: &Entry,
        colors: WorkbenchColors,
        palette: &AppPalette,
    ) -> Option<super::super::DocumentPosition> {
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
        } else if !entry.directory && self.paths.len() < 128 {
            self.paths.insert(entry.path.clone());
            return self
                .files
                .show_document(ui, rect, entry, self.columns, colors, palette);
        }
        None
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
        let sort = self.list_sort();
        if let Some(column) = columns.sort_header(ui, header, sort, colors) {
            self.sort_by(column);
            self.focus_list = true;
        }
        ui.separator();
        columns
    }
    fn list_contents(&mut self, ui: &mut Ui, colors: WorkbenchColors, palette: &AppPalette) {
        self.tick_metadata(ui.ctx());
        let columns = self.list_header(ui, colors);
        let anchor = self.list_anchor();
        if self.containers.poll(ui.ctx()) | self.previews.poll(ui.ctx()) {
            self.reconcile_selection();
            self.rebuild_list_offsets();
            self.restore_list_anchor(anchor);
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
        self.midi.render_feedback(ui, palette);
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
        let mut actions = ListActions::for_browser(self);
        let can_edit = self.can_edit();
        let selection_count = self.selection_count();
        let background = actions.background(ui, can_edit, selection_count > 0);
        let mut targets = Vec::new();
        let preview_paths = std::collections::BTreeSet::new();
        self.containers.protect(selected);
        let sort = self.list_sort();
        let mut previews = ListPreviews {
            selected,
            can_edit,
            selection_count,
            child_targets: Vec::new(),
            containers: &mut self.containers,
            container_paths: Default::default(),
            hidden: self.show_hidden,
            columns,
            midi: &mut self.midi,
            metadata_only: self.search.has_offline_results(),
            files: &mut self.previews,
            music: &mut self.music_previews,
            videos: &mut self.video_previews,
            selection: &mut self.music_selection,
            playback: &self.music,
            video_start: &mut self.video_start,
            decimation: self.video_timeline_decimation,
            can_request: self.pending.is_none(),
            paths: preview_paths,
            music_paths: Default::default(),
            visible_videos: Vec::new(),
            hovered_video: false,
            play: None,
        };
        let mut group_sort = None;
        let mut hash_priority = Vec::new();
        let metadata = &self.metadata;
        let search = &self.search;
        let images = &self.search.images;
        let answers = &self.search.answers;
        let mut answer_priority = Vec::new();
        let output = media_rows(ui, scroll, &self.list_offsets, |ui, index, rect| {
            let family = sort.and_then(|sort| sort.group_at(directory, &self.visible, index));
            let entry = &directory.entries[self.visible[index]];
            let columns = columns.for_entry(sort, entry);
            previews.columns = columns;
            let (rect, clicked) = group_header(ui, rect, family, columns, sort, metadata, colors);
            group_sort = clicked.or(group_sort);
            let marked = selected.is_some_and(|paths| paths.contains(&entry.path));
            let (rect, detail) = search.result_row(ui, rect, entry, colors);
            let (mut response, engaged) =
                previews.row(ui, rect, entry, marked, colors, palette, &mut actions);
            if let Some(detail) = detail {
                response = response.union(detail);
            }
            metadata.paint_cells(ui, columns, rect, entry, colors);
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
        previews
            .containers
            .finish_frame(&mut previews.container_paths);
        let play = previews.play.take();
        targets.append(&mut previews.child_targets);
        drop(previews);
        self.drop_targets.extend(targets);
        self.finish_listing(ui, actions, output.state.offset.y, &background);
        self.list_audio(play);
        if let Some(column) = group_sort {
            column.apply(self);
            self.focus_list = true;
        }
    }

    fn list_audio(&mut self, play: Option<(PathBuf, f32)>) {
        if std::mem::take(&mut self.previews.multisample.started) {
            self.music.stop();
            self.midi.stop();
        }
        if let Some((path, revision, document, time)) = self.midi.queued.take() {
            self.previews.multisample.clear();
            self.music.stop();
            self.midi.play(path, revision, document, time, None);
        }
        if let Some((path, time)) = play {
            self.previews.multisample.clear();
            self.midi.stop();
            let guard = self.search_source_guard(&path);
            self.music.play(&path, guard);
            self.music.seek(time);
        }
    }
}

#[derive(Clone, Copy)]
enum GroupSort {
    File(super::super::sorting::Column),
    Metadata(
        super::super::sorting::Category,
        super::super::metadata::Field,
    ),
}
impl GroupSort {
    fn apply(self, manager: &mut FileManager) {
        match self {
            Self::File(column) => manager.sort_by(column),
            Self::Metadata(family, field) => {
                manager.metadata.toggle_sort(family, field);
                manager.finish_sort();
            }
        }
    }
}
fn group_header(
    ui: &mut Ui,
    rect: Rect,
    family: Option<super::super::sorting::Category>,
    columns: ListColumns,
    sort: Option<super::super::sorting::Sort>,
    metadata: &super::super::metadata::Metadata,
    colors: WorkbenchColors,
) -> (Rect, Option<GroupSort>) {
    let Some(family) = family else {
        return (rect, None);
    };
    let content = Rect::from_min_max(
        pos2(rect.left(), rect.top() + GROUP_HEADER_HEIGHT),
        rect.max,
    );
    let rect = Rect::from_min_max(rect.min, pos2(rect.right(), content.top()));
    ui.painter().rect_filled(rect, 0.0, colors.recessed);
    let title = Rect::from_min_max(rect.min, pos2(rect.right(), rect.top() + 24.0));
    let label = if !super::super::metadata::Field::for_family(family).is_empty() {
        format!("{} · {}", family.label(), metadata.status(family))
    } else {
        family.label().into()
    };
    paint_text(
        ui,
        title,
        8.0,
        title.width() - 16.0,
        &label,
        colors.text_primary,
    );
    let header = Rect::from_min_max(pos2(rect.left(), title.bottom()), rect.max);
    let clicked = ui
        .push_id(("family_header", family), |ui| {
            let file = columns
                .sort_header(ui, header, sort, colors)
                .map(GroupSort::File);
            metadata
                .paint_header(ui, columns, header, family, colors)
                .map(|field| GroupSort::Metadata(family, field))
                .or(file)
        })
        .inner;
    (content, clicked)
}
