//! Read-only handwritten notebook formats and bounded page preparation.
pub mod supernote;
mod worker;
pub mod xournal;

pub use worker::{Info, PageSize, inspect, render_page, run_worker_cli};

/// Cheap scheduling hint only; the worker must qualify the source header.
pub fn is_candidate_path(path: &std::path::Path) -> bool {
    path.extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("note"))
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
    let mut header = Vec::with_capacity(24);
    Read::by_ref(&mut input)
        .take(24)
        .read_to_end(&mut header)
        .map_err(|error| error.to_string())?;
    source.check_file(&input)?;
    Ok(supernote::has_signature(&header))
}
