//! Timelens rows use ordinary file selection/actions and a playing hover preview.
mod trim;

use super::super::{
    filesystem::Entry,
    preview_copy,
    video_timeline::{
        Ready,
        selection::{Clip, CopyKind, FrameCopy, Selection, focus_id, timestamp},
    },
};
use super::*;

impl ListActions {
    pub(super) fn video_click(
        &mut self,
        ui: &Ui,
        response: &egui::Response,
        entry: &Entry,
        time: Option<u64>,
        start: &mut Option<(PathBuf, f64)>,
    ) {
        if response.clicked()
            && !ui.input(|i| i.modifiers.ctrl || i.modifiers.command || i.modifiers.shift)
            && let Some(time) = time
        {
            self.open = Some(entry.path.clone());
            *start = Some((entry.path.clone(), time as f64 / 1000.0));
        }
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "Drawing keeps target, geometry, content and palette explicit at the rendering boundary"
)]
pub(super) fn preview_row(
    ui: &mut Ui,
    rect: Rect,
    entry: &Entry,
    columns: Option<super::ListColumns>,
    previews: &mut super::super::previews::EntryPreviews,
    video_previews: &mut super::super::video_timeline::Previews,
    decimation: u8,
    can_request: bool,
    colors: WorkbenchColors,
    row_response: &mut egui::Response,
    actions: &mut ListActions,
) -> (Option<u64>, bool) {
    let thumbnail = if entry.is_video() {
        let thumbnail = previews.show_video(
            ui,
            rect.min + vec2(6.0, 32.0),
            (rect.height() - 42.0).max(1.0),
            entry,
        );
        if ui.is_enabled() && can_request {
            video_previews.request(
                entry,
                (((rect.right() - thumbnail.right() - 14.0).max(1.0) * ui.ctx().pixels_per_point())
                    .ceil() as u32)
                    .div_ceil(u32::from(decimation)),
            );
        }
        Some(thumbnail)
    } else {
        None
    };
    let ready = video_previews.get(entry);
    let timeline = video_card(ui, rect, thumbnail, entry, columns, ready, colors);
    let metadata = ready
        .and_then(|ready| ready.as_ref().ok())
        .map(|ready| ready.metadata.clone());
    let (active, click_time) = match (timeline, &metadata) {
        (Some(timeline), Some(metadata)) => timeline_interaction(
            ui,
            timeline,
            entry,
            video_previews.source(entry, metadata.clone()),
            video_previews,
            row_response,
            actions,
        ),
        _ => (false, None),
    };
    if active {
        return (None, true);
    }
    if video_previews
        .selection
        .as_ref()
        .is_some_and(|selection| selection.preview_open)
    {
        return (click_time, false);
    }
    let time = video_hover(
        ui,
        entry,
        thumbnail,
        timeline,
        metadata,
        video_previews,
        colors,
    );
    (click_time.or(time), false)
}

