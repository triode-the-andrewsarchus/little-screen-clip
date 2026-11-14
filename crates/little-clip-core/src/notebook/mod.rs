//! Read-only handwritten notebook formats and bounded page preparation.
mod container;
mod embedded_pdf;
pub mod remarkable;
pub mod supernote;
mod vector;
mod worker;
pub mod xournal;

pub use worker::{inspect, render_page, run_worker_cli};

use serde::{Deserialize, Serialize};

/// Native page dimensions, in authored order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PageSize {
    pub width: u32,
    pub height: u32,
}

/// Read-only notebook summary without decoded ink or source-specific offsets.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Info {
    pub format: String,
    pub pages: Vec<PageSize>,
    /// Source artwork that is unavailable while the actual ink remains usable.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

const HEADER_BYTES: usize = 4096;

/// Cheap scheduling hint only; the worker must qualify the source header.
pub fn is_candidate_path(path: &std::path::Path) -> bool {
    path.extension().is_some_and(|ext| {
        ["note", "xopp", "xoj", "rm", "rmdoc", "rmn"]
            .iter()
            .any(|candidate| ext.eq_ignore_ascii_case(candidate))
    })
}

/// Checks a bounded ordinary-file header on a worker, independently of its suffix.
///
/// # Errors
/// Returns cancellation, content-policy, source replacement or read failures.
pub fn has_header(
    path: &std::path::Path,
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<bool, String> {
    use std::io::Read;
    if cancel.load(std::sync::atomic::Ordering::Acquire) {
        return Err("Notebook preview cancelled".into());
    }
    let (mut input, source) = crate::file_source::ReadCheck::default().open(path, None, cancel)?;
    let mut header = Vec::with_capacity(HEADER_BYTES);
    Read::by_ref(&mut input)
        .take(HEADER_BYTES as u64)
        .read_to_end(&mut header)
        .map_err(|error| error.to_string())?;
    let recognized = if supernote::has_signature(&header) || remarkable::has_signature(&header) {
        true
    } else if xournal::has_container_prefix(&header) || header.get(257..262) == Some(b"ustar") {
        worker::identify(path, cancel)?
    } else {
        false
    };
    source.check_file(&input)?;
    Ok(recognized)
}

pub(super) fn check_cancel(cancel: &std::sync::atomic::AtomicBool) -> Result<(), String> {
    if cancel.load(std::sync::atomic::Ordering::Acquire) {
        Err("Notebook preview cancelled".into())
    } else {
        Ok(())
    }
}
