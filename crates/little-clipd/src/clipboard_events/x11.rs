//! Native X11 XFixes + ICCCM clipboard watcher and publisher.

use super::{
    Command, Incoming, IncomingReader, Request, TRANSFER_LIMIT, TRANSFER_TIMEOUT, preferred_mime,
};
use std::{
    collections::HashMap,
    io::{self, Read as _},
    os::{fd::AsFd, unix::net::UnixStream},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    time::Instant,
};
use tokio::sync::{oneshot, watch};
use x11rb::{
    COPY_DEPTH_FROM_PARENT, CURRENT_TIME,
    connection::{Connection as _, RequestConnection as _},
    protocol::{
        Event, xfixes,
        xfixes::ConnectionExt as _,
        xproto::{self, Atom, ConnectionExt as _, PropMode, Window},
    },
    rust_connection::RustConnection,
    wrapper::ConnectionExt as _,
};

const MAX_OUTGOING_INCR: usize = 8;
const INCR_THRESHOLD: usize = 200 * 1024;
const INCR_CHUNK: usize = 128 * 1024;
const MAX_ATOM_CACHE: usize = 2048;

x11rb::atom_manager! {
    Atoms: AtomsCookie {
        CLIPBOARD,
        TARGETS,
        TIMESTAMP,
        INCR,
        ATOM,
        INTEGER,
        UTF8_STRING,
        STRING,
        TEXT,
        TEXT_PLAIN: b"text/plain",
        TEXT_PLAIN_UTF8: b"text/plain;charset=utf-8",
        XFER_PROP: b"_LITTLE_CLIP_XFER",
        TARGETS_PROP: b"_LITTLE_CLIP_TARGETS",
    }
}

pub(super) fn spawn(
    requests: mpsc::Receiver<Request>,
    wakeup_rd: UnixStream,
    selection: watch::Sender<Option<Arc<Incoming>>>,
    serial: Arc<AtomicU64>,
) -> io::Result<()> {
    std::thread::Builder::new()
        .name("clipboard-events-x11".into())
        .spawn(move || {
            let result = run(requests, wakeup_rd, selection, serial.clone());
            serial.fetch_add(1, Ordering::AcqRel);
            if let Err(error) = result {
                tracing::warn!(%error, "X11 clipboard event connection stopped");
            }
        })?;
    Ok(())
}

struct InProgressIncoming {
    window: Window,
    initial: bool,
    observed: tokio::time::Instant,
    started: Instant,
    target: Option<Atom>,
    reply: Option<oneshot::Sender<Result<Vec<u8>, String>>>,
    incr: Option<Vec<u8>>,
}

struct OutgoingIncr {
    requestor: Window,
    property: Atom,
    target_type: Atom,
    bytes: Arc<[u8]>,
    offset: usize,
    started: Instant,
}

struct Publication {
    bytes: Arc<[u8]>,
    targets: Vec<Atom>,
    data_targets: Vec<Atom>,
    timestamp: u32,
}

struct X11State {
    conn: RustConnection,
    atoms: Atoms,
    root: Window,
    helper_win: Window,
    atom_names: HashMap<Atom, String>,
    name_atoms: HashMap<String, Atom>,
    selection: watch::Sender<Option<Arc<Incoming>>>,
    serial: Arc<AtomicU64>,
    incoming: Option<InProgressIncoming>,
    publication: Option<Publication>,
    outgoing_incr: Vec<OutgoingIncr>,
    max_property_bytes: usize,
}

fn run(
    requests: mpsc::Receiver<Request>,
    wakeup_rd: UnixStream,
    selection: watch::Sender<Option<Arc<Incoming>>>,
    serial: Arc<AtomicU64>,
) -> Result<(), Box<dyn std::error::Error>> {
    let (conn, screen_num) = RustConnection::connect(None)?;
    let atoms = Atoms::new(&conn)?.reply()?;
    let root = conn.setup().roots[screen_num].root;
    let helper_win = selection_window(&conn, root)?;
    conn.xfixes_query_version(2, 0)?.reply()?;
    conn.xfixes_select_selection_input(
        helper_win,
        atoms.CLIPBOARD,
        xfixes::SelectionEventMask::SET_SELECTION_OWNER
            | xfixes::SelectionEventMask::SELECTION_WINDOW_DESTROY
            | xfixes::SelectionEventMask::SELECTION_CLIENT_CLOSE,
    )?
    .check()?;

    let initial_owner = conn.get_selection_owner(atoms.CLIPBOARD)?.reply()?.owner;
    conn.flush()?;

    let max_property_bytes = conn.maximum_request_bytes().saturating_sub(28);
    let mut state = X11State {
        conn,
        atoms,
        root,
        helper_win,
        atom_names: HashMap::new(),
        name_atoms: HashMap::new(),
        selection,
        serial,
        incoming: None,
        publication: None,
        outgoing_incr: Vec::new(),
        max_property_bytes,
    };
    state.seed_known_atoms();
    if initial_owner != x11rb::NONE {
        state.serial.fetch_add(1, Ordering::AcqRel);
        state.request_targets(true)?;
    }
    state.run_loop(&requests, &wakeup_rd)
}

