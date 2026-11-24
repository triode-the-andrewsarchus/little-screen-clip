//! X11 RandR monitor discovery, DPI scaling, EWMH single-instance focus, and centering.

use super::{Placement, SCREEN_FRACTION};
use eframe::egui::{Pos2, Vec2};
use std::time::{Duration, Instant};
use x11rb::connection::Connection as _;
use x11rb::protocol::{
    randr::ConnectionExt as _,
    xproto::{
        self, Atom, AtomEnum, ClientMessageData, ClientMessageEvent, ConfigureWindowAux,
        ConnectionExt as _, MapState, Window,
    },
};
use x11rb::rust_connection::RustConnection;

const WORKBENCH_CLASS: &str = "little-clip-workbench";

x11rb::atom_manager! {
    Atoms: AtomsCookie {
        WM_CLASS,
        RESOURCE_MANAGER,
        STRING,
        _NET_ACTIVE_WINDOW,
        _NET_CURRENT_DESKTOP,
        _NET_WM_DESKTOP,
        _NET_WM_PID,
        _NET_CLIENT_LIST,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct MonitorRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub primary: bool,
}

impl MonitorRect {
    fn contains(self, px: i32, py: i32) -> bool {
        px >= self.x
            && py >= self.y
            && px
                < self
                    .x
                    .saturating_add(i32::try_from(self.width).unwrap_or(i32::MAX))
            && py
                < self
                    .y
                    .saturating_add(i32::try_from(self.height).unwrap_or(i32::MAX))
    }
}

pub(super) fn discover_placement() -> Result<Placement, String> {
    let (conn, screen_num) = RustConnection::connect(None).map_err(|e| e.to_string())?;
    let atoms = Atoms::new(&conn)
        .map_err(|e| e.to_string())?
        .reply()
        .map_err(|e| e.to_string())?;
    let screen = &conn.setup().roots[screen_num];
    let root = screen.root;

    let monitors = query_monitors(&conn, root, screen);
    let focus = query_focus_point(&conn, root, &atoms);
    let monitor =
        select_monitor(&monitors, focus).ok_or_else(|| "No active X11 monitor".to_owned())?;
    let scale = resolve_scale(&conn, root, &atoms);
    Ok(placement_for_monitor(monitor, scale))
}

pub(super) fn focus_existing() {
    let Ok((conn, screen_num)) = RustConnection::connect(None) else {
        return;
    };
    let Ok(atoms_cookie) = Atoms::new(&conn) else {
        return;
    };
    let Ok(atoms) = atoms_cookie.reply() else {
        return;
    };
    let root = conn.setup().roots[screen_num].root;
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut target = None;
    while Instant::now() < deadline {
        target = find_workbench_window(&conn, root, &atoms, None, false);
        if target.is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(30));
    }
    let Some(win) = target else { return };
    activate_window(&conn, root, win, &atoms);
}

pub(super) fn center_when_mapped(physical_pos: (i32, i32)) {
    std::thread::spawn(move || {
        let Ok((conn, screen_num)) = RustConnection::connect(None) else {
            return;
        };
        let Ok(atoms_cookie) = Atoms::new(&conn) else {
            return;
        };
        let Ok(atoms) = atoms_cookie.reply() else {
            return;
        };
        let root = conn.setup().roots[screen_num].root;
        let pid = std::process::id();
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            if let Some(win) = find_workbench_window(&conn, root, &atoms, Some(pid), true) {
                // Move-only: AGENTS.md forbids resizing an already mapped workbench popup.
                let _ = conn.configure_window(
                    win,
                    &ConfigureWindowAux::new()
                        .x(physical_pos.0)
                        .y(physical_pos.1),
                );
                let _ = conn.flush();
                return;
            }
            std::thread::sleep(Duration::from_millis(30));
        }
    });
}

