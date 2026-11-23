use super::{Job, read_job};
use crate::file_manager::signal_view::take_query_result;
use egui::Context;
use std::{sync::atomic::AtomicBool, time::Duration};

pub(super) struct Read<K, T> {
    current: Option<(K, Result<T, String>)>,
    pending: Option<(K, Job<T>)>,
    next: f64,
}

impl<K, T> Default for Read<K, T> {
    fn default() -> Self {
        Self {
            current: None,
            pending: None,
            next: 0.0,
        }
    }
}

impl<K: Copy + PartialEq, T: Send + 'static> Read<K, T> {
    pub(super) fn value(&self, key: &K) -> Option<&T> {
        self.result(key)?.ok()
    }

    pub(super) fn result(&self, key: &K) -> Option<Result<&T, &str>> {
        let (old, result) = self.current.as_ref()?;
        (old == key).then(|| result.as_ref().map_err(String::as_str))
    }

    pub(super) fn update(
        &mut self,
        ctx: &Context,
        key: Option<K>,
        timeout: Duration,
        interval: f64,
        work: impl FnOnce(&AtomicBool) -> T + Send + 'static,
    ) {
        if self
            .pending
            .as_ref()
            .is_some_and(|(old, _)| Some(*old) != key)
        {
            self.pending = None;
        }
        if let Some(result) = take_query_result(&mut self.pending) {
            self.current = Some(result);
        }
        let Some(key) = key else { return };
        if self.current.as_ref().is_some_and(|(old, _)| *old == key) {
            return;
        }
        let now = ctx.input(|input| input.time);
        if self.pending.is_none() && now >= self.next {
            self.next = now + interval;
            self.pending = Some((key, read_job(ctx, timeout, move |cancel| Ok(work(cancel)))));
        }
        if self.pending.is_some() {
            ctx.request_repaint_after(Duration::from_millis(24));
        } else {
            ctx.request_repaint_after(Duration::from_secs_f64((self.next - now).max(0.001)));
        }
    }
}
