//! Clipboard exports outlive the preview that requested them.
use super::{preview_job::Task, search::PreviewContext};
use crate::file_source::{FileSource, ReadCheck};
use little_clip_core::{HandSide, clipboard::ClipboardError, ipc::IpcRequest};
use std::{
    fs::File,
    os::fd::AsRawFd,
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
    time::Duration,
};

#[derive(Default)]
pub(super) struct Copies {
    job: Option<Task<Result<String, String>>>,
}

impl Copies {
    pub fn busy(&self) -> bool {
        self.job.is_some()
    }

    pub fn start(
        &mut self,
        ctx: &egui::Context,
        work: impl FnOnce(&AtomicBool) -> Result<String, String> + Send + 'static,
    ) {
        if self.busy() {
            return;
        }
        let ctx = ctx.clone();
        self.job = Some(Task::start_unbounded(
            crate::desktop_ipc::mutation_jobs(),
            move |cancel| {
                let result = work(cancel);
                ctx.request_repaint();
                result
            },
        ));
    }

    pub fn poll(&mut self, ctx: &egui::Context) -> Option<Result<String, String>> {
        let Some(result) = self.job.as_ref()?.poll() else {
            ctx.request_repaint_after(Duration::from_millis(40));
            return None;
        };
        self.job = None;
        Some(
            result
                .map_err(|error| error.to_string())
                .and_then(|result| result),
        )
    }
}

pub(super) fn read_range<T>(
    context: &PreviewContext,
    path: &Path,
    source: &FileSource,
    cancel: &AtomicBool,
    read: impl FnOnce(&mut File) -> Result<T, String>,
) -> Result<T, String> {
    context.read(path, cancel, || {
        let (mut file, _) = ReadCheck::default().open(path, Some(source), cancel)?;
        let result = read(&mut file)?;
        source.check_file(&file)?;
        if FileSource::capture(path)? != *source {
            return Err("Source changed while reading the selection".into());
        }
        Ok(result)
    })
}

pub(super) fn copy_file(
    source: &Path,
    range_name: &str,
    extension: &str,
    write: impl FnOnce(&mut File) -> Result<(), String>,
) -> Result<(), String> {
    let staging =
        little_clip_core::staging::get_staging_dir().map_err(|error| error.to_string())?;
    let directory = little_clip_core::kernel_interface::create_content_directory(
        &staging.join("preview-segments"),
    )
    .map_err(|error| error.to_string())?;
    let anchor = PathBuf::from(format!("/proc/self/fd/{}", directory.as_raw_fd()));
    let stem = source
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or("segment");
    let stem = &stem[..stem.floor_char_boundary(stem.len().min(120))];
    let prefix = format!("{stem}-{range_name}-");
    let mut output = tempfile::Builder::new()
        .prefix(&prefix)
        .suffix(&format!(".{extension}"))
        .tempfile_in(&anchor)
        .map_err(|error| error.to_string())?;
    write(output.as_file_mut())?;
    let published = FileSource::capture_file(output.as_file())?;
    published.check()?;
    let path = published.path().to_path_buf();
    let clipboard =
        little_clip_core::copy_file_uris_to_system_clipboard(std::slice::from_ref(&path));
    if clipboard.is_ok() || matches!(&clipboard, Err(ClipboardError::Failed(_))) {
        output.disable_cleanup(true);
    }
    clipboard.map_err(|error| error.to_string())?;
    let _ = clip_studios::send_ipc_request_with_timeout(
        &IpcRequest::PushFile {
            path,
            hand: HandSide::Left,
        },
        Some(Duration::from_secs(2)),
    );
    Ok(())
}

/// Claims a primary-button range gesture for the strip and its keyboard focus.
pub(super) fn drag_range(
    ui: &egui::Ui,
    response: &egui::Response,
    focus: egui::Id,
    modified: bool,
) -> (bool, Option<(f32, f32)>) {
    drag_range_by(ui, response, focus, modified, |point| {
        ((point.x - response.rect.left()) / response.rect.width()).clamp(0.0, 1.0)
    })
}

pub(super) fn drag_range_by(
    ui: &egui::Ui,
    response: &egui::Response,
    focus: egui::Id,
    modified: bool,
    fraction: impl Fn(egui::Pos2) -> f32,
) -> (bool, Option<(f32, f32)>) {
    use egui::PointerButton::Primary;
    let dragging = !modified && (response.dragged_by(Primary) || response.drag_stopped_by(Primary));
    if !dragging {
        return (false, None);
    }
    ui.memory_mut(|memory| memory.request_focus(focus));
    (true, drag_fractions(ui, response, fraction))
}

/// Handles a row's own range selection while preserving its file-list response.
pub(super) fn strip_range(
    ui: &egui::Ui,
    rect: egui::Rect,
    id: egui::Id,
    row: &mut egui::Response,
) -> (egui::Response, bool, Option<(f32, f32)>) {
    let response = ui.interact(rect, id, egui::Sense::click_and_drag());
    *row = row.clone().union(response.clone());
    let modified = ui.input(|input| input.modifiers.command || input.modifiers.alt);
    let (engaged, drag) = drag_range(ui, &response, id, modified);
    let range = remember_range(ui, &response, drag);
    if let Some(range) = range {
        paint_range(ui, rect, range);
    }
    (response, engaged, range)
}

