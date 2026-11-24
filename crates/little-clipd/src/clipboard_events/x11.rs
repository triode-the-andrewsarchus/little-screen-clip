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
    connection::Connection as _,
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
            if let Err(error) = run(requests, wakeup_rd, selection, serial) {
                tracing::warn!(%error, "X11 clipboard event connection stopped");
            }
        })?;
    Ok(())
}

struct InProgressIncoming {
    initial: bool,
    started: Instant,
    target: Option<(Atom, String, bool)>,
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
    mime: String,
    targets: Vec<Atom>,
    data_targets: Vec<Atom>,
    timestamp: u32,
}

struct X11State {
    conn: RustConnection,
    atoms: Atoms,
    helper_win: Window,
    marker_atom: Atom,
    marker_name: String,
    atom_names: HashMap<Atom, String>,
    name_atoms: HashMap<String, Atom>,
    selection: watch::Sender<Option<Arc<Incoming>>>,
    serial: Arc<AtomicU64>,
    initialized: bool,
    owning: bool,
    last_time: u32,
    incoming: Option<InProgressIncoming>,
    publication: Option<Publication>,
    outgoing_incr: Vec<OutgoingIncr>,
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
    let helper_win = conn.generate_id()?;
    conn.create_window(
        COPY_DEPTH_FROM_PARENT,
        helper_win,
        root,
        0,
        0,
        1,
        1,
        0,
        xproto::WindowClass::INPUT_ONLY,
        x11rb::COPY_FROM_PARENT,
        &xproto::CreateWindowAux::new().event_mask(xproto::EventMask::PROPERTY_CHANGE),
    )?;
    conn.xfixes_query_version(5, 0)?.reply()?;
    conn.xfixes_select_selection_input(
        helper_win,
        atoms.CLIPBOARD,
        xfixes::SelectionEventMask::SET_SELECTION_OWNER,
    )?;

    let marker_name = format!("application/x-little-screen-clip-{}", uuid::Uuid::new_v4());
    let marker_atom = conn
        .intern_atom(false, marker_name.as_bytes())?
        .reply()?
        .atom;
    let initial_owner = conn.get_selection_owner(atoms.CLIPBOARD)?.reply()?.owner;
    conn.flush()?;

