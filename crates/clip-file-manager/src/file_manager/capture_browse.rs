//! Bounded source-row browsing shared by offline network and bus captures.

pub(super) mod rows;
pub(super) mod table;

use super::preview_job::Task;
use egui::Context;
use little_clip_core::BackgroundJobs;
use std::{sync::atomic::AtomicBool, time::Duration};

pub(super) const PAGE_ROWS: u64 = 8192;
pub(super) type Job<T> = Task<Result<T, String>>;

pub(super) fn start<T: Send + 'static>(
    jobs: &BackgroundJobs,
    ctx: &Context,
    timeout: Duration,
    work: impl FnOnce(&AtomicBool) -> Result<T, String> + Send + 'static,
) -> Job<T> {
    let ctx = ctx.clone();
    Task::start(jobs, timeout, move |cancel| {
        let result = work(cancel);
        ctx.request_repaint();
        result
    })
}

pub(super) fn poll<T: Send + 'static>(job: &mut Option<Job<T>>) -> Option<Result<T, String>> {
    let result = job.as_ref()?.poll()?;
    *job = None;
    Some(
        result
            .map_err(|error| error.to_string())
            .and_then(|result| result),
    )
}

pub(super) fn keyed_poll<K, T: Send + 'static>(
    job: &mut Option<(K, Job<T>)>,
) -> Option<(K, Result<T, String>)> {
    let result = job.as_ref()?.1.poll()?;
    let (key, _) = job.take()?;
    Some((
        key,
        result
            .map_err(|error| error.to_string())
            .and_then(|result| result),
    ))
}
