//! Visible-only timelines and one coalesced hover frame, on separate bounded queues.
mod backend;
mod decoder;
mod dispatch;
mod scrub;
use super::{filesystem::Entry, media::VideoMetadata};
use egui::{Context, TextureHandle};
use little_clip_core::{BackgroundJobs, ImageThumbnail, JobError, JobTicket};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{
        Arc, LazyLock,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};

struct Task<T> {
    ticket: JobTicket,
    token: Arc<AtomicBool>,
    receiver: mpsc::Receiver<Result<T, JobError>>,
}
impl<T: Send + 'static> Task<T> {
    fn start(
        pool: &BackgroundJobs,
        timeout: Duration,
        work: impl FnOnce(&AtomicBool) -> T + Send + 'static,
    ) -> Self {
        let token = Arc::new(AtomicBool::new(false));
        let worker = token.clone();
        let (ticket, receiver) = pool.submit(timeout, move || work(&worker));
        Self {
            ticket,
            token,
            receiver,
        }
    }
    fn poll(&self) -> Option<Result<T, JobError>> {
        match self.receiver.try_recv() {
            Ok(result) => Some(result),
            Err(mpsc::TryRecvError::Empty) => None,
            Err(_) => Some(Err(JobError::Cancelled)),
        }
    }
}
impl<T> Drop for Task<T> {
    fn drop(&mut self) {
        self.token.store(true, Ordering::Relaxed);
        self.ticket.cancel();
    }
}
static TIMELINES: LazyLock<BackgroundJobs> =
    LazyLock::new(|| BackgroundJobs::new(*dispatch::FILE_LIMIT, 4));
static FRAMES: LazyLock<BackgroundJobs> = LazyLock::new(|| BackgroundJobs::new(1, 2));