fn timeline_interaction(
    ui: &mut Ui,
    rect: Rect,
    entry: &Entry,
    source: super::super::video_timeline::selection::Source,
    previews: &mut super::super::video_timeline::Previews,
    row: &mut egui::Response,
    actions: &mut ListActions,
) -> (bool, Option<u64>) {
    if rect.width() <= 0.0 || source.metadata.duration_ms == 0 {
        return (false, None);
    }
    let response = ui.interact(
        rect,
        Id::unique(("video_timeline", &entry.path)),
        Sense::click_and_drag(),
    );
    let duration = source.metadata.duration_ms;
    let at = |point: egui::Pos2| {
        let x = ((point.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
        (f64::from(x) * duration as f64).round() as u64
    };
    if response.secondary_clicked() {
        if let Some(point) = response.interact_pointer_pos() {
            let time = at(point).min(duration.saturating_sub(1));
            previews.copy_frame = Some(
                previews
                    .displayed_frame(entry, time)
                    .unwrap_or(FrameCopy::Position(source, time)),
            );
        }
        return (true, None);
    }
    let modified =
        ui.input(|input| input.modifiers.ctrl || input.modifiers.command || input.modifiers.shift);
    let (dragging, range) = preview_copy::drag_range(ui, &response, focus_id(), modified);
    if let Some(clip) = range.and_then(|range| Clip::new(source, range)) {
        let mut selection = Selection::new(clip, entry.revision);
        if response.drag_stopped() {
            selection.prepare_frames(ui.ctx());
        }
        previews.selection = Some(selection);
    }
    if !dragging {
        *row = row.union(response.clone());
    }
    if let Some(mut selection) = previews.selection.take() {
        if selection.matches(&entry.path, entry.revision) {
            let overview = previews
                .get(entry)
                .and_then(|ready| ready.as_ref().ok())
                .and_then(|ready| ready.texture.as_ref());
            match show_selection(ui, &response, &mut selection, overview) {
                Some(SelectionAction::Copy(kind)) => previews.copy_clip = Some(kind),
                Some(SelectionAction::EditAudio) => {
                    actions.open = Some(selection.clip.source.path.clone());
                    actions.open_position = Some(super::super::DocumentPosition::VideoAudioRange {
                        start_ms: selection.clip.start_ms,
                        end_ms: selection.clip.end_ms,
                        revision: selection.revision,
                    });
                }
                None => {}
            }
        }
        previews.selection = Some(selection);
    }
    let click_time = if response.clicked() {
        response.interact_pointer_pos().map(at)
    } else {
        None
    };
    (dragging, click_time)
}

enum SelectionAction {
    Copy(CopyKind),
    EditAudio,
}

fn show_selection(
    ui: &Ui,
    response: &egui::Response,
    selection: &mut Selection,
    overview: Option<&egui::TextureHandle>,
) -> Option<SelectionAction> {
    let clip = &selection.clip;
    let duration = clip.source.metadata.duration_ms as f64;
    preview_copy::paint_range(
        ui,
        response.rect,
        (
            (clip.start_ms as f64 / duration) as f32,
            (clip.end_ms as f64 / duration) as f32,
        ),
    );
    let screen = ui.ctx().content_rect();
    let width = response
        .rect
        .width()
        .clamp(360.0, 960.0)
        .min((screen.width() - 24.0).max(1.0));
    let height = (width * 0.24).min(screen.height() * 0.35);
    let mut open = selection.preview_open;
    let has_audio = clip.source.metadata.audio_tracks > 0;
    let action = egui::Popup::from_response(response)
        .open_bool(&mut open)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .gap(4.0)
        .show(|ui| {
            ui.set_width(width);
            let edit = ui.columns_const(|[start, preview, end]| {
                show_endpoint(
                    start,
                    height,
                    "Start",
                    selection.frames.as_ref().map(|[start, _]| start),
                );
                let edit = show_playback(preview, height, selection);
                show_endpoint(
                    end,
                    height,
                    "End",
                    selection.frames.as_ref().map(|[_, end]| end),
                );
                edit
            });
            trim::show(ui, selection, overview);
            let copy = ui
                .with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let video = ui.button("Copy clip");
                    let audio = ui
                        .add_enabled(has_audio, egui::Button::new("Copy audio from clip"))
                        .on_disabled_hover_text("This video has no audio track");
                    video
                        .clicked()
                        .then_some(CopyKind::Video)
                        .or_else(|| audio.clicked().then_some(CopyKind::Audio))
                })
                .inner;
            edit.then_some(SelectionAction::EditAudio)
                .or_else(|| copy.map(SelectionAction::Copy))
        })
        .and_then(|popup| popup.inner);
    selection.preview_open = open;
    if !open {
        selection.close_preview();
    }
    if matches!(action, Some(SelectionAction::Copy(_))) {
        ui.memory_mut(|memory| memory.request_focus(focus_id()));
    }
    action
}

fn show_endpoint(
    ui: &mut Ui,
    height: f32,
    label: &str,
    frame: Option<&Result<egui::TextureHandle, String>>,
) {
    ui.label(label);
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
    let texture = frame.and_then(|frame| frame.as_ref().ok());
    let status = if frame.is_some() {
        "Unavailable"
    } else {
        "Loading…"
    };
    paint_preview(ui, rect, texture, status);
}