    let mut state = X11State {
        conn,
        atoms,
        helper_win,
        marker_atom,
        marker_name,
        atom_names: HashMap::new(),
        name_atoms: HashMap::new(),
        selection,
        serial,
        initialized: false,
        owning: false,
        last_time: CURRENT_TIME,
        incoming: None,
        publication: None,
        outgoing_incr: Vec::new(),
    };
    state.seed_known_atoms();
    state.handle_initial_owner(initial_owner)?;
    state.run_loop(&requests, &wakeup_rd)
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
            (self.marker_atom, self.marker_name.as_str()),
        ];
        for (atom, name) in pairs {
            self.atom_names.insert(atom, name.to_owned());
            self.name_atoms.insert(name.to_owned(), atom);
        }
    }

    fn handle_initial_owner(&mut self, owner: Window) -> Result<(), Box<dyn std::error::Error>> {
        self.initialized = true;
        if owner == x11rb::NONE || owner == self.helper_win {
            self.serial.fetch_add(1, Ordering::AcqRel);
            self.selection.send_replace(None);
            return Ok(());
        }
        let serial = self.serial.fetch_add(1, Ordering::AcqRel) + 1;
        self.request_targets(serial, true)
    }

    fn run_loop(
        &mut self,
        requests: &mpsc::Receiver<Request>,
        mut wakeup_rd: &UnixStream,
    ) -> Result<(), Box<dyn std::error::Error>> {
        loop {
            while let Some(event) = self.conn.poll_for_event()? {
                self.handle_event(event)?;
            }
            if !self.process_requests(requests)? {
                return Ok(());
            }
            self.evict_expired();
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

    fn process_requests(
        &mut self,
        requests: &mpsc::Receiver<Request>,
    ) -> Result<bool, Box<dyn std::error::Error>> {
        loop {
            match requests.try_recv() {
                Ok(request) => {
                    match request.command {
                        Command::Publish { bytes, mime } => self.publish(bytes, mime)?,
                        Command::Barrier => {
                            self.conn.sync()?;
                        }
                    }
                    while let Some(event) = self.conn.poll_for_event()? {
                        self.handle_event(event)?;
                    }
                    let _ = request.reply.send(());
                }
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
        let mut targets = vec![self.atoms.TARGETS, self.atoms.TIMESTAMP, self.marker_atom];
        let mut data_targets = Vec::new();
        for name in build_offered_target_names(&mime) {
            let atom = self.intern(name)?;
            targets.push(atom);
            data_targets.push(atom);
        }
        self.publication = Some(Publication {
            bytes,
            mime,
            targets,
            data_targets,
            timestamp: self.last_time,
        });
        self.owning = true;
        self.conn
            .set_selection_owner(self.helper_win, self.atoms.CLIPBOARD, CURRENT_TIME)?;
        self.conn.sync()?;
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
        let mut missing = Vec::new();
        for &atom in atoms {
            if atom != x11rb::NONE && !self.atom_names.contains_key(&atom) {
                missing.push(atom);
            }
        }
        if !missing.is_empty() {
            let cookies: Vec<_> = missing
                .into_iter()
                .filter_map(|atom| Some((atom, self.conn.get_atom_name(atom).ok()?)))
                .collect();
            for (atom, cookie) in cookies {
                if let Ok(reply) = cookie.reply()
                    && let Ok(name) = String::from_utf8(reply.name)
                    && self.atom_names.len() < MAX_ATOM_CACHE
                {
                    self.name_atoms.insert(name.clone(), atom);
                    self.atom_names.insert(atom, name);
                }
            }
        }
        atoms
            .iter()
            .filter_map(|atom| self.atom_names.get(atom).cloned())
            .collect()
    }

    fn handle_event(&mut self, event: Event) -> Result<(), Box<dyn std::error::Error>> {
        match event {
            Event::XfixesSelectionNotify(ev) => self.on_xfixes_notify(ev)?,
            Event::SelectionNotify(ev) => self.on_selection_notify(ev)?,
            Event::PropertyNotify(ev) => self.on_property_notify(ev)?,
            Event::SelectionRequest(req) => self.on_selection_request(req)?,
            Event::SelectionClear(ev) if ev.selection == self.atoms.CLIPBOARD => {
                self.last_time = ev.time;
                self.owning = false;
                self.publication = None;
            }
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
        self.last_time = ev.timestamp;
        let serial = self.serial.fetch_add(1, Ordering::AcqRel) + 1;
        let initial = !std::mem::replace(&mut self.initialized, true);
        self.abort_pending("Selection replaced before transfer completed");
        if ev.owner == self.helper_win {
            self.owning = true;
            self.selection.send_replace(None);
            return Ok(());
        }
        self.owning = false;
        self.publication = None;
        if ev.owner == x11rb::NONE {
            self.selection.send_replace(None);
            return Ok(());
        }
        self.request_targets(serial, initial)
    }

    fn request_targets(
        &mut self,
        _serial: u64,
        initial: bool,
    ) -> Result<(), Box<dyn std::error::Error>> {
        self.incoming = Some(InProgressIncoming {
            initial,
            started: Instant::now(),
            target: None,
            reply: None,
            incr: None,
        });
        self.conn.convert_selection(
            self.helper_win,
            self.atoms.CLIPBOARD,
            self.atoms.TARGETS,
            self.atoms.TARGETS_PROP,
            CURRENT_TIME,
        )?;
        Ok(())
    }

    fn abort_pending(&mut self, reason: &str) {
        if let Some(mut old) = self.incoming.take()
            && let Some(reply) = old.reply.take()
        {
            let _ = reply.send(Err(reason.to_owned()));
        }
    }

    fn on_selection_notify(
        &mut self,
        ev: xproto::SelectionNotifyEvent,
    ) -> Result<(), Box<dyn std::error::Error>> {
        if ev.requestor != self.helper_win || ev.selection != self.atoms.CLIPBOARD {
            return Ok(());
        }
        self.last_time = ev.time;
        if ev.target == self.atoms.TARGETS {
            self.on_targets_notify(ev)?;
        } else {
            self.on_payload_notify(ev)?;
        }
        Ok(())
    }

    fn on_targets_notify(
        &mut self,
        ev: xproto::SelectionNotifyEvent,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let Some(mut pending) = self.incoming.take() else {
            return Ok(());
        };
        if ev.property == x11rb::NONE {
            return Ok(());
        }
        let reply = self
            .conn
            .get_property(true, self.helper_win, ev.property, self.atoms.ATOM, 0, 4096)?
            .reply()?;
        let Some(atoms_iter) = reply.value32() else {
            return Ok(());
        };
        let target_atoms: Vec<Atom> = atoms_iter.collect();
        let names = self.resolve_atom_names(&target_atoms);
        if is_echo_targets(self.owning, &self.marker_name, &names) {
            self.selection.send_replace(None);
            return Ok(());
        }
        let Some((raw_target, mime, latin1)) = select_incoming_target(&names) else {
            return Ok(());
        };
        let target_atom = self.intern(&raw_target)?;
        let (tx, rx) = oneshot::channel();
        let initial = pending.initial;
        pending.target = Some((target_atom, mime.clone(), latin1));
        pending.reply = Some(tx);
        pending.started = Instant::now();
        self.incoming = Some(pending);

        let serial = self.serial.load(Ordering::Acquire);
        self.selection.send_replace(Some(Arc::new(Incoming {
            serial,
            observed: tokio::time::Instant::now(),
            initial,
            mime,
            reader: Mutex::new(Some(IncomingReader::X11(rx))),
        })));
        self.conn.convert_selection(
            self.helper_win,
            self.atoms.CLIPBOARD,
            target_atom,
            self.atoms.XFER_PROP,
            CURRENT_TIME,
        )?;
        Ok(())
    }

    fn on_payload_notify(
        &mut self,
        ev: xproto::SelectionNotifyEvent,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let Some(mut pending) = self.incoming.take() else {
            return Ok(());
        };
        if ev.property == x11rb::NONE {
            if let Some(reply) = pending.reply.take() {
                let _ = reply.send(Err("Clipboard owner rejected conversion".into()));
            }
            return Ok(());
        }
        let long_len = u32::try_from(TRANSFER_LIMIT / 4 + 4).unwrap_or(u32::MAX);
        let reply = self
            .conn
            .get_property(
                true,
                self.helper_win,
                ev.property,
                xproto::GetPropertyType::ANY,
                0,
                long_len,
            )?
            .reply()?;
        if reply.type_ == self.atoms.INCR {
            pending.incr = Some(Vec::new());
            pending.started = Instant::now();
            self.incoming = Some(pending);
            return Ok(());
        }
        let latin1 = pending.target.as_ref().is_some_and(|t| t.2);
        Self::deliver_incoming(pending.reply, reply.value, latin1);
        Ok(())
    }

    fn deliver_incoming(
        reply: Option<oneshot::Sender<Result<Vec<u8>, String>>>,
        mut bytes: Vec<u8>,
        latin1: bool,
    ) {
        let Some(tx) = reply else { return };
        if bytes.len() as u64 > TRANSFER_LIMIT {
            let _ = tx.send(Err("Clipboard selection exceeds 256 MiB".into()));
            return;
        }
        if latin1 && std::str::from_utf8(&bytes).is_err() {
            bytes = bytes
                .into_iter()
                .map(|b| b as char)
                .collect::<String>()
                .into_bytes();
        }
        let _ = tx.send(Ok(bytes));
    }

    fn on_property_notify(
        &mut self,
        ev: xproto::PropertyNotifyEvent,
    ) -> Result<(), Box<dyn std::error::Error>> {
        self.last_time = ev.time;
        if ev.window == self.helper_win
            && ev.atom == self.atoms.XFER_PROP
            && ev.state == xproto::Property::NEW_VALUE
        {
            self.handle_incoming_incr_chunk()?;
        } else if ev.state == xproto::Property::DELETE {
            self.advance_outgoing_incr(ev.window, ev.atom)?;
        }
        Ok(())
    }

    fn handle_incoming_incr_chunk(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let Some(mut pending) = self.incoming.take() else {
            return Ok(());
        };
        let Some(mut buf) = pending.incr.take() else {
            return Ok(());
        };
        let long_len = u32::try_from(TRANSFER_LIMIT / 4 + 4).unwrap_or(u32::MAX);
        let reply = self
            .conn
            .get_property(
                true,
                self.helper_win,
                self.atoms.XFER_PROP,
                xproto::GetPropertyType::ANY,
                0,
                long_len,
            )?
            .reply()?;
        if reply.value.is_empty() {
            let latin1 = pending.target.as_ref().is_some_and(|t| t.2);
            Self::deliver_incoming(pending.reply, buf, latin1);
            return Ok(());
        }
        match append_incr_chunk(&mut buf, &reply.value, TRANSFER_LIMIT) {
            Ok(()) => {
                pending.incr = Some(buf);
                pending.started = Instant::now();
                self.incoming = Some(pending);
            }
            Err(err) => {
                if let Some(tx) = pending.reply.take() {
                    let _ = tx.send(Err(err.to_owned()));
                }
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
        let mut done = false;
        if let Some(xfer) = self.outgoing_incr.get_mut(idx) {
            let chunk = next_incr_chunk(&xfer.bytes, xfer.offset, INCR_CHUNK);
            self.conn.change_property8(
                PropMode::REPLACE,
                xfer.requestor,
                xfer.property,
                xfer.target_type,
                chunk,
            )?;
            if chunk.is_empty() {
                done = true;
            } else {
                xfer.offset += chunk.len();
                xfer.started = Instant::now();
            }
        }
        if done {
            self.outgoing_incr.swap_remove(idx);
        }
        Ok(())
    }

    fn on_selection_request(
        &mut self,
        req: xproto::SelectionRequestEvent,
    ) -> Result<(), Box<dyn std::error::Error>> {
        if req.selection != self.atoms.CLIPBOARD {
            self.send_selection_notify(&req, x11rb::NONE)?;
            return Ok(());
        }
        let dest = if req.property == x11rb::NONE {
            req.target
        } else {
            req.property
        };
        let Some(pub_data) = &self.publication else {
            self.send_selection_notify(&req, x11rb::NONE)?;
            return Ok(());
        };
        if req.target == self.atoms.TARGETS {
            self.conn.change_property32(
                PropMode::REPLACE,
                req.requestor,
                dest,
                self.atoms.ATOM,
                &pub_data.targets,
            )?;
            return self.send_selection_notify(&req, dest);
        }
        if req.target == self.atoms.TIMESTAMP {
            self.conn.change_property32(
                PropMode::REPLACE,
                req.requestor,
                dest,
                self.atoms.INTEGER,
                &[pub_data.timestamp],
            )?;
            return self.send_selection_notify(&req, dest);
        }
        if !pub_data.data_targets.contains(&req.target) {
            return self.send_selection_notify(&req, x11rb::NONE);
        }
        let bytes = pub_data.bytes.clone();
        let target_type = if pub_data.mime.starts_with("text/") && req.target == self.atoms.STRING {
            self.atoms.STRING
        } else {
            req.target
        };
        if bytes.len() <= INCR_THRESHOLD {
            self.conn.change_property8(
                PropMode::REPLACE,
                req.requestor,
                dest,
                target_type,
                &bytes,
            )?;
            return self.send_selection_notify(&req, dest);
        }
        if self.start_outgoing_incr(&req, dest, target_type, bytes)? {
            self.send_selection_notify(&req, dest)
        } else {
            self.send_selection_notify(&req, x11rb::NONE)
        }
    }

    fn start_outgoing_incr(
        &mut self,
        req: &xproto::SelectionRequestEvent,
        dest: Atom,
        target_type: Atom,
        bytes: Arc<[u8]>,
    ) -> Result<bool, Box<dyn std::error::Error>> {
        if self.outgoing_incr.len() >= MAX_OUTGOING_INCR {
            return Ok(false);
        }
        let total = u32::try_from(bytes.len()).unwrap_or(u32::MAX);
        let _ = self.conn.change_window_attributes(
            req.requestor,
            &xproto::ChangeWindowAttributesAux::new()
                .event_mask(xproto::EventMask::PROPERTY_CHANGE),
        );
        self.conn.change_property32(
            PropMode::REPLACE,
            req.requestor,
            dest,
            self.atoms.INCR,
            &[total],
        )?;
        self.outgoing_incr.push(OutgoingIncr {
            requestor: req.requestor,
            property: dest,
            target_type,
            bytes,
            offset: 0,
            started: Instant::now(),
        });
        Ok(true)
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
            .send_event(false, req.requestor, xproto::EventMask::NO_EVENT, notify)?;
        Ok(())
    }

    fn evict_expired(&mut self) {
        if let Some(pending) = &self.incoming
            && pending.started.elapsed() > TRANSFER_TIMEOUT
        {
            self.abort_pending("Clipboard source timed out");
        }
        self.outgoing_incr
            .retain(|xfer| xfer.started.elapsed() <= TRANSFER_TIMEOUT);
    }
}

pub(super) fn select_incoming_target(names: &[String]) -> Option<(String, String, bool)> {
    let raw = preferred_mime(names)?;
    let normalized = if raw == "UTF8_STRING" || raw.eq_ignore_ascii_case("text/plain;charset=utf-8")
    {
        "text/plain;charset=utf-8".to_owned()
    } else if raw == "STRING" || raw == "TEXT" {
        "text/plain".to_owned()
    } else {
        raw.clone()
    };
    let latin1 = raw == "STRING";
    Some((raw, normalized, latin1))
}

pub(super) fn build_offered_target_names(mime: &str) -> Vec<&str> {
    let mut names = vec![mime];
    if mime == "text/plain;charset=utf-8" {
        names.extend(["UTF8_STRING", "text/plain", "STRING"]);
    }
    names
}

pub(super) fn is_echo_targets(owning: bool, marker_name: &str, targets: &[String]) -> bool {
    owning && targets.iter().any(|target| target == marker_name)
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
            Some(("image/png".into(), "image/png".into(), false))
        );

        let text_targets = vec!["STRING".into(), "UTF8_STRING".into()];
        assert_eq!(
            select_incoming_target(&text_targets),
            Some((
                "UTF8_STRING".into(),
                "text/plain;charset=utf-8".into(),
                false
            ))
        );

        let legacy = vec!["STRING".into()];
        assert_eq!(
            select_incoming_target(&legacy),
            Some(("STRING".into(), "text/plain".into(), true))
        );
    }

    #[test]
    fn echo_marker_only_suppresses_while_daemon_owns_clipboard() {
        let targets = vec!["UTF8_STRING".into(), "application/x-marker-1".into()];
        assert!(is_echo_targets(true, "application/x-marker-1", &targets));
        assert!(!is_echo_targets(false, "application/x-marker-1", &targets));
        assert!(!is_echo_targets(true, "application/x-marker-2", &targets));
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
