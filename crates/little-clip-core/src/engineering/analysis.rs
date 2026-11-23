//! Numerical tools for inspecting bounded windows of captured signals.

mod expression;
mod filter;
mod spectrum;

pub use expression::{Expression, ExpressionError, Value};
pub use filter::{FilterChain, FilterKind, FilterStage};
pub use spectrum::{
    MAX_ANALYSIS_SAMPLES, Spectrum, SpectrumWindow, fundamental_hz, spectrum, spectrum_with_window,
};

/// Returns the nearest zero crossing within a sample window.
///
/// The target, radius, and return value use fractional indices relative to the
/// slice. Opposite signs are linearly interpolated. An exact zero is only a
/// crossing if its adjoining finite nonzero samples have opposite signs.
/// Skips gaps and returns `None` when there is no crossing within the radius.
pub fn nearest_zero_crossing(
    samples: &[f32],
    target_index: f64,
    radius_samples: f64,
) -> Option<f64> {
    if samples.len() < 2
        || !target_index.is_finite()
        || !radius_samples.is_finite()
        || radius_samples < 0.0
    {
        return None;
    }

    // Include the adjacent signs when the radius lands on an exact zero.
    let start = ((target_index - radius_samples).floor().max(0.0) as usize).saturating_sub(1);
    let end = ((target_index + radius_samples).ceil().max(0.0) as usize)
        .saturating_add(2)
        .min(samples.len());
    let window = samples.get(start..end)?;
    let mut previous = None::<(usize, f64)>;
    let mut best = None::<(f64, f64)>;
    for (index, &sample) in window.iter().enumerate() {
        let sample = f64::from(sample);
        if !sample.is_finite() {
            previous = None;
            continue;
        }
        if sample == 0.0 {
            continue;
        }
        if let Some((left, value)) = previous
            && value.is_sign_positive() != sample.is_sign_positive()
        {
            let crossing = if index == left + 1 {
                left as f64 - value / (sample - value)
            } else {
                // Every intervening point is zero. Choose the position on the
                // zero plateau nearest the cursor.
                (target_index - start as f64).clamp((left + 1) as f64, (index - 1) as f64)
            };
            let crossing = start as f64 + crossing;
            let distance = (crossing - target_index).abs();
            if distance <= radius_samples && best.is_none_or(|(_, old)| distance < old) {
                best = Some((crossing, distance));
            }
        }
        previous = Some((index, sample));
    }
    best.map(|(index, _)| index)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crossing_is_interpolated_and_limited_to_radius() {
        let signal = [-1.0, -1.0, 3.0, 2.0, -2.0];
        assert_eq!(nearest_zero_crossing(&signal, 1.0, 2.0), Some(1.25));
        assert_eq!(nearest_zero_crossing(&signal, 1.0, 0.24), None);
        assert_eq!(nearest_zero_crossing(&signal, 3.1, 1.0), Some(3.5));
    }

    #[test]
    fn requires_a_sign_change_across_exact_zeros() {
        let crossing = [2.0, 0.0, 0.0, -1.0];
        assert_eq!(nearest_zero_crossing(&crossing, 1.8, 2.0), Some(1.8));
        assert_eq!(
            nearest_zero_crossing(&[2.0, 0.0, -1.0], 1.0, 0.0),
            Some(1.0)
        );
        assert_eq!(nearest_zero_crossing(&[0.0; 5], 2.0, 2.0), None);
        assert_eq!(nearest_zero_crossing(&[1.0, 0.0, 1.0], 1.0, 2.0), None);
        assert_eq!(
            nearest_zero_crossing(&[1.0, f32::NAN, -1.0], 1.0, 2.0),
            None
        );
    }
}
