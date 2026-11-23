use super::reads::{self, DETAIL_SAMPLES, RawChannel};
use crate::file_manager::preview_job::Task;
use egui::Context;
use little_clip_core::{
    BackgroundJobs,
    engineering::{scope::Channel, waveforms::Recording},
};
use std::{
    ops::Range,
    sync::{Arc, LazyLock},
    time::Duration,
};

static JOBS: LazyLock<BackgroundJobs> = LazyLock::new(|| BackgroundJobs::new(2, 2));
const PREFETCH: u64 = DETAIL_SAMPLES / 2;

#[derive(Clone, PartialEq)]
struct Target {
    channel: usize,
    time: f64,
    radius: f64,
    rows: Range<u64>,
}

impl Target {
    fn new(channel: usize, meta: &Channel, time: f64, radius: f64) -> Self {
        let radius = radius.min(meta.interval_seconds * DETAIL_SAMPLES as f64);
        let rows = reads::padded_span(meta, &(time - radius..time + radius), 2);
        Self {
            channel,
            time,
            radius,
            rows,
        }
    }

    fn resolve(&self, raw: &RawChannel) -> Option<f64> {
        raw.covers(&self.rows)
            .then(|| raw.snap(self.time, self.radius).unwrap_or(self.time))
    }
}

#[derive(Default)]
pub(super) struct CursorSnap {
    target: Option<Target>,
    window: Option<(usize, RawChannel)>,
    job: Option<(Target, Task<Result<RawChannel, String>>)>,
}

impl CursorSnap {
    /// Retires work for a prior gesture while retaining its bounded sample cache.
    pub fn clear(&mut self) {
        self.target = None;
        self.job = None;
    }

    pub fn pending(&self) -> bool {
        self.job.is_some()
    }

    pub fn place(
        &mut self,
        channel: usize,
        meta: &Channel,
        time: f64,
        radius: f64,
        free: bool,
        detail: Option<&RawChannel>,
    ) -> f64 {
        if free {
            self.clear();
            return time;
        }
        let target = Target::new(channel, meta, time, radius);
        if target.rows.is_empty() {
            self.clear();
            return time;
        }
        let cached = self
            .window
            .as_ref()
            .filter(|(channel, _)| *channel == target.channel)
            .map(|(_, raw)| raw);
        let snapped = detail
            .and_then(|raw| target.resolve(raw))
            .or_else(|| cached.and_then(|raw| target.resolve(raw)));
        if let Some(snapped) = snapped {
            self.target = None;
            return snapped;
        }
        self.target = Some(target);
        time
    }

    pub fn request(&mut self, recording: &Arc<Recording>, ctx: &Context) {
        let Some(target) = self.target.as_ref().filter(|_| self.job.is_none()).cloned() else {
            return;
        };
        let recording = recording.clone();
        let ctx = ctx.clone();
        let channel = target.channel;
        let last = recording.channels()[channel].sample_count;
        let rows = target.rows.start.saturating_sub(PREFETCH)
            ..target.rows.end.saturating_add(PREFETCH).min(last);
        self.job = Some((
            target,
            Task::start(&JOBS, Duration::from_secs(4), move |cancel| {
                let result = reads::snap_window(&recording, channel, rows, cancel);
                ctx.request_repaint();
                result
            }),
        ));
    }

    pub fn poll(&mut self, active: usize) -> Option<f64> {
        let result = self.job.as_ref()?.1.poll()?;
        let (requested, _) = self.job.take()?;
        if let Ok(Ok(raw)) = result {
            return self.arrive(active, requested.channel, raw);
        }
        if self.target.as_ref() == Some(&requested) {
            self.target = None;
        }
        None
    }

    fn arrive(&mut self, active: usize, channel: usize, raw: RawChannel) -> Option<f64> {
        self.window = Some((channel, raw));
        let target = self.target.as_ref()?;
        if target.channel != active || channel != active {
            return None;
        }
        let resolved = target.resolve(&self.window.as_ref()?.1)?;
        self.target = None;
        Some(resolved)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn channel() -> Channel {
        Channel {
            name: "C1".into(),
            unit: "V".into(),
            sample_count: 10,
            start_seconds: 0.0,
            interval_seconds: 1.0,
        }
    }

    fn window() -> RawChannel {
        RawChannel {
            first: 0,
            start: 0.0,
            step: 1.0,
            samples: vec![1.0, 1.0, -1.0, -1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0],
        }
    }

    #[test]
    fn arriving_window_snaps_the_latest_pointer_and_is_reused_during_drag() {
        let mut snap = CursorSnap::default();
        let channel = channel();
        assert_eq!(snap.place(0, &channel, 1.7, 0.8, false, None), 1.7);
        assert_eq!(snap.place(0, &channel, 3.7, 0.8, false, None), 3.7);
        assert_eq!(snap.arrive(0, 0, window()), Some(3.5));
        assert_eq!(snap.place(0, &channel, 1.7, 0.8, false, None), 1.5);
    }

    #[test]
    fn shift_discards_in_flight_snaps_and_channel_switches_ignore_stale_windows() {
        let mut snap = CursorSnap::default();
        let channel = channel();
        snap.place(0, &channel, 1.7, 0.8, false, None);
        assert_eq!(snap.place(0, &channel, 1.7, 0.8, true, None), 1.7);
        assert_eq!(snap.arrive(0, 0, window()), None);
        assert_eq!(snap.place(1, &channel, 1.7, 0.8, false, None), 1.7);
        assert_eq!(snap.arrive(1, 0, window()), None);
        assert_eq!(snap.arrive(1, 1, window()), Some(1.5));
    }
}