fn query_monitors(
    conn: &RustConnection,
    root: Window,
    screen: &xproto::Screen,
) -> Vec<MonitorRect> {
    if let Ok(cookie) = conn.randr_get_monitors(root, true)
        && let Ok(reply) = cookie.reply()
    {
        let active: Vec<MonitorRect> = reply
            .monitors
            .into_iter()
            .filter(|mon| mon.width > 0 && mon.height > 0)
            .map(|mon| MonitorRect {
                x: i32::from(mon.x),
                y: i32::from(mon.y),
                width: u32::from(mon.width),
                height: u32::from(mon.height),
                primary: mon.primary,
            })
            .collect();
        if !active.is_empty() {
            return active;
        }
    }
    if let Ok(cookie) = conn.randr_get_screen_resources_current(root)
        && let Ok(res) = cookie.reply()
    {
        let primary_output = conn
            .randr_get_output_primary(root)
            .ok()
            .and_then(|c| c.reply().ok())
            .map(|r| r.output);
        let crtcs: Vec<MonitorRect> = res
            .crtcs
            .into_iter()
            .filter_map(|crtc| conn.randr_get_crtc_info(crtc, res.config_timestamp).ok())
            .filter_map(|cookie| cookie.reply().ok())
            .filter(|info| info.width > 0 && info.height > 0)
            .map(|info| MonitorRect {
                x: i32::from(info.x),
                y: i32::from(info.y),
                width: u32::from(info.width),
                height: u32::from(info.height),
                primary: primary_output.is_some_and(|p| info.outputs.contains(&p)),
            })
            .collect();
        if !crtcs.is_empty() {
            return crtcs;
        }
    }
    vec![MonitorRect {
        x: 0,
        y: 0,
        width: u32::from(screen.width_in_pixels),
        height: u32::from(screen.height_in_pixels),
        primary: true,
    }]
}

fn query_focus_point(conn: &RustConnection, root: Window, atoms: &Atoms) -> Option<(i32, i32)> {
    if let Some(active) = read_cardinal_u32(conn, root, atoms._NET_ACTIVE_WINDOW, AtomEnum::WINDOW)
        && active != x11rb::NONE
        && let Some(geom) = conn.get_geometry(active).ok().and_then(|c| c.reply().ok())
        && let Some(coords) = conn
            .translate_coordinates(
                active,
                root,
                i16::try_from(geom.width / 2).unwrap_or(0),
                i16::try_from(geom.height / 2).unwrap_or(0),
            )
            .ok()
            .and_then(|c| c.reply().ok())
    {
        return Some((i32::from(coords.dst_x), i32::from(coords.dst_y)));
    }
    conn.query_pointer(root)
        .ok()
        .and_then(|c| c.reply().ok())
        .map(|ptr| (i32::from(ptr.root_x), i32::from(ptr.root_y)))
}

fn resolve_scale(conn: &RustConnection, root: Window, atoms: &Atoms) -> f32 {
    if let Ok(raw) = std::env::var("WINIT_X11_SCALE_FACTOR")
        && let Ok(scale) = raw.trim().parse::<f32>()
        && scale.is_finite()
        && scale > 0.0
    {
        return scale.clamp(0.5, 8.0);
    }
    if let Ok(cookie) = conn.get_property(
        false,
        root,
        atoms.RESOURCE_MANAGER,
        atoms.STRING,
        0,
        64 * 1024,
    ) && let Ok(reply) = cookie.reply()
        && let Ok(text) = std::str::from_utf8(&reply.value)
        && let Some(dpi) = parse_xft_dpi(text)
    {
        return (dpi / 96.0).clamp(0.5, 8.0);
    }
    1.0
}

fn find_workbench_window(
    conn: &RustConnection,
    root: Window,
    atoms: &Atoms,
    expected_pid: Option<u32>,
    require_viewable: bool,
) -> Option<Window> {
    let mut candidates = read_window_list(conn, root, atoms._NET_CLIENT_LIST);
    if candidates.is_empty()
        && let Some(tree) = conn.query_tree(root).ok().and_then(|c| c.reply().ok())
    {
        candidates = tree.children;
    }
    for win in candidates {
        if matches_workbench_window(conn, win, atoms, expected_pid, require_viewable) {
            return Some(win);
        }
    }
    None
}

