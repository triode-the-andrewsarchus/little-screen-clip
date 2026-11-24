//! Selection identities and tagged daemon publications on a native Wayland or X11 connection.

mod wayland;
mod x11;

use little_clip_core::SessionKind;
use std::{
    io::{self, Write as _},
    os::unix::net::UnixStream,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    time::Duration,
};
use tokio::sync::{oneshot, watch};
#[cfg(test)]
use {std::fs::File, std::os::fd::AsFd, wayland_client::Connection};

pub(super) const TRANSFER_LIMIT: u64 = 256 * 1024 * 1024;
const TRANSFER_TIMEOUT: Duration = Duration::from_secs(5);

/// Runs a read-only compatibility adapter with the native transfer timeout.
/// Incomplete, oversized and failed reads never become clipboard content.
#[cfg(test)]
pub(super) async fn read_with_tool(
    request: little_clip_core::CommandRequest,
    limit: usize,
) -> Option<Vec<u8>> {
    let request = request
        .timeout(TRANSFER_TIMEOUT)
        .max_output_bytes(limit)
        .own_process_group()
        .die_with_parent();
    let output = tokio::task::spawn_blocking(move || request.run())
        .await
        .ok()?
        .ok()?;
    (output.status.success() && !output.stdout_truncated).then_some(output.stdout)
}

enum IncomingReader {
    Wayland(UnixStream),
    X11(oneshot::Receiver<Result<Vec<u8>, String>>),
}

pub(super) struct Incoming {
    pub serial: u64,
    pub observed: tokio::time::Instant,
    pub initial: bool,
    mime: String,
    reader: Mutex<Option<IncomingReader>>,
}

impl Incoming {
    pub async fn read(&self) -> Result<(Vec<u8>, String), String> {
        let reader = self
            .reader
            .lock()
            .map_err(|_| "Clipboard selection state is unavailable")?
            .take()
            .ok_or("Selection already read")?;
        match reader {
            IncomingReader::Wayland(stream) => self.read_wayland(stream).await,
            IncomingReader::X11(rx) => self.read_x11(rx).await,
        }
    }

    async fn read_wayland(&self, reader: UnixStream) -> Result<(Vec<u8>, String), String> {
        use tokio::io::AsyncReadExt;
        reader
            .set_nonblocking(true)
            .map_err(|error| error.to_string())?;
        let reader = tokio::net::UnixStream::from_std(reader).map_err(|error| error.to_string())?;
        let mut bytes = Vec::new();
        tokio::time::timeout(
            TRANSFER_TIMEOUT,
            reader.take(TRANSFER_LIMIT + 1).read_to_end(&mut bytes),
        )
        .await
        .map_err(|_| "Clipboard source timed out".to_string())?
        .map_err(|error| error.to_string())?;
        if bytes.len() as u64 > TRANSFER_LIMIT {
            return Err("Clipboard selection exceeds 256 MiB".into());
        }
        Ok((bytes, self.mime.clone()))
    }

    async fn read_x11(
        &self,
        rx: oneshot::Receiver<Result<Vec<u8>, String>>,
    ) -> Result<(Vec<u8>, String), String> {
        let result = tokio::time::timeout(TRANSFER_TIMEOUT, rx)
            .await
            .map_err(|_| "Clipboard source timed out".to_string())?
            .map_err(|_| "Clipboard selection state is unavailable".to_string())?;
        result.map(|bytes| (bytes, self.mime.clone()))
    }
}

enum Command {
    Publish { bytes: Arc<[u8]>, mime: String },
    Barrier,
}

struct Request {
    command: Command,
    reply: oneshot::Sender<()>,
}

pub(super) struct Clipboard {
    commands: mpsc::SyncSender<Request>,
    wakeup: Option<UnixStream>,
    serial: Arc<AtomicU64>,
}

