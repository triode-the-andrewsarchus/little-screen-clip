//! Timelens rows use ordinary file selection/actions and a precise hover frame.
use super::super::{filesystem::Entry, video_timeline::Ready};
use super::*;

impl FileManager {
    pub(super) fn video_ui(&mut self, ui: &mut Ui, colors: WorkbenchColors) {
        ui.horizontal_wrapped(|ui| {
            ui.label("Hover to preview a frame; click the timeline to play there. Ctrl/Shift-click selects files.");
            if ui.button("Refresh previews").clicked() { self.video_previews.clear(); }
            if ui.button("Use automatic view").clicked() {
                if let Some(location) = self.location_mut() { location.manual_view = None; }
                self.activate_media_view();
            }
        });
        self.video_previews.poll(ui.ctx());
        let scroll = self.list_scroll(ui, 108.0);
        let Some(directory) = &self.directory else {
            ui.label("Loading folder…");
            return;
        };
        let range = visible_media_rows(
            &self.video_offsets,
            self.location().map_or(0.0, |l| l.scroll),
            ui.available_height(),
        );
        let nearby: Vec<_> = range
            .clone()
            .filter_map(|index| directory.entries.get(self.visible[index]))
            .filter(|entry| entry.is_video())
            .collect();
        self.video_previews.retain(&nearby);
        let selected = current_selection(&self.history, self.index);
        let can_edit = self.can_edit();
        let count = self.selection_count();
        let mut actions = ListActions::default();
        let background = actions.background(ui, can_edit, count > 0);
        let mut targets = Vec::new();
        let mut hovered = false;
        let mut visible = Vec::new();
        let output = media_rows(ui, scroll, &self.video_offsets, |ui, index, rect| {
            let entry = &directory.entries[self.visible[index]];
            let response = media_row(
                ui,
                rect,
                selected.is_some_and(|s| s.contains(&entry.path)),
                colors,
            );
            if entry.is_video() {
                visible.push(entry);
                if ui.is_enabled() && self.pending.is_none() {
                    self.video_previews.request(
                        entry,
                        (((rect.width() - 86.0).max(1.0) * ui.ctx().pixels_per_point()).ceil()
                            as u32)
                            .div_ceil(u32::from(self.video_timeline_decimation)),
                    );
                }
            }
            let ready = self.video_previews.get(entry);
            let timeline = video_card(ui, rect, entry, ready, colors);
            let metadata = ready
                .and_then(|ready| ready.as_ref().ok())
                .map(|ready| ready.metadata.clone());
            let time = video_hover(
                ui,
                entry,
                timeline,
                metadata,
                &mut self.video_previews,
                colors,
            );
            hovered |= time.is_some();
            actions.row(ui, &response, entry, selected, can_edit, count);
            if response.clicked()
                && !ui.input(|i| i.modifiers.ctrl || i.modifiers.command || i.modifiers.shift)
            {
                if let Some(time) = time {
                    actions.open = Some(entry.path.clone());
                    self.video_start = Some((entry.path.clone(), time as f64 / 1000.0));
                }
            }
            if entry.directory {
                targets.push((rect.intersect(ui.clip_rect()), entry.path.clone()));
            }
        });
        self.video_previews.retain(&visible);
        if !hovered {
            self.video_previews.clear_hover();
        }
        if ui.is_enabled() {
            self.drop_targets.extend(targets);
        }
        self.finish_listing(ui, actions, output.state.offset.y, &background);
    }
}

