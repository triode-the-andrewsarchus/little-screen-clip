//! One owned, cancellable player shared by full video and hover presentation.
use super::{media::VideoMetadata, preview_job::Task, search::PreviewContext};
use crate::file_source::FileSource;
use little_clip_core::{
    BackgroundJobs, JobError,
    media_backend::{
        self,
        playback::{Control, Settings, Update},
    },
};
use std::{
    fs::File,
    io::{self, Read, Write},
    os::fd::AsRawFd,
    process::{Child, ChildStdin, ChildStdout, Stdio},
    sync::{
        Arc, LazyLock, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

static PLAYERS: LazyLock<BackgroundJobs> = LazyLock::new(|| BackgroundJobs::new(1, 2));
#[derive(Default)]
struct Delivery {
    state: Option<Update>,
    frame: Option<(Update, Vec<u8>)>,
}

pub(super) struct Session {
    task: Task<Result<(), String>>,
    worker: Worker,
    control: Control,
    pending: Arc<Mutex<Option<Control>>>,
    delivery: Arc<Mutex<Delivery>>,
    source: Arc<Mutex<FileSource>>,
    pub texture: Option<egui::TextureHandle>,
    pub position_ms: u64,
    pub frame_position_ms: Option<u64>,
    frame_epoch: Option<u64>,
    pub duration_ms: u64,
    frame_interval_ms: u64,
    pub ended: bool,
    pub error: Option<String>,
    finished: bool,
}

impl Session {
    pub fn new(
        metadata: &VideoMetadata,
        start_ms: u64,
        silent: bool,
        width: u32,
        ctx: egui::Context,
        context: PreviewContext,
    ) -> Self {
        let start_ms = start_ms.min(
            metadata
                .duration_ms
                .saturating_sub(metadata.frame_interval_ms),
        );
        let pending = Arc::new(Mutex::new(None));
        let delivery = Arc::new(Mutex::new(Delivery::default()));
        let source = Arc::new(Mutex::new(metadata.source.clone()));
        let settings = Settings {
            stream: 0,
            width,
            height: width * 9 / 16,
            fps: if silent { 20 } else { 30 },
            start_ms,
            duration_ms: metadata.duration_ms,
            silent,
        };
        let worker = Worker {
            pending: pending.clone(),
            delivery: delivery.clone(),
            source: source.clone(),
            settings,
            ctx,
            context,
        };
        let job = worker.clone();
        let task = Task::start_unbounded(&PLAYERS, move |cancel| job.run(cancel));
        Self {
            task,
            worker,
            control: Control {
                epoch: 0,
                seek_ms: None,
                playing: true,
                volume: 1.0,
            },
            pending,
            delivery,
            source,
            texture: None,
            position_ms: start_ms,
            frame_position_ms: None,
            frame_epoch: None,
            duration_ms: metadata.duration_ms,
            frame_interval_ms: metadata.frame_interval_ms,
            ended: false,
            error: None,
            finished: false,
        }
    }
    pub fn relocate(&self, source: FileSource) {
        *self.source.lock().unwrap() = source;
    }
    pub fn playing(&self) -> bool {
        self.control.playing && !self.ended && self.error.is_none()
    }
    pub fn volume(&self) -> f64 {
        self.control.volume
    }
    pub fn seek(&mut self, milliseconds: u64) {
        self.control.epoch = self.control.epoch.wrapping_add(1);
        self.control.seek_ms =
            Some(milliseconds.min(self.duration_ms.saturating_sub(self.frame_interval_ms)));
        self.position_ms = self.control.seek_ms.unwrap();
        self.ended = false;
        self.send();
    }
    pub fn set_playing(&mut self, playing: bool) {
        self.control.playing = playing;
        if playing && self.ended {
            self.seek(0);
        } else {
            self.send();
        }
    }
    pub fn set_volume(&mut self, volume: f64) {
        self.control.volume = volume.clamp(0.0, 1.0);
        self.send();
    }
    fn send(&mut self) {
        let mut pending = self.pending.lock().unwrap();
        let mut next = self.control.clone();
        if next.seek_ms.is_none() {
            next.seek_ms = pending.as_ref().and_then(|old| old.seek_ms);
        }
        *pending = Some(next);
        self.control.seek_ms = None;
    }
    pub fn poll(&mut self, ctx: &egui::Context) {
        let mut delivery = self.delivery.lock().unwrap();
        if let Some((frame, pixels)) = delivery.frame.take()
            && frame.epoch == self.control.epoch
        {
            self.frame_position_ms = Some(frame.position_ms);
            self.frame_epoch = Some(frame.epoch);
            let image = egui::ColorImage::from_rgba_unmultiplied(
                [frame.width as usize, frame.height as usize],
                &pixels,
            );
            if let Some(texture) = &mut self.texture {
                texture.set(image, egui::TextureOptions::LINEAR);
            } else {
                self.texture =
                    Some(ctx.load_texture("video_playback", image, egui::TextureOptions::LINEAR));
            }
        }
        if let Some(state) = delivery.state.take()
            && state.epoch == self.control.epoch
        {
            self.position_ms = state.position_ms;
            if state.duration_ms != 0 {
                self.duration_ms = state.duration_ms;
            }
            self.ended = state.ended;
            if let Some(error) = state.error {
                self.error = Some(error);
            }
        }
        drop(delivery);
        if !self.finished
            && let Some(result) = self.task.poll()
        {
            self.finished = true;
            match result {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    self.error.get_or_insert(error);
                }
                Err(JobError::QueueFull) => {
                    let worker = self.worker.clone();
                    self.task = Task::start_unbounded(&PLAYERS, move |cancel| worker.run(cancel));
                    self.finished = false;
                }
                Err(JobError::Cancelled) => {}
                Err(error) => {
                    self.error = Some(error.to_string());
                }
            }
        }
        if !self.finished {
            ctx.request_repaint_after(Duration::from_millis(16));
        }
    }
}

#[derive(Clone)]
struct Worker {
    pending: Arc<Mutex<Option<Control>>>,
    delivery: Arc<Mutex<Delivery>>,
    source: Arc<Mutex<FileSource>>,
    settings: Settings,
    ctx: egui::Context,
    context: PreviewContext,
}
impl Worker {
    fn run(self, cancel: &AtomicBool) -> Result<(), String> {
        let source = self.source.lock().unwrap().clone();
        let (pin, _) =
            crate::file_source::ReadCheck::default().open(source.path(), Some(&source), cancel)?;
        let mut connection = self.context.read(source.path(), cancel, || {
            source.check_file(&pin)?;
            Connection::open(
                pin.try_clone().map_err(|error| error.to_string())?,
                &self.settings,
            )
        })?;
        let mut reader = Reader {
            connection: &mut connection,
            cancel,
            worker: &self,
            pin: &pin,
            last_guard: Instant::now(),
            last_data: Instant::now(),
        };
        while !cancel.load(Ordering::Relaxed) {
            let packet = Update::read(&mut reader).map_err(|error| error.to_string())?;
            self.check(&pin, cancel)?;
            let mut delivery = self.delivery.lock().unwrap();
            delivery.state = Some(packet.0.clone());
            if !packet.1.is_empty() {
                delivery.frame = Some(packet);
            }
            drop(delivery);
            self.ctx.request_repaint();
        }
        Ok(())
    }
    fn check(&self, pin: &File, cancel: &AtomicBool) -> Result<(), String> {
        let source = self.source.lock().unwrap().clone();
        self.context
            .read(source.path(), cancel, || source.check_file(pin))
    }
}

struct Connection {
    child: Child,
    input: ChildStdin,
    output: ChildStdout,
    pending: Option<Control>,
}
impl Connection {
    fn open(input: File, settings: &Settings) -> Result<Self, String> {
        let request = media_backend::playback_request(settings)?
            .arg_owned_content_file(input)
            .map_err(|error| error.to_string())?;
        let mut child = request
            .build_command()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| error.to_string())?;
        let input = child.stdin.take().unwrap();
        let output = child.stdout.take().unwrap();
        let connection = Self {
            child,
            input,
            output,
            pending: None,
        };
        // SAFETY: this connection exclusively owns its control pipe descriptor.
        let flags = unsafe { libc::fcntl(connection.input.as_raw_fd(), libc::F_GETFL) };
        if flags < 0
            || unsafe {
                libc::fcntl(
                    connection.input.as_raw_fd(),
                    libc::F_SETFL,
                    flags | libc::O_NONBLOCK,
                )
            } < 0
        {
            return Err(io::Error::last_os_error().to_string());
        }
        Ok(connection)
    }
    fn controls(&mut self, pending: &Mutex<Option<Control>>) -> io::Result<()> {
        if let Some(mut latest) = pending.lock().unwrap().take() {
            if latest.seek_ms.is_none() {
                latest.seek_ms = self
                    .pending
                    .as_ref()
                    .filter(|old| old.epoch == latest.epoch)
                    .and_then(|old| old.seek_ms);
            }
            self.pending = Some(latest);
        }
        if let Some(control) = &self.pending {
            let mut bytes = serde_json::to_vec(control)?;
            bytes.push(b'\n');
            // One nonblocking write below PIPE_BUF is atomic: backpressure keeps
            // the latest command for retry, never a partial JSON request.
            if bytes.len() > 1024 {
                return Err(io::Error::other("Playback command exceeds budget"));
            }
            match self.input.write(&bytes) {
                Ok(count) if count == bytes.len() => self.pending = None,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                Err(error) => return Err(error),
                _ => return Err(io::Error::other("Incomplete playback command")),
            }
        }
        Ok(())
    }
}
impl Drop for Connection {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
struct Reader<'a> {
    connection: &'a mut Connection,
    cancel: &'a AtomicBool,
    worker: &'a Worker,
    pin: &'a File,
    last_guard: Instant,
    last_data: Instant,
}
impl Read for Reader<'_> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        loop {
            if self.cancel.load(Ordering::Relaxed) {
                return Err(io::Error::other("Playback cancelled"));
            }
            if self.last_data.elapsed() > Duration::from_secs(30) {
                return Err(io::Error::other("Playback decoder timed out"));
            }
            if self.last_guard.elapsed() > Duration::from_secs(1) {
                self.worker
                    .check(self.pin, self.cancel)
                    .map_err(io::Error::other)?;
                self.last_guard = Instant::now();
            }
            self.connection.controls(&self.worker.pending)?;
            let mut descriptor = libc::pollfd {
                fd: self.connection.output.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            // SAFETY: the descriptor borrows this worker's live, exclusively read pipe.
            let ready = unsafe { libc::poll(&mut descriptor, 1, 10) };
            if ready < 0 {
                let error = io::Error::last_os_error();
                if error.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(error);
            }
            if ready == 0 {
                continue;
            }
            let count = self.connection.output.read(bytes)?;
            if count == 0 {
                return Err(io::Error::other(
                    "Playback decoder closed; check GStreamer codec support",
                ));
            }
            self.last_data = Instant::now();
            return Ok(count);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn metadata() -> VideoMetadata {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../little-clip-core/tests/fixtures/video/playback-colors.mp4");
        super::super::media::inspect_video(&path).unwrap()
    }
    fn wait(player: &mut Session, ctx: &egui::Context, ready: impl Fn(&Session) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            player.poll(ctx);
            assert!(player.error.is_none(), "{:?}", player.error);
            if ready(player) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "playback did not deliver expected state"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    #[test]
    fn shared_player_delivers_real_frames_and_rejects_stale_seek_epochs() {
        let ctx = egui::Context::default();
        let mut player = Session::new(&metadata(), 100, true, 320, ctx.clone(), Default::default());
        wait(&mut player, &ctx, |player| {
            player.frame_position_ms.is_some_and(|ms| ms >= 400)
        });
        player.seek(1800);
        let expected = player.control.epoch;
        wait(&mut player, &ctx, |player| {
            player.frame_epoch == Some(expected)
        });
        assert!(player.frame_position_ms.unwrap() >= 1800);
        player.set_playing(false);
        let stale = Update {
            epoch: expected - 1,
            position_ms: 10,
            duration_ms: 3000,
            playing: true,
            ended: false,
            width: 1,
            height: 1,
            bytes: 4,
            error: None,
        };
        player.delivery.lock().unwrap().frame = Some((stale, vec![0; 4]));
        player.poll(&ctx);
        assert_eq!(player.frame_epoch, Some(expected));
        assert_ne!(player.texture.as_ref().unwrap().size(), [1, 1]);
        let token = player.task.token.clone();
        drop(player);
        assert!(token.load(Ordering::Relaxed));
        // The same single-player executor cannot run this until the decoder's
        // owner has returned through kill/reap, proving cancellation releases it.
        let task = Task::start_unbounded(&PLAYERS, |_| ());
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(result) = task.poll() {
                result.unwrap();
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    #[test]
    fn playback_never_opens_a_source_outside_published_search_results() {
        let ctx = egui::Context::default();
        let mut player = Session::new(
            &metadata(),
            0,
            true,
            320,
            ctx.clone(),
            PreviewContext::empty_results(),
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        while player.error.is_none() {
            player.poll(&ctx);
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(player.error.unwrap().contains("outside"));
        assert!(player.texture.is_none());
    }
}