fn matches_workbench_window(
    conn: &RustConnection,
    win: Window,
    atoms: &Atoms,
    expected_pid: Option<u32>,
    require_viewable: bool,
) -> bool {
    let Some(reply) = conn
        .get_property(false, win, atoms.WM_CLASS, atoms.STRING, 0, 256)
        .ok()
        .and_then(|c| c.reply().ok())
    else {
        return false;
    };
    if !wm_class_matches(&reply.value) {
        return false;
    }
    if let Some(pid) = expected_pid
        && read_cardinal_u32(conn, win, atoms._NET_WM_PID, AtomEnum::CARDINAL) != Some(pid)
    {
        return false;
    }
    if require_viewable {
        let Some(attrs) = conn
            .get_window_attributes(win)
            .ok()
            .and_then(|c| c.reply().ok())
        else {
            return false;
        };
        if attrs.map_state != MapState::VIEWABLE {
            return false;
        }
    }
    true
}

fn activate_window(conn: &RustConnection, root: Window, win: Window, atoms: &Atoms) {
    let target_desktop = read_cardinal_u32(conn, win, atoms._NET_WM_DESKTOP, AtomEnum::CARDINAL);
    let current_desktop =
        read_cardinal_u32(conn, root, atoms._NET_CURRENT_DESKTOP, AtomEnum::CARDINAL);
    let mask = xproto::EventMask::SUBSTRUCTURE_REDIRECT | xproto::EventMask::SUBSTRUCTURE_NOTIFY;
    if let (Some(target), Some(current)) = (target_desktop, current_desktop)
        && target != current
    {
        let switch = ClientMessageEvent::new(
            32,
            root,
            atoms._NET_CURRENT_DESKTOP,
            ClientMessageData::from([target, x11rb::CURRENT_TIME, 0, 0, 0]),
        );
        let _ = conn.send_event(false, root, mask, switch);
    }
    let focus = ClientMessageEvent::new(
        32,
        win,
        atoms._NET_ACTIVE_WINDOW,
        ClientMessageData::from([2, x11rb::CURRENT_TIME, 0, 0, 0]),
    );
    let _ = conn.send_event(false, root, mask, focus);
    let _ = conn.flush();
}

fn read_cardinal_u32(
    conn: &RustConnection,
    win: Window,
    prop: Atom,
    kind: AtomEnum,
) -> Option<u32> {
    conn.get_property(false, win, prop, kind, 0, 1)
        .ok()?
        .reply()
        .ok()?
        .value32()?
        .next()
}

fn read_window_list(conn: &RustConnection, root: Window, prop: Atom) -> Vec<Window> {
    let Some(reply) = conn
        .get_property(false, root, prop, AtomEnum::WINDOW, 0, 4096)
        .ok()
        .and_then(|c| c.reply().ok())
    else {
        return Vec::new();
    };
    reply
        .value32()
        .map(|iter| iter.collect())
        .unwrap_or_default()
}

pub(super) fn parse_xft_dpi(resources: &str) -> Option<f32> {
    for line in resources.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        if key.trim() == "Xft.dpi"
            && let Ok(dpi) = value.trim().parse::<f32>()
            && dpi.is_finite()
            && dpi > 0.0
        {
            return Some(dpi);
        }
    }
    None
}

pub(super) fn select_monitor(
    monitors: &[MonitorRect],
    focus: Option<(i32, i32)>,
) -> Option<MonitorRect> {
    if let Some((px, py)) = focus
        && let Some(&found) = monitors.iter().find(|mon| mon.contains(px, py))
    {
        return Some(found);
    }
    monitors
        .iter()
        .copied()
        .find(|mon| mon.primary)
        .or_else(|| monitors.first().copied())
}

