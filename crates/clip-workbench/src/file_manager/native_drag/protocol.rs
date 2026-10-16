use super::*;
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::net::UnixStream;
use wayland_client::{Dispatch, WEnum, delegate_noop, event_created_child, protocol::wl_registry};

impl Dispatch<wl_registry::WlRegistry, ()> for State {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        {
            match interface.as_str() {
                "wl_data_device_manager" if version >= 3 => {
                    state.manager = Some(registry.bind(name, 3, qh, ()));
                    state.devices(qh);
                }
                "wl_seat" if version >= 5 => {
                    state
                        .seats
                        .push(registry.bind(name, version.min(7), qh, ()));
                    state.devices(qh);
                }
                _ => {}
            }
        }
    }
}
impl Dispatch<wl_seat::WlSeat, ()> for State {
    fn event(
        state: &mut Self,
        seat: &wl_seat::WlSeat,
        event: wl_seat::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_seat::Event::Capabilities {
            capabilities: WEnum::Value(caps),
        } = event
            && caps.contains(wl_seat::Capability::Pointer)
            && !state
                .pointers
                .iter()
                .any(|p| p.data::<wl_seat::WlSeat>() == Some(seat))
        {
            state.pointers.push(seat.get_pointer(qh, seat.clone()));
        }
    }
}
impl Dispatch<wl_pointer::WlPointer, wl_seat::WlSeat> for State {
    fn event(
        state: &mut Self,
        _: &wl_pointer::WlPointer,
        event: wl_pointer::Event,
        seat: &wl_seat::WlSeat,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_pointer::Event::Enter { surface, .. } => state.pointer_surface = Some(surface.id()),
            wl_pointer::Event::Leave { .. } => state.pointer_surface = None,
            wl_pointer::Event::Button {
                serial,
                button: 0x110,
                state: WEnum::Value(wl_pointer::ButtonState::Pressed),
                ..
            } if state.pointer_surface.as_ref() == Some(&state.surface.id()) => {
                state.grab = Some((seat.clone(), serial))
            }
            wl_pointer::Event::Button {
                state: WEnum::Value(wl_pointer::ButtonState::Released),
                ..
            } => state.grab = None,
            _ => {}
        }
    }
}
impl Dispatch<wl_data_device::WlDataDevice, ()> for State {
    event_created_child!(State, wl_data_device::WlDataDevice, [0 => (wl_data_offer::WlDataOffer, ())]);
    fn event(
        state: &mut Self,
        _: &wl_data_device::WlDataDevice,
        event: wl_data_device::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_data_device::Event::DataOffer { id } => state.offers.push(id),
            wl_data_device::Event::Selection { id: Some(id) } => state.destroy_offer(&id),
            wl_data_device::Event::Enter {
                serial,
                surface,
                x,
                y,
                id: Some(offer),
            } => {
                if surface.id() != state.surface.id() {
                    state.destroy_offer(&offer);
                    return;
                }
                state.incoming = Some(Incoming {
                    offer: offer.clone(),
                    point: egui::pos2(x as f32, y as f32),
                    action: Action::Copy,
                    dropped: false,
                });
                offer.accept(serial, Some(MIME.into()));
                state.update_drop_action();
            }
            wl_data_device::Event::Motion { x, y, .. } => {
                if let Some(incoming) = &mut state.incoming {
                    incoming.point = egui::pos2(x as f32, y as f32);
                }
                state.update_drop_action();
            }
            wl_data_device::Event::Leave => {
                if let Some(incoming) = state.incoming.take()
                    && !incoming.dropped
                {
                    state.destroy_offer(&incoming.offer);
                }
            }
            wl_data_device::Event::Drop => state.read_drop(),
            _ => {}
        }
    }
}
impl State {
    fn update_drop_action(&self) {
        if let Some(incoming) = &self.incoming {
            if self
                .reference_target
                .is_some_and(|rect| rect.contains(incoming.point))
            {
                incoming.offer.set_actions(Action::Copy, Action::Copy);
            } else {
                incoming
                    .offer
                    .set_actions(Action::Copy | Action::Move, Action::Move);
            }
        }
    }
    fn read_drop(&mut self) {
        let Some(incoming) = &mut self.incoming else {
            return;
        };
        if incoming.dropped {
            return;
        }
        let reference = self
            .reference_target
            .is_some_and(|rect| rect.contains(incoming.point));
        if !self.uri_offers.contains(&incoming.offer.id())
            || (reference && !self.copy_offers.contains(&incoming.offer.id()))
            || (!reference && !matches!(incoming.action, Action::Copy | Action::Move))
        {
            return;
        }
        if reference {
            // Motion and Drop can arrive in one dispatch. Request Copy before
            // receiving data instead of relying on an older Action notification.
            // The source explicitly offers Copy and receives these requests in order.
            incoming.offer.set_actions(Action::Copy, Action::Copy);
        }
        incoming.dropped = true;
        self.next += 1;
        let token = self.next;
        self.events.push(Event::Drop(token, incoming.point));
        let offer = incoming.offer.clone();
        self.offers.retain(|existing| existing != &offer);
        let result = UnixStream::pair().map_err(|e| e.to_string());
        let (reader, writer) = match result {
            Ok(pair) => pair,
            Err(error) => {
                offer.destroy();
                self.events.push(Event::Failed(token, error));
                return;
            }
        };
        offer.receive(MIME.into(), writer.as_fd());
        drop(writer);
        let cancelled = Arc::new(AtomicBool::new(false));
        let cancel = Arc::clone(&cancelled);
        let deadline = Instant::now() + Duration::from_secs(10);
        let (_, receiver) = BackgroundJobs::global().submit(Duration::MAX, move || {
            io::read(OwnedFd::from(reader), deadline, cancel)
        });
        self.reading.insert(
            token,
            Reading {
                receiver,
                cancelled,
                offer,
                moving: !reference && incoming.action == Action::Move,
            },
        );
    }
}
impl Dispatch<wl_data_offer::WlDataOffer, ()> for State {
    fn event(
        state: &mut Self,
        offer: &wl_data_offer::WlDataOffer,
        event: wl_data_offer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_data_offer::Event::Offer { ref mime_type } = event
            && mime_type == MIME
        {
            state.uri_offers.push(offer.id());
        }
        if let wl_data_offer::Event::SourceActions {
            source_actions: WEnum::Value(actions),
        } = event
            && actions.contains(Action::Copy)
        {
            state.copy_offers.push(offer.id());
        }
        if let wl_data_offer::Event::Action {
            dnd_action: WEnum::Value(action),
        } = event
            && let Some(incoming) = &mut state.incoming
            && &incoming.offer == offer
        {
            incoming.action = action;
        }
    }
}
impl Dispatch<wl_data_source::WlDataSource, ()> for State {
    fn event(
        state: &mut Self,
        proxy: &wl_data_source::WlDataSource,
        event: wl_data_source::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let Some(source) = &mut state.source else {
            return;
        };
        if &source.proxy != proxy {
            return;
        }
        match event {
            wl_data_source::Event::Send { mime_type, fd } if mime_type == MIME => {
                let bytes = Arc::clone(&source.payload);
                let roots = source.identities.clone();
                let snapshot = Arc::clone(&source.snapshot);
                let cancelled = Arc::clone(&source.cancelled);
                let written = Arc::clone(&source.written);
                let movable = source.movable && source.action == Action::Move;
                let deadline = Instant::now() + Duration::from_secs(10);
                let (_, receiver) = BackgroundJobs::global().submit(Duration::MAX, move || {
                    if movable {
                        let entries = io::snapshot(&roots, deadline, &cancelled)?;
                        *snapshot.lock().unwrap_or_else(|e| e.into_inner()) = entries;
                    }
                    io::write(fd, bytes, deadline, cancelled, written)
                });
                state.writes.push(receiver);
            }
            wl_data_source::Event::Action {
                dnd_action: WEnum::Value(action),
            } => source.action = action,
            wl_data_source::Event::Cancelled => {
                let source = state.source.take().expect("current source");
                source.cancelled.store(true, Ordering::Relaxed);
                source.proxy.destroy();
                state.release_pointer = true;
                state.events.push(Event::Refresh);
            }
            wl_data_source::Event::DndFinished => {
                let source = state.source.take().expect("current source");
                if source.movable
                    && source.action == Action::Move
                    && source.written.load(Ordering::Acquire)
                {
                    let snapshot = source
                        .snapshot
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .clone();
                    // A later action change cannot turn a reference-only send
                    // into removal without a successful original-tree snapshot.
                    if !snapshot.is_empty() {
                        state
                            .events
                            .push(Event::Remove(source.identities, snapshot));
                    }
                }
                source.proxy.destroy();
                state.release_pointer = true;
                state.events.push(Event::Refresh);
            }
            wl_data_source::Event::DndDropPerformed => state.release_pointer = true,
            _ => {}
        }
    }
}
delegate_noop!(State: ignore wl_data_device_manager::WlDataDeviceManager);
