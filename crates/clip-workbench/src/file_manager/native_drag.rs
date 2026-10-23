//! Native Wayland URI drags on eframe's existing display, using an independent event queue.
mod io;
mod protocol;

use super::filesystem::Identity;
use little_clip_core::{BackgroundJobs, JobError};
use raw_window_handle::{HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::time::{Duration, Instant};
use wayland_client::{
    Connection, EventQueue, Proxy, QueueHandle,
    backend::{Backend, ObjectId},
    protocol::{
        wl_data_device, wl_data_device_manager, wl_data_offer, wl_data_source, wl_pointer, wl_seat,
        wl_surface,
    },
};

const MIME: &str = "text/uri-list";
type Action = wl_data_device_manager::DndAction;
type ResultReceiver<T> = mpsc::Receiver<Result<Result<T, String>, JobError>>;

pub(super) enum Event {
    Drop(u64, egui::Pos2),
    Files(u64, Vec<PathBuf>, bool),
    Remove(
        BTreeMap<PathBuf, Option<Identity>>,
        BTreeMap<PathBuf, Identity>,
    ),
    Failed(u64, String),
    Notice(String),
    Refresh,
}
struct Incoming {
    offer: wl_data_offer::WlDataOffer,
    point: egui::Pos2,
    action: Action,
    dropped: bool,
}
struct Reading {
    receiver: ResultReceiver<Vec<PathBuf>>,
    cancelled: Arc<AtomicBool>,
    offer: wl_data_offer::WlDataOffer,
    moving: bool,
}
struct Source {
    proxy: wl_data_source::WlDataSource,
    payload: Arc<Vec<u8>>,
    identities: BTreeMap<PathBuf, Option<Identity>>,
    snapshot: Arc<Mutex<BTreeMap<PathBuf, Identity>>>,
    cancelled: Arc<AtomicBool>,
    written: Arc<AtomicBool>,
    action: Action,
    movable: bool,
}
struct State {
    surface: wl_surface::WlSurface,
    manager: Option<wl_data_device_manager::WlDataDeviceManager>,
    seats: Vec<wl_seat::WlSeat>,
    devices: Vec<(wl_seat::WlSeat, wl_data_device::WlDataDevice)>,
    pointers: Vec<wl_pointer::WlPointer>,
    pointer_surface: Option<ObjectId>,
    grab: Option<(wl_seat::WlSeat, u32)>,
    offers: Vec<wl_data_offer::WlDataOffer>,
    uri_offers: Vec<ObjectId>,
    copy_offers: Vec<ObjectId>,
    incoming: Option<Incoming>,
    reference_targets: Vec<egui::Rect>,
    reading: BTreeMap<u64, Reading>,
    awaiting: BTreeMap<u64, wl_data_offer::WlDataOffer>,
    source: Option<Source>,
    writes: Vec<ResultReceiver<()>>,
    events: Vec<Event>,
    next: u64,
    release_pointer: bool,
}

pub(super) struct NativeDrag {
    connection: Connection,
    queue: EventQueue<State>,
    state: State,
}

impl NativeDrag {
    pub(super) fn new(cc: &eframe::CreationContext<'_>) -> Option<Self> {
        let RawDisplayHandle::Wayland(display) = cc.display_handle().ok()?.as_raw() else {
            return None;
        };
        let RawWindowHandle::Wayland(window) = cc.window_handle().ok()?.as_raw() else {
            return None;
        };
        // SAFETY: eframe owns this display and surface for the app's entire lifetime.
        // The foreign backend never disconnects the display. We never destroy the borrowed surface.
        let backend = unsafe { Backend::from_foreign_display(display.display.as_ptr().cast()) };
        let connection = Connection::from_backend(backend);
        let id = unsafe {
            ObjectId::from_ptr(
                wl_surface::WlSurface::interface(),
                window.surface.as_ptr().cast(),
            )
            .ok()?
        };
        let surface = wl_surface::WlSurface::from_id(&connection, id).ok()?;
        let queue = connection.new_event_queue();
        connection.display().get_registry(&queue.handle(), ());
        let state = State {
            surface,
            manager: None,
            seats: Vec::new(),
            devices: Vec::new(),
            pointers: Vec::new(),
            pointer_surface: None,
            grab: None,
            offers: Vec::new(),
            uri_offers: Vec::new(),
            copy_offers: Vec::new(),
            incoming: None,
            reference_targets: Vec::new(),
            reading: BTreeMap::new(),
            awaiting: BTreeMap::new(),
            source: None,
            writes: Vec::new(),
            events: Vec::new(),
            next: 0,
            release_pointer: false,
        };
        connection.flush().ok()?;
        Some(Self {
            connection,
            queue,
            state,
        })
    }

    pub(super) fn set_reference_targets(&mut self, rects: Vec<egui::Rect>) {
        self.state.reference_targets = rects;
    }
    pub(super) fn drag_position(&self) -> Option<egui::Pos2> {
        self.state
            .incoming
            .as_ref()
            .filter(|incoming| !incoming.dropped)
            .map(|incoming| incoming.point)
    }

    pub(super) fn tick(&mut self) -> Vec<Event> {
        if let Err(error) = self.queue.dispatch_pending(&mut self.state) {
            self.state
                .events
                .push(Event::Notice(format!("Desktop drag unavailable: {error}")));
        }
        let ready: Vec<_> = self
            .state
            .reading
            .iter()
            .filter_map(|(id, read)| match read.receiver.try_recv() {
                Ok(result) => Some((*id, result.map_err(|e| e.to_string()).and_then(|r| r))),
                Err(mpsc::TryRecvError::Disconnected) => {
                    Some((*id, Err("Desktop reader disconnected".into())))
                }
                Err(mpsc::TryRecvError::Empty) => None,
            })
            .collect();
        for (id, result) in ready {
            let read = self.state.reading.remove(&id).expect("ready read");
            match result {
                Ok(paths) => {
                    self.state.awaiting.insert(id, read.offer);
                    self.state.events.push(Event::Files(id, paths, read.moving));
                }
                Err(error) => {
                    read.offer.destroy();
                    self.state.events.push(Event::Failed(id, error));
                }
            }
        }
        self.state
            .writes
            .retain(|receiver| match receiver.try_recv() {
                Ok(Ok(Ok(()))) => false,
                Ok(result) => {
                    self.state.events.push(Event::Notice(format!(
                        "Desktop transfer failed: {result:?}"
                    )));
                    false
                }
                Err(mpsc::TryRecvError::Disconnected) => false,
                Err(mpsc::TryRecvError::Empty) => true,
            });
        let _ = self.connection.flush();
        std::mem::take(&mut self.state.events)
    }

    pub(super) fn start(
        &mut self,
        identities: BTreeMap<PathBuf, Option<Identity>>,
        copy: bool,
    ) -> Result<bool, String> {
        if self.state.source.is_some() {
            return Ok(false);
        }
        let Some((seat, serial)) = self.state.grab.clone() else {
            return Ok(false);
        };
        let Some((_, device)) = self.state.devices.iter().find(|(s, _)| s == &seat) else {
            return Ok(false);
        };
        let paths: Vec<_> = identities.keys().cloned().collect();
        let payload = little_clip_core::file_uri_list(&paths)
            .map_err(|e| e.to_string())?
            .into_bytes();
        if payload.len() > io::LIMIT {
            return Err("Desktop file references exceed 1 MiB".into());
        }
        let source = self
            .state
            .manager
            .as_ref()
            .expect("device has manager")
            .create_data_source(&self.queue.handle(), ());
        source.offer(MIME.into());
        let movable = !copy && identities.values().all(Option::is_some);
        source.set_actions(if movable {
            Action::Copy | Action::Move
        } else {
            Action::Copy
        });
        device.start_drag(Some(&source), &self.state.surface, None, serial);
        self.state.source = Some(Source {
            proxy: source,
            payload: Arc::new(payload),
            identities,
            snapshot: Arc::new(Mutex::new(BTreeMap::new())),
            cancelled: Arc::new(AtomicBool::new(false)),
            written: Arc::new(AtomicBool::new(false)),
            action: Action::Copy,
            movable,
        });
        self.state.release_pointer = true;
        let _ = self.connection.flush();
        Ok(true)
    }

    pub(super) fn finish(&mut self, token: u64, success: bool) {
        if let Some(offer) = self.state.awaiting.remove(&token) {
            self.state.uri_offers.retain(|id| id != &offer.id());
            self.state.copy_offers.retain(|id| id != &offer.id());
            if success {
                offer.finish();
            }
            offer.destroy();
            let _ = self.connection.flush();
        }
    }

    pub(super) fn busy(&self) -> bool {
        self.state.source.is_some()
            || self.state.incoming.is_some()
            || !self.state.reading.is_empty()
            || !self.state.awaiting.is_empty()
    }

    pub(super) fn raw_input(&mut self, ctx: &egui::Context, input: &mut egui::RawInput) {
        if self.state.release_pointer {
            self.state.release_pointer = false;
            egui::DragAndDrop::clear_payload(ctx);
            ctx.stop_dragging();
            let pos = ctx.pointer_latest_pos().unwrap_or_default();
            input.events.push(egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: input.modifiers,
            });
        }
    }
}