fn selection_window(
    conn: &RustConnection,
    root: Window,
) -> Result<Window, Box<dyn std::error::Error>> {
    let window = conn.generate_id()?;
    conn.create_window(
        COPY_DEPTH_FROM_PARENT,
        window,
        root,
        0,
        0,
        1,
        1,
        0,
        xproto::WindowClass::INPUT_ONLY,
        x11rb::COPY_FROM_PARENT,
        &xproto::CreateWindowAux::new().event_mask(xproto::EventMask::PROPERTY_CHANGE),
    )?
    .check()?;
    Ok(window)
}

impl X11State {
    fn seed_known_atoms(&mut self) {
        let pairs = [
            (self.atoms.TARGETS, "TARGETS"),
            (self.atoms.TIMESTAMP, "TIMESTAMP"),
            (self.atoms.UTF8_STRING, "UTF8_STRING"),
            (self.atoms.STRING, "STRING"),
            (self.atoms.TEXT, "TEXT"),
            (self.atoms.TEXT_PLAIN, "text/plain"),
            (self.atoms.TEXT_PLAIN_UTF8, "text/plain;charset=utf-8"),
        ];
        for (atom, name) in pairs {
            self.atom_names.insert(atom, name.to_owned());
            self.name_atoms.insert(name.to_owned(), atom);
        }
    }

    fn pump_events(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        while let Some(event) = self.conn.poll_for_event()? {
            self.handle_event(event)?;
        }
        Ok(())
    }

    fn run_loop(
        &mut self,
        requests: &mpsc::Receiver<Request>,
        mut wakeup_rd: &UnixStream,
    ) -> Result<(), Box<dyn std::error::Error>> {
        loop {
            self.pump_events()?;
            if !self.process_requests(requests)? {
                return Ok(());
            }
            self.evict_expired()?;
            self.conn.flush()?;
            self.wait_events(&mut wakeup_rd)?;
        }
    }

    fn drain_wakeup(wakeup_rd: &mut &UnixStream) {
        let mut buf = [0u8; 64];
        while matches!(wakeup_rd.read(&mut buf), Ok(n) if n > 0) {}
    }

    fn wait_events(&self, wakeup_rd: &mut &UnixStream) -> Result<(), Box<dyn std::error::Error>> {
        let x11_fd = self.conn.stream().as_fd();
        let wake_fd = wakeup_rd.as_fd();
        let mut fds = [
            rustix::event::PollFd::new(&x11_fd, rustix::event::PollFlags::IN),
            rustix::event::PollFd::new(&wake_fd, rustix::event::PollFlags::IN),
        ];
        let timeout = rustix::event::Timespec {
            tv_sec: 0,
            tv_nsec: 100_000_000,
        };
        match rustix::event::poll(&mut fds, Some(&timeout)) {
            Ok(_) | Err(rustix::io::Errno::INTR) => {
                Self::drain_wakeup(wakeup_rd);
                Ok(())
            }
            Err(error) => Err(error.into()),
        }
    }

    fn dispatch_request(&mut self, request: Request) -> Result<(), Box<dyn std::error::Error>> {
        if request.reply.is_closed() {
            return Ok(());
        }
        match request.command {
            Command::Publish { bytes, mime } => self.publish(bytes, mime)?,
            Command::Barrier => {
                self.conn.sync()?;
            }
        }
        self.pump_events()?;
        let _ = request.reply.send(());
        Ok(())
    }

