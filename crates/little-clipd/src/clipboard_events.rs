//! Selection identities and tagged daemon publications on a dedicated Wayland connection.
use std::{
    collections::HashMap,
    fs::File,
    io::{self, Write},
    os::{
        fd::{AsFd, OwnedFd},
        unix::net::UnixStream,
    },
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    time::Duration,
};
use tokio::sync::{Semaphore, oneshot, watch};
use wayland_client::{
    Connection, Dispatch, Proxy, QueueHandle,
    backend::ObjectId,
    delegate_noop, event_created_child,
    globals::{GlobalListContents, registry_queue_init},
    protocol::{wl_callback, wl_registry, wl_seat},
};
use wayland_protocols_wlr::data_control::v1::client::{
    zwlr_data_control_device_v1 as device, zwlr_data_control_manager_v1 as manager,
    zwlr_data_control_offer_v1 as offer, zwlr_data_control_source_v1 as source,
};

const TRANSFER_LIMIT: u64 = 256 * 1024 * 1024;
const TRANSFER_TIMEOUT: Duration = Duration::from_secs(5);
type Reply = Mutex<Option<oneshot::Sender<()>>>;

pub(super) struct Incoming {
    pub serial: u64,
    pub observed: tokio::time::Instant,
    pub initial: bool,
    mime: String,
    reader: Mutex<Option<UnixStream>>,
}

impl Incoming {
    pub async fn read(&self) -> Result<(Vec<u8>, String), String> {
        use tokio::io::AsyncReadExt;
        let reader = self
            .reader
            .lock()
            .map_err(|_| "Clipboard selection state is unavailable")?
            .take()
            .ok_or("Selection already read")?;
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
    serial: Arc<AtomicU64>,
}

impl Clipboard {
    pub fn spawn() -> io::Result<(Self, watch::Receiver<Option<Arc<Incoming>>>)> {
        Self::spawn_on(|| Connection::connect_to_env().map_err(|error| error.to_string()))
    }

