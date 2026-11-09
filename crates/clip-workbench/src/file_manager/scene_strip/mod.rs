//! Four progressive orbit views and one coalesced live hover renderer.
mod backend;
mod hover;
mod ui;
use super::{filesystem::Entry, preview_job::Task};
use backend::Metadata;
use egui::{Context, TextureHandle};
use little_clip_core::{BackgroundJobs, JobError};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::{LazyLock, mpsc},
    time::Duration,
};

static STRIPS: LazyLock<BackgroundJobs> = LazyLock::new(|| BackgroundJobs::new(1, 4));
static LIVE: LazyLock<BackgroundJobs> = LazyLock::new(|| BackgroundJobs::new(1, 2));
const SIZE: [u32; 2] = [192, 108];
struct Progress {
    metadata: Metadata,
    view: usize,
    frame: backend::Frame,
}
struct Pending {
    task: Task<Result<(), String>>,
    progress: mpsc::Receiver<Progress>,
}
struct Cached {
    revision: u64,
    palette: little_clip_core::palette::AppPalette,
    metadata: Option<Metadata>,
    textures: [Option<TextureHandle>; 4],
    pending: Option<Pending>,
    error: Option<String>,
    complete: bool,
}
#[derive(Default)]
pub(super) struct Previews {
    context: super::search::PreviewContext,
    files: BTreeMap<PathBuf, Cached>,
    hover: hover::Hover,
}
impl Previews {
    pub fn set_context(&mut self, context: &super::search::PreviewContext) {
        if self.context.replace(context) {
            self.clear();
            self.hover.set_context(context);
        }
    }

    pub fn clear(&mut self) {
        self.files.clear();
        self.hover.clear();
    }
    pub fn retain(&mut self, visible: &BTreeSet<PathBuf>) {
        self.files.retain(|path, _| visible.contains(path));
        self.hover.retain(visible);
    }
    pub fn poll(&mut self, ctx: &Context) {
        self.hover.poll(ctx);
        for (path, cached) in &mut self.files {
            poll_strip(ctx, path, cached);
        }
    }
    fn request(&mut self, entry: &Entry, palette: &little_clip_core::palette::AppPalette) {
        if self
            .files
            .get(&entry.path)
            .is_some_and(|cached| cached.revision != entry.revision || cached.palette != *palette)
        {
            self.files.remove(&entry.path);
        }
        let pending = self
            .files
            .values()
            .filter(|cached| cached.pending.is_some())
            .count();
        let cached = self
            .files
            .entry(entry.path.clone())
            .or_insert_with(|| Cached {
                revision: entry.revision,
                palette: palette.clone(),
                metadata: None,
                textures: Default::default(),
                pending: None,
                error: None,
                complete: false,
            });
        if cached.pending.is_some() || cached.complete || pending >= 2 {
            return;
        }
        let (tx, progress) = mpsc::sync_channel(4);
        let path = entry.path.clone();
        let revision = entry.revision;
        let palette = palette.clone();
        let context = self.context.clone();
        let task = Task::start(&STRIPS, Duration::from_secs(120), move |token| {
            let mut loaded = context.read(&path, token, || {
                backend::load(
                    &path,
                    revision,
                    backend::camera(0, 0.0),
                    SIZE,
                    &palette,
                    token,
                )
            })?;
            let _ = tx.send(Progress {
                metadata: loaded.metadata.clone(),
                view: 0,
                frame: loaded.frame,
            });
            for view in 1..4 {
                let frame = context.read(&path, token, || {
                    backend::render(
                        &mut loaded.renderer,
                        backend::camera(view, 0.0),
                        SIZE,
                        token,
                    )
                })?;
                super::filesystem::check_revision(&path, revision)?;
                let _ = tx.send(Progress {
                    metadata: loaded.metadata.clone(),
                    view,
                    frame,
                });
            }
            Ok(())
        });
        cached.pending = Some(Pending { task, progress });
    }
}
fn poll_strip(ctx: &Context, path: &std::path::Path, cached: &mut Cached) {
    let Some(pending) = &cached.pending else {
        return;
    };
    for progress in pending.progress.try_iter() {
        cached.metadata = Some(progress.metadata);
        set_texture(
            ctx,
            &mut cached.textures[progress.view],
            &format!("scene-strip-{}-{}", path.display(), progress.view),
            &progress.frame.image,
        );
        ctx.request_repaint();
    }
    if let Some(result) = pending.task.poll() {
        cached.pending = None;
        match result {
            Err(JobError::QueueFull) => {}
            Ok(Ok(())) => cached.complete = true,
            Ok(Err(error)) => {
                cached.error = Some(error);
                cached.complete = true;
            }
            Err(error) => {
                cached.error = Some(error.to_string());
                cached.complete = true;
            }
        }
        ctx.request_repaint();
    } else {
        ctx.request_repaint_after(Duration::from_millis(16));
    }
}
fn set_texture(
    ctx: &Context,
    texture: &mut Option<TextureHandle>,
    name: &str,
    image: &image::RgbaImage,
) {
    let image = egui::ColorImage::from_rgba_unmultiplied(
        [image.width() as usize, image.height() as usize],
        image.as_raw(),
    );
    if let Some(texture) = texture {
        texture.set(image, egui::TextureOptions::LINEAR);
    } else {
        *texture = Some(ctx.load_texture(name, image, egui::TextureOptions::LINEAR));
    }
}
