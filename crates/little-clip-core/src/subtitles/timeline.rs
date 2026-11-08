use super::Cue;
use std::{
    collections::BTreeSet,
    sync::atomic::{AtomicBool, Ordering},
};
const BINS: usize = 256;

/// Aggregated cue activity in one full-duration timeline bin.
#[derive(Clone, Debug, Default)]
pub struct Bin {
    /// Greatest number of genuinely simultaneous nonzero-duration cues.
    pub peak: usize,
    /// Source index of one cue intersecting this bin, for navigation.
    pub cue: Option<usize>,
    /// At most 160 characters from the representative cue, for passive hover previews.
    pub excerpt: String,
}
/// A bounded time projection that distinguishes gaps from real overlaps.
#[derive(Clone, Debug)]
pub struct Timeline {
    /// End of the last cue, measured from the media origin.
    pub duration_ms: u64,
    /// Exact greatest number of simultaneous nonzero-duration cues.
    pub peak_concurrency: usize,
    /// At most 256 bins; empty documents have no bins.
    pub bins: Vec<Bin>,
}
impl Timeline {
    pub(super) fn build(cues: &[Cue], token: &AtomicBool) -> Result<Self, String> {
        let duration_ms = cues.iter().map(|cue| cue.end_ms).max().unwrap_or(0);
        let mut timeline = Self {
            duration_ms,
            peak_concurrency: 0,
            bins: vec![Bin::default(); if cues.is_empty() { 0 } else { BINS }],
        };
        let mut events = Vec::with_capacity(cues.len() * 2);
        for (index, cue) in cues.iter().enumerate() {
            if token.load(Ordering::Relaxed) {
                return Err("Subtitle loading cancelled".into());
            }
            if cue.start_ms < cue.end_ms {
                events.push((cue.start_ms, true, index));
                events.push((cue.end_ms, false, index));
            } else {
                let bin = timeline.index(cue.start_ms);
                timeline.bins[bin].cue.get_or_insert(index);
            }
        }
        events.sort_unstable(); // Ends precede starts at the same timestamp.
        let mut active = BTreeSet::new();
        let mut previous_time = 0;
        for (time, start, index) in events {
            if token.load(Ordering::Relaxed) {
                return Err("Subtitle loading cancelled".into());
            }
            if time > previous_time {
                let first = timeline.index(previous_time);
                let last = ((u128::from(time) * timeline.bins.len() as u128 - 1)
                    / u128::from(duration_ms.max(1))) as usize;
                for cell in &mut timeline.bins[first..=last] {
                    cell.peak = cell.peak.max(active.len());
                    if let Some(cue) = active.first() {
                        cell.cue.get_or_insert(*cue);
                    }
                }
            }
            if start {
                active.insert(index);
            } else {
                active.remove(&index);
            }
            timeline.peak_concurrency = timeline.peak_concurrency.max(active.len());
            previous_time = time;
        }
        for bin in &mut timeline.bins {
            if let Some(cue) = bin.cue {
                bin.excerpt = cues[cue].text.chars().take(160).collect();
            }
        }
        Ok(timeline)
    }
    /// Returns the clamped bin containing a media timestamp.
    pub fn index(&self, time_ms: u64) -> usize {
        if self.bins.is_empty() {
            return 0;
        }
        (u128::from(time_ms) * self.bins.len() as u128 / u128::from(self.duration_ms.max(1)))
            .min((self.bins.len() - 1) as u128) as usize
    }
}
#[cfg(test)]
mod tests {
    use super::super::Document;
    use super::*;
    #[test]
    fn millisecond_cue_covers_the_whole_projection_and_timestamps_clamp() {
        let doc = Document::parse_srt(
            "1\n00:00:00,000 --> 00:00:00,001\nx",
            &AtomicBool::new(false),
        )
        .unwrap();
        assert!(
            doc.timeline
                .bins
                .iter()
                .all(|bin| bin.peak == 1 && bin.cue == Some(0))
        );
        assert_eq!(doc.timeline.index(u64::MAX), 255);
        let empty = Document::parse_srt("", &AtomicBool::new(false)).unwrap();
        assert!(empty.timeline.bins.is_empty());
        assert_eq!(empty.timeline.index(u64::MAX), 0);
        assert!(Document::parse_srt("", &AtomicBool::new(true)).is_err());
    }
    #[test]
    fn abutting_short_cues_in_one_bin_do_not_fabricate_overlaps() {
        let source = "1\n00:00:00,000 --> 00:00:00,001\na\n\n2\n00:00:00,001 --> 00:00:00,002\nb\n\n3\n00:01:00,000 --> 00:01:01,000\nlast";
        let doc = Document::parse_srt(source, &AtomicBool::new(false)).unwrap();
        assert_eq!(doc.timeline.peak_concurrency, 1);
        assert_eq!(doc.timeline.bins[0].peak, 1);
        assert_eq!(doc.timeline.bins[128].peak, 0);
        assert_eq!(doc.timeline.bins.len(), 256);
    }
    #[test]
    fn instant_and_unordered_cues_remain_navigable() {
        let source = "1\n00:00:10,000 --> 00:00:10,000\nmarker\n\n2\n00:00:01,000 --> 00:00:04,000\na\n\n3\n00:00:02,000 --> 00:00:03,000\nb";
        let doc = Document::parse_srt(source, &AtomicBool::new(false)).unwrap();
        assert_eq!(doc.timeline.duration_ms, 10_000);
        assert_eq!(doc.timeline.peak_concurrency, 2);
        assert_eq!(doc.timeline.bins[255].cue, Some(0));
    }
}
