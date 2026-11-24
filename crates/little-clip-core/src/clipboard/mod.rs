//! Local Wayland/X11 clipboard reading, writing, and shortcut synthesis.
//!
//! Each adapter uses the same display selection as the desktop windows.
//! Content is written through standard input rather than command-line arguments so it is not
//! exposed through process listings. Callers that watch the clipboard must perform their own
//! echo-loop suppression around these functions.

mod x11_input;

use crate::{CommandOutput, CommandRequest, command_available};
use std::path::Path;
use std::time::Duration;
use thiserror::Error;
use url::Url;

const CLIPBOARD_TIMEOUT: Duration = Duration::from_secs(5);
const SHORTCUT_TIMEOUT: Duration = Duration::from_millis(500);
const TYPES_LIMIT: usize = 64 * 1024;
const SHORTCUT_OUTPUT_LIMIT: usize = 16 * 1024;

/// Active graphical session kind inferred from display environment variables.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionKind {
    Wayland,
    X11,
    Auto,
}

/// Target X11/Wayland selection buffer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClipboardSelection {
    Clipboard,
    Primary,
}

/// Clipboard shortcut synthesized via `XTEST` or `wtype`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClipboardShortcutAction {
    Copy,
    Paste,
}

/// Detects the display backend using the same environment precedence as winit.
#[must_use]
pub fn detect_session_kind() -> SessionKind {
    if has_nonempty_env("WAYLAND_DISPLAY") || has_nonempty_env("WAYLAND_SOCKET") {
        SessionKind::Wayland
    } else if has_nonempty_env("DISPLAY") {
        SessionKind::X11
    } else {
        SessionKind::Auto
    }
}

fn has_nonempty_env(name: &str) -> bool {
    std::env::var_os(name).is_some_and(|value| !value.is_empty())
}

/// Writes UTF-8 text to the system clipboard.
///
/// # Errors
///
/// Returns an error when the session's clipboard adapter is unavailable or fails.
pub fn copy_text_to_system_clipboard(text: &str) -> Result<(), ClipboardError> {
    copy_bytes_to_system_clipboard(text.as_bytes(), "text/plain;charset=utf-8")
}

/// Writes bytes to the system clipboard using the supplied MIME type.
///
/// # Errors
///
/// Returns an error when the session's clipboard adapter is unavailable or fails.
pub fn copy_bytes_to_system_clipboard(bytes: &[u8], mime: &str) -> Result<(), ClipboardError> {
    let (program, request) = if detect_session_kind() == SessionKind::X11 {
        let target = if mime.eq_ignore_ascii_case("text/plain;charset=utf-8") {
            "UTF8_STRING"
        } else {
            mime
        };
        (
            "xclip",
            CommandRequest::new("xclip").args(["-selection", "clipboard", "-t", target]),
        )
    } else {
        ("wl-copy", CommandRequest::new("wl-copy").args(["-t", mime]))
    };
    if command_available(program).is_none() {
        return Err(ClipboardError::Unavailable);
    }
    let output = request
        .stdin_bytes(bytes)
        .discard_output()
        .timeout(CLIPBOARD_TIMEOUT)
        // xclip and wl-copy fork a selection owner. It must outlive this command.
        .run()
        .map_err(|error| ClipboardError::Failed(error.to_string()))?;
    if !output.status.success() {
        return Err(ClipboardError::Failed(format!(
            "{program} exited {}",
            output.status
        )));
    }
    Ok(())
}

/// Lists advertised MIME types or target atoms on the requested selection.
///
/// # Errors
///
/// Returns an error when the session's clipboard adapter is unavailable, fails or exceeds 64 KiB.
pub fn list_system_clipboard_types(
    selection: ClipboardSelection,
) -> Result<Vec<String>, ClipboardError> {
    let bytes = if detect_session_kind() == SessionKind::X11 {
        read_xclip(selection, Some("TARGETS"), TYPES_LIMIT)
    } else {
        let args: &[&str] = if selection == ClipboardSelection::Primary {
            &["--primary", "--list-types"]
        } else {
            &["--list-types"]
        };
        run_read_command("wl-paste", args, TYPES_LIMIT)
    }?;
    Ok(String::from_utf8_lossy(&bytes)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect())
}