    fn process_requests(
        &mut self,
        requests: &mpsc::Receiver<Request>,
    ) -> Result<bool, Box<dyn std::error::Error>> {
        loop {
            match requests.try_recv() {
                Ok(request) => self.dispatch_request(request)?,
                Err(mpsc::TryRecvError::Empty) => return Ok(true),
                Err(mpsc::TryRecvError::Disconnected) => return Ok(false),
            }
        }
    }

    fn publish(
        &mut self,
        bytes: Arc<[u8]>,
        mime: String,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mut targets = vec![self.atoms.TARGETS, self.atoms.TIMESTAMP];
        let mut data_targets = Vec::new();
        for name in build_offered_target_names(&mime) {
            let atom = self.intern(name)?;
            targets.push(atom);
            data_targets.push(atom);
        }
        self.publication = Some(Publication {
            bytes,
            targets,
            data_targets,
            timestamp: CURRENT_TIME,
        });
        self.conn
            .set_selection_owner(self.helper_win, self.atoms.CLIPBOARD, CURRENT_TIME)?
            .check()?;
        if self
            .conn
            .get_selection_owner(self.atoms.CLIPBOARD)?
            .reply()?
            .owner
            != self.helper_win
        {
            return Err("X11 clipboard ownership changed during publication".into());
        }
        Ok(())
    }

    fn intern(&mut self, name: &str) -> Result<Atom, Box<dyn std::error::Error>> {
        if let Some(&atom) = self.name_atoms.get(name) {
            return Ok(atom);
        }
        let atom = self.conn.intern_atom(false, name.as_bytes())?.reply()?.atom;
        if self.atom_names.len() < MAX_ATOM_CACHE {
            self.atom_names.insert(atom, name.to_owned());
            self.name_atoms.insert(name.to_owned(), atom);
        }
        Ok(atom)
    }

    fn resolve_atom_names(&mut self, atoms: &[Atom]) -> Vec<String> {
        let mut names = HashMap::new();
        let mut missing: Vec<_> = atoms
            .iter()
            .copied()
            .filter(|atom| *atom != x11rb::NONE && !self.atom_names.contains_key(atom))
            .collect();
        missing.sort_unstable();
        missing.dedup();
        let cookies: Vec<_> = missing
            .into_iter()
            .filter_map(|atom| Some((atom, self.conn.get_atom_name(atom).ok()?)))
            .collect();
        for (atom, cookie) in cookies {
            if let Ok(reply) = cookie.reply()
                && let Ok(name) = String::from_utf8(reply.name)
            {
                if self.atom_names.len() < MAX_ATOM_CACHE {
                    self.name_atoms.insert(name.clone(), atom);
                    self.atom_names.insert(atom, name);
                } else {
                    names.insert(atom, name);
                }
            }
        }
        atoms
            .iter()
            .filter_map(|atom| {
                self.atom_names
                    .get(atom)
                    .or_else(|| names.get(atom))
                    .cloned()
            })
            .collect()
    }

    fn handle_event(&mut self, event: Event) -> Result<(), Box<dyn std::error::Error>> {
        match event {
            Event::XfixesSelectionNotify(ev) => self.on_xfixes_notify(ev)?,
            Event::SelectionNotify(ev) => self.on_selection_notify(ev)?,
            Event::PropertyNotify(ev) => self.on_property_notify(ev)?,
            Event::SelectionRequest(req) => self.on_selection_request(req)?,
            _ => {}
        }
        Ok(())
    }

    fn on_xfixes_notify(
        &mut self,
        ev: xfixes::SelectionNotifyEvent,
    ) -> Result<(), Box<dyn std::error::Error>> {
        if ev.selection != self.atoms.CLIPBOARD {
            return Ok(());
        }
        if ev.owner != self.helper_win
            && self.publication.is_some()
            && self
                .conn
                .get_selection_owner(self.atoms.CLIPBOARD)?
                .reply()?
                .owner
                == self.helper_win
        {
            // The server queued this event before it processed our ownership request.
            return Ok(());
        }
        self.serial.fetch_add(1, Ordering::AcqRel);
        self.abort_pending("Selection replaced before transfer completed");
        self.selection.send_replace(None);
        if ev.owner == self.helper_win {
            if let Some(publication) = &mut self.publication {
                publication.timestamp = ev.selection_timestamp;
            }
        } else {
            self.publication = None;
            if ev.owner != x11rb::NONE {
                self.request_targets(false)?;
            }
        }
        Ok(())
    }

