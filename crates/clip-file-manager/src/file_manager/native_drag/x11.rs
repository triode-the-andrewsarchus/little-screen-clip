//! Native X11 XDND v5 incoming and outgoing file drag-and-drop.

use super::super::filesystem::Identity;
use super::super::search::SourceGuard;
use super::{Event, io};
use little_clip_core::{BackgroundJobs, JobError};
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::time::{Duration, Instant};
use x11rb::{
    COPY_DEPTH_FROM_PARENT, CURRENT_TIME,
    connection::{Connection as _, RequestConnection as _},
    protocol::{
        Event as XEvent,
        xproto::{
            self, Atom, AtomEnum, ClientMessageData, ClientMessageEvent, ConnectionExt as _,
            KeyButMask, PropMode, SelectionRequestEvent, Window,
        },
    },
    rust_connection::RustConnection,
    wrapper::ConnectionExt as _,
};

const MAX_WALK_DEPTH: u8 = 32;
const DROP_STATUS_TIMEOUT: Duration = Duration::from_millis(500);
const DROP_FINISH_TIMEOUT: Duration = Duration::from_secs(10);

type SnapshotResult = Result<BTreeMap<PathBuf, Identity>, String>;
type SnapshotReceiver = mpsc::Receiver<Result<SnapshotResult, JobError>>;

