//! Spectrum measurements and bounded peak hold for saved I/Q data.

use little_clip_core::engineering::iq::{Capture, Spectrum, Waterfall};
use std::{ops::Range, sync::atomic::AtomicBool};

pub(super) const MAX_PEAK_WINDOWS: u64 = 64;

#[derive(Clone, PartialEq, Eq)]
pub(super) enum Request {
    Window(Range<u64>),
    PeakHold { selection: Range<u64>, fft: usize },
}

pub(super) struct Measurement {
    pub(super) frame: Spectrum,
    pub(super) coverage: Option<Coverage>,
    pub(super) strongest: Option<usize>,
}

pub(super) struct Coverage {
    pub(super) selection: Range<u64>,
    pub(super) windows: Vec<Range<u64>>,
    pub(super) observed_samples: u64,
    pub(super) has_invalid_samples: bool,
    source_rows: Vec<Option<usize>>,
}

impl Request {
    pub(super) fn read(
        self,
        capture: &Capture,
        cancel: &AtomicBool,
    ) -> Result<Measurement, String> {
        match self {
            Self::Window(range) => {
                let frame = capture.spectrum(range, cancel)?;
                let strongest = strongest_bin(&frame.bins_dbfs, 0..frame.bins_dbfs.len());
                Ok(Measurement {
                    frame,
                    coverage: None,
                    strongest,
                })
            }
            Self::PeakHold { selection, fft } => {
                let center = shared_center(capture, &selection).map_err(
                    |()| "Peak hold needs one tuning center. Select a range within one tuning.",
                )?;
                let count = (selection.end - selection.start).div_ceil(fft as u64);
                let data = capture.waterfall(
                    selection,
                    fft,
                    count.min(MAX_PEAK_WINDOWS) as usize,
                    cancel,
                )?;
                Measurement::peak_hold(data, center)
            }
        }
    }
}

impl Measurement {
    fn peak_hold(data: Waterfall, center: Option<f64>) -> Result<Self, String> {
        let count = data.rows.len();
        let mut rows = data.rows.into_iter();
        let mut frame = rows.next().ok_or("Peak hold has no FFT windows.")?;
        let mut coverage = Coverage {
            selection: data.samples,
            windows: Vec::with_capacity(count),
            observed_samples: 0,
            has_invalid_samples: false,
            source_rows: frame
                .bins_dbfs
                .iter()
                .map(|level| level.is_finite().then_some(0))
                .collect(),
        };
        coverage.add(&frame);
        for row in rows {
            let row_index = coverage.windows.len();
            coverage.add(&row);
            for ((peak, source), level) in frame
                .bins_dbfs
                .iter_mut()
                .zip(&mut coverage.source_rows)
                .zip(row.bins_dbfs)
            {
                if level.is_finite() && (source.is_none() || level > *peak) {
                    *peak = level;
                    *source = Some(row_index);
                }
            }
        }
        let strongest = strongest_bin(&frame.bins_dbfs, 0..frame.bins_dbfs.len());
        // The aggregate is calibrated coherent amplitude. Its windows may have different
        // noise bandwidths when samples are invalid, so there is no single density offset.
        frame.center_hz = center;
        frame.crosses_tuning = false;
        frame.noise_bandwidth_hz = None;
        Ok(Self {
            frame,
            coverage: Some(coverage),
            strongest,
        })
    }

    pub(super) fn measured_windows(&self) -> &[Range<u64>] {
        self.coverage
            .as_ref()
            .map_or(std::slice::from_ref(&self.frame.samples), |coverage| {
                &coverage.windows
            })
    }
}

// Prefer the first bin when levels are tied. The same rule serves the
// measurement summary and the peak shown in a decimated display column.
pub(super) fn strongest_bin(bins: &[f32], range: Range<usize>) -> Option<usize> {
    range
        .filter(|&bin| bins[bin].is_finite())
        .reduce(|best, bin| if bins[bin] > bins[best] { bin } else { best })
}