    fn request_targets(&mut self, initial: bool) -> Result<(), Box<dyn std::error::Error>> {
        // Each owner change gets its own requestor, so a late reply or INCR chunk
        // from the old owner cannot be consumed as data from the new owner.
        let window = selection_window(&self.conn, self.root)?;
        self.incoming = Some(InProgressIncoming {
            window,
            initial,
            observed: tokio::time::Instant::now(),
            started: Instant::now(),
            target: None,
            reply: None,
            incr: None,
        });
        self.conn.convert_selection(
            window,
            self.atoms.CLIPBOARD,
            self.atoms.TARGETS,
            self.atoms.TARGETS_PROP,
            CURRENT_TIME,
        )?;
        Ok(())
    }

    fn take_pending(&mut self) -> Option<InProgressIncoming> {
        let pending = self.incoming.take()?;
        if let Ok(cookie) = self.conn.destroy_window(pending.window) {
            cookie.ignore_error();
        }
        Some(pending)
    }

    fn abort_pending(&mut self, reason: &str) {
        if let Some(pending) = self.take_pending()
            && let Some(reply) = pending.reply
        {
            let _ = reply.send(Err(reason.to_owned()));
        }
    }

    fn on_selection_notify(
        &mut self,
        ev: xproto::SelectionNotifyEvent,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let Some(pending) = &self.incoming else {
            return Ok(());
        };
        if ev.requestor != pending.window || ev.selection != self.atoms.CLIPBOARD {
            return Ok(());
        }
        let (expected_target, expected_property) = pending
            .target
            .map_or((self.atoms.TARGETS, self.atoms.TARGETS_PROP), |target| {
                (target, self.atoms.XFER_PROP)
            });
        if ev.target != expected_target || pending.incr.is_some() {
            return Ok(());
        }
        if ev.property == x11rb::NONE {
            self.abort_pending("Clipboard owner rejected conversion");
        } else if ev.property == expected_property {
            if pending.target.is_none() {
                self.on_targets_notify(ev)?;
            } else {
                self.on_payload_notify(ev)?;
            }
        }
        Ok(())
    }

    fn on_targets_notify(
        &mut self,
        ev: xproto::SelectionNotifyEvent,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let reply = self
            .conn
            .get_property(true, ev.requestor, ev.property, self.atoms.ATOM, 0, 4096)?
            .reply()?;
        let Some(atoms_iter) = reply.value32().filter(|_| reply.bytes_after == 0) else {
            self.abort_pending("Invalid clipboard targets");
            return Ok(());
        };
        let target_atoms: Vec<Atom> = atoms_iter.collect();
        let names = self.resolve_atom_names(&target_atoms);
        let Some((raw_target, mime)) = select_incoming_target(&names) else {
            self.abort_pending("No supported clipboard target");
            return Ok(());
        };
        let target_atom = self.intern(&raw_target)?;
        let (tx, rx) = oneshot::channel();
        let Some(pending) = &mut self.incoming else {
            return Ok(());
        };
        pending.target = Some(target_atom);
        pending.reply = Some(tx);

        self.selection.send_replace(Some(Arc::new(Incoming {
            serial: self.serial.load(Ordering::Acquire),
            observed: pending.observed,
            initial: pending.initial,
            mime,
            reader: Mutex::new(Some(IncomingReader::X11(rx))),
        })));
        self.conn.convert_selection(
            pending.window,
            self.atoms.CLIPBOARD,
            target_atom,
            self.atoms.XFER_PROP,
            CURRENT_TIME,
        )?;
        Ok(())
    }

    fn read_xfer_property(
        &self,
        requestor: Window,
    ) -> Result<xproto::GetPropertyReply, Box<dyn std::error::Error>> {
        let long_len = u32::try_from(TRANSFER_LIMIT / 4 + 4).unwrap_or(u32::MAX);
        Ok(self
            .conn
            .get_property(
                true,
                requestor,
                self.atoms.XFER_PROP,
                xproto::GetPropertyType::ANY,
                0,
                long_len,
            )?
            .reply()?)
    }