    fn spawn_on(
        connect: impl FnOnce() -> Result<Connection, String> + Send + 'static,
    ) -> io::Result<(Self, watch::Receiver<Option<Arc<Incoming>>>)> {
        let (commands, requests) = mpsc::sync_channel(16);
        let (selection, receiver) = watch::channel(None);
        let serial = Arc::new(AtomicU64::new(0));
        let worker_serial = serial.clone();
        let runtime = tokio::runtime::Handle::current();
        std::thread::Builder::new()
            .name("clipboard-events".into())
            .spawn(move || {
                if let Err(error) = connect()
                    .map_err(|error| -> Box<dyn std::error::Error> { error.into() })
                    .and_then(|connection| {
                        run(connection, requests, selection, worker_serial, runtime)
                    })
                {
                    tracing::warn!(%error, "Wayland clipboard event connection stopped");
                }
            })?;
        Ok((Self { commands, serial }, receiver))
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
            Ok(()) => {}
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

struct State {
    selection: watch::Sender<Option<Arc<Incoming>>>,
    serial: Arc<AtomicU64>,
    own_markers: HashMap<ObjectId, String>,
    offers: HashMap<ObjectId, Vec<String>>,
    current: Option<offer::ZwlrDataControlOfferV1>,
    initialized: bool,
    finished: bool,
    runtime: tokio::runtime::Handle,
    writes: Arc<Semaphore>,
}

fn run(
    connection: Connection,
    requests: mpsc::Receiver<Request>,
    selection: watch::Sender<Option<Arc<Incoming>>>,
    serial: Arc<AtomicU64>,
    runtime: tokio::runtime::Handle,
) -> Result<(), Box<dyn std::error::Error>> {
    let (globals, mut queue) = registry_queue_init::<State>(&connection)?;
    let qh = queue.handle();
    let manager: manager::ZwlrDataControlManagerV1 = globals.bind(&qh, 1..=2, ())?;
    let seat: wl_seat::WlSeat = globals.bind(&qh, 1..=1, ())?;
    let device = manager.get_data_device(&seat, &qh, ());
    let mut state = State {
        selection,
        serial,
        own_markers: HashMap::new(),
        offers: HashMap::new(),
        current: None,
        initialized: false,
        finished: false,
        runtime,
        writes: Arc::new(Semaphore::new(8)),
    };
    loop {
        queue.dispatch_pending(&mut state)?;
        if state.finished {
            return Err("Clipboard seat retired".into());
        }
        loop {
            match requests.try_recv() {
                Ok(request) => {
                    if let Command::Publish { bytes, mime } = request.command {
                        let source = manager.create_data_source(&qh, bytes);
                        source.offer(mime.clone());
                        if mime == "text/plain;charset=utf-8" {
                            source.offer("text/plain".into());
                            source.offer("UTF8_STRING".into());
                        }
                        let marker =
                            format!("application/x-little-screen-clip-{}", uuid::Uuid::new_v4());
                        source.offer(marker.clone());
                        state.own_markers.insert(source.id(), marker);
                        device.set_selection(Some(&source));
                    }
                    connection
                        .display()
                        .sync(&qh, Mutex::new(Some(request.reply)));
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => return Ok(()),
            }
        }
        connection.flush()?;
        let Some(read) = queue.prepare_read() else {
            continue;
        };
        let ready = {
            let descriptor = read.connection_fd();
            let mut fds = [rustix::event::PollFd::new(
                &descriptor,
                rustix::event::PollFlags::IN,
            )];
            rustix::event::poll(
                &mut fds,
                Some(&rustix::event::Timespec {
                    tv_sec: 0,
                    tv_nsec: 25_000_000,
                }),
            )
        };
        match ready {
            Ok(0) | Err(rustix::io::Errno::INTR) => {}
            Ok(_) => {
                read.read()?;
            }
            Err(error) => return Err(error.into()),
        }
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
            .find_map(|wanted| types.iter().find(|mime| mime.as_str() == wanted))
        })
        .or_else(|| types.iter().find(|mime| mime.starts_with("text/")))
        .or_else(|| types.first())
        .cloned()
}

impl Dispatch<device::ZwlrDataControlDeviceV1, ()> for State {
    event_created_child!(State, device::ZwlrDataControlDeviceV1, [0 => (offer::ZwlrDataControlOfferV1, ())]);
    fn event(
        state: &mut Self,
        _: &device::ZwlrDataControlDeviceV1,
        event: device::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            device::Event::Selection { id } => {
                if let Some(old) = state.current.take() {
                    old.destroy();
                }
                let serial = state.serial.fetch_add(1, Ordering::AcqRel) + 1;
                let initial = !std::mem::replace(&mut state.initialized, true);
                let incoming = id.as_ref().and_then(|offer| {
                    let types = state.offers.remove(&offer.id()).unwrap_or_default();
                    if state
                        .own_markers
                        .values()
                        .any(|marker| types.contains(marker))
                    {
                        return None;
                    }
                    let mime = preferred_mime(&types)?;
                    let (reader, writer) = UnixStream::pair().ok()?;
                    offer.receive(mime.clone(), writer.as_fd());
                    Some(Arc::new(Incoming {
                        serial,
                        observed: tokio::time::Instant::now(),
                        initial,
                        mime,
                        reader: Mutex::new(Some(reader)),
                    }))
                });
                state.current = id;
                state.selection.send_replace(incoming);
            }
            device::Event::PrimarySelection { id: Some(offer) } => {
                state.offers.remove(&offer.id());
                offer.destroy();
            }
            device::Event::Finished => state.finished = true,
            _ => {}
        }
    }
}
impl Dispatch<offer::ZwlrDataControlOfferV1, ()> for State {
    fn event(
        state: &mut Self,
        proxy: &offer::ZwlrDataControlOfferV1,
        event: offer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let offer::Event::Offer { mime_type } = event {
            state.offers.entry(proxy.id()).or_default().push(mime_type);
        }
    }
}
impl Dispatch<source::ZwlrDataControlSourceV1, Arc<[u8]>> for State {
    fn event(
        state: &mut Self,
        source: &source::ZwlrDataControlSourceV1,
        event: source::Event,
        bytes: &Arc<[u8]>,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            source::Event::Cancelled => {
                state.own_markers.remove(&source.id());
                source.destroy();
            }
            source::Event::Send { fd, .. } => {
                let Ok(permit) = state.writes.clone().try_acquire_owned() else {
                    return;
                };
                let bytes = bytes.clone();
                state.runtime.spawn(async move {
                    let _permit = permit;
                    let _ =
                        tokio::time::timeout(TRANSFER_TIMEOUT, write_selection(fd, &bytes)).await;
                });
            }
            _ => {}
        }
    }
}

async fn write_selection(fd: OwnedFd, mut bytes: &[u8]) -> io::Result<()> {
    // SAFETY: fd is owned and valid. Only its file-status flags are changed.
    if let Err(error) = rustix::fs::fcntl_getfl(&fd)
        .and_then(|flags| rustix::fs::fcntl_setfl(&fd, flags | rustix::fs::OFlags::NONBLOCK))
    {
        return Err(error.into());
    }
    // SAFETY: AsyncFd owns this nonblocking file until registration is removed.
    // It is never closed or replaced through an alias while registered.
    let fd = unsafe { tokio::io::unix::AsyncFd::register(File::from(fd))? };
    while !bytes.is_empty() {
        let mut ready = fd.writable().await?;
        if let Ok(result) = ready.try_io(|inner| inner.get_ref().write(bytes)) {
            let written = result?;
            if written == 0 {
                return Err(io::ErrorKind::WriteZero.into());
            }
            bytes = &bytes[written..];
        }
    }
    Ok(())
}

impl Dispatch<wl_callback::WlCallback, Reply> for State {
    fn event(
        _: &mut Self,
        _: &wl_callback::WlCallback,
        _: wl_callback::Event,
        reply: &Reply,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // The option only owns a one-shot acknowledgement, so taking it is
        // safe even if an earlier callback panicked while holding the guard.
        if let Some(reply) = reply
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take()
        {
            let _ = reply.send(());
        }
    }
}
impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}
delegate_noop!(State: ignore manager::ZwlrDataControlManagerV1);
delegate_noop!(State: ignore wl_seat::WlSeat);

#[cfg(test)]
mod tests;