impl Coverage {
    fn add(&mut self, row: &Spectrum) {
        let samples = &row.samples;
        let covered_end = self
            .windows
            .last()
            .map_or(samples.start, |previous| previous.end);
        self.observed_samples += samples.end.saturating_sub(samples.start.max(covered_end));
        self.has_invalid_samples |= row.finite_samples < samples.end - samples.start;
        self.windows.push(samples.clone());
    }

    pub(super) fn source(&self, bin: usize) -> Option<&Range<u64>> {
        self.windows
            .get(self.source_rows.get(bin).copied().flatten()?)
    }

    pub(super) fn sampled(&self) -> bool {
        self.observed_samples < self.selection.end - self.selection.start
    }
}

pub(super) fn shared_center(capture: &Capture, samples: &Range<u64>) -> Result<Option<f64>, ()> {
    let center = capture
        .tuning_at(samples.start)
        .and_then(|tuning| tuning.center_hz);
    let tunings = &capture.metadata().tunings;
    let after_start = tunings.partition_point(|tuning| tuning.start_sample <= samples.start);
    let mut in_range = tunings[after_start..]
        .iter()
        .take_while(|tuning| tuning.start_sample < samples.end);
    if in_range.any(|tuning| tuning.center_hz != center) {
        Err(())
    } else {
        Ok(center)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_source::ReadCheck;
    use little_clip_core::engineering::iq::{Encoding, RawInterpretation, Recording};
    use std::{f32::consts::TAU, io::Write};

    #[test]
    fn peak_hold_detects_a_transient_outside_the_center_window() {
        let mut file = tempfile::tempfile().unwrap();
        let burst = 256..512;
        let second = 3328..3584;
        for sample in 0..4096 {
            let (angle, amplitude) = if second.contains(&sample) {
                (-TAU * (sample % 4) as f32 / 4.0, 0.25)
            } else {
                (
                    TAU * (sample % 8) as f32 / 8.0,
                    if burst.contains(&sample) { 0.5 } else { 0.0 },
                )
            };
            file.write_all(&(amplitude * angle.cos()).to_le_bytes())
                .unwrap();
            file.write_all(&(amplitude * angle.sin()).to_le_bytes())
                .unwrap();
        }
        let capture = Capture::open_raw(
            file,
            RawInterpretation {
                encoding: Encoding::Cf32Le,
                sample_rate_hz: Some(32_768.0),
                center_hz: Some(915_000_000.0),
            },
        )
        .unwrap();
        let cancel = AtomicBool::new(false);
        let centered = Request::Window(1920..2176).read(&capture, &cancel).unwrap();
        assert_eq!(
            centered.measured_windows(),
            std::slice::from_ref(&(1920..2176))
        );
        let peak_hold = Request::PeakHold {
            selection: 0..4096,
            fft: 256,
        }
        .read(&capture, &cancel)
        .unwrap();
        let coverage = peak_hold.coverage.as_ref().unwrap();
        assert!(!coverage.sampled());
        assert_eq!(coverage.observed_samples, 4096);
        assert_eq!(peak_hold.measured_windows().len(), 16);
        assert!(peak_hold.measured_windows().contains(&burst));
        let bin = 160;
        assert!(centered.frame.bins_dbfs[bin] < -120.0);
        assert!((peak_hold.frame.bins_dbfs[bin] + 6.0206).abs() < 0.001);
        assert_eq!(peak_hold.frame.frequency_hz_at(bin), Some(915_004_096.0));
        assert_eq!(peak_hold.strongest, Some(bin));
        assert_eq!(coverage.source(bin), Some(&burst));
        assert_eq!(coverage.source(64), Some(&second));
        assert_eq!(peak_hold.frame.frequency_hz_at(64), Some(914_991_808.0));
    }

    #[test]
    fn peak_hold_keeps_the_first_tied_source_and_ignores_invalid_bins() {
        let row = |start, bins_dbfs: [f32; 3]| Spectrum {
            samples: start..start + 8,
            bins_dbfs: bins_dbfs.into(),
            sample_rate_hz: None,
            center_hz: None,
            crosses_tuning: false,
            finite_samples: 4,
            noise_bandwidth_hz: None,
        };
        let measurement = Measurement::peak_hold(
            Waterfall {
                samples: 0..24,
                rows: vec![
                    row(0, [f32::NAN, -10.0, f32::NAN]),
                    row(8, [-4.0, -10.0, f32::NAN]),
                    row(16, [-4.0, f32::NAN, f32::NAN]),
                ],
                sampled: false,
            },
            None,
        )
        .unwrap();
        let coverage = measurement.coverage.unwrap();
        assert_eq!(coverage.source(0), Some(&(8..16)));
        assert_eq!(coverage.source(1), Some(&(0..8)));
        assert_eq!(coverage.source(2), None);
        assert_eq!(measurement.strongest, Some(0));
    }

    #[test]
    fn peak_hold_counts_the_union_of_fft_windows() {
        let file = tempfile::tempfile().unwrap();
        let count = 65_536;
        file.set_len(2 * count).unwrap();
        let capture = Capture::open_raw(
            file,
            RawInterpretation {
                encoding: Encoding::Ci8,
                sample_rate_hz: None,
                center_hz: None,
            },
        )
        .unwrap();
        let cancel = AtomicBool::new(false);
        let peak_hold = |selection| {
            Request::PeakHold {
                selection,
                fft: 128,
            }
            .read(&capture, &cancel)
            .unwrap()
        };
        let complete = peak_hold(100..301).coverage.unwrap();
        assert!(!complete.sampled());
        assert_eq!(complete.observed_samples, 201);
        assert_eq!(complete.windows, [100..228, 173..301]);
        let sampled = peak_hold(0..count).coverage.unwrap();
        assert!(sampled.sampled());
        assert_eq!(sampled.observed_samples, MAX_PEAK_WINDOWS * 128);
        assert_eq!(sampled.windows[0], 0..128);
        assert_eq!(sampled.windows.last(), Some(&(count - 128..count)));
    }

    #[test]
    fn peak_hold_rejects_a_retune_between_sampled_fft_windows() {
        let directory = tempfile::tempdir().unwrap();
        let metadata = directory.path().join("capture.sigmf-meta");
        std::fs::write(
            &metadata,
            br#"{
                "global":{"core:version":"1.0.0","core:datatype":"ci8","core:sample_rate":4000},
                "captures":[{"core:sample_start":0,"core:frequency":1000000},
                            {"core:sample_start":130,"core:frequency":2000000},
                            {"core:sample_start":150,"core:frequency":1000000}],
                "annotations":[]
            }"#,
        )
        .unwrap();
        let count = 65_536;
        let data = metadata.with_extension("sigmf-data");
        std::fs::write(&data, vec![0; 2 * count as usize]).unwrap();
        let cancel = AtomicBool::new(false);
        let recording = Recording::open_sigmf(&data, None, ReadCheck::default(), &cancel).unwrap();
        let capture = recording.capture();
        // With this range and budget, the first two windows skip the retune.
        let waterfall = capture
            .waterfall(0..count, 128, MAX_PEAK_WINDOWS as usize, &cancel)
            .unwrap();
        assert!(
            waterfall
                .rows
                .iter()
                .all(|row| row.center_hz == Some(1_000_000.0))
        );
        assert_eq!(shared_center(capture, &(0..count)), Err(()));
        assert_eq!(shared_center(capture, &(0..130)), Ok(Some(1_000_000.0)));
        assert!(
            Request::PeakHold {
                selection: 0..count,
                fft: 128,
            }
            .read(capture, &cancel)
            .err()
            .unwrap()
            .contains("one tuning center")
        );
    }
}