    fn on_payload_notify(
        &mut self,
        ev: xproto::SelectionNotifyEvent,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let reply = self.read_xfer_property(ev.requestor)?;
        if reply.type_ == self.atoms.INCR {
            let declared_size = reply.value32().and_then(|mut values| values.next());
            if !declared_size.is_some_and(|size| u64::from(size) <= TRANSFER_LIMIT)
                || reply.value_len != 1
                || reply.bytes_after != 0
            {
                self.abort_pending("Invalid or oversized clipboard INCR transfer");
            } else if let Some(pending) = &mut self.incoming {
                pending.incr = Some(Vec::new());
            }
        } else if self.valid_payload(&reply) {
            self.deliver_incoming(reply.value, reply.type_ == self.atoms.STRING);
        } else {
            self.abort_pending("Invalid or oversized clipboard payload");
        }
        Ok(())
    }

    fn valid_payload(&self, reply: &xproto::GetPropertyReply) -> bool {
        let target = self.incoming.as_ref().and_then(|pending| pending.target);
        reply.format == 8
            && reply.bytes_after == 0
            && reply.value.len() as u64 <= TRANSFER_LIMIT
            && (Some(reply.type_) == target
                || (target == Some(self.atoms.TEXT)
                    && [self.atoms.UTF8_STRING, self.atoms.STRING].contains(&reply.type_)))
    }

    fn deliver_incoming(&mut self, mut bytes: Vec<u8>, latin1: bool) {
        let Some(tx) = self.take_pending().and_then(|pending| pending.reply) else {
            return;
        };
        if latin1 {
            let utf8_len = bytes.len() + bytes.iter().filter(|byte| **byte >= 128).count();
            if utf8_len as u64 > TRANSFER_LIMIT {
                let _ = tx.send(Err("Clipboard selection exceeds 256 MiB".into()));
                return;
            }
            let mut text = String::with_capacity(utf8_len);
            text.extend(bytes.into_iter().map(char::from));
            bytes = text.into_bytes();
        }
        let _ = tx.send(Ok(bytes));
    }

    fn on_property_notify(
        &mut self,
        ev: xproto::PropertyNotifyEvent,
    ) -> Result<(), Box<dyn std::error::Error>> {
        if ev.state == xproto::Property::NEW_VALUE
            && ev.atom == self.atoms.XFER_PROP
            && self
                .incoming
                .as_ref()
                .is_some_and(|pending| ev.window == pending.window && pending.incr.is_some())
        {
            self.handle_incoming_incr_chunk(ev.window)?;
        } else if ev.state == xproto::Property::DELETE {
            self.advance_outgoing_incr(ev.window, ev.atom)?;
        }
        Ok(())
    }

    fn handle_incoming_incr_chunk(
        &mut self,
        window: Window,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let reply = self.read_xfer_property(window)?;
        if !self.valid_payload(&reply) {
            self.abort_pending("Invalid or oversized clipboard INCR chunk");
            return Ok(());
        }
        if let Some(buffer) = self
            .incoming
            .as_mut()
            .and_then(|pending| pending.incr.as_mut())
        {
            if reply.value.is_empty() {
                let bytes = std::mem::take(buffer);
                self.deliver_incoming(bytes, reply.type_ == self.atoms.STRING);
            } else if let Err(error) = append_incr_chunk(buffer, &reply.value, TRANSFER_LIMIT) {
                self.abort_pending(error);
            }
        }
        Ok(())
    }

    fn advance_outgoing_incr(
        &mut self,
        window: Window,
        atom: Atom,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let Some(idx) = self
            .outgoing_incr
            .iter()
            .position(|xfer| xfer.requestor == window && xfer.property == atom)
        else {
            return Ok(());
        };
        let xfer = &mut self.outgoing_incr[idx];
        let chunk = next_incr_chunk(
            &xfer.bytes,
            xfer.offset,
            INCR_CHUNK.min(self.max_property_bytes),
        );
        let written = self
            .conn
            .change_property8(
                PropMode::REPLACE,
                xfer.requestor,
                xfer.property,
                xfer.target_type,
                chunk,
            )?
            .check()
            .is_ok();
        if !written || chunk.is_empty() {
            self.retire_outgoing(idx)?;
        } else {
            xfer.offset += chunk.len();
            xfer.started = Instant::now();
        }
        Ok(())
    }

