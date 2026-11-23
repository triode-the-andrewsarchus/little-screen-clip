use super::{Error, TracePoint};
use std::f64::consts::{PI, TAU};

/// A group-delay estimate and the measured frequency aperture that produced it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GroupDelayPoint {
    pub index: u64,
    pub frequency_hz: f64,
    pub seconds: Option<f64>,
    pub aperture_hz: Option<f64>,
}

/// Estimates group delay from the slope of the measured complex phase.
///
/// `radius` selects how many neighboring records on either side contribute
/// to the slope. A missing or zero-amplitude value ends the unwrapped phase
/// run. At an edge, only available neighbors contribute. The measured
/// apertures can differ on a nonuniform frequency grid.
///
/// # Errors
///
/// Rejects non-increasing or non-finite frequencies and invalid sweep sizes.
pub fn group_delay(points: &[TracePoint], radius: usize) -> Result<Vec<GroupDelayPoint>, Error> {
    if !(2..=16_384).contains(&points.len()) || !(1..=128).contains(&radius) {
        return Err(Error::Limit(
            "group delay requires 2 to 16,384 points and a radius of 1 to 128",
        ));
    }
    if points.iter().any(|point| !point.frequency_hz.is_finite())
        || points
            .windows(2)
            .any(|pair| pair[1].frequency_hz <= pair[0].frequency_hz)
    {
        return Err(Error::Invalid(
            "group delay requires increasing, finite frequencies",
        ));
    }
    let has_phase = |point: &TracePoint| {
        point.value.re.is_finite()
            && point.value.im.is_finite()
            && (point.value.re != 0.0 || point.value.im != 0.0)
    };
    let mut output = Vec::with_capacity(points.len());
    for run in points.chunk_by(|left, right| has_phase(left) && has_phase(right)) {
        if run.len() < 2 {
            output.push(no_estimate(run[0]));
            continue;
        }
        estimate_run(run, radius, &mut output);
    }
    Ok(output)
}

fn estimate_run(run: &[TracePoint], radius: usize, output: &mut Vec<GroupDelayPoint>) {
    let mut previous = run[0].value.arg();
    let mut cumulative = 0.0;
    let mut phase = Vec::with_capacity(run.len());
    for point in run {
        let angle = point.value.arg();
        cumulative += (angle - previous + PI).rem_euclid(TAU) - PI;
        phase.push(cumulative);
        previous = angle;
    }
    for (index, &point) in run.iter().enumerate() {
        let left = index.saturating_sub(radius);
        let right = (index + radius).min(run.len() - 1);
        let aperture = run[right].frequency_hz - run[left].frequency_hz;
        let seconds = -(phase[right] - phase[left]) / aperture / TAU;
        let mut estimated = no_estimate(point);
        if seconds.is_finite() && aperture.is_finite() {
            estimated.seconds = Some(seconds);
            estimated.aperture_hz = Some(aperture);
        }
        output.push(estimated);
    }
}

fn no_estimate(point: TracePoint) -> GroupDelayPoint {
    GroupDelayPoint {
        index: point.index,
        frequency_hz: point.frequency_hz,
        seconds: None,
        aperture_hz: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engineering::touchstone::Complex64;

    #[test]
    fn measures_delay_on_a_nonuniform_sweep_across_phase_wraps() {
        let expected = 24.0e-9;
        let mut hz = 100.0e6;
        let points: Vec<_> = (0..39)
            .map(|index| {
                hz += 1.0e6 + (index % 4) as f64 * 0.7e6;
                TracePoint {
                    index,
                    frequency_hz: hz,
                    value: Complex64::from_polar(0.2, -TAU * hz * expected),
                }
            })
            .collect();
        for radius in [1, 7] {
            let measured = group_delay(&points, radius).unwrap();
            for point in measured {
                assert!((point.seconds.unwrap() - expected).abs() < 1e-20);
                assert!(point.aperture_hz.unwrap() > 0.0);
            }
        }
    }

    #[test]
    fn an_undefined_phase_prevents_the_aperture_crossing_the_gap() {
        let points: Vec<_> = (0..5)
            .map(|index| TracePoint {
                index,
                frequency_hz: 1e6 * (index + 1) as f64,
                value: if index == 2 {
                    Complex64::ZERO
                } else {
                    Complex64::new(0.25, 0.0)
                },
            })
            .collect();
        let estimate = group_delay(&points, 4).unwrap();
        assert_eq!(estimate[2].seconds, None);
        for index in [0, 1, 3, 4] {
            assert_eq!(estimate[index].seconds, Some(0.0));
            assert_eq!(estimate[index].aperture_hz, Some(1e6));
        }
    }
}