fn show_playback(ui: &mut Ui, height: f32, selection: &mut Selection) -> bool {
    ui.label("Preview");
    let (rect, response) =
        ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::click());
    let has_audio = selection.clip.source.metadata.audio_tracks > 0;
    if has_audio && response.clicked_by(egui::PointerButton::Primary) {
        selection.toggle_mute();
        ui.memory_mut(|memory| memory.request_focus(focus_id()));
    }
    let muted = selection.muted;
    let width = (rect.width().max(rect.height() * 16.0 / 9.0) * ui.ctx().pixels_per_point())
        .ceil()
        .clamp(128.0, 768.0) as u32;
    let player = selection.looping_preview(ui.ctx(), width);
    let status = match player {
        Some(player) if player.error.is_some() => "Unavailable",
        Some(_) => "Opening…",
        None => "Adjusting…",
    };
    paint_preview(
        ui,
        rect,
        player.and_then(|player| player.texture.as_ref()),
        status,
    );
    if has_audio {
        paint_mute_status(ui, rect, muted);
    }
    let mut edit = false;
    response.context_menu(|ui| {
        edit = ui
            .add_enabled(has_audio, egui::Button::new("Edit audio track"))
            .on_disabled_hover_text("This video has no audio track")
            .clicked();
        if edit {
            ui.close();
        }
    });
    if has_audio {
        response
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text(if muted {
                "Click to unmute"
            } else {
                "Click to mute"
            });
    }
    edit
}

fn paint_mute_status(ui: &Ui, rect: Rect, muted: bool) {
    let badge = Rect::from_min_size(rect.left_bottom() + vec2(4.0, -28.0), vec2(24.0, 24.0));
    let painter = ui.painter_at(rect);
    painter.rect_filled(badge, 4.0, Color32::from_black_alpha(190));
    let icon = badge.shrink(4.0);
    Icon::Volume.paint(&painter, icon, Color32::WHITE);
    if muted {
        painter.line_segment(
            [icon.left_bottom(), icon.right_top()],
            egui::Stroke::new(2.0, Color32::WHITE),
        );
    }
}

fn paint_preview(ui: &Ui, rect: Rect, texture: Option<&egui::TextureHandle>, status: &str) {
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, ui.visuals().extreme_bg_color);
    if let Some(texture) = texture {
        let size = texture.size_vec2();
        let scale = (rect.width() / size.x).min(rect.height() / size.y);
        painter.image(
            texture.id(),
            Rect::from_center_size(rect.center(), size * scale),
            unit_uv(),
            Color32::WHITE,
        );
    } else {
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            status,
            FontId::proportional(13.0),
            ui.visuals().text_color(),
        );
    }
}

pub(in crate::file_manager) fn paint_timelens(ui: &Ui, rect: Rect, texture: &egui::TextureHandle) {
    ui.painter()
        .image(texture.id(), rect, unit_uv(), Color32::WHITE);
}

fn unit_uv() -> Rect {
    Rect::from_min_max(egui::Pos2::ZERO, pos2(1.0, 1.0))
}

fn video_hover(
    ui: &mut Ui,
    entry: &Entry,
    thumbnail: Option<Rect>,
    timeline: Option<Rect>,
    metadata: Option<std::sync::Arc<super::super::media::VideoMetadata>>,
    previews: &mut super::super::video_timeline::Previews,
    colors: WorkbenchColors,
) -> Option<u64> {
    let metadata = metadata?;
    let point = ui.ctx().pointer_hover_pos()?;
    let time = if thumbnail.is_some_and(|rect| super::preview_hovered(ui, rect)) {
        0
    } else {
        let timeline = timeline.filter(|rect| super::preview_hovered(ui, *rect))?;
        let fraction = ((point.x - timeline.left()) / timeline.width()).clamp(0.0, 1.0);
        ui.painter().line_segment(
            [
                pos2(point.x, timeline.top()),
                pos2(point.x, timeline.bottom()),
            ],
            egui::Stroke::new(1.0_f32, colors.text_primary),
        );
        ((fraction as f64 * metadata.duration_ms as f64).round() as u64)
            .min(metadata.duration_ms.saturating_sub(1))
    };
    previews.hover(ui.ctx(), entry, metadata, time);
    // Scrubbing must bypass the ordinary tooltip's stationary-pointer delay.
    egui::Tooltip::always_open(
        ui.ctx().clone(),
        ui.layer_id(),
        Id::unique(("video_timeline_hover", &entry.path)),
        egui::PopupAnchor::Pointer,
    )
    .show(|ui| {
        previews.hover_ui(ui);
        if timeline.is_some_and(|rect| rect.contains(point)) {
            ui.small("Drag to select a clip · Right-click to copy the frame");
        }
    });
    Some(time)
}