x11rb::atom_manager! {
    Atoms: AtomsCookie {
        XdndAware,
        XdndProxy,
        XdndEnter,
        XdndPosition,
        XdndStatus,
        XdndLeave,
        XdndDrop,
        XdndFinished,
        XdndSelection,
        XdndActionCopy,
        XdndActionMove,
        XdndActionPrivate,
        TEXT_URI_LIST: b"text/uri-list",
        TARGETS,
        ATOM,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct XdndTarget {
    window: Window,
    destination: Window,
    version: u8,
}

struct OutgoingDrag {
    source: Window,
    payload: Arc<[u8]>,
    identities: BTreeMap<PathBuf, Option<Identity>>,
    snapshot: Option<BTreeMap<PathBuf, Identity>>,
    snapshot_job: Option<SnapshotReceiver>,
    pending_requests: Vec<SelectionRequestEvent>,
    cancelled: Arc<AtomicBool>,
    written: bool,
    movable: bool,
    search_guards: HashMap<PathBuf, SourceGuard>,
    target: Option<XdndTarget>,
    waiting_status: bool,
    release: Option<(Instant, (i16, i16), bool)>,
    accepted_action: Atom,
    last_sent_pos: Option<(i16, i16)>,
    last_sent_action: Option<Atom>,
    dropped_at: Option<Instant>,
}

pub(super) struct X11Drag {
    conn: RustConnection,
    root: Window,
    uri_limit: usize,
    host_window: Window,
    atoms: Atoms,
    incoming_hover: Option<egui::Pos2>,
    outgoing: Option<OutgoingDrag>,
    events: Vec<Event>,
    next_token: u64,
    release_pointer: bool,
    escape_pressed: bool,
}

impl X11Drag {
    pub(super) fn new(host_window: Window) -> Option<Self> {
        let (conn, _) = RustConnection::connect(None).ok()?;
        let atoms = Atoms::new(&conn).ok()?.reply().ok()?;
        let root = conn.get_geometry(host_window).ok()?.reply().ok()?.root;
        // Query once before the first drag. Selection data is sent in one property.
        let uri_limit = io::LIMIT.min(conn.maximum_request_bytes().saturating_sub(28));
        Some(Self {
            conn,
            root,
            uri_limit,
            host_window,
            atoms,
            incoming_hover: None,
            outgoing: None,
            events: Vec::new(),
            next_token: 0,
            release_pointer: false,
            escape_pressed: false,
        })
    }

    pub(super) fn set_reference_targets(&mut self, _rects: Vec<egui::Rect>) {}

    pub(super) fn drag_position(&self) -> Option<egui::Pos2> {
        self.incoming_hover
    }

    pub(super) fn busy(&self) -> bool {
        self.outgoing.is_some() || self.incoming_hover.is_some()
    }

    pub(super) fn finish(&mut self, _token: u64, _success: bool) {}

    pub(super) fn raw_input(&mut self, ctx: &egui::Context, input: &mut egui::RawInput) {
        if self.outgoing.is_some()
            && input.events.iter().any(|event| {
                matches!(
                    event,
                    egui::Event::Key {
                        key: egui::Key::Escape,
                        pressed: true,
                        ..
                    }
                )
            })
        {
            self.escape_pressed = true;
        }
        let native_scale = ctx.native_pixels_per_point().unwrap_or(1.0).max(0.5);
        if !input.hovered_files.is_empty() {
            if let Some(pos) = self.query_host_pointer(native_scale) {
                self.incoming_hover = Some(pos);
            }
        } else if input.dropped_files.is_empty() {
            self.incoming_hover = None;
        }

        if !input.dropped_files.is_empty() {
            let point = self
                .query_host_pointer(native_scale)
                .or(self.incoming_hover)
                .unwrap_or_else(|| {
                    ctx.pointer_latest_pos().unwrap_or_default() * ctx.zoom_factor()
                });
            let paths: Vec<PathBuf> = input
                .dropped_files
                .drain(..)
                .map(|file| file.path().to_path_buf())
                .filter(|path| !path.as_os_str().is_empty())
                .collect();
            self.incoming_hover = None;
            if !paths.is_empty() {
                self.next_token += 1;
                let token = self.next_token;
                self.events.push(Event::Drop(token, point));
                // External incoming X11 drops via winit are always Copy-only.
                self.events
                    .push(Event::Files(token, paths, false, HashMap::new()));
            }
        }

        if self.release_pointer {
            self.release_pointer = false;
            super::release_egui_pointer(ctx, input);
        }
    }

    pub(super) fn start(
        &mut self,
        identities: BTreeMap<PathBuf, Option<Identity>>,
        copy: bool,
        search_guards: HashMap<PathBuf, SourceGuard>,
    ) -> Result<bool, String> {
        if self.outgoing.is_some() {
            return Ok(false);
        }
        let Ok(ptr) = self
            .conn
            .query_pointer(self.host_window)
            .map_err(|e| e.to_string())
            .and_then(|c| c.reply().map_err(|e| e.to_string()))
        else {
            return Ok(false);
        };
        let Ok(geom) = self
            .conn
            .get_geometry(self.host_window)
            .map_err(|e| e.to_string())
            .and_then(|c| c.reply().map_err(|e| e.to_string()))
        else {
            return Ok(false);
        };
        if !is_outside_host(
            ptr.same_screen,
            ptr.win_x,
            ptr.win_y,
            geom.width,
            geom.height,
        ) {
            return Ok(false);
        }
        if !ptr.mask.contains(KeyButMask::BUTTON1) {
            return Ok(false);
        }

        let paths: Vec<_> = identities.keys().cloned().collect();
        let payload = little_clip_core::file_uri_list(&paths)
            .map_err(|e| e.to_string())?
            .into_bytes();
        if payload.len() > self.uri_limit {
            return Err(format!(
                "Desktop file references exceed the {}-byte X11 limit",
                self.uri_limit
            ));
        }

        let movable = !copy && identities.values().all(Option::is_some);
        let source = self.conn.generate_id().map_err(|e| e.to_string())?;
        self.conn
            .create_window(
                COPY_DEPTH_FROM_PARENT,
                source,
                self.root,
                0,
                0,
                1,
                1,
                0,
                xproto::WindowClass::INPUT_ONLY,
                x11rb::COPY_FROM_PARENT,
                &xproto::CreateWindowAux::new(),
            )
            .map_err(|e| e.to_string())?
            .check()
            .map_err(|e| e.to_string())?;
        let ownership = self
            .conn
            .set_selection_owner(source, self.atoms.XdndSelection, CURRENT_TIME)
            .map_err(|e| e.to_string())
            .and_then(|cookie| cookie.check().map_err(|e| e.to_string()));
        if let Err(error) = ownership {
            let _ = self.conn.destroy_window(source);
            return Err(error);
        }

        self.outgoing = Some(OutgoingDrag {
            source,
            payload: payload.into(),
            identities,
            snapshot: None,
            snapshot_job: None,
            pending_requests: Vec::new(),
            cancelled: Arc::new(AtomicBool::new(false)),
            written: false,
            movable,
            search_guards,
            target: None,
            waiting_status: false,
            release: None,
            accepted_action: x11rb::NONE,
            last_sent_pos: None,
            last_sent_action: None,
            dropped_at: None,
        });
        self.escape_pressed = false;
        self.release_pointer = true;
        Ok(true)
    }

    pub(super) fn tick(&mut self) -> Vec<Event> {
        self.poll_snapshot_job();
        while let Ok(Some(event)) = self.conn.poll_for_event() {
            self.handle_x11_event(event);
        }
        self.drive_outgoing_drag();
        let _ = self.conn.flush();
        std::mem::take(&mut self.events)
    }

    fn query_host_pointer(&self, native_scale: f32) -> Option<egui::Pos2> {
        let reply = self
            .conn
            .query_pointer(self.host_window)
            .ok()?
            .reply()
            .ok()?;
        if !reply.same_screen {
            return None;
        }
        Some(egui::pos2(
            f32::from(reply.win_x) / native_scale,
            f32::from(reply.win_y) / native_scale,
        ))
    }

    fn poll_snapshot_job(&mut self) {
        let Some(outgoing) = &mut self.outgoing else {
            return;
        };
        let Some(rx) = &outgoing.snapshot_job else {
            return;
        };
        let result = match rx.try_recv() {
            Ok(inner) => inner.map_err(|e| e.to_string()).and_then(|r| r),
            Err(mpsc::TryRecvError::Disconnected) => {
                Err("Desktop source verification disconnected".into())
            }
            Err(mpsc::TryRecvError::Empty) => return,
        };
        outgoing.snapshot_job = None;
        let pending = std::mem::take(&mut outgoing.pending_requests);
        match result {
            Ok(entries) => {
                outgoing.snapshot = Some(entries);
                let payload = Arc::clone(&outgoing.payload);
                for req in pending {
                    self.write_uri_selection(&req, &payload);
                }
            }
            Err(error) => {
                for req in pending {
                    self.send_selection_notify(&req, x11rb::NONE);
                }
                self.events.push(Event::Notice(error));
            }
        }
    }

    fn handle_x11_event(&mut self, event: XEvent) {
        match event {
            XEvent::SelectionRequest(req) if req.selection == self.atoms.XdndSelection => {
                self.on_selection_request(req);
            }
            XEvent::ClientMessage(msg) => self.on_client_message(msg),
            XEvent::SelectionClear(ev) if ev.selection == self.atoms.XdndSelection => {
                if let Some(outgoing) = &self.outgoing
                    && outgoing.source == ev.owner
                {
                    self.cancel_outgoing();
                }
            }
            _ => {}
        }
    }

    fn on_selection_request(&mut self, req: SelectionRequestEvent) {
        let Some(outgoing) = &mut self.outgoing else {
            self.send_selection_notify(&req, x11rb::NONE);
            return;
        };
        if req.owner != outgoing.source || outgoing.target.is_none() {
            self.send_selection_notify(&req, x11rb::NONE);
            return;
        }
        if req.target == self.atoms.TARGETS {
            let dest = dest_property(&req);
            let result = self
                .conn
                .change_property32(
                    PropMode::REPLACE,
                    req.requestor,
                    dest,
                    self.atoms.ATOM,
                    &[self.atoms.TARGETS, self.atoms.TEXT_URI_LIST],
                )
                .is_ok_and(|cookie| cookie.check().is_ok());
            self.send_selection_notify(&req, if result { dest } else { x11rb::NONE });
            return;
        }
        if req.target != self.atoms.TEXT_URI_LIST {
            self.send_selection_notify(&req, x11rb::NONE);
            return;
        }
        let move_active = [
            outgoing.accepted_action,
            outgoing.last_sent_action.unwrap_or(x11rb::NONE),
        ]
        .contains(&self.atoms.XdndActionMove);
        let need_snapshot = outgoing.movable && outgoing.snapshot.is_none() && move_active;
        if !need_snapshot {
            let payload = Arc::clone(&outgoing.payload);
            self.write_uri_selection(&req, &payload);
            return;
        }
        if outgoing.pending_requests.len() >= 8 {
            self.send_selection_notify(&req, x11rb::NONE);
            return;
        }
        outgoing.pending_requests.push(req);
        if outgoing.snapshot_job.is_none() {
            let roots = outgoing.identities.clone();
            let guards = outgoing.search_guards.clone();
            let cancelled = Arc::clone(&outgoing.cancelled);
            let deadline = Instant::now() + DROP_FINISH_TIMEOUT;
            let (_, receiver) = BackgroundJobs::global().submit(Duration::MAX, move || {
                io::verify_and_snapshot(&roots, &guards, deadline, &cancelled)
            });
            outgoing.snapshot_job = Some(receiver);
        }
    }

    fn write_uri_selection(&mut self, req: &SelectionRequestEvent, payload: &[u8]) {
        let dest = dest_property(req);
        let written = self
            .conn
            .change_property8(PropMode::REPLACE, req.requestor, dest, req.target, payload)
            .is_ok_and(|cookie| cookie.check().is_ok());
        if written && let Some(outgoing) = &mut self.outgoing {
            outgoing.written = true;
        }
        self.send_selection_notify(req, if written { dest } else { x11rb::NONE });
    }

    fn send_selection_notify(&self, req: &SelectionRequestEvent, property: Atom) {
        let notify = xproto::SelectionNotifyEvent {
            response_type: xproto::SELECTION_NOTIFY_EVENT,
            sequence: 0,
            time: req.time,
            requestor: req.requestor,
            selection: req.selection,
            target: req.target,
            property,
        };
        let _ = self
            .conn
            .send_event(false, req.requestor, xproto::EventMask::NO_EVENT, notify);
    }

    fn on_client_message(&mut self, msg: ClientMessageEvent) {
        let Some(outgoing) = &mut self.outgoing else {
            return;
        };
        let data = msg.data.as_data32();
        if msg.format != 32
            || msg.window != outgoing.source
            || !target_matches(outgoing.target, data[0])
        {
            return;
        }
        if msg.type_ == self.atoms.XdndStatus && outgoing.waiting_status {
            outgoing.waiting_status = false;
            let action = data[4];
            let supported = [self.atoms.XdndActionCopy, self.atoms.XdndActionPrivate]
                .contains(&action)
                || outgoing.last_sent_action == Some(action);
            outgoing.accepted_action = if (data[1] & 1) != 0 && supported {
                action
            } else {
                x11rb::NONE
            };
        } else if msg.type_ == self.atoms.XdndFinished && outgoing.dropped_at.is_some() {
            self.finish_outgoing((data[1] & 1) != 0, data[2]);
        }
    }

    fn finish_outgoing(&mut self, success: bool, action: Atom) {
        let Some(mut outgoing) = self.outgoing.take() else {
            return;
        };
        self.retire_source(&outgoing);
        let move_action = self.atoms.XdndActionMove;
        if should_remove_after_finished(
            outgoing.movable && outgoing.target.is_some_and(|target| target.version >= 5),
            success,
            outgoing.written,
            action == move_action && outgoing.accepted_action == move_action,
            outgoing
                .snapshot
                .as_ref()
                .is_some_and(|snapshot| !snapshot.is_empty()),
        ) && let Some(snapshot) = outgoing.snapshot.take()
        {
            self.events.push(Event::Remove(
                outgoing.identities,
                snapshot,
                outgoing.search_guards,
            ));
        }
    }

    fn drive_outgoing_drag(&mut self) {
        let Some(outgoing) = &self.outgoing else {
            return;
        };
        if let Some(dropped_at) = outgoing.dropped_at {
            if dropped_at.elapsed() > DROP_FINISH_TIMEOUT {
                self.cancel_outgoing();
                self.events.push(Event::Notice(
                    "Desktop drop target did not finish in time".into(),
                ));
            }
            return;
        }
        if self.escape_pressed {
            self.escape_pressed = false;
            self.cancel_outgoing();
            return;
        }
        let (pos, copy_requested) = match outgoing.release {
            Some((released, pos, copy)) if released.elapsed() < DROP_STATUS_TIMEOUT => (pos, copy),
            Some(_) => {
                self.cancel_outgoing();
                return;
            }
            None => match self.update_outgoing_pointer() {
                Some(pointer) => pointer,
                None => {
                    self.cancel_outgoing();
                    return;
                }
            },
        };
        self.advance_drop_target(pos, copy_requested);
    }

    fn update_outgoing_pointer(&mut self) -> Option<((i16, i16), bool)> {
        let ptr = self.conn.query_pointer(self.root).ok()?.reply().ok()?;
        let pos = (ptr.root_x, ptr.root_y);
        let copy = ptr.mask.contains(KeyButMask::CONTROL);
        let target = if ptr.same_screen {
            self.find_xdnd_target(pos.0, pos.1)
        } else {
            None
        };
        self.sync_target_change(target);
        if !ptr.mask.contains(KeyButMask::BUTTON1) {
            self.outgoing.as_mut()?.release = Some((Instant::now(), pos, copy));
        }
        Some((pos, copy))
    }

    fn advance_drop_target(&mut self, pos: (i16, i16), copy_requested: bool) {
        let Some(outgoing) = &mut self.outgoing else {
            return;
        };
        let Some(target) = outgoing.target else {
            if outgoing.release.is_some() {
                self.cancel_outgoing();
            }
            return;
        };
        let desired_action = if outgoing.movable && target.version >= 5 && !copy_requested {
            self.atoms.XdndActionMove
        } else {
            self.atoms.XdndActionCopy
        };
        if outgoing.waiting_status {
            return;
        }
        if outgoing.last_sent_pos != Some(pos) || outgoing.last_sent_action != Some(desired_action)
        {
            send_xdnd_msg(
                &self.conn,
                target,
                self.atoms.XdndPosition,
                [
                    outgoing.source,
                    0,
                    pack_xdnd_coords(pos.0, pos.1),
                    CURRENT_TIME,
                    desired_action,
                ],
            );
            outgoing.last_sent_pos = Some(pos);
            outgoing.last_sent_action = Some(desired_action);
            outgoing.waiting_status = true;
            return;
        }
        if outgoing.release.is_some() {
            if outgoing.accepted_action == x11rb::NONE {
                self.cancel_outgoing();
                return;
            }
            send_xdnd_msg(
                &self.conn,
                target,
                self.atoms.XdndDrop,
                [outgoing.source, 0, CURRENT_TIME, 0, 0],
            );
            outgoing.dropped_at = Some(Instant::now());
        }
    }

    fn sync_target_change(&mut self, next: Option<XdndTarget>) {
        let Some(outgoing) = &mut self.outgoing else {
            return;
        };
        if outgoing.target == next {
            return;
        }
        if let Some(old) = outgoing.target {
            send_xdnd_msg(
                &self.conn,
                old,
                self.atoms.XdndLeave,
                [outgoing.source, 0, 0, 0, 0],
            );
        }
        let pending = std::mem::take(&mut outgoing.pending_requests);
        outgoing.target = next;
        outgoing.waiting_status = false;
        outgoing.accepted_action = x11rb::NONE;
        outgoing.last_sent_pos = None;
        outgoing.last_sent_action = None;
        outgoing.written = false;
        if let Some(target) = next {
            send_xdnd_msg(
                &self.conn,
                target,
                self.atoms.XdndEnter,
                [
                    outgoing.source,
                    xdnd_enter_flags(target.version),
                    self.atoms.TEXT_URI_LIST,
                    0,
                    0,
                ],
            );
        }
        for req in pending {
            self.send_selection_notify(&req, x11rb::NONE);
        }
    }

    fn cancel_outgoing(&mut self) {
        if let Some(mut outgoing) = self.outgoing.take() {
            for req in outgoing.pending_requests.drain(..) {
                self.send_selection_notify(&req, x11rb::NONE);
            }
            if let Some(target) = outgoing.target {
                send_xdnd_msg(
                    &self.conn,
                    target,
                    self.atoms.XdndLeave,
                    [outgoing.source, 0, 0, 0, 0],
                );
            }
            self.retire_source(&outgoing);
        }
    }

    fn retire_source(&mut self, outgoing: &OutgoingDrag) {
        outgoing.cancelled.store(true, Ordering::Relaxed);
        let _ = self.conn.destroy_window(outgoing.source);
        self.events.push(Event::Refresh);
    }

    fn find_xdnd_target(&self, root_x: i16, root_y: i16) -> Option<XdndTarget> {
        let mut parent = self.root;
        for _ in 0..MAX_WALK_DEPTH {
            // The server returns the topmost mapped child at the pointer. An
            // obscured window must never receive the drop through its cover.
            let child = self
                .conn
                .translate_coordinates(self.root, parent, root_x, root_y)
                .ok()?
                .reply()
                .ok()?
                .child;
            if child == x11rb::NONE || child == self.host_window {
                return None;
            }
            if let Some(target) = self.inspect_xdnd_target(child) {
                return Some(target);
            }
            parent = child;
        }
        None
    }

    fn read_u32_prop(&self, win: Window, prop: Atom, kind: impl Into<Atom>) -> Option<u32> {
        self.conn
            .get_property(false, win, prop, kind, 0, 1)
            .ok()?
            .reply()
            .ok()?
            .value32()?
            .next()
    }

    fn inspect_xdnd_target(&self, win: Window) -> Option<XdndTarget> {
        let proxy = self.valid_xdnd_proxy(win);
        let destination = proxy.unwrap_or(win);
        let raw_version = self.read_u32_prop(
            destination,
            self.atoms.XdndAware,
            xproto::GetPropertyType::ANY,
        )?;
        let version = u8::try_from(raw_version.min(5)).ok()?;
        (version >= 3).then_some(XdndTarget {
            window: win,
            destination,
            version,
        })
    }

    fn valid_xdnd_proxy(&self, win: Window) -> Option<Window> {
        let proxy = self.read_u32_prop(win, self.atoms.XdndProxy, AtomEnum::WINDOW)?;
        if proxy == x11rb::NONE {
            return None;
        }
        let back = self.read_u32_prop(proxy, self.atoms.XdndProxy, AtomEnum::WINDOW)?;
        (back == proxy).then_some(proxy)
    }
}

impl Drop for X11Drag {
    fn drop(&mut self) {
        self.cancel_outgoing();
        let _ = self.conn.flush();
    }
}

fn send_xdnd_msg(conn: &RustConnection, target: XdndTarget, msg_type: Atom, data: [u32; 5]) {
    let msg = ClientMessageEvent::new(32, target.window, msg_type, ClientMessageData::from(data));
    let _ = conn.send_event(false, target.destination, xproto::EventMask::NO_EVENT, msg);
}

fn dest_property(req: &SelectionRequestEvent) -> Atom {
    if req.property == x11rb::NONE {
        req.target
    } else {
        req.property
    }
}

fn target_matches(target: Option<XdndTarget>, window: Window) -> bool {
    target.is_some_and(|t| t.window == window || t.destination == window)
}

pub(super) fn is_outside_host(
    same_screen: bool,
    win_x: i16,
    win_y: i16,
    width: u16,
    height: u16,
) -> bool {
    !same_screen
        || win_x < 0
        || win_y < 0
        || i32::from(win_x) >= i32::from(width)
        || i32::from(win_y) >= i32::from(height)
}

pub(super) fn pack_xdnd_coords(x: i16, y: i16) -> u32 {
    (u32::from(x as u16) << 16) | u32::from(y as u16)
}

pub(super) fn xdnd_enter_flags(version: u8) -> u32 {
    u32::from(version.min(5)) << 24
}

pub(super) fn should_remove_after_finished(
    confirms_success: bool,
    finished_ok: bool,
    written: bool,
    action_is_move: bool,
    snapshot_ready: bool,
) -> bool {
    confirms_success && finished_ok && written && action_is_move && snapshot_ready
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remove_requires_movable_written_accepted_move_and_verified_snapshot() {
        assert!(should_remove_after_finished(true, true, true, true, true));
        for case in [
            (false, true, true, true, true), // Older targets do not report successful completion.
            (true, false, true, true, true),
            (true, true, false, true, true),
            (true, true, true, false, true),
            (true, true, true, true, false),
        ] {
            assert!(
                !should_remove_after_finished(case.0, case.1, case.2, case.3, case.4),
                "{case:?}"
            );
        }
    }

    #[test]
    fn host_boundary_check_respects_edges_and_screen() {
        assert!(!is_outside_host(true, 0, 0, 800, 600));
        assert!(!is_outside_host(true, 799, 599, 800, 600));
        assert!(is_outside_host(true, -1, 100, 800, 600));
        assert!(is_outside_host(true, 100, -1, 800, 600));
        assert!(is_outside_host(true, 800, 100, 800, 600));
        assert!(is_outside_host(true, 100, 600, 800, 600));
        assert!(is_outside_host(false, 100, 100, 800, 600));
    }

    #[test]
    fn xdnd_protocol_bit_packing_matches_spec() {
        assert_eq!(xdnd_enter_flags(5), 5 << 24);
        assert_eq!(xdnd_enter_flags(9), 5 << 24);
        assert_eq!(pack_xdnd_coords(1920, 420), (1920 << 16) | 420);
        assert_eq!(
            pack_xdnd_coords(-10, 5),
            (u32::from((-10i16) as u16) << 16) | 5
        );
    }
}
