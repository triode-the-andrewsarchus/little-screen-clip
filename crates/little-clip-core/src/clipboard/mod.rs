//! Local Wayland/X11 clipboard reading, writing, and shortcut synthesis.
//!
//! Adapter precedence tracks the active display session (`WAYLAND_DISPLAY` vs `DISPLAY`) while
//! preserving Wayland-first ordering when neither variable is set so isolated tests keep working.
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

/// Detects the active display session from `WAYLAND_DISPLAY` and `DISPLAY`.
#[must_use]
pub fn detect_session_kind() -> SessionKind {
    if has_nonempty_env("WAYLAND_DISPLAY") {
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

fn adapter_order<'a>(wayland: &'a str, x11: &'a str) -> [&'a str; 2] {
    match detect_session_kind() {
        SessionKind::X11 => [x11, wayland],
        SessionKind::Wayland | SessionKind::Auto => [wayland, x11],
    }
}

/// Writes UTF-8 text to the system clipboard as `text/plain;charset=utf-8`.
///
/// # Errors
///
/// Returns [`ClipboardError::Unavailable`] if neither `wl-copy` nor `xclip` is installed,
/// or [`ClipboardError::Failed`] if the invoked clipboard tool fails.
pub fn copy_text_to_system_clipboard(text: &str) -> Result<(), ClipboardError> {
    copy_bytes_to_system_clipboard(text.as_bytes(), "text/plain;charset=utf-8")
}

/// Writes bytes to the system clipboard using the supplied MIME type.
///
/// # Errors
///
/// Returns an error when neither supported clipboard program is present, a program times out, or
/// it exits unsuccessfully. This function blocks for at most five seconds per attempted adapter.
pub fn copy_bytes_to_system_clipboard(bytes: &[u8], mime: &str) -> Result<(), ClipboardError> {
    let mut failures = Vec::new();
    for program in adapter_order("wl-copy", "xclip") {
        if command_available(program).is_none() {
            continue;
        }
        let request = match program {
            "wl-copy" => CommandRequest::new(program)
                .arg("-t")
                .arg(mime)
                .stdin_bytes(bytes)
                .discard_output()
                .timeout(CLIPBOARD_TIMEOUT),
            "xclip" => CommandRequest::new(program)
                .arg("-selection")
                .arg("clipboard")
                .arg("-t")
                .arg(mime)
                .stdin_bytes(bytes)
                .discard_output()
                .timeout(CLIPBOARD_TIMEOUT),
            _ => unreachable!("clipboard write adapters are fixed above"),
        };
        match request.run() {
            Ok(output) if output.status.success() => return Ok(()),
            Ok(output) => failures.push(format_exit_failure(program, &output)),
            Err(error) => failures.push(format!("{program}: {error}")),
        }
    }
    finish_failures(failures)
}

/// Lists advertised MIME / target atoms on the requested system selection.
///
/// # Errors
///
/// Returns [`ClipboardError::Unavailable`] when no supported clipboard adapter is installed,
/// [`ClipboardError::Truncated`] when target output exceeds the 64 KiB cap, or
/// [`ClipboardError::Failed`] when the invoked tool exits unsuccessfully.
pub fn list_system_clipboard_types(
    selection: ClipboardSelection,
) -> Result<Vec<String>, ClipboardError> {
    let mut failures = Vec::new();
    for program in adapter_order("wl-paste", "xclip") {
        if command_available(program).is_none() {
            continue;
        }
        let args = match (program, selection) {
            ("wl-paste", ClipboardSelection::Clipboard) => vec!["--list-types"],
            ("wl-paste", ClipboardSelection::Primary) => vec!["--primary", "--list-types"],
            ("xclip", ClipboardSelection::Clipboard) => {
                vec!["-selection", "clipboard", "-o", "-t", "TARGETS"]
            }
            ("xclip", ClipboardSelection::Primary) => {
                vec!["-selection", "primary", "-o", "-t", "TARGETS"]
            }
            _ => unreachable!("clipboard read adapters are fixed above"),
        };
        match run_read_command(program, &args, TYPES_LIMIT) {
            Ok(bytes) => {
                let text = String::from_utf8_lossy(&bytes);
                return Ok(text
                    .lines()
                    .map(str::trim)
                    .filter(|line| !line.is_empty())
                    .map(str::to_owned)
                    .collect());
            }
            Err(ReadAttempt::Fatal(err)) => return Err(err),
            Err(ReadAttempt::Failure(msg)) => failures.push(msg),
        }
    }
    finish_failures(failures)
}

/// Reads bytes from the requested system selection (`clipboard` or `primary`).
///
/// When `mime` is `None` under `xclip`, `UTF8_STRING` is attempted first before falling back to
/// `xclip`'s default selection output so UTF-8 text is preserved without rejecting `STRING`-only
/// owners.
///
/// # Errors
///
/// Returns [`ClipboardError::Unavailable`] if no adapter exists, [`ClipboardError::Truncated`] if
/// payload exceeds `max_bytes`, or [`ClipboardError::Failed`] if reading fails.
pub fn read_system_clipboard_bytes(
    selection: ClipboardSelection,
    mime: Option<&str>,
    max_bytes: usize,
) -> Result<Vec<u8>, ClipboardError> {
    let mut failures = Vec::new();
    for program in adapter_order("wl-paste", "xclip") {
        if command_available(program).is_none() {
            continue;
        }
        let attempt = match program {
            "wl-paste" => read_wl_paste(selection, mime, max_bytes),
            "xclip" => read_xclip(selection, mime, max_bytes),
            _ => unreachable!("clipboard read adapters are fixed above"),
        };
        match attempt {
            Ok(bytes) => return Ok(bytes),
            Err(ReadAttempt::Fatal(err)) => return Err(err),
            Err(ReadAttempt::Failure(msg)) => failures.push(msg),
        }
    }
    finish_failures(failures)
}

/// Synthesizes Ctrl+C or Ctrl+V for the active desktop session.
///
/// Under X11 this uses native XTEST keystroke injection (with `xdotool` fallback when installed).
/// Under Wayland or `Auto` it invokes `wtype`.
///
/// # Errors
///
/// Returns [`ClipboardError::Unavailable`] or [`ClipboardError::Failed`] if no keystroke backend
/// succeeds.
pub fn send_clipboard_shortcut(
    action: ClipboardShortcutAction,
    release_alt: bool,
) -> Result<(), ClipboardError> {
    if detect_session_kind() == SessionKind::X11 {
        return send_x11_shortcut(action, release_alt);
    }
    let key = match action {
        ClipboardShortcutAction::Copy => "c",
        ClipboardShortcutAction::Paste => "v",
    };
    if command_available("wtype").is_none() {
        return Err(ClipboardError::Unavailable);
    }
    let args: &[&str] = if release_alt {
        &["-m", "alt", "-M", "ctrl", "-k", key, "-m", "ctrl"]
    } else {
        &["-M", "ctrl", "-k", key, "-m", "ctrl"]
    };
    let output = CommandRequest::new("wtype")
        .args(args.iter().copied())
        .timeout(SHORTCUT_TIMEOUT)
        .max_output_bytes(SHORTCUT_OUTPUT_LIMIT)
        .own_process_group()
        .run()
        .map_err(|e| ClipboardError::Failed(format!("wtype: {e}")))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(ClipboardError::Failed(format_exit_failure(
            "wtype", &output,
        )))
    }
}

