use std::sync::atomic::{AtomicBool, Ordering};

const MAX_EVALUATIONS: usize = 24_000_000;

/// Agreement between a reference and an offset window in another recording.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Correlation {
    /// First target sample compared with reference sample zero.
    pub start: usize,
    /// Pearson coefficient. Constant or largely unavailable windows have no score.
    pub coefficient: Option<f64>,
    /// Number of finite pairs used for the score.
    pub overlap: usize,
}

/// Computes normalized correlation over every complete placement of a reference.
///
/// The arrays share a sample interval. NaNs and other non-finite samples
/// leave a gap. At least half of the reference and at least eight pairs must
/// be present at an offset. Both positive and inverted features can match.
///
/// # Errors
///
/// Returns an error if the search is too large or small, or was cancelled.
pub fn correlate(
    reference: &[f32],
    target: &[f32],
    cancel: &AtomicBool,
) -> Result<Vec<Correlation>, String> {
    if reference.len() < 8 || target.len() < reference.len() {
        return Err("Alignment needs at least eight samples inside the target search range".into());
    }
    let count = target.len() - reference.len() + 1;
    if reference.len().saturating_mul(count) > MAX_EVALUATIONS {
        return Err("Shorten the reference or the offset search range".into());
    }
    let minimum = reference.len().div_ceil(2).max(8);
    let mut output = Vec::with_capacity(count);
    for (start, candidate) in target.windows(reference.len()).enumerate() {
        if cancel.load(Ordering::Relaxed) {
            return Err("Alignment cancelled".into());
        }
        let mut totals = Moments::default();
        for (&a, &b) in reference.iter().zip(candidate) {
            if a.is_finite() && b.is_finite() {
                totals.push(f64::from(a), f64::from(b));
            }
        }
        output.push(Correlation {
            start,
            coefficient: totals.correlation(minimum),
            overlap: totals.overlap,
        });
    }
    Ok(output)
}

/// Finds distinct local peaks in the absolute similarity score.
///
/// Both positive and negative correlations can be useful. Neighboring samples
/// on a flat peak are collapsed to one candidate.
pub fn strongest_peaks(points: &[Correlation], maximum: usize) -> Vec<Correlation> {
    let strength = |point: &Correlation| point.coefficient.map_or(0.0, f64::abs);
    let mut peaks = Vec::new();
    let mut previous = 0.0;
    for (index, point) in points.iter().enumerate() {
        let current = strength(point);
        let next = points.get(index + 1).map_or(0.0, strength);
        if current > previous && current >= next {
            peaks.push(*point);
        }
        previous = current;
    }
    peaks.sort_by(|a, b| {
        strength(b)
            .total_cmp(&strength(a))
            .then_with(|| b.overlap.cmp(&a.overlap))
    });
    peaks.truncate(maximum);
    peaks
}

#[derive(Default)]
struct Moments {
    overlap: usize,
    mean_a: f64,
    mean_b: f64,
    variance_a: f64,
    variance_b: f64,
    covariance: f64,
}

impl Moments {
    fn push(&mut self, a: f64, b: f64) {
        self.overlap += 1;
        let inverse = (self.overlap as f64).recip();
        let delta_a = a - self.mean_a;
        let delta_b = b - self.mean_b;
        self.mean_a += delta_a * inverse;
        self.mean_b += delta_b * inverse;
        // Updating around the running means leaves flat windows exactly flat
        // and avoids subtracting large nearly equal sums for biased channels.
        self.variance_a += delta_a * (a - self.mean_a);
        self.variance_b += delta_b * (b - self.mean_b);
        self.covariance += delta_a * (b - self.mean_b);
    }

    fn correlation(&self, minimum: usize) -> Option<f64> {
        if self.overlap < minimum || self.variance_a <= 0.0 || self.variance_b <= 0.0 {
            return None;
        }
        let score = self.covariance / (self.variance_a * self.variance_b).sqrt();
        score.is_finite().then_some(score.clamp(-1.0, 1.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifies_an_inverted_feature_even_with_gaps_and_a_different_offset() {
        let reference = [
            1., 2., 4., 1., 7., -6., 2., -4., 5., 9., 0., -9., 7., 4., 0.,
        ];
        let mut target = vec![0.0; 83];
        let shift = 27;
        for (index, &value) in reference.iter().enumerate() {
            target[shift + index] = 12.0 - 0.5 * value;
        }
        target[shift + 6] = f32::NAN;
        let points = correlate(&reference, &target, &AtomicBool::new(false)).unwrap();
        let peak = strongest_peaks(&points, 3)[0];
        assert_eq!(peak.start, shift);
        assert_eq!(peak.overlap, reference.len() - 1);
        assert!((peak.coefficient.unwrap() + 1.0).abs() < 1e-12);
    }

    #[test]
    fn low_overlap_and_constant_regions_have_no_candidate() {
        let reference = (0..32).map(|value| value as f32).collect::<Vec<_>>();
        let constant = [0.0; 64];
        let holes = [f32::NAN; 64];
        let cancel = AtomicBool::new(false);
        for target in [constant, holes] {
            let output = correlate(&reference, &target, &cancel).unwrap();
            assert!(strongest_peaks(&output, 3).is_empty());
            assert!(output.iter().all(|point| point.coefficient.is_none()));
        }
    }
}