fn video_card(
    ui: &Ui,
    rect: Rect,
    thumbnail: Option<Rect>,
    entry: &Entry,
    columns: Option<super::ListColumns>,
    ready: Option<&Result<Ready, String>>,
    colors: WorkbenchColors,
) -> Option<Rect> {
    let video = entry.is_video();
    if !video {
        entry_icon(entry).paint(
            ui.painter(),
            Rect::from_min_size(rect.min + vec2(6.0, 6.0), vec2(20.0, 20.0)),
            colors.text_secondary,
        );
    }
    let left = if video { 6.0 } else { 32.0 };
    let header = Rect::from_min_size(
        rect.min + vec2(left, 0.0),
        vec2((rect.width() - left - 6.0).max(1.0), 24.0),
    );
    let details = ready.and_then(|ready| ready.as_ref().ok()).map(|ready| {
        if columns.is_some_and(|columns| !columns.metadata_fields().is_empty()) {
            return ready.metadata.audio_description.clone();
        }
        format!(
            "{} · {}",
            ready.metadata.description, ready.metadata.audio_description
        )
    });
    media_header(ui, header, entry, details, columns, colors);
    if !video {
        return None;
    }
    let timeline = Rect::from_min_max(
        pos2(thumbnail?.right() + 8.0, rect.top() + 32.0),
        pos2(rect.right() - 6.0, rect.bottom() - 10.0),
    );
    ui.painter().rect_filled(timeline, 0.0, colors.recessed);
    match ready {
        Some(Ok(ready)) => {
            if let Some(texture) = &ready.texture {
                paint_timelens(ui, timeline, texture);
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
                    "Loading timeline — hover to play now",
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

impl FileManager {
    pub(in crate::file_manager) fn copy_video_range(
        &mut self,
        ctx: &egui::Context,
        kind: CopyKind,
    ) {
        let Some(clip) = self
            .video_previews
            .selection
            .as_ref()
            .map(|selection| selection.clip.clone())
        else {
            return;
        };
        if self.preview_copies.busy() {
            return;
        }
        if kind == CopyKind::Video
            && let Some(bytes) = clip.large_estimate()
        {
            self.large_video_copy = Some((clip, bytes));
            ctx.request_repaint();
            return;
        }
        self.preview_copies.start(ctx, move |cancel| match kind {
            CopyKind::Video => clip.export(cancel),
            CopyKind::Audio => clip.export_audio(cancel),
        });
    }

    pub(super) fn large_video_copy_ui(&mut self, ctx: &egui::Context, palette: &AppPalette) {
        use clip_ui::buttons::ButtonRole;
        let Some((clip, bytes)) = &self.large_video_copy else {
            return;
        };
        let dialog = egui::Modal::new(Id::unique("copy_large_video_clip")).show(ctx, |ui| {
            ui.set_width(350.0);
            ui.heading("Copy a large video clip?");
            ui.label(format!(
                "The selected clip from {} to {} is estimated at {:.2} GB.",
                timestamp(clip.start_ms),
                timestamp(clip.end_ms),
                *bytes as f64 / 1_000_000_000.0,
            ));
            ui.horizontal(|ui| {
                let cancel = ui
                    .add(ButtonRole::Secondary.button("Cancel", palette))
                    .clicked();
                let copy = ui
                    .add(ButtonRole::Primary.button("Copy clip", palette))
                    .clicked();
                (cancel, copy)
            })
            .inner
        });
        if dialog.inner.1 {
            if let Some((clip, _)) = self.large_video_copy.take() {
                self.preview_copies
                    .start(ctx, move |cancel| clip.export(cancel));
            }
        } else if dialog.inner.0 || dialog.should_close() {
            self.large_video_copy = None;
        }
    }
}