impl State {
    fn devices(&mut self, qh: &QueueHandle<Self>) {
        let Some(manager) = &self.manager else { return };
        for seat in &self.seats {
            if !self.devices.iter().any(|(existing, _)| existing == seat) {
                self.devices
                    .push((seat.clone(), manager.get_data_device(seat, qh, ())));
            }
        }
    }
    fn destroy_offer(&mut self, offer: &wl_data_offer::WlDataOffer) {
        self.offers.retain(|existing| existing != offer);
        self.uri_offers.retain(|id| id != &offer.id());
        self.copy_offers.retain(|id| id != &offer.id());
        offer.destroy();
    }
}

impl Drop for NativeDrag {
    fn drop(&mut self) {
        if let Some(source) = self.state.source.take() {
            source.cancelled.store(true, Ordering::Relaxed);
            source.proxy.destroy();
        }
        for (_, read) in std::mem::take(&mut self.state.reading) {
            read.cancelled.store(true, Ordering::Relaxed);
            read.offer.destroy();
        }
        for (_, offer) in std::mem::take(&mut self.state.awaiting) {
            offer.destroy();
        }
        for offer in self.state.offers.drain(..) {
            offer.destroy();
        }
        for pointer in self.state.pointers.drain(..) {
            pointer.release();
        }
        for (_, device) in self.state.devices.drain(..) {
            device.release();
        }
        for seat in self.state.seats.drain(..) {
            seat.release();
        }
    }
}