pub(super) struct Ready {
    pub metadata: Arc<VideoMetadata>,
    pub texture: Option<TextureHandle>,
    pub error: Option<String>,
    pub refining: bool,
}
struct Cached {
    revision: u64,
    width: u32,
    value: Result<Ready, String>,
}
struct Pending {
    revision: u64,
    width: u32,
    task: Task<Result<backend::Rendered, String>>,
    inspected: mpsc::Receiver<Arc<VideoMetadata>>,
    progress: mpsc::Receiver<ImageThumbnail>,
}
#[derive(Clone, PartialEq, Eq)]
struct FrameKey {
    path: PathBuf,
    revision: u64,
    time_ms: u64,
    width: u32,
}
impl FrameKey {
    fn same_file(&self, other: &Self) -> bool {
        self.path == other.path && self.revision == other.revision && self.width == other.width
    }
}
struct Hover {
    key: FrameKey,
    frame_time: Option<u64>,
    texture: Option<TextureHandle>,
    error: Option<String>,
}
#[derive(Default)]
pub(super) struct Previews {
    ready: BTreeMap<PathBuf, Cached>,
    pending: BTreeMap<PathBuf, Pending>,
    hover: Option<Hover>,
    frame: Option<scrub::Session>,
    visible_jobs: usize,
}
fn texture(ctx: &Context, name: String, image: ImageThumbnail) -> TextureHandle {
    ctx.load_texture(
        name,
        egui::ColorImage::from_rgba_premultiplied(
            [image.width as usize, image.height as usize],
            &image.premultiplied_rgba,
        ),
        egui::TextureOptions::LINEAR,
    )
}
impl Previews {
    pub fn clear(&mut self) {
        self.ready.clear();
        self.pending.clear();
        self.clear_hover();
    }
    pub fn clear_hover(&mut self) {
        self.hover = None;
        self.frame = None;
    }
    pub fn retain(&mut self, entries: &[&Entry]) {
        self.visible_jobs = entries.len().clamp(1, *dispatch::FILE_LIMIT);
        let keys: BTreeMap<_, _> = entries
            .iter()
            .map(|entry| (entry.path.clone(), entry.revision))
            .collect();
        self.ready
            .retain(|path, value| keys.get(path) == Some(&value.revision));
        self.pending
            .retain(|path, value| keys.get(path) == Some(&value.revision));
        if self
            .hover
            .as_ref()
            .is_some_and(|h| keys.get(&h.key.path) != Some(&h.key.revision))
        {
            self.clear_hover();
        }
    }
    pub fn request(&mut self, entry: &Entry, width: u32) {
        let width = width.clamp(1, 16384);
        if self
            .ready
            .get(&entry.path)
            .is_some_and(|r| r.width >= width)
            || self
                .pending
                .get(&entry.path)
                .is_some_and(|r| r.width >= width)
        {
            return;
        }
        self.pending.remove(&entry.path);
        if self.pending.len() >= *dispatch::FILE_LIMIT {
            return;
        }
        let path = entry.path.clone();
        let revision = entry.revision;
        let peers = self.visible_jobs;
        let (inspected, metadata) = mpsc::sync_channel(1);
        let (progress, images) = mpsc::sync_channel(1);
        self.pending.insert(
            entry.path.clone(),
            Pending {
                revision,
                width,
                inspected: metadata,
                progress: images,
                task: Task::start(&TIMELINES, Duration::from_secs(370), move |token| {
                    backend::load(&path, revision, width, peers, token, inspected, progress)
                }),
            },
        );
    }
    pub fn get(&self, entry: &Entry) -> Option<&Result<Ready, String>> {
        self.ready.get(&entry.path).map(|r| &r.value)
    }
    pub fn poll(&mut self, ctx: &Context) {
        // Metadata arrives before the strip so large-file scrubbing can start immediately.
        for (path, pending) in &self.pending {
            if let Ok(metadata) = pending.inspected.try_recv() {
                self.ready.entry(path.clone()).or_insert(Cached {
                    revision: pending.revision,
                    width: 0,
                    value: Ok(Ready {
                        metadata,
                        texture: None,
                        error: None,
                        refining: true,
                    }),
                });
                ctx.request_repaint();
            }
        }
        for (path, pending) in &self.pending {
            if let Ok(image) = pending.progress.try_recv() {
                if let Some(Ok(ready)) = self.ready.get_mut(path).map(|cached| &mut cached.value) {
                    ready.refining = true;
                    let pixels = egui::ColorImage::from_rgba_premultiplied(
                        [image.width as usize, image.height as usize],
                        &image.premultiplied_rgba,
                    );
                    if let Some(texture) = &mut ready.texture {
                        texture.set(pixels, egui::TextureOptions::LINEAR);
                    } else {
                        ready.texture = Some(texture(
                            ctx,
                            format!("video_timeline:{}", path.display()),
                            image,
                        ));
                    }
                    ctx.request_repaint();
                }
            }
        }
        let done: Vec<_> = self
            .pending
            .iter()
            .filter_map(|(path, job)| job.task.poll().map(|result| (path.clone(), result)))
            .collect();
        for (path, result) in done {
            let pending = self.pending.remove(&path).unwrap();
            if matches!(result, Err(JobError::QueueFull)) {
                continue;
            }
            let value = result
                .map_err(|e| e.to_string())
                .and_then(|r| r)
                .map(|rendered| {
                    let (image, error) = match rendered.timeline {
                        Ok(image) => (Some(image), None),
                        Err(error) => (None, Some(error)),
                    };
                    Ready {
                        metadata: rendered.metadata,
                        texture: image.map(|image| {
                            texture(ctx, format!("video_timeline:{}", path.display()), image)
                        }),
                        error,
                        refining: false,
                    }
                });
            self.ready.insert(
                path,
                Cached {
                    revision: pending.revision,
                    width: pending.width,
                    value,
                },
            );
            ctx.request_repaint();
        }
        self.poll_frame(ctx);
        if !self.pending.is_empty() || self.frame.is_some() {
            ctx.request_repaint_after(Duration::from_millis(16));
        }
    }
    pub fn hover(
        &mut self,
        ctx: &Context,
        entry: &Entry,
        metadata: Arc<VideoMetadata>,
        time_ms: u64,
    ) {
        let key = FrameKey {
            path: entry.path.clone(),
            revision: entry.revision,
            time_ms: time_ms / 50 * 50,
            width: (480.0 * ctx.pixels_per_point().clamp(1.0, 2.0)).ceil() as u32,
        };
        let same = self.hover.as_ref().is_some_and(|h| h.key.same_file(&key));
        if !same {
            self.clear_hover();
            self.hover = Some(Hover {
                key: key.clone(),
                frame_time: None,
                texture: None,
                error: None,
            });
        } else {
            self.hover.as_mut().unwrap().key = key.clone();
        }
        if self.hover.as_ref().unwrap().error.is_some() {
            return;
        }
        if self.frame.is_none() {
            self.frame = Some(scrub::Session::new(key.clone(), metadata, ctx.clone()));
        }
        self.frame.as_mut().unwrap().request(key);
    }
    fn poll_frame(&mut self, ctx: &Context) {
        let Some(session) = &mut self.frame else {
            return;
        };
        let completion = session.poll();
        if session.finished() {
            self.frame = None;
        }
        let Some(completion) = completion else {
            return;
        };
        let Some(hover) = &mut self.hover else {
            return;
        };
        if !hover.key.same_file(&completion.key) {
            return;
        }
        match completion.result {
            Ok(frame) => {
                hover.frame_time = Some(frame.time_ms);
                hover.texture = Some(texture(ctx, "video_hover_frame".into(), frame.image));
                hover.error = None;
            }
            Err(error) => hover.error = Some(error),
        }
        ctx.request_repaint();
    }
    pub fn hover_ui(&self, ui: &mut egui::Ui) {
        if let Some(hover) = &self.hover {
            ui.label(format!("{:.3} s", hover.key.time_ms as f64 / 1000.0));
            if let Some(time) = hover.frame_time {
                ui.label(format!("Keyframe: {:.3} s", time as f64 / 1000.0));
            }
            if let Some(texture) = &hover.texture {
                ui.image((
                    texture.id(),
                    texture.size_vec2() / ui.ctx().pixels_per_point(),
                ));
            } else if let Some(error) = &hover.error {
                ui.label(error);
            } else {
                ui.spinner();
                ui.label("Loading frame…");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_source::FileSource;

    #[test]
    fn partial_textures_update_before_completion_and_retire_with_the_job() {
        let source = tempfile::NamedTempFile::new().unwrap();
        let path = source.path().to_path_buf();
        let metadata = Arc::new(VideoMetadata {
            source: FileSource::capture(&path).unwrap(),
            dimensions: (640, 360),
            duration_ms: 1000,
            stream: 0,
            codec: "h264".into(),
            size_bytes: 0,
            frame_interval_ms: 33,
        });
        let pool = BackgroundJobs::new(1, 1);
        let (gate, wait) = mpsc::sync_channel::<()>(1);
        let task = Task::start(&pool, Duration::from_secs(5), move |_| {
            let _ = wait.recv_timeout(Duration::from_secs(5));
            Err("test complete".to_owned())
        });
        let cancelled = task.token.clone();
        let (inspected, inspection) = mpsc::sync_channel(1);
        let (updates, progress) = mpsc::sync_channel(1);
        let mut previews = Previews::default();
        previews.pending.insert(
            path.clone(),
            Pending {
                revision: 1,
                width: 2,
                task,
                inspected: inspection,
                progress,
            },
        );
        let ctx = Context::default();
        inspected.send(metadata).unwrap();
        let image = || ImageThumbnail {
            width: 2,
            height: 64,
            premultiplied_rgba: [12, 64, 192, 255].repeat(2 * 64),
        };
        updates.send(image()).unwrap();
        previews.poll(&ctx);
        let first = previews.ready[&path]
            .value
            .as_ref()
            .unwrap()
            .texture
            .as_ref()
            .unwrap()
            .id();
        assert!(previews.pending.contains_key(&path));
        assert_eq!(previews.ready[&path].width, 0);
        updates.send(image()).unwrap();
        previews.poll(&ctx);
        let texture = previews.ready[&path]
            .value
            .as_ref()
            .unwrap()
            .texture
            .as_ref()
            .unwrap();
        assert_eq!(texture.id(), first);
        assert_eq!(texture.size(), [2, 64]);
        previews.retain(&[]);
        assert!(previews.ready.is_empty());
        assert!(previews.pending.is_empty());
        assert!(cancelled.load(Ordering::Relaxed));
        assert!(updates.try_send(image()).is_err());
        drop(gate);
    }
}
