//! Coalesces pointer requests without blocking the UI or queuing old mouse positions.
use super::{FRAMES, FrameKey, Task, VideoMetadata, decoder};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};

pub(super) struct Completion {
    pub key: FrameKey,
    pub result: Result<decoder::Frame, String>,
}
pub(super) struct Session {
    latest: Arc<Mutex<Option<FrameKey>>>,
    wake: mpsc::SyncSender<()>,
    results: mpsc::Receiver<Completion>,
    task: Task<()>,
    requested: Option<FrameKey>,
    ended: bool,
}
impl Session {
    pub fn new(
        file: FrameKey,
        metadata: Arc<VideoMetadata>,
        ctx: egui::Context,
        context: super::super::search::PreviewContext,
    ) -> Self {
        let latest = Arc::new(Mutex::new(None));
        let worker = latest.clone();
        let (wake, receiver) = mpsc::sync_channel(1);
        let (output, results) = mpsc::sync_channel(2);
        let source = file.clone();
        let task = Task::start(&FRAMES, Duration::from_secs(86400), move |token| {
            Worker {
                file: source,
                metadata,
                latest: worker,
                wake: receiver,
                output,
                ctx,
                context,
            }
            .run(token);
        });
        Self {
            latest,
            wake,
            results,
            task,
            requested: None,
            ended: false,
        }
    }
    pub fn request(&mut self, key: FrameKey) {
        if self.requested.as_ref() == Some(&key) {
            return;
        }
        self.requested = Some(key.clone());
        *self.latest.lock().unwrap() = Some(key);
        let _ = self.wake.try_send(());
    }
    pub fn poll(&mut self) -> Option<Completion> {
        let newest = self.results.try_iter().last();
        if self.task.poll().is_some() {
            self.ended = true;
        }
        newest
    }
    pub fn finished(&self) -> bool {
        self.ended
    }
}
struct Worker {
    file: FrameKey,
    metadata: Arc<VideoMetadata>,
    latest: Arc<Mutex<Option<FrameKey>>>,
    wake: mpsc::Receiver<()>,
    output: mpsc::SyncSender<Completion>,
    ctx: egui::Context,
    context: super::super::search::PreviewContext,
}
impl Worker {
    fn run(self, token: &AtomicBool) {
        let Self {
            file,
            metadata,
            latest,
            wake,
            output,
            ctx,
            context,
        } = self;
        let mut decoder = None;
        while !token.load(Ordering::Relaxed) {
            let _ = wake.recv_timeout(Duration::from_millis(50));
            let Some(key) = latest.lock().unwrap().take() else {
                continue;
            };
            let result = context.read(&file.path, token, || {
                extract(&file, &metadata, &mut decoder, &key, token)
            });
            if token.load(Ordering::Relaxed) {
                break;
            }
            let failed = result.is_err();
            let _ = output.try_send(Completion { key, result });
            ctx.request_repaint();
            if failed {
                break;
            }
        }
    }
}
fn extract(
    file: &FrameKey,
    metadata: &VideoMetadata,
    decoder: &mut Option<decoder::Decoder>,
    key: &FrameKey,
    token: &AtomicBool,
) -> Result<decoder::Frame, String> {
    metadata.source.check()?;
    if decoder.is_none() {
        *decoder = Some(decoder::Decoder::open(&file.path, metadata, file.width)?);
    }
    let frame = decoder.as_mut().unwrap().frame(
        key.time_ms.min(
            metadata
                .duration_ms
                .saturating_sub(metadata.frame_interval_ms),
        ),
        token,
    )?;
    metadata.source.check()?;
    Ok(frame)
}
