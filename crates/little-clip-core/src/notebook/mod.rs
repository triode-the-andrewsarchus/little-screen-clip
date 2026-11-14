//! Read-only handwritten notebook formats and bounded page preparation.
pub mod supernote;
mod worker;

pub use worker::{Info, PageSize, inspect, render_page, run_worker_cli};
