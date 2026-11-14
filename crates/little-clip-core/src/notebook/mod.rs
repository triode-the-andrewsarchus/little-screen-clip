//! Read-only handwritten notebook formats and bounded page preparation.
pub mod supernote;
mod worker;

pub use worker::{Info, PageSize, inspect, render_page, run_worker_cli};

/// Cheap scheduling hint only; the worker must qualify the source header.
pub fn is_candidate_path(path: &std::path::Path) -> bool {
    path.extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("note"))
}