fn remember_range(
    ui: &egui::Ui,
    response: &egui::Response,
    drag: Option<(f32, f32)>,
) -> Option<(f32, f32)> {
    let clear = response.secondary_clicked();
    let selection_id = response.id.with("range");
    ui.data_mut(|data| {
        if let Some(range) = drag {
            data.insert_temp(selection_id, range);
        } else if clear {
            data.remove::<(f32, f32)>(selection_id);
        }
        data.get_temp(selection_id)
    })
}

/// Ascending fractions, including the final pointer movement on release.
fn drag_fractions(
    ui: &egui::Ui,
    response: &egui::Response,
    fraction: impl Fn(egui::Pos2) -> f32,
) -> Option<(f32, f32)> {
    let point = response.interact_pointer_pos()?;
    let origin = ui.input(|input| input.pointer.press_origin());
    let start = ui.ctx().data_mut(|data| {
        if response.drag_stopped() {
            let start = data.get_temp::<f32>(response.id);
            data.remove::<f32>(response.id);
            start
        } else {
            let start = origin.map(&fraction)?;
            data.insert_temp(response.id, start);
            Some(start)
        }
    })?;
    let end = fraction(point);
    Some((start.min(end), start.max(end)))
}

pub(super) const RANGE_FOOTER_HEIGHT: f32 = 24.0;

pub(super) fn paint_range(ui: &egui::Ui, rect: egui::Rect, (start, end): (f32, f32)) {
    use egui::{Rect, Stroke, pos2};
    let painter = ui.painter_at(rect);
    let selection = Rect::from_min_max(
        pos2(egui::lerp(rect.x_range(), start), rect.top()),
        pos2(egui::lerp(rect.x_range(), end), rect.bottom()),
    );
    let accent = ui.visuals().selection.stroke.color;
    painter.rect_filled(selection, 0.0, accent.gamma_multiply(0.35));
    for x in [selection.left(), selection.right()] {
        painter.line_segment(
            [pos2(x, rect.top()), pos2(x, rect.bottom())],
            Stroke::new(2.0, accent),
        );
    }
}

pub(super) fn paint_range_footer(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    label: &str,
    action: &str,
) -> bool {
    use egui::{Align2, FontId, Rect, pos2, vec2};
    let painter = ui.painter_at(rect);
    let label_rect = Rect::from_min_size(
        rect.left_bottom() - vec2(0.0, RANGE_FOOTER_HEIGHT),
        vec2(rect.width(), RANGE_FOOTER_HEIGHT),
    );
    painter.rect_filled(label_rect, 0.0, ui.visuals().extreme_bg_color);
    let has_button = label_rect.width() >= 290.0;
    let text_area = if has_button {
        Rect::from_min_max(label_rect.min, label_rect.right_bottom() - vec2(100.0, 0.0))
    } else {
        label_rect
    };
    painter.with_clip_rect(text_area).text(
        text_area.center(),
        Align2::CENTER_CENTER,
        label,
        FontId::monospace(12.0),
        ui.visuals().text_color(),
    );
    has_button
        && ui
            .put(
                Rect::from_min_max(
                    pos2(text_area.right() + 4.0, label_rect.top() + 1.0),
                    label_rect.right_bottom(),
                ),
                egui::Button::new(action)
                    .small()
                    .wrap_mode(egui::TextWrapMode::Extend),
            )
            .clicked()
}

#[cfg(test)]
mod tests {
    use super::remember_range;
    use egui::{Event, Modifiers, PointerButton, RawInput, Sense, pos2, vec2};

    #[test]
    fn strip_range_survives_primary_click_and_clears_on_secondary_click() {
        let ctx = egui::Context::default();
        let pos = pos2(30.0, 24.0);
        let selection = Some((0.2, 0.6));
        let frame = |event: Event, drag| {
            let mut current = None;
            let _ = clip_ui::test_support::run_frame(
                &ctx,
                RawInput {
                    events: vec![event],
                    ..Default::default()
                },
                |ctx| {
                    ctx.root_ui(|ui| {
                        let response = ui.allocate_response(vec2(120.0, 50.0), Sense::click());
                        current = remember_range(ui, &response, drag);
                    });
                },
            );
            current
        };
        let button_event = |button, pressed| Event::PointerButton {
            pos,
            button,
            pressed,
            modifiers: Modifiers::NONE,
        };

        assert_eq!(frame(Event::PointerMoved(pos), selection), selection);
        frame(button_event(PointerButton::Primary, true), None);
        assert_eq!(
            frame(button_event(PointerButton::Primary, false), None),
            selection,
        );
        frame(button_event(PointerButton::Secondary, true), None);
        assert_eq!(
            frame(button_event(PointerButton::Secondary, false), None),
            None
        );
    }
}