impl Clipboard {
    pub fn spawn() -> io::Result<(Self, watch::Receiver<Option<Arc<Incoming>>>)> {
        match little_clip_core::detect_session_kind() {
            SessionKind::X11 => Self::spawn_x11(),
            SessionKind::Wayland | SessionKind::Auto => Self::spawn_on(|| {
                wayland_client::Connection::connect_to_env().map_err(|error| error.to_string())
            }),
        }
    }

    fn spawn_x11() -> io::Result<(Self, watch::Receiver<Option<Arc<Incoming>>>)> {
        let (wakeup_rd, wakeup_wr) = UnixStream::pair()?;
        wakeup_rd.set_nonblocking(true)?;
        wakeup_wr.set_nonblocking(true)?;
        let (commands, requests) = mpsc::sync_channel(16);
        let (selection, receiver) = watch::channel(None);
        let serial = Arc::new(AtomicU64::new(0));
        let worker_serial = serial.clone();
        x11::spawn(requests, wakeup_rd, selection, worker_serial)?;
        Ok((
            Self {
                commands,
                wakeup: Some(wakeup_wr),
                serial,
            },
            receiver,
        ))
    }

    fn spawn_on(
        connect: impl FnOnce() -> Result<wayland_client::Connection, String> + Send + 'static,
    ) -> io::Result<(Self, watch::Receiver<Option<Arc<Incoming>>>)> {
        let (commands, requests) = mpsc::sync_channel(16);
        let (selection, receiver) = watch::channel(None);
        let serial = Arc::new(AtomicU64::new(0));
        let worker_serial = serial.clone();
        let runtime = tokio::runtime::Handle::current();
        wayland::spawn_on(connect, requests, selection, worker_serial, runtime)?;
        Ok((
            Self {
                commands,
                wakeup: None,
                serial,
            },
            receiver,
        ))
    }

    pub fn is_current(&self, serial: u64) -> bool {
        self.serial.load(Ordering::Acquire) == serial
    }

    pub async fn publish(&self, bytes: Arc<[u8]>, mime: String) -> Result<bool, String> {
        self.request(Command::Publish { bytes, mime }).await
    }

    pub async fn barrier(&self) -> Result<bool, String> {
        self.request(Command::Barrier).await
    }

    // A closed command channel allows the existing external adapter to take over.
    async fn request(&self, command: Command) -> Result<bool, String> {
        let (reply, received) = oneshot::channel();
        match self.commands.try_send(Request { command, reply }) {
            Ok(()) => {
                if let Some(mut wakeup) = self.wakeup.as_ref() {
                    let _ = wakeup.write(&[1u8]);
                }
            }
            Err(mpsc::TrySendError::Disconnected(_)) => return Ok(false),
            Err(mpsc::TrySendError::Full(_)) => {
                return Err("Clipboard request queue is full".into());
            }
        }
        Ok(tokio::time::timeout(TRANSFER_TIMEOUT, received)
            .await
            .map_err(|_| "Clipboard compositor acknowledgement timed out".to_string())?
            .is_ok())
    }
}

fn preferred_mime(types: &[String]) -> Option<String> {
    types
        .iter()
        .find(|mime| mime.as_str() == "image/png")
        .or_else(|| {
            types
                .iter()
                .find(|mime| little_clip_core::item::image_extension_for_mime(mime).is_some())
        })
        .or_else(|| {
            [
                "text/uri-list",
                "text/plain;charset=utf-8",
                "text/plain",
                "UTF8_STRING",
                "TEXT",
                "STRING",
            ]
            .into_iter()
            .find_map(|wanted| {
                types.iter().find(|mime| {
                    mime.as_str() == wanted
                        || (wanted == "text/plain;charset=utf-8"
                            && mime.eq_ignore_ascii_case(wanted))
                })
            })
        })
        .or_else(|| types.iter().find(|mime| mime.starts_with("text/")))
        .or_else(|| types.first())
        .cloned()
}

#[cfg(test)]
mod tests;
