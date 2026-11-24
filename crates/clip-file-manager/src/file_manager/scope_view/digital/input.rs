use crate::file_manager::logic_view::protocols::TraceInput;
use little_clip_core::engineering::{
    analysis::Threshold,
    logic::{Lane, Trace},
    waveforms::{Recording, sample_span},
};
use std::{
    ops::Range,
    sync::{Arc, atomic::AtomicBool},
};

const MAX_WINDOW_SAMPLES: u64 = 1 << 20;
const CONTEXT_SAMPLES: u64 = 4096;

#[derive(Clone)]
pub(super) struct Configuration {
    pub lanes: Vec<Lane>,
    pub rules: Vec<Threshold>,
    pub revision: u64,
}

impl Configuration {
    pub fn new(recording: &Recording) -> Self {
        let lanes = recording
            .channels()
            .iter()
            .map(|channel| Lane {
                name: format!("{} digital", channel.name),
                start_seconds: channel.start_seconds,
                end_seconds: channel.time_at(channel.sample_count),
                sample_rate: Some(channel.interval_seconds.recip()),
            })
            .collect::<Vec<_>>();
        Self {
            rules: vec![Threshold::default(); lanes.len()],
            lanes,
            revision: 0,
        }
    }
}

pub(in crate::file_manager) struct ThresholdInput {
    recording: Arc<Recording>,
    config: Arc<Configuration>,
}

impl ThresholdInput {
    pub(super) fn new(recording: Arc<Recording>, config: Arc<Configuration>) -> Self {
        Self { recording, config }
    }

    pub(in crate::file_manager) fn lanes(&self) -> &[Lane] {
        &self.config.lanes
    }

    pub(in crate::file_manager) fn revision(&self) -> u64 {
        self.config.revision
    }

    pub(in crate::file_manager) fn channel_name(&self, index: usize) -> &str {
        &self.recording.channels()[index].name
    }
}

impl TraceInput for ThresholdInput {
    fn validate(&self, cancel: &AtomicBool) -> Result<(), String> {
        self.recording.checked(cancel, |_| Ok(()))
    }

    fn time_bounds(&self, index: usize) -> Option<Range<f64>> {
        let lane = self.config.lanes.get(index)?;
        Some(lane.start_seconds..lane.end_seconds)
    }

    fn read_edges(
        &self,
        index: usize,
        time: Range<f64>,
        limit: usize,
        cancel: &AtomicBool,
    ) -> Result<Trace, String> {
        let bounds = self.time_bounds(index).ok_or("Invalid threshold channel")?;
        let time = time.start.max(bounds.start)..time.end.min(bounds.end);
        if time.is_empty() {
            return Ok(Trace {
                initial: None,
                edges: Vec::new(),
                inspected_until: time.start,
            });
        }
        self.recording.checked(cancel, |capture| {
            let channel = &capture.channels()[index];
            let rows = sample_span(channel, time.clone());
            // Pre-roll establishes a stable state on the visible left edge in
            // hysteresis mode. It also supplies a sample for a sub-sample view.
            let start = rows.start.saturating_sub(CONTEXT_SAMPLES);
            let stop = rows
                .end
                .max(rows.start.saturating_add(1))
                .min(rows.start.saturating_add(MAX_WINDOW_SAMPLES))
                .min(channel.sample_count);
            let values = capture.read_samples(index, start..stop, cancel)?;
            self.config.rules[index].trace(
                &values,
                channel.time_at(start),
                channel.interval_seconds,
                &time,
                limit,
                cancel,
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{file_manager::scope_view::Viewer, file_source::ReadCheck};
    use std::io::Write;

    #[test]
    fn threshold_reads_inherit_the_scope_source_guard() {
        let mut source = tempfile::Builder::new().suffix(".csv").tempfile().unwrap();
        source
            .write_all(b"X,CH1,Start,Increment,\nSequence,Volt,0,0.001,\n0,0,\n1,3,\n2,0,\n3,3,\n")
            .unwrap();
        let cancel = AtomicBool::new(false);
        let mut view = Viewer::load(source.path(), &cancel).unwrap();
        view.set_check(ReadCheck::new(|_| {
            Err("Test source guard rejected the read".into())
        }));
        let error = view
            .threshold_inputs()
            .read_edges(0, 0.0..0.004, 16, &cancel)
            .unwrap_err();
        assert!(error.contains("Test source guard"));
    }
}
