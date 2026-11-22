//! Clipboard exports outlive the preview that requested them.
use super::preview_job::Task;
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

    pub fn poll(&mut self) -> Option<Result<String, String>> {
        let result = self.job.as_ref()?.poll()?;
        self.job = None;
        Some(
            result
                .map_err(|error| error.to_string())
                .and_then(|result| result),
        )
    }
}

pub(super) fn copy_file(
    source: &Path,
    range_name: &str,
    extension: Option<&str>,
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
    let suffix = extension
        .or_else(|| source.extension().and_then(|ext| ext.to_str()))
        .filter(|ext| ext.len() <= 32)
        .map_or_else(String::new, |ext| format!(".{ext}"));
    let mut output = tempfile::Builder::new()
        .prefix(&prefix)
        .suffix(&suffix)
        .tempfile_in(&anchor)
        .map_err(|error| error.to_string())?;
    write(output.as_file_mut())?;
    let published = crate::file_source::FileSource::capture_file(output.as_file())?;
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

/// Ascending fractions, including the final pointer movement on release.
pub(super) fn drag_fractions(ui: &egui::Ui, response: &egui::Response) -> Option<(f32, f32)> {
    let fraction = |x: f32| ((x - response.rect.left()) / response.rect.width()).clamp(0.0, 1.0);
    let point = response.interact_pointer_pos()?;
    let origin = ui.input(|input| input.pointer.press_origin());
    let start = ui.ctx().data_mut(|data| {
        if response.drag_stopped() {
            let start = data.get_temp::<f32>(response.id);
            data.remove::<f32>(response.id);
            start
        } else {
            let start = origin.map(|origin| fraction(origin.x))?;
            data.insert_temp(response.id, start);
            Some(start)
        }
    })?;
    let end = fraction(point.x);
    Some((start.min(end), start.max(end)))
}

pub(super) fn paint_range(
    ui: &egui::Ui,
    rect: egui::Rect,
    (start, end): (f32, f32),
    label: &str,
) -> egui::Rect {
    use egui::{Align2, FontId, Rect, Stroke, pos2, vec2};
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
    let label_rect = Rect::from_min_size(
        rect.left_bottom() - vec2(0.0, 21.0),
        vec2(rect.width(), 21.0),
    );
    painter.rect_filled(label_rect, 0.0, ui.visuals().extreme_bg_color);
    painter.text(
        label_rect.center(),
        Align2::CENTER_CENTER,
        label,
        FontId::monospace(12.0),
        ui.visuals().text_color(),
    );
    label_rect
}
