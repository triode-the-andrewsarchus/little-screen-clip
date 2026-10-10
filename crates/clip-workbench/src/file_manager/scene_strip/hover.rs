use super::super::{filesystem::Entry, preview_job::Task};
use super::{LIVE, backend, set_texture};
use backend::Renderer;
use egui::{Context, Rect, TextureHandle, Ui, vec2};
use little_clip_core::JobError;
use little_clip_core::palette::AppPalette;
use std::{collections::BTreeSet, path::PathBuf, time::Duration};

struct Completion {
    renderer: Renderer,
    frame: backend::Frame,
}
struct Pending {
    size: [u32; 2],
    task: Task<Result<Completion, String>>,
}
#[derive(Default)]
pub(super) struct Hover {
    key: Option<(PathBuf, u64, AppPalette)>,
    renderer: Option<Renderer>,
    pending: Option<Pending>,
    texture: Option<TextureHandle>,
    completed_size: Option<[u32; 2]>,
    error: Option<String>,
    started: f64,
    next: f64,
    seen: bool,
}
impl Hover {
    pub fn clear(&mut self) {
        *self = Self::default();
    }
    pub fn retain(&mut self, visible: &BTreeSet<PathBuf>) {
        if !self.seen
            || self
                .key
                .as_ref()
                .is_some_and(|(path, _, _)| !visible.contains(path))
        {
            self.clear();
        }
        self.seen = false;
    }
    pub fn poll(&mut self, ctx: &Context) {
        let Some(pending) = &self.pending else {
            return;
        };
        let Some(result) = pending.task.poll() else {
            ctx.request_repaint_after(Duration::from_millis(16));
            return;
        };
        let size = pending.size;
        self.pending = None;
        match result {
            Ok(Ok(completion)) => {
                self.renderer = Some(completion.renderer);
                self.completed_size = Some(size);
                set_texture(
                    ctx,
                    &mut self.texture,
                    "scene-strip-live",
                    &completion.frame.image,
                );
                // Cap ordinary hover playback at 20 fps, with backoff for heavy scenes.
                self.next = ctx.input(|input| input.time)
                    + (f64::from(completion.frame.render_ms) * 0.002).clamp(0.05, 1.0);
            }
            Err(JobError::QueueFull) => self.next = ctx.input(|input| input.time) + 0.05,
            Ok(Err(error)) => self.error = Some(error),
            Err(error) => self.error = Some(error.to_string()),
        }
        ctx.request_repaint();
    }
    pub fn show(
        &mut self,
        ui: &Ui,
        anchor: Rect,
        entry: &Entry,
        view: usize,
        fallback: Option<&TextureHandle>,
        palette: &AppPalette,
    ) {
        let screen = ui.ctx().screen_rect().shrink(8.0);
        let top = anchor.bottom() + 6.0;
        let height = (screen.bottom() - top).min(360.0);
        if height < 16.0 {
            return;
        }
        self.seen = true;
        let now = ui.input(|input| input.time);
        if !self
            .key
            .as_ref()
            .is_some_and(|key| key.0 == entry.path && key.1 == entry.revision && key.2 == *palette)
        {
            self.clear();
            self.seen = true;
            self.key = Some((entry.path.clone(), entry.revision, palette.clone()));
            self.started = now;
        }
        let width = screen.width().min(640.0).min(height * 16.0 / 9.0);
        let size = vec2(width, width * 9.0 / 16.0);
        let pixels = size * ui.ctx().pixels_per_point();
        let resolution = [
            pixels.x.ceil().clamp(32.0, 1280.0) as u32,
            pixels.y.ceil().clamp(18.0, 720.0) as u32,
        ];
        self.request(
            entry,
            backend::camera(view, (now - self.started) as f32),
            resolution,
            now,
            palette,
        );
        let texture = self
            .texture
            .as_ref()
            .filter(|_| self.completed_size == Some(resolution))
            .or(fallback);
        super::super::ui::preview_popup(
            ui,
            egui::Id::new("scene-strip-live-hover"),
            anchor,
            size,
            texture,
            self.error.as_deref(),
        );
        ui.ctx()
            .request_repaint_after(Duration::from_secs_f64((self.next - now).clamp(0.016, 1.0)));
    }
    fn request(
        &mut self,
        entry: &Entry,
        camera: little_clip_scene::OrbitCamera,
        size: [u32; 2],
        now: f64,
        palette: &AppPalette,
    ) {
        if self.pending.is_some()
            || self.error.is_some()
            || (self.completed_size == Some(size) && now < self.next)
        {
            return;
        }
        let renderer = self.renderer.take();
        let path = entry.path.clone();
        let revision = entry.revision;
        let palette = palette.clone();
        let task = Task::start(&LIVE, Duration::from_secs(120), move |token| {
            if let Some(mut renderer) = renderer {
                super::super::filesystem::check_revision(&path, revision)?;
                let frame = backend::render(&mut renderer, camera, size, token)?;
                super::super::filesystem::check_revision(&path, revision)?;
                Ok(Completion { renderer, frame })
            } else {
                let loaded = backend::load(&path, revision, camera, size, &palette, token)?;
                Ok(Completion {
                    renderer: loaded.renderer,
                    frame: loaded.frame,
                })
            }
        });
        self.pending = Some(Pending { size, task });
    }
}