/// Reads bytes from the requested system selection (`clipboard` or `primary`).
///
/// An unspecified X11 target requests UTF-8 text, with a legacy ISO-8859-1 fallback.
///
/// # Errors
///
/// Returns an error when the session's clipboard adapter is unavailable, fails or exceeds the cap.
pub fn read_system_clipboard_bytes(
    selection: ClipboardSelection,
    mime: Option<&str>,
    max_bytes: usize,
) -> Result<Vec<u8>, ClipboardError> {
    if detect_session_kind() == SessionKind::X11 {
        read_xclip(selection, mime, max_bytes)
    } else {
        read_wl_paste(selection, mime, max_bytes)
    }
}

/// Synthesizes Ctrl+C or Ctrl+V for the active desktop session.
///
/// Under X11 this uses native XTEST keystroke injection (with `xdotool` fallback when installed).
/// Under Wayland or `Auto` it invokes `wtype`.
///
/// # Errors
///
/// Returns an error when the session's keystroke backend is unavailable or fails.
pub fn send_clipboard_shortcut(
    action: ClipboardShortcutAction,
    release_alt: bool,
) -> Result<(), ClipboardError> {
    let key = match action {
        ClipboardShortcutAction::Copy => "c",
        ClipboardShortcutAction::Paste => "v",
    };
    let mut native_error = None;
    let (program, args) = if detect_session_kind() == SessionKind::X11 {
        match x11_input::send_shortcut(action, release_alt) {
            Ok(()) => return Ok(()),
            Err(error) => native_error = Some(error),
        }
        let combo = format!("ctrl+{key}");
        let mut args = vec!["key".to_owned()];
        if release_alt {
            args.push("--clearmodifiers".to_owned());
        }
        args.push(combo);
        ("xdotool", args)
    } else {
        let mut args = Vec::new();
        if release_alt {
            args.extend(["-m", "alt"]);
        }
        args.extend(["-M", "ctrl", "-k", key, "-m", "ctrl"]);
        ("wtype", args.into_iter().map(str::to_owned).collect())
    };
    if command_available(program).is_none() {
        return Err(native_error.unwrap_or_else(|| {
            ClipboardError::Failed(format!("clipboard shortcut requires {program}"))
        }));
    }
    let output = CommandRequest::new(program)
        .args(args)
        .timeout(SHORTCUT_TIMEOUT)
        .max_output_bytes(SHORTCUT_OUTPUT_LIMIT)
        .own_process_group()
        .die_with_parent()
        .run()
        .map_err(|error| ClipboardError::Failed(format!("{program}: {error}")))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(ClipboardError::Failed(format_exit_failure(
            program, &output,
        )))
    }
}

fn read_wl_paste(
    selection: ClipboardSelection,
    mime: Option<&str>,
    max_bytes: usize,
) -> Result<Vec<u8>, ClipboardError> {
    let mut args = Vec::with_capacity(4);
    if selection == ClipboardSelection::Primary {
        args.push("--primary");
    }
    args.push("--no-newline");
    if let Some(mime) = mime {
        args.push("--type");
        args.push(mime);
    }
    run_read_command("wl-paste", &args, max_bytes)
}

fn read_xclip(
    selection: ClipboardSelection,
    mime: Option<&str>,
    max_bytes: usize,
) -> Result<Vec<u8>, ClipboardError> {
    let sel = match selection {
        ClipboardSelection::Clipboard => "clipboard",
        ClipboardSelection::Primary => "primary",
    };
    if let Some(mime) = mime {
        return run_read_command("xclip", &["-selection", sel, "-o", "-t", mime], max_bytes);
    }
    match run_read_command(
        "xclip",
        &["-selection", sel, "-o", "-t", "UTF8_STRING"],
        max_bytes,
    ) {
        Ok(bytes) => Ok(bytes),
        Err(ClipboardError::Failed(_)) => {
            let text: String = run_read_command(
                "xclip",
                &["-selection", sel, "-o", "-t", "STRING"],
                max_bytes,
            )?
            .into_iter()
            .map(char::from)
            .collect();
            if text.len() > max_bytes {
                return Err(ClipboardError::Truncated(max_bytes));
            }
            Ok(text.into_bytes())
        }
        Err(error) => Err(error),
    }
}

fn run_read_command(
    program: &str,
    args: &[&str],
    max_bytes: usize,
) -> Result<Vec<u8>, ClipboardError> {
    if command_available(program).is_none() {
        return Err(ClipboardError::Unavailable);
    }
    let output = CommandRequest::new(program)
        .args(args.iter().copied())
        .timeout(CLIPBOARD_TIMEOUT)
        .max_output_bytes(max_bytes)
        .own_process_group()
        .die_with_parent()
        .run()
        .map_err(|error| ClipboardError::Failed(format!("{program}: {error}")))?;
    if output.stdout_truncated {
        return Err(ClipboardError::Truncated(max_bytes));
    }
    if !output.status.success() {
        return Err(ClipboardError::Failed(format_exit_failure(
            program, &output,
        )));
    }
    Ok(output.stdout)
}

