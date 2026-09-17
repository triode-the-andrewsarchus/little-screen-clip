//! Local Wayland/X clipboard writing with a consistent MIME and failure contract.
//!
//! `wl-copy` is preferred because it is the native Wayland adapter. `xclip` is used only as a
//! fallback. Content is written through standard input rather than command-line arguments so it
//! is not exposed through process listings. Callers that watch the clipboard must perform their
//! own echo-loop suppression around these functions.

use crate::{CommandRequest, command_available};
use std::path::Path;
use thiserror::Error;
use url::Url;

const CLIPBOARD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

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
    for program in ["wl-copy", "xclip"] {
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
            _ => unreachable!("clipboard adapters are fixed above"),
        };
        match request.run() {
            Ok(output) if output.status.success() => return Ok(()),
            Ok(output) => failures.push(format!(
                "{program} exited {:?}: {}",
                output.status.code(),
                output.stderr_lossy().trim()
            )),
            Err(error) => failures.push(format!("{program}: {error}")),
        }
    }
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

/// Failure while writing through the supported local clipboard adapters.
#[derive(Debug, Error)]
pub enum ClipboardError {
    /// Neither `wl-copy` nor `xclip` was available through `PATH`.
    #[error("no supported clipboard command found (install wl-clipboard or xclip)")]
    Unavailable,
    /// A local path could not be represented as a file URI.
    #[error("path cannot be represented as a file URI: {0}")]
    InvalidPath(String),
    /// Installed adapters could not accept the requested clipboard data.
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
}
