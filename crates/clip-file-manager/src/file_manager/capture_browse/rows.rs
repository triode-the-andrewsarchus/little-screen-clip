use super::{Job, keyed_poll, start};
use egui::Context;
use little_clip_core::BackgroundJobs;
use std::{collections::VecDeque, ops::Range, sync::atomic::AtomicBool, time::Duration};

const WINDOW_ROWS: u64 = 256;

pub(in crate::file_manager) fn needed_window(
    visible: Range<u64>,
    total: u64,
    cached: impl Fn(u64) -> bool,
) -> Option<Range<u64>> {
    let missing = (visible.start..visible.end.min(total)).find(|&row| !cached(row))?;
    let start = missing / WINDOW_ROWS * WINDOW_ROWS;
    Some(start..start.saturating_add(WINDOW_ROWS).min(total))
}

/// Accepted windows retain their source ordinals across page changes.
pub(in crate::file_manager) struct RowWindows<T> {
    ready: VecDeque<Window<T>>,
    load: Option<(Range<u64>, Job<Vec<T>>)>,
    failed: Option<Range<u64>>,
    capacity: usize,
}

struct Window<T> {
    range: Range<u64>,
    rows: Vec<T>,
}

impl<T: Send + 'static> RowWindows<T> {
    pub fn new(capacity: usize) -> Self {
        Self {
            ready: VecDeque::new(),
            load: None,
            failed: None,
            capacity,
        }
    }

    pub fn get(&self, row: u64) -> Option<&T> {
        let window = self
            .ready
            .iter()
            .find(|window| window.range.contains(&row))?;
        window.rows.get((row - window.range.start) as usize)
    }

    pub fn pending(&self) -> bool {
        self.load.is_some()
    }

    pub fn clear_request(&mut self) {
        self.load = None;
        self.failed = None;
    }

    pub fn request(
        &mut self,
        jobs: &BackgroundJobs,
        ctx: &Context,
        timeout: Duration,
        range: Range<u64>,
        read: impl FnOnce(Range<u64>, &AtomicBool) -> Result<Vec<T>, String> + Send + 'static,
    ) {
        if self
            .load
            .as_ref()
            .is_some_and(|(active, _)| *active == range)
            || self.failed.as_ref() == Some(&range)
        {
            return;
        }
        let requested = range.clone();
        self.load = Some((
            range,
            start(jobs, ctx, timeout, move |cancel| read(requested, cancel)),
        ));
    }

    pub fn poll_error(&mut self) -> Option<String> {
        let (range, result) = keyed_poll(&mut self.load)?;
        match result {
            Ok(rows) => {
                self.store(range, rows);
                None
            }
            Err(error) => {
                self.failed = Some(range);
                Some(error)
            }
        }
    }

    fn store(&mut self, range: Range<u64>, rows: Vec<T>) {
        self.ready.push_front(Window { range, rows });
        self.ready.truncate(self.capacity);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visible_windows_keep_source_ordinals_and_preserve_cache_on_page_change() {
        let mut windows = RowWindows::new(2);
        windows.store(0..256, (0..256).collect());
        windows.store(256..512, (256..512).collect());
        assert_eq!(
            needed_window(495..530, 524, |row| windows.get(row).is_some()),
            Some(512..524)
        );
        windows.store(512..524, (512..524).collect());
        windows.clear_request();
        assert_eq!(windows.get(15), None);
        assert_eq!(windows.get(500), Some(&500));
        assert_eq!(windows.get(523), Some(&523));
        assert_eq!(
            needed_window(500..700, 524, |row| windows.get(row).is_some()),
            None
        );
    }
}
