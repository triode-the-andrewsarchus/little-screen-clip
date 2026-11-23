//! Bounded peak spectra with a time-domain origin for every displayed bin.

use super::{MAX_ANALYSIS_SAMPLES, Spectrum, SpectrumWindow, spectrum::spectrum_with_fft_size};
use std::ops::Range;

const MAX_WINDOWS: u64 = 12;

/// Peak amplitudes measured across evenly distributed time windows.
pub struct SpectralSurvey {
    /// Largest observed amplitude in each bin, in the input's unit.
    pub spectrum: Spectrum,
    peak_intervals: Vec<Range<u64>>,
    /// Number of usable FFT windows.
    pub windows: u64,
    /// Number of distinct samples covered by usable windows.
    pub covered_samples: u64,
    /// Number of samples in the requested interval.
    pub requested_samples: u64,
}

impl SpectralSurvey {
    /// Returns the half-open sample range supplying one frequency bin's peak.
    pub fn peak_interval(&self, bin: usize) -> Option<Range<u64>> {
        self.peak_intervals.get(bin).cloned()
    }
}

/// Measures a physical or derived channel using at most twelve bounded FFTs.
///
/// A short interval uses a single transform. Long intervals use a peak
/// amplitude survey over equally distributed contiguous windows. Each bin
/// remembers where its peak was found. For an interval with gaps, the longest
/// contiguous finite segment is transformed on the same frequency grid.
/// Coverage counts only distinct samples used by those transforms.
///
/// # Errors
/// Propagates errors from the supplied sample reader.
pub fn spectral_survey<E>(
    range: Range<u64>,
    sample_rate: f64,
    window: SpectrumWindow,
    mut read: impl FnMut(Range<u64>) -> Result<Vec<f32>, E>,
) -> Result<Option<SpectralSurvey>, E> {
    let requested = range.end.saturating_sub(range.start);
    if requested < 2 {
        return Ok(None);
    }
    let width = requested.min(MAX_ANALYSIS_SAMPLES as u64);
    let count = requested.div_ceil(width).min(MAX_WINDOWS);
    let mut survey: Option<SpectralSurvey> = None;
    let mut previous_end = range.start;
    for index in 0..count {
        let leftover = requested - width;
        let offset = if count == 1 {
            0
        } else {
            (u128::from(leftover) * u128::from(index) / u128::from(count - 1)) as u64
        };
        let start = range.start + offset;
        let end = start + width;
        let samples = read(start..end)?;
        if samples.len() as u64 != width {
            continue;
        }
        let finite = longest_run(&samples);
        let Some(next) = spectrum_with_fft_size(
            &samples[finite.clone()],
            sample_rate,
            window,
            (width as usize).next_power_of_two(),
        ) else {
            continue;
        };
        let observed = start + finite.start as u64..start + finite.end as u64;
        let covered = observed
            .end
            .saturating_sub(previous_end.max(observed.start));
        previous_end = previous_end.max(observed.end);
        match &mut survey {
            Some(survey) => survey.merge(observed, covered, next),
            None => {
                survey = Some(SpectralSurvey {
                    peak_intervals: vec![observed; next.peak_amplitude.len()],
                    spectrum: next,
                    windows: 1,
                    covered_samples: covered,
                    requested_samples: requested,
                });
            }
        }
    }
    Ok(survey)
}

impl SpectralSurvey {
    fn merge(&mut self, observed: Range<u64>, covered: u64, next: Spectrum) {
        for ((peak, origin), candidate) in self
            .spectrum
            .peak_amplitude
            .iter_mut()
            .zip(&mut self.peak_intervals)
            .zip(next.peak_amplitude)
        {
            if candidate > *peak {
                *peak = candidate;
                *origin = observed.clone();
            }
        }
        self.windows += 1;
        self.covered_samples += covered;
    }
}

fn longest_run(samples: &[f32]) -> Range<usize> {
    let mut best = 0..0;
    let mut start = 0;
    for end in samples
        .iter()
        .enumerate()
        .filter_map(|(index, value)| (!value.is_finite()).then_some(index))
        .chain(std::iter::once(samples.len()))
    {
        if end - start > best.len() {
            best = start..end;
        }
        start = end + 1;
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{convert::Infallible, f64::consts::TAU};

    #[test]
    fn strongest_frequency_links_to_the_window_that_contains_it() {
        let block = MAX_ANALYSIS_SAMPLES as u64;
        let sample_rate = 65_536.0;
        let survey = spectral_survey(
            block..block * 4,
            sample_rate,
            SpectrumWindow::Rectangular,
            |range| {
                let amplitude = if range.start == block * 2 { 2.0 } else { 0.2 };
                Ok::<_, Infallible>(
                    range
                        .map(|index| {
                            (amplitude * (index as f64 / sample_rate * TAU * 70.0).sin()) as f32
                        })
                        .collect(),
                )
            },
        )
        .unwrap()
        .unwrap();
        assert_eq!(survey.peak_interval(70), Some(block * 2..block * 3));
        assert!((survey.spectrum.peak_amplitude[70] - 2.0).abs() < 1e-6);
        assert_eq!(survey.covered_samples, block * 3);
    }

    #[test]
    fn gaps_do_not_erase_an_observed_tone_or_extend_its_time_range() {
        let survey = spectral_survey(0..128, 64.0, SpectrumWindow::Rectangular, |rows| {
            Ok::<_, Infallible>(
                rows.map(|row| {
                    if (32..96).contains(&row) {
                        (TAU * row as f64 * 6.0 / 64.0).sin() as f32
                    } else {
                        f32::NAN
                    }
                })
                .collect(),
            )
        })
        .unwrap()
        .unwrap();
        assert_eq!(survey.peak_interval(12), Some(32..96));
        assert_eq!(survey.covered_samples, 64);
        assert!((survey.spectrum.peak_amplitude[12] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn large_survey_limits_io_and_reports_only_valid_window_coverage() {
        let block = MAX_ANALYSIS_SAMPLES as u64;
        let mut reads = Vec::new();
        let survey = spectral_survey(0..block * 100, 1000.0, SpectrumWindow::Hann, |range| {
            reads.push(range.clone());
            let value = if range.start == 0 { f32::NAN } else { 2.0 };
            Ok::<_, Infallible>(vec![value; block as usize])
        })
        .unwrap()
        .unwrap();
        assert_eq!(reads.len(), MAX_WINDOWS as usize);
        assert_eq!(reads[0], 0..block);
        assert_eq!(reads.last(), Some(&(block * 99..block * 100)));
        assert_eq!(survey.covered_samples, block * (MAX_WINDOWS - 1));
        assert_eq!(survey.requested_samples, block * 100);
    }
}