fn send_x11_shortcut(
    action: ClipboardShortcutAction,
    release_alt: bool,
) -> Result<(), ClipboardError> {
    let x11_error = match x11_input::send_shortcut(action, release_alt) {
        Ok(()) => return Ok(()),
        Err(error) => error,
    };
    if command_available("xdotool").is_none() {
        return Err(x11_error);
    }
    let combo = match action {
        ClipboardShortcutAction::Copy => "ctrl+c",
        ClipboardShortcutAction::Paste => "ctrl+v",
    };
    let args: &[&str] = if release_alt {
        &["key", "--clearmodifiers", combo]
    } else {
        &["key", combo]
    };
    let output = CommandRequest::new("xdotool")
        .args(args.iter().copied())
        .timeout(SHORTCUT_TIMEOUT)
        .max_output_bytes(SHORTCUT_OUTPUT_LIMIT)
        .own_process_group()
        .run()
        .map_err(|e| ClipboardError::Failed(format!("{x11_error}; xdotool: {e}")))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(ClipboardError::Failed(format!(
            "{x11_error}; {}",
            format_exit_failure("xdotool", &output)
        )))
    }
}

enum ReadAttempt {
    Failure(String),
    Fatal(ClipboardError),
}

fn read_wl_paste(
    selection: ClipboardSelection,
    mime: Option<&str>,
    max_bytes: usize,
) -> Result<Vec<u8>, ReadAttempt> {
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
) -> Result<Vec<u8>, ReadAttempt> {
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
        Err(ReadAttempt::Fatal(err)) => Err(ReadAttempt::Fatal(err)),
        Err(ReadAttempt::Failure(_)) => {
            run_read_command("xclip", &["-selection", sel, "-o"], max_bytes)
        }
    }
}

