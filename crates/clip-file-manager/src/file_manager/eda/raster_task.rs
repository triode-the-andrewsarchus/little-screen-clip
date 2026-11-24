use little_clip_core::jobs::{JobError, task::Task};
use std::{sync::atomic::Ordering, task::Poll, time::Duration};

pub(in crate::file_manager) type RasterTask<K, T> = Option<(K, Task<Result<T, String>>)>;

pub(in crate::file_manager) const REPAINT_INTERVAL: Duration = Duration::from_millis(20);

/// Polls one cancellable raster request. Cancelled or obsolete jobs cannot publish.
pub(in crate::file_manager) fn poll_current<K: PartialEq, T: Send + 'static>(
    ctx: &egui::Context,
    pending: &mut RasterTask<K, T>,
    wanted: &K,
) -> Poll<Option<Result<T, String>>> {
    let Some((key, task)) = pending else {
        return Poll::Ready(None);
    };
    if &*key != wanted {
        task.token.store(true, Ordering::Relaxed);
    }
    let Some(result) = task.poll() else {
        ctx.request_repaint_after(REPAINT_INTERVAL);
        return Poll::Pending;
    };
    let current = &*key == wanted && !task.token.load(Ordering::Relaxed);
    *pending = None;
    if !current {
        return Poll::Ready(None);
    }
    if matches!(result, Err(JobError::QueueFull)) {
        ctx.request_repaint_after(Duration::from_millis(50));
        return Poll::Pending;
    }
    Poll::Ready(Some(
        result.map_err(|error| error.to_string()).and_then(|r| r),
    ))
}
