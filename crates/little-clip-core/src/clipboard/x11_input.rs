//! X11 keyboard shortcut synthesis via the XTEST extension.

use super::{ClipboardError, ClipboardShortcutAction};
use x11rb::connection::Connection;
use x11rb::protocol::{xproto, xproto::ConnectionExt as _, xtest::ConnectionExt as _};
use x11rb::wrapper::ConnectionExt as _;

const KEYSYM_CTRL_L: u32 = 0xffe3;
const KEYSYM_CTRL_R: u32 = 0xffe4;
const KEYSYM_ALT_L: u32 = 0xffe9;
const KEYSYM_ALT_R: u32 = 0xffea;
const KEYSYM_SUPER_L: u32 = 0xffeb;
const KEYSYM_SUPER_R: u32 = 0xffec;
const KEYSYM_C: u32 = 0x0063;
const KEYSYM_V: u32 = 0x0076;

struct XtestKeymap {
    conn: x11rb::rust_connection::RustConnection,
    min_kc: u8,
    per_kc: usize,
    syms: Vec<u32>,
    keymap: [u8; 32],
}

pub(super) fn send_shortcut(
    action: ClipboardShortcutAction,
    release_alt: bool,
) -> Result<(), ClipboardError> {
    let km = query_xtest_keymap()?;
    let (ctrl_kc, key_kc, ctrl_held) =
        resolve_shortcut_keys(action, km.min_kc, km.per_kc, &km.syms, &km.keymap)?;

    if release_alt {
        release_held_modifiers(&km.conn, km.min_kc, km.per_kc, &km.syms, &km.keymap)?;
    }
    if !ctrl_held {
        emit_key(&km.conn, xproto::KEY_PRESS_EVENT, ctrl_kc)?;
    }
    emit_key(&km.conn, xproto::KEY_PRESS_EVENT, key_kc)?;
    emit_key(&km.conn, xproto::KEY_RELEASE_EVENT, key_kc)?;
    if !ctrl_held {
        emit_key(&km.conn, xproto::KEY_RELEASE_EVENT, ctrl_kc)?;
    }
    km.conn
        .sync()
        .map_err(|e| ClipboardError::Failed(format!("XTEST sync: {e}")))?;
    Ok(())
}

fn query_xtest_keymap() -> Result<XtestKeymap, ClipboardError> {
    let (conn, _) =
        x11rb::connect(None).map_err(|e| ClipboardError::Failed(format!("X11 connect: {e}")))?;
    conn.xtest_get_version(2, 2)
        .map_err(|e| ClipboardError::Failed(format!("XTEST query: {e}")))?
        .reply()
        .map_err(|e| ClipboardError::Failed(format!("XTEST unavailable: {e}")))?;
    let min_kc = conn.setup().min_keycode;
    let count = conn
        .setup()
        .max_keycode
        .saturating_sub(min_kc)
        .saturating_add(1);
    let mapping = conn
        .get_keyboard_mapping(min_kc, count)
        .map_err(|e| ClipboardError::Failed(format!("keymap query: {e}")))?
        .reply()
        .map_err(|e| ClipboardError::Failed(format!("keymap reply: {e}")))?;
    let keymap = conn
        .query_keymap()
        .map_err(|e| ClipboardError::Failed(format!("query_keymap: {e}")))?
        .reply()
        .map_err(|e| ClipboardError::Failed(format!("query_keymap reply: {e}")))?
        .keys;
    let per_kc = usize::from(mapping.keysyms_per_keycode).max(1);
    Ok(XtestKeymap {
        conn,
        min_kc,
        per_kc,
        syms: mapping.keysyms,
        keymap,
    })
}

fn resolve_shortcut_keys(
    action: ClipboardShortcutAction,
    min_kc: u8,
    per_kc: usize,
    syms: &[u32],
    keymap: &[u8; 32],
) -> Result<(u8, u8, bool), ClipboardError> {
    let target_sym = match action {
        ClipboardShortcutAction::Copy => KEYSYM_C,
        ClipboardShortcutAction::Paste => KEYSYM_V,
    };
    let ctrl_kc = find_keycode(min_kc, per_kc, syms, KEYSYM_CTRL_L)
        .or_else(|| find_keycode(min_kc, per_kc, syms, KEYSYM_CTRL_R))
        .ok_or_else(|| ClipboardError::Failed("X11 Control keycode not mapped".into()))?;
    let key_kc = find_keycode(min_kc, per_kc, syms, target_sym)
        .ok_or_else(|| ClipboardError::Failed("X11 shortcut keycode not mapped".into()))?;
    let ctrl_held = [KEYSYM_CTRL_L, KEYSYM_CTRL_R]
        .into_iter()
        .filter_map(|sym| find_keycode(min_kc, per_kc, syms, sym))
        .any(|kc| is_key_pressed(keymap, kc));
    Ok((ctrl_kc, key_kc, ctrl_held))
}

fn release_held_modifiers<C: x11rb::protocol::xtest::ConnectionExt>(
    conn: &C,
    min_kc: u8,
    per_kc: usize,
    syms: &[u32],
    keymap: &[u8; 32],
) -> Result<(), ClipboardError> {
    for sym in [KEYSYM_ALT_L, KEYSYM_ALT_R, KEYSYM_SUPER_L, KEYSYM_SUPER_R] {
        if let Some(kc) = find_keycode(min_kc, per_kc, syms, sym)
            && is_key_pressed(keymap, kc)
        {
            emit_key(conn, xproto::KEY_RELEASE_EVENT, kc)?;
        }
    }
    Ok(())
}

fn emit_key<C: x11rb::protocol::xtest::ConnectionExt>(
    conn: &C,
    event_type: u8,
    keycode: u8,
) -> Result<(), ClipboardError> {
    conn.xtest_fake_input(
        event_type,
        keycode,
        x11rb::CURRENT_TIME,
        x11rb::NONE,
        0,
        0,
        0,
    )
    .map_err(|e| ClipboardError::Failed(format!("XTEST fake input: {e}")))?
    .check()
    .map_err(|e| ClipboardError::Failed(format!("XTEST fake input error: {e}")))?;
    Ok(())
}

pub(super) fn find_keycode(
    min_keycode: u8,
    per_keycode: usize,
    keysyms: &[u32],
    target: u32,
) -> Option<u8> {
    if per_keycode == 0 {
        return None;
    }
    for col in 0..per_keycode {
        for (idx, chunk) in keysyms.chunks_exact(per_keycode).enumerate() {
            if chunk[col] == target {
                return u8::try_from(usize::from(min_keycode) + idx).ok();
            }
        }
    }
    None
}

pub(super) fn is_key_pressed(keymap: &[u8; 32], keycode: u8) -> bool {
    let byte = usize::from(keycode / 8);
    let bit = keycode % 8;
    (keymap[byte] & (1 << bit)) != 0
}