pub(super) fn placement_for_monitor(monitor: MonitorRect, scale: f32) -> Placement {
    let scale = scale.clamp(0.5, 8.0);
    let logical_w = (monitor.width as f32 / scale * SCREEN_FRACTION)
        .floor()
        .max(1.0);
    let logical_h = (monitor.height as f32 / scale * SCREEN_FRACTION)
        .floor()
        .max(1.0);
    let phys_w = (logical_w * scale).round();
    let phys_h = (logical_h * scale).round();
    let phys_x = monitor.x + ((monitor.width as f32 - phys_w) * 0.5).round() as i32;
    let phys_y = monitor.y + ((monitor.height as f32 - phys_h) * 0.5).round() as i32;
    Placement {
        size: Vec2::new(logical_w, logical_h),
        logical_position: Some(Pos2::new(phys_x as f32 / scale, phys_y as f32 / scale)),
        physical_position: Some((phys_x, phys_y)),
    }
}

pub(super) fn wm_class_matches(raw: &[u8]) -> bool {
    raw.split(|b| *b == 0)
        .any(|part| part == WORKBENCH_CLASS.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_xft_dpi_handles_valid_and_invalid_entries() {
        assert_eq!(
            parse_xft_dpi("Xcursor.size: 24\nXft.dpi:\t144\n"),
            Some(144.0)
        );
        assert_eq!(parse_xft_dpi("Xft.dpi: 96"), Some(96.0));
        assert_eq!(parse_xft_dpi("Xft.dpi: 0"), None);
        assert_eq!(parse_xft_dpi("Xft.dpi: NaN"), None);
        assert_eq!(parse_xft_dpi("Xcursor.size: 24"), None);
    }

    #[test]
    fn select_monitor_picks_focused_output_with_primary_fallback() {
        let monitors = [
            MonitorRect {
                x: 0,
                y: 0,
                width: 1920,
                height: 1080,
                primary: true,
            },
            MonitorRect {
                x: 1920,
                y: 0,
                width: 3840,
                height: 2160,
                primary: false,
            },
        ];
        assert_eq!(
            select_monitor(&monitors, Some((2500, 900))),
            Some(monitors[1])
        );
        assert_eq!(
            select_monitor(&monitors, Some((100, 100))),
            Some(monitors[0])
        );
        assert_eq!(
            select_monitor(&monitors, Some((-50, 100))),
            Some(monitors[0])
        );
        assert_eq!(select_monitor(&monitors, None), Some(monitors[0]));
    }

    #[test]
    fn placement_for_monitor_centers_and_scales_consistently() {
        let mon_1x = MonitorRect {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
            primary: true,
        };
        let p1 = placement_for_monitor(mon_1x, 1.0);
        assert_eq!(p1.size, Vec2::new(1766.0, 993.0));
        assert_eq!(p1.physical_position, Some((77, 44)));
        assert_eq!(p1.logical_position, Some(Pos2::new(77.0, 44.0)));

        let mon_2x = MonitorRect {
            x: 1920,
            y: 0,
            width: 3840,
            height: 2160,
            primary: false,
        };
        let p2 = placement_for_monitor(mon_2x, 2.0);
        assert_eq!(p2.size, Vec2::new(1766.0, 993.0));
        assert_eq!(p2.physical_position, Some((1920 + 154, 87)));
        assert_eq!(p2.logical_position, Some(Pos2::new(1037.0, 43.5)));
    }

    #[test]
    fn wm_class_matching_checks_nul_separated_segments() {
        assert!(wm_class_matches(
            b"little-clip-workbench\0little-clip-workbench\0"
        ));
        assert!(wm_class_matches(b"\0little-clip-workbench\0"));
        assert!(!wm_class_matches(
            b"clip-file-manager\0little-clip-file-manager\0"
        ));
        assert!(!wm_class_matches(b"little-clip-workbench-extra\0"));
    }
}