fn format_exit_failure(program: &str, output: &CommandOutput) -> String {
    format!(
        "{program} exited {:?}: {}",
        output.status.code(),
        output.stderr_lossy().trim()
    )
}

/// Writes a standards-compliant `text/uri-list` for local paths to the system clipboard.
///
/// # Errors
///
/// Returns [`ClipboardError::InvalidPath`] if any path cannot be converted to a `file://` URI,
/// [`ClipboardError::Unavailable`] if no clipboard tool is available, or [`ClipboardError::Failed`]
/// if writing to the clipboard fails.
pub fn copy_file_uris_to_system_clipboard(
    paths: &[impl AsRef<Path>],
) -> Result<(), ClipboardError> {
    copy_bytes_to_system_clipboard(file_uri_list(paths)?.as_bytes(), "text/uri-list")
}

/// Builds a standards-compliant, newline-terminated `text/uri-list` for local paths.
///
/// This is useful to clipboard watchers that must register the exact payload for echo-loop
/// suppression before writing it through [`copy_bytes_to_system_clipboard`].
///
/// # Errors
///
/// Returns [`ClipboardError::InvalidPath`] if any path cannot be represented as a `file://` URI.
pub fn file_uri_list(paths: &[impl AsRef<Path>]) -> Result<String, ClipboardError> {
    let uris = paths
        .iter()
        .map(|path| {
            Url::from_file_path(path.as_ref())
                .map(|url| url.into())
                .map_err(|_| ClipboardError::InvalidPath(path.as_ref().display().to_string()))
        })
        .collect::<Result<Vec<String>, _>>()?
        .join("\n");
    Ok(format!("{uris}\n"))
}

/// Failure while reading, writing, or driving local clipboard adapters.
#[derive(Debug, Error)]
pub enum ClipboardError {
    /// Neither `wl-copy`/`wl-paste` nor `xclip` (or XTEST/`wtype`) was available.
    #[error("no supported clipboard command found (install wl-clipboard or xclip)")]
    Unavailable,
    /// A local path could not be represented as a file URI.
    #[error("path cannot be represented as a file URI: {0}")]
    InvalidPath(String),
    /// The read clipboard payload exceeded the configured byte ceiling.
    #[error("clipboard payload exceeds {0} bytes")]
    Truncated(usize),
    /// Installed adapters or XTEST could not complete the clipboard operation.
    #[error("clipboard operation failed: {0}")]
    Failed(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn file_uri_list_encodes_spaces_without_invoking_an_adapter() {
        let path = PathBuf::from("/tmp/little clip.txt");
        assert_eq!(
            file_uri_list(&[path]).unwrap(),
            "file:///tmp/little%20clip.txt\n"
        );
    }

    #[test]
    fn x11_keyboard_mapping_lookup_prefers_base_column_and_checks_keymap_bits() {
        let mut keysyms = vec![0u32; 12];
        // keycode 8: col 0 = 'c', col 1 = 'C'
        keysyms[0] = 0x0063;
        keysyms[1] = 0x0043;
        // keycode 9: col 1 = 'v'
        keysyms[3] = 0x0076;
        // keycode 10: col 0 = Control_L
        keysyms[4] = 0xffe3;
        // keycode 11: col 0 = 'v' (should win over col 1 on keycode 9)
        keysyms[6] = 0x0076;

        assert_eq!(x11_input::find_keycode(8, 2, &keysyms, 0x0063), Some(8));
        assert_eq!(x11_input::find_keycode(8, 2, &keysyms, 0x0076), Some(11));
        assert_eq!(x11_input::find_keycode(8, 2, &keysyms, 0xffe3), Some(10));
        assert_eq!(x11_input::find_keycode(8, 2, &keysyms, 0xdead), None);

        let mut keymap = [0u8; 32];
        keymap[4] = 1 << 5; // keycode 37
        assert!(x11_input::is_key_pressed(&keymap, 37));
        assert!(!x11_input::is_key_pressed(&keymap, 38));
    }

    #[test]
    fn xclip_uses_utf8_targets_and_stays_on_its_display() {
        use std::os::unix::fs::PermissionsExt;

        const CHILD_ENV: &str = "LITTLE_CLIP_TEST_XCLIP";
        if let Some(directory) = std::env::var_os(CHILD_ENV) {
            if detect_session_kind() == SessionKind::Wayland {
                assert!(matches!(
                    copy_text_to_system_clipboard("Wayland text"),
                    Err(ClipboardError::Unavailable)
                ));
                assert!(matches!(
                    list_system_clipboard_types(ClipboardSelection::Clipboard),
                    Err(ClipboardError::Unavailable)
                ));
                return;
            }
            assert_eq!(detect_session_kind(), SessionKind::X11);
            copy_bytes_to_system_clipboard(b"hello x11", "text/plain;charset=utf-8").unwrap();
            assert_eq!(
                std::fs::read_to_string(Path::new(&directory).join("mime")).unwrap(),
                "UTF8_STRING"
            );
            let types = list_system_clipboard_types(ClipboardSelection::Clipboard).unwrap();
            assert_eq!(types, vec!["TARGETS", "UTF8_STRING", "image/png"]);
            let got = read_system_clipboard_bytes(ClipboardSelection::Clipboard, None, 64).unwrap();
            assert_eq!(got, b"hello x11");
            let truncated =
                read_system_clipboard_bytes(ClipboardSelection::Clipboard, Some("UTF8_STRING"), 4);
            assert!(matches!(truncated, Err(ClipboardError::Truncated(4))));
            return;
        }

        let dir = tempfile::tempdir().unwrap();
        let xclip = dir.path().join("xclip");
        std::fs::write(
            &xclip,
            r#"#!/bin/sh
out=0
target=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    -o) out=1; shift ;;
    -t) target="$2"; shift 2 ;;
    -selection) shift 2 ;;
    *) shift ;;
  esac