fn run_read_command(
    program: &str,
    args: &[&str],
    max_bytes: usize,
) -> Result<Vec<u8>, ReadAttempt> {
    let output = CommandRequest::new(program)
        .args(args.iter().copied())
        .timeout(CLIPBOARD_TIMEOUT)
        .max_output_bytes(max_bytes)
        .own_process_group()
        .die_with_parent()
        .run()
        .map_err(|error| ReadAttempt::Failure(format!("{program}: {error}")))?;
    if output.stdout_truncated {
        return Err(ReadAttempt::Fatal(ClipboardError::Truncated(max_bytes)));
    }
    if !output.status.success() {
        return Err(ReadAttempt::Failure(format_exit_failure(program, &output)));
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

fn finish_failures<T>(failures: Vec<String>) -> Result<T, ClipboardError> {
    if failures.is_empty() {
        Err(ClipboardError::Unavailable)
    } else {
        Err(ClipboardError::Failed(failures.join("; ")))
    }
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
    #[error("clipboard write failed: {0}")]
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
    fn x11_session_prefers_xclip_and_enforces_truncation_limit() {
        use std::os::unix::fs::PermissionsExt;

        const CHILD_ENV: &str = "LITTLE_CLIP_TEST_XCLIP";
        if std::env::var_os(CHILD_ENV).is_some() {
            assert_eq!(detect_session_kind(), SessionKind::X11);
            assert_eq!(adapter_order("wl-copy", "xclip"), ["xclip", "wl-copy"]);
            copy_bytes_to_system_clipboard(b"hello x11", "text/plain;charset=utf-8").unwrap();
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

        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "clipboard::tests::x11_session_prefers_xclip_and_enforces_truncation_limit",
                "--test-threads=1",
            ])
            .env(CHILD_ENV, dir.path())
            .env("PATH", dir.path())
            .env("DISPLAY", ":99")
            .env_remove("WAYLAND_DISPLAY")
            .output()
            .unwrap();
        assert!(
            status.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&status.stdout),
            String::from_utf8_lossy(&status.stderr)
        );
    }

    #[test]
    fn wayland_and_unset_session_keep_wayland_first_order() {
        const CHILD_ENV: &str = "LITTLE_CLIP_TEST_SESSION";
        if let Some(mode) = std::env::var_os(CHILD_ENV) {
            if mode == "wayland" {
                assert_eq!(detect_session_kind(), SessionKind::Wayland);
            } else {
                assert_eq!(detect_session_kind(), SessionKind::Auto);
            }
            assert_eq!(adapter_order("wl-copy", "xclip"), ["wl-copy", "xclip"]);
            return;
        }

        for (mode, wayland, display) in [
            ("wayland", Some("wayland-1"), Some(":0")),
            ("auto", None, None),
        ] {
            let mut cmd = std::process::Command::new(std::env::current_exe().unwrap());
            cmd.args([
                "--exact",
                "clipboard::tests::wayland_and_unset_session_keep_wayland_first_order",
                "--test-threads=1",
            ])
            .env(CHILD_ENV, mode);
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