    fn retire_outgoing(&mut self, index: usize) -> Result<(), Box<dyn std::error::Error>> {
        let transfer = self.outgoing_incr.swap_remove(index);
        if !self
            .outgoing_incr
            .iter()
            .any(|xfer| xfer.requestor == transfer.requestor)
        {
            self.conn
                .change_window_attributes(
                    transfer.requestor,
                    &xproto::ChangeWindowAttributesAux::new()
                        .event_mask(xproto::EventMask::NO_EVENT),
                )?
                .ignore_error();
        }
        Ok(())
    }

    fn on_selection_request(
        &mut self,
        req: xproto::SelectionRequestEvent,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let Some(pub_data) = &self.publication else {
            return self.send_selection_notify(&req, x11rb::NONE);
        };
        if req.selection != self.atoms.CLIPBOARD || req.owner != self.helper_win {
            return self.send_selection_notify(&req, x11rb::NONE);
        }
        let dest = if req.property == x11rb::NONE {
            req.target
        } else {
            req.property
        };
        if req.target == self.atoms.TARGETS {
            let written = self
                .conn
                .change_property32(
                    PropMode::REPLACE,
                    req.requestor,
                    dest,
                    self.atoms.ATOM,
                    &pub_data.targets,
                )?
                .check()
                .is_ok();
            return self.send_selection_notify(&req, if written { dest } else { x11rb::NONE });
        }
        if req.target == self.atoms.TIMESTAMP {
            let written = self
                .conn
                .change_property32(
                    PropMode::REPLACE,
                    req.requestor,
                    dest,
                    self.atoms.INTEGER,
                    &[pub_data.timestamp],
                )?
                .check()
                .is_ok();
            return self.send_selection_notify(&req, if written { dest } else { x11rb::NONE });
        }
        self.serve_data_request(&req, dest)
    }

    fn serve_data_request(
        &mut self,
        req: &xproto::SelectionRequestEvent,
        dest: Atom,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let Some(pub_data) = &self.publication else {
            return self.send_selection_notify(req, x11rb::NONE);
        };
        if !pub_data.data_targets.contains(&req.target) {
            return self.send_selection_notify(req, x11rb::NONE);
        }
        let bytes = pub_data.bytes.clone();
        let target_type = req.target;
        if bytes.len() <= INCR_THRESHOLD.min(self.max_property_bytes) {
            let written = self
                .conn
                .change_property8(PropMode::REPLACE, req.requestor, dest, target_type, &bytes)?
                .check()
                .is_ok();
            return self.send_selection_notify(req, if written { dest } else { x11rb::NONE });
        }
        if self.start_outgoing_incr(req, dest, target_type, bytes)? {
            self.send_selection_notify(req, dest)
        } else {
            self.send_selection_notify(req, x11rb::NONE)
        }
    }

    fn start_outgoing_incr(
        &mut self,
        req: &xproto::SelectionRequestEvent,
        dest: Atom,
        target_type: Atom,
        bytes: Arc<[u8]>,
    ) -> Result<bool, Box<dyn std::error::Error>> {
        if self.outgoing_incr.len() >= MAX_OUTGOING_INCR
            || self
                .outgoing_incr
                .iter()
                .any(|transfer| transfer.requestor == req.requestor && transfer.property == dest)
        {
            return Ok(false);
        }
        let total = u32::try_from(bytes.len()).unwrap_or(u32::MAX);
        if self
            .conn
            .change_window_attributes(
                req.requestor,
                &xproto::ChangeWindowAttributesAux::new()
                    .event_mask(xproto::EventMask::PROPERTY_CHANGE),
            )?
            .check()
            .is_err()
        {
            return Ok(false);
        }
        let written = self
            .conn
            .change_property32(
                PropMode::REPLACE,
                req.requestor,
                dest,
                self.atoms.INCR,
                &[total],
            )?
            .check()
            .is_ok();
        self.outgoing_incr.push(OutgoingIncr {
            requestor: req.requestor,
            property: dest,
            target_type,
            bytes,
            offset: 0,
            started: Instant::now(),
        });
        if !written {
            self.retire_outgoing(self.outgoing_incr.len() - 1)?;
        }
        Ok(written)
    }

    fn send_selection_notify(
        &self,
        req: &xproto::SelectionRequestEvent,
        property: Atom,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let notify = xproto::SelectionNotifyEvent {
            response_type: xproto::SELECTION_NOTIFY_EVENT,
            sequence: 0,
            time: req.time,
            requestor: req.requestor,
            selection: req.selection,
            target: req.target,
            property,
        };
        self.conn
            .send_event(false, req.requestor, xproto::EventMask::NO_EVENT, notify)?
            .ignore_error();
        Ok(())
    }