done
if [ "$out" -eq 0 ]; then
  printf '%s' "$target" > "$LITTLE_CLIP_TEST_XCLIP/mime"
  /bin/cat > "$LITTLE_CLIP_TEST_XCLIP/data"
elif [ "$target" = "TARGETS" ]; then
  printf 'TARGETS\nUTF8_STRING\nimage/png\n'
else
  /bin/cat "$LITTLE_CLIP_TEST_XCLIP/data"
fi
"#,
        )
        .unwrap();
        std::fs::set_permissions(&xclip, std::fs::Permissions::from_mode(0o700)).unwrap();

        for wayland in ["", "wayland-test"] {
            let status = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "clipboard::tests::xclip_uses_utf8_targets_and_stays_on_its_display",
                    "--test-threads=1",
                ])
                .env(CHILD_ENV, dir.path())
                .env("PATH", dir.path())
                .env("DISPLAY", ":99")
                .env("WAYLAND_DISPLAY", wayland)
                .env_remove("WAYLAND_SOCKET")
                .output()
                .unwrap();
            assert!(
                status.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&status.stdout),
                String::from_utf8_lossy(&status.stderr)
            );
        }
    }

    #[test]
    fn wayland_endpoints_take_precedence_over_x11() {
        const CHILD_ENV: &str = "LITTLE_CLIP_TEST_SESSION";
        if let Some(mode) = std::env::var_os(CHILD_ENV) {
            if mode != "auto" {
                assert_eq!(detect_session_kind(), SessionKind::Wayland);
            } else {
                assert_eq!(detect_session_kind(), SessionKind::Auto);
            }
            return;
        }

        for (mode, wayland, display) in [
            ("wayland", Some("wayland-1"), Some(":0")),
            ("socket", None, Some(":0")),
            ("auto", None, None),
        ] {
            let mut cmd = std::process::Command::new(std::env::current_exe().unwrap());
            cmd.args([
                "--exact",
                "clipboard::tests::wayland_endpoints_take_precedence_over_x11",
                "--test-threads=1",
            ])
            .env(CHILD_ENV, mode)
            .env_remove("WAYLAND_SOCKET");
            if mode == "socket" {
                cmd.env("WAYLAND_SOCKET", "3");
            }
            match wayland {
                Some(v) => cmd.env("WAYLAND_DISPLAY", v),
                None => cmd.env_remove("WAYLAND_DISPLAY"),
            };
            match display {
                Some(v) => cmd.env("DISPLAY", v),
                None => cmd.env_remove("DISPLAY"),
            };
            let output = cmd.output().unwrap();
            assert!(output.status.success());
        }
    }
}