fn video_hover(
    ui: &mut Ui,
    entry: &Entry,
    timeline: Option<Rect>,
    metadata: Option<std::sync::Arc<super::super::media::VideoMetadata>>,
    previews: &mut super::super::video_timeline::Previews,
    colors: WorkbenchColors,
) -> Option<u64> {
    let timeline = timeline?;
    let metadata = metadata?;
    let point = ui.ctx().pointer_hover_pos()?;
    if !ui.is_enabled() || !timeline.intersect(ui.clip_rect()).contains(point) {
        return None;
    }
    let fraction = ((point.x - timeline.left()) / timeline.width()).clamp(0.0, 1.0);
    let time = (fraction as f64 * metadata.duration_ms as f64).round() as u64;
    let time = time.min(metadata.duration_ms.saturating_sub(1));
    previews.hover(ui.ctx(), entry, metadata, time);
    ui.painter().line_segment(
        [
            pos2(point.x, timeline.top()),
            pos2(point.x, timeline.bottom()),
        ],
        egui::Stroke::new(1.0_f32, colors.text_primary),
    );
    // Scrubbing must bypass the ordinary tooltip's stationary-pointer delay.
    egui::show_tooltip_at_pointer(
        ui.ctx(),
        Id::new(("video_timeline_hover", &entry.path)),
        |ui| previews.hover_ui(ui),
    );
    Some(time)
}

fn video_card(
    ui: &Ui,
    rect: Rect,
    entry: &Entry,
    ready: Option<&Result<Ready, String>>,
    colors: WorkbenchColors,
) -> Option<Rect> {
    let video = entry.is_video();
    entry_icon(entry).paint(
        ui.painter(),
        Rect::from_min_size(
            rect.min + vec2(6.0, 6.0),
            if video {
                vec2(64.0, 64.0)
            } else {
                vec2(20.0, 20.0)
            },
        ),
        colors.text_secondary,
    );
    let left = if video { 80.0 } else { 32.0 };
    let header = Rect::from_min_size(
        rect.min + vec2(left, 0.0),
        vec2((rect.width() - left).max(1.0), 28.0),
    );
    paint_text(
        ui,
        header,
        0.0,
        (header.width() - if video { 175.0 } else { 0.0 }).max(1.0),
        &entry.name,
        colors.text_primary,
    );
    if !video {
        return None;
    }
    let timeline = Rect::from_min_max(
        pos2(header.left(), rect.top() + 32.0),
        pos2(rect.right() - 6.0, rect.bottom() - 10.0),
    );
    ui.painter().rect_filled(timeline, 0.0, colors.recessed);
    match ready {
        Some(Ok(ready)) => {
            let m = &ready.metadata;
            paint_text(
                ui,
                header,
                (header.width() - 175.0).max(0.0),
                175.0,
                &format!(
                    "{}×{} · {:.1} s",
                    m.dimensions.0,
                    m.dimensions.1,
                    m.duration_ms as f64 / 1000.0
                ),
                colors.text_secondary,
            );
            if let Some(texture) = &ready.texture {
                ui.painter().image(
                    texture.id(),
                    timeline,
                    Rect::from_min_max(egui::Pos2::ZERO, pos2(1.0, 1.0)),
                    Color32::WHITE,
                );
                if ready.refining {
                    let label = Rect::from_min_size(
                        timeline.left_bottom() - vec2(0.0, 20.0),
                        vec2(94.0, 20.0),
                    );
                    ui.painter().rect_filled(label, 0.0, colors.recessed);
                    paint_text(ui, label, 4.0, 86.0, "Refining…", colors.text_secondary);
                }
            } else if let Some(error) = &ready.error {
                paint_text(
                    ui,
                    timeline,
                    4.0,
                    timeline.width() - 8.0,
                    error,
                    colors.text_secondary,
                );
            } else {
                paint_text(
                    ui,
                    timeline,
                    4.0,
                    timeline.width() - 8.0,
                    "Loading timeline — hover to scrub now",
                    colors.text_secondary,
                );
            }
        }
        Some(Err(error)) => {
            paint_text(
                ui,
                timeline,
                4.0,
                timeline.width() - 8.0,
                error,
                colors.text_secondary,
            );
        }
        None => {
            paint_text(
                ui,
                timeline,
                4.0,
                timeline.width() - 8.0,
                "Loading video timeline…",
                colors.text_secondary,
            );
        }
    }
    Some(timeline)
}