    fn evict_expired(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        if let Some(pending) = &self.incoming
            && pending.started.elapsed() > TRANSFER_TIMEOUT
        {
            self.abort_pending("Clipboard source timed out");
        }
        for index in (0..self.outgoing_incr.len()).rev() {
            if self.outgoing_incr[index].started.elapsed() > TRANSFER_TIMEOUT {
                self.retire_outgoing(index)?;
            }
        }
        Ok(())
    }
}

fn select_incoming_target(names: &[String]) -> Option<(String, String)> {
    let supported: Vec<_> = names
        .iter()
        .filter(|name| {
            name.starts_with("text/")
                || little_clip_core::item::image_extension_for_mime(name).is_some()
                || matches!(name.as_str(), "STRING" | "TEXT" | "UTF8_STRING")
        })
        .cloned()
        .collect();
    let raw = preferred_mime(&supported)?;
    let normalized = if raw == "UTF8_STRING" || raw.eq_ignore_ascii_case("text/plain;charset=utf-8")
    {
        "text/plain;charset=utf-8".to_owned()
    } else if raw == "STRING" || raw == "TEXT" {
        "text/plain".to_owned()
    } else {
        raw.clone()
    };
    Some((raw, normalized))
}

pub(super) fn build_offered_target_names(mime: &str) -> Vec<&str> {
    let mut names = vec![mime];
    if mime == "text/plain;charset=utf-8" {
        names.extend(["UTF8_STRING", "text/plain"]);
    } else if mime == "text/plain" {
        names.extend(["UTF8_STRING", "text/plain;charset=utf-8"]);
    }
    names
}

pub(super) fn next_incr_chunk(bytes: &[u8], offset: usize, chunk_size: usize) -> &[u8] {
    let end = offset.saturating_add(chunk_size).min(bytes.len());
    bytes.get(offset..end).unwrap_or(&[])
}

pub(super) fn append_incr_chunk(
    buf: &mut Vec<u8>,
    chunk: &[u8],
    limit: u64,
) -> Result<(), &'static str> {
    if (buf.len() + chunk.len()) as u64 > limit {
        return Err("Clipboard selection exceeds 256 MiB");
    }
    buf.extend_from_slice(chunk);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn incoming_target_normalizes_x11_text_atoms_and_prefers_images() {
        let targets = vec![
            "TARGETS".into(),
            "UTF8_STRING".into(),
            "STRING".into(),
            "image/png".into(),
        ];
        assert_eq!(
            select_incoming_target(&targets),
            Some(("image/png".into(), "image/png".into()))
        );

        let text_targets = vec!["STRING".into(), "UTF8_STRING".into()];
        assert_eq!(
            select_incoming_target(&text_targets),
            Some(("UTF8_STRING".into(), "text/plain;charset=utf-8".into(),))
        );

        let legacy = vec!["STRING".into()];
        assert_eq!(
            select_incoming_target(&legacy),
            Some(("STRING".into(), "text/plain".into()))
        );
    }

    #[test]
    fn x11_clipboard_targets_exclude_protocol_metadata_and_unconverted_latin1() {
        assert_eq!(
            select_incoming_target(&["TARGETS".into(), "TIMESTAMP".into()]),
            None
        );
        assert_eq!(
            build_offered_target_names("text/plain"),
            ["text/plain", "UTF8_STRING", "text/plain;charset=utf-8"]
        );
    }

    #[test]
    fn incr_chunk_slicing_and_limit_enforcement_work_across_boundaries() {
        let payload = b"abcdef";
        assert_eq!(next_incr_chunk(payload, 0, 4), b"abcd");
        assert_eq!(next_incr_chunk(payload, 4, 4), b"ef");
        assert_eq!(next_incr_chunk(payload, 6, 4), b"");

        let mut buf = Vec::new();
        append_incr_chunk(&mut buf, b"abcd", 6).unwrap();
        append_incr_chunk(&mut buf, b"ef", 6).unwrap();
        assert_eq!(buf, b"abcdef");
        assert!(append_incr_chunk(&mut buf, b"g", 6).is_err());
    }
}
