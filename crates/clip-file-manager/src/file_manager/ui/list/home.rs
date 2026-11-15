//! Home folder panes borrow the ordinary list renderer and its one media session.
use super::super::home::cards::Action;
use super::*;
use crate::file_manager::home::{
    Card, Content,
    folders::{Options, Presentation, Snapshot},
};
use std::sync::Arc;
use uuid::Uuid;

pub(in crate::file_manager::ui) struct Frame {
    pub card: Uuid,
    pub snapshot: Option<Arc<Snapshot>>,
    pub error: Option<String>,
}
pub(in crate::file_manager::ui) struct Rows<'a> {
    previews: ListPreviews<'a>,
    frames: &'a [Frame],
    enabled: bool,
    arrange: bool,
    scale: f32,
    filters: &'a mut std::collections::BTreeSet<Uuid>,
    palette: &'a AppPalette,
}
impl FileManager {
    pub(in crate::file_manager::ui) fn with_home_rows<'a, R>(
        &'a mut self,
        ui: &mut Ui,
        frames: &'a [Frame],
        filters: &'a mut std::collections::BTreeSet<Uuid>,
        renderer: &super::super::home::cards::Renderer<'a, '_>,
        paint: impl FnOnce(&mut Ui, &mut Rows<'a>) -> R,
    ) -> (R, Option<(PathBuf, f32)>) {
        self.previews.poll(ui.ctx());
        self.music_previews.poll(ui.ctx());
        self.video_previews.poll(ui.ctx());
        ui.interact(
            ui.available_rect_before_wrap(),
            music::waveform_id(),
            Sense::focusable_noninteractive(),
        );
        ui.interact(
            ui.available_rect_before_wrap(),
            super::super::super::midi_strip::focus_id(),
            Sense::focusable_noninteractive(),
        );
        let previews = ListPreviews {
            columns: ListColumns::new(ui.available_rect_before_wrap()),
            selected: None,
            can_edit: false,
            selection_count: 0,
            child_targets: Vec::new(),
            containers: &mut self.containers,
            container_paths: Default::default(),
            inline_containers: false,
            hidden: false,
            metadata_only: false,
            files: &mut self.previews,
            music: &mut self.music_previews,
            videos: &mut self.video_previews,
            midi: &mut self.midi,
            selection: &mut self.music_selection,
            playback: &self.music,
            video_start: &mut self.video_start,
            decimation: self.video_timeline_decimation,
            can_request: self.pending.is_none(),
            paths: Default::default(),
            music_paths: Default::default(),
            visible_videos: Vec::new(),
            hovered_video: false,
            play: None,
        };
        let mut rows = Rows {
            previews,
            frames,
            enabled: renderer.enabled,
            arrange: renderer.arrange,
            scale: renderer.viewport.scale() as f32,
            filters,
            palette: renderer.palette,
        };
        let result = paint(ui, &mut rows);
        if rows.previews.paths.is_empty() {
            rows.previews.files.retire();
        } else {
            rows.previews.files.retain(&rows.previews.paths);
        }
        rows.previews.music.retain(&rows.previews.music_paths);
        rows.previews.videos.retain(&rows.previews.visible_videos);
        if !rows.previews.hovered_video {
            rows.previews.videos.clear_hover();
        }
        let play = rows.previews.play.take();
        drop(rows);
        (result, play)
    }
}
impl<'a> Rows<'a> {
    pub(in crate::file_manager::ui) fn folder(
        &mut self,
        ui: &mut Ui,
        card: &Card,
        rect: Rect,
        actions: &mut Vec<Action>,
    ) {
        let Content::Folder { pin, options, .. } = &card.content else {
            return;
        };
        let body = Rect::from_min_max(
            rect.min + vec2(10.0, 52.0) * self.scale,
            rect.max - vec2(10.0, 10.0) * self.scale,
        );
        let clipped = body.intersect(ui.clip_rect());
        if clipped.width() < 120.0 || clipped.height() < 50.0 {
            return;
        }
        let frames = self.frames;
        let Some(frame) = frames.iter().find(|frame| frame.card == card.id) else {
            return;
        };
        let mut live = ui.child_ui(body, egui::Layout::top_down(egui::Align::Min));
        live.set_clip_rect(clipped);
        let mut options = options.clone();
        live.push_id(("home_folder", card.id), |ui| {
            self.controls(ui, card.id, *pin, &mut options, actions);
            self.contents(ui, frame, &options, actions);
        });
        if let Content::Folder { options: old, .. } = &card.content
            && *old != options
        {
            actions.push(Action::FolderOptions(card.id, options));
        }
    }
    fn controls(
        &mut self,
        ui: &mut Ui,
        id: Uuid,
        pin: Uuid,
        options: &mut Options,
        actions: &mut Vec<Action>,
    ) {
        ui.add_enabled_ui(self.enabled, |ui| {
            ui.horizontal_wrapped(|ui| {
                if ui.button("Open folder").clicked() {
                    actions.push(Action::Open(pin));
                }
                if ui
                    .selectable_label(options.presentation == Presentation::List, "List")
                    .clicked()
                {
                    options.presentation = Presentation::List;
                }
                if ui
                    .selectable_label(options.presentation == Presentation::Icons, "Icons")
                    .clicked()
                {
                    options.presentation = Presentation::Icons;
                }
                if ui.button("Filter").clicked() && !self.filters.remove(&id) {
                    self.filters.insert(id);
                }
                if ui.small_button("Refresh").clicked() {
                    actions.push(Action::RefreshFolder(pin));
                }
            });
            if self.filters.contains(&id) {
                filter_controls(ui, options);
            }
        });
    }
    fn contents(
        &mut self,
        ui: &mut Ui,
        frame: &'a Frame,
        options: &Options,
        actions: &mut Vec<Action>,
    ) {
        if let Some(error) = &frame.error {
            ui.colored_label(ui.visuals().error_fg_color, error);
            return;
        }
        let Some(snapshot) = &frame.snapshot else {
            ui.spinner();
            ui.weak("Reading folder…");
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(50));
            return;
        };
        let now = std::time::SystemTime::now();
        let entries: Vec<_> = snapshot
            .directory
            .entries
            .iter()
            .filter(|entry| {
                snapshot
                    .visibility
                    .visible(entry, options.show_hidden, false)
                    && options.matches(entry, now)
            })
            .collect();
        ui.weak(format!(
            "{} matching {}{}",
            entries.len(),
            if entries.len() == 1 { "item" } else { "items" },
            if snapshot.directory.skipped > 0 {
                " · partial listing; open folder for full view"
            } else {
                ""
            }
        ));
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_secs(1));
        if entries.is_empty() {
            ui.weak(if snapshot.directory.skipped > 0 {
                "No matches in this partial listing. Open folder for the full view."
            } else {
                "No files match this pane's filter."
            });
            return;
        }
        ui.add_enabled_ui(self.enabled && !self.arrange, |ui| {
            match options.presentation {
                Presentation::List => self.list(ui, &entries, actions),
                Presentation::Icons => icon_grid(ui, &entries, actions),
            }
        });
    }
    fn list(&mut self, ui: &mut Ui, entries: &[&'a Entry], actions: &mut Vec<Action>) {
        let colors = WorkbenchColors::from_palette(self.palette);
        let mut row_actions = ListActions::default();
        egui::ScrollArea::vertical()
            .id_source("files")
            .min_scrolled_height(0.0)
            .max_height(ui.available_height().max(0.0))
            .show_rows(ui, 110.0, entries.len(), |ui, range| {
                for index in range {
                    let entry = entries[index];
                    let (rect, _) =
                        ui.allocate_exact_size(vec2(ui.available_width(), 110.0), Sense::hover());
                    self.previews.columns = ListColumns::new(rect);
                    let (response, _) = self.previews.row(
                        ui,
                        rect,
                        entry,
                        false,
                        colors,
                        self.palette,
                        &mut row_actions,
                    );
                    if response.double_clicked() {
                        open(entry, actions);
                    }
                    if let Some(path) = row_actions.open.take() {
                        actions.push(Action::OpenDocument(path));
                    }
                }
            });
    }
}
fn filter_controls(ui: &mut Ui, options: &mut Options) {
    ui.horizontal_wrapped(|ui| {
        ui.label("Name contains");
        ui.add(
            egui::TextEdit::singleline(&mut options.name)
                .desired_width(140.0)
                .char_limit(64),
        );
        ui.label("Extensions");
        ui.add(
            egui::TextEdit::singleline(&mut options.extensions)
                .desired_width(120.0)
                .char_limit(128)
                .hint_text("jpg, png"),
        );
    });
    ui.horizontal_wrapped(|ui| {
        ui.label("Modified within (minutes)");
        ui.add(egui::DragValue::new(&mut options.recent_minutes).clamp_range(0..=5_256_000));
        if ui.small_button("Last hour").clicked() {
            options.recent_minutes = 60;
        }
        if ui.small_button("Any time").clicked() {
            options.recent_minutes = 0;
        }
        ui.checkbox(&mut options.show_hidden, "Hidden files");
    });
    ui.weak("0 minutes = any age · applies only to this Home pane");
}
fn open(entry: &Entry, actions: &mut Vec<Action>) {
    actions.push(if entry.directory {
        Action::OpenFolder(entry.path.clone())
    } else {
        Action::OpenDocument(entry.path.clone())
    });
}
fn icon_grid(ui: &mut Ui, entries: &[&Entry], actions: &mut Vec<Action>) {
    let columns = (ui.available_width() / 110.0).floor().max(1.0) as usize;
    egui::ScrollArea::vertical()
        .id_source("icons")
        .min_scrolled_height(0.0)
        .max_height(ui.available_height().max(0.0))
        .show_rows(ui, 104.0, entries.len().div_ceil(columns), |ui, range| {
            for row in range {
                let (rect, _) =
                    ui.allocate_exact_size(vec2(ui.available_width(), 104.0), Sense::hover());
                let width = rect.width() / columns as f32;
                for column in 0..columns {
                    let Some(entry) = entries.get(row * columns + column) else {
                        break;
                    };
                    let tile = Rect::from_min_size(
                        rect.min + vec2(column as f32 * width, 0.0),
                        vec2(width, 100.0),
                    );
                    let response = ui
                        .interact(tile, ui.id().with(&entry.path), Sense::click())
                        .on_hover_text(display_path(&entry.path));
                    entry_icon(entry).paint(
                        ui.painter(),
                        Rect::from_center_size(tile.center() - vec2(0.0, 12.0), vec2(48.0, 48.0)),
                        ui.visuals().text_color(),
                    );
                    ui.put(
                        Rect::from_min_size(tile.min + vec2(4.0, 70.0), vec2(width - 8.0, 26.0)),
                        egui::Label::new(&entry.name).truncate(true),
                    );
                    if response.double_clicked() {
                        open(entry, actions);
                    }
                }
            }
        });
}
