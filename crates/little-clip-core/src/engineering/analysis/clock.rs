use serde::{Deserialize, Serialize};
use std::ops::Range;

/// A monotonic affine map from recording seconds to workspace seconds.
///
/// Epochs keep the skew calculation centered near the recorded landmarks.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Clock {
    native_epoch: f64,
    workspace_epoch: f64,
    scale: f64,
}

impl Default for Clock {
    fn default() -> Self {
        Self::offset(0.0)
    }
}

impl Clock {
    /// Constructs an unscaled clock displaced by the given number of seconds.
    pub fn offset(seconds: f64) -> Self {
        Self {
            native_epoch: 0.0,
            workspace_epoch: seconds,
            scale: 1.0,
        }
    }

    /// Maps a native epoch to a workspace epoch and scales elapsed time.
    pub fn anchored(native_epoch: f64, workspace_epoch: f64, scale: f64) -> Option<Self> {
        let clock = Self {
            native_epoch,
            workspace_epoch,
            scale,
        };
        clock.valid().then_some(clock)
    }

    /// Confirms that the parameters describe a finite, monotonic clock.
    pub fn valid(self) -> bool {
        self.native_epoch.is_finite()
            && self.workspace_epoch.is_finite()
            && self.scale.is_finite()
            && self.scale > 0.0
            && self.scale.recip().is_finite()
    }

    pub fn native_epoch(self) -> f64 {
        self.native_epoch
    }

    pub fn epoch_offset(self) -> f64 {
        self.workspace_epoch - self.native_epoch
    }

    /// Returns the number of workspace seconds per native second.
    pub fn scale(self) -> f64 {
        self.scale
    }

    /// Compares clock parameters without rounding for query keys.
    pub fn key(self) -> [u64; 3] {
        [
            self.native_epoch.to_bits(),
            self.workspace_epoch.to_bits(),
            self.scale.to_bits(),
        ]
    }

    pub fn to_workspace(self, native: f64) -> f64 {
        (native - self.native_epoch).mul_add(self.scale, self.workspace_epoch)
    }

    pub fn to_native(self, workspace: f64) -> f64 {
        (workspace - self.workspace_epoch) / self.scale + self.native_epoch
    }

    pub fn workspace_range(self, native: &Range<f64>) -> Range<f64> {
        self.to_workspace(native.start)..self.to_workspace(native.end)
    }

    /// Checks whether the mapped recording has a representable workspace interval.
    pub fn maps_range(self, native: &Range<f64>) -> bool {
        let mapped = self.workspace_range(native);
        self.valid()
            && self.epoch_offset().is_finite()
            && mapped.start.is_finite()
            && mapped.end.is_finite()
            && !mapped.is_empty()
    }

    pub fn native_range(self, workspace: &Range<f64>) -> Range<f64> {
        self.to_native(workspace.start)..self.to_native(workspace.end)
    }

    /// Adds a constant shift while keeping the reference epoch and clock scale.
    pub fn shifted(self, seconds: f64) -> Self {
        Self {
            workspace_epoch: self.workspace_epoch + seconds,
            ..self
        }
    }
}

/// A matched time in the target recording and reference workspace.
#[derive(Debug, Clone, Copy)]
pub struct Landmark {
    pub native: f64,
    pub workspace: f64,
}

/// Fits a clock to ordered landmarks by least squares.
///
/// One landmark retains the supplied scale and fits its offset. Multiple
/// landmarks must be distinct and occur in the same order on both timelines.
/// Centering and normalizing the two spans keeps large epochs out of the fit.
///
/// # Errors
///
/// Returns an error for missing, coincident, reversed, or non-finite times.
pub fn fit_clock(landmarks: &[Landmark], one_pair_scale: f64) -> Result<Clock, &'static str> {
    if landmarks
        .iter()
        .any(|point| !point.native.is_finite() || !point.workspace.is_finite())
    {
        return Err("Landmark times must be finite.");
    }
    let mut points = landmarks.to_vec();
    points.sort_unstable_by(|a, b| a.native.total_cmp(&b.native));
    let Some(first) = points.first() else {
        return Err("Add a landmark pair to estimate an offset.");
    };
    if points.len() == 1 {
        return Clock::anchored(first.native, first.workspace, one_pair_scale)
            .ok_or("The offset exceeds the clock precision.");
    }
    for pair in points.windows(2) {
        if pair[0].native == pair[1].native || pair[0].workspace == pair[1].workspace {
            return Err("Each pair needs a distinct time in both recordings.");
        }
        if pair[0].workspace > pair[1].workspace {
            return Err("Landmark pairs must occur in the same order in both recordings.");
        }
    }
    let last = points.last().expect("at least two landmarks");
    let width = last.native - first.native;
    let height = last.workspace - first.workspace;
    let coordinates = || {
        points.iter().map(|point| {
            (
                (point.native - first.native) / width,
                (point.workspace - first.workspace) / height,
            )
        })
    };
    let count = points.len() as f64;
    let (x, y) = coordinates().fold((0.0, 0.0), |(x, y), (a, b)| (x + a / count, y + b / count));
    let (variance, covariance) =
        coordinates().fold((0.0, 0.0), |(variance, covariance), (a, b)| {
            let dx = a - x;
            (dx.mul_add(dx, variance), dx.mul_add(b - y, covariance))
        });
    let slope = covariance / variance;
    let origin = (-slope).mul_add(x, y).mul_add(height, first.workspace);
    Clock::anchored(first.native, origin, slope * (height / width))
        .ok_or("The landmarks exceed the clock precision.")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_pair_keeps_the_previously_calibrated_rate() {
        let fit = fit_clock(
            &[Landmark {
                native: 8.0,
                workspace: 9.5,
            }],
            0.9995,
        )
        .unwrap();
        assert_eq!(fit.scale(), 0.9995);
        assert_eq!(fit.epoch_offset(), 1.5);
        assert_eq!(fit.to_native(9.5), 8.0);
    }

    #[test]
    fn centered_fit_recovers_a_small_rate_change_at_a_large_epoch() {
        let expected = Clock::anchored(1_000_000_000.0, 1_000_000_000.25, 1.000125).unwrap();
        let endpoints = [100.25, 0.0, 1405.75].map(|delta| {
            let native = expected.native_epoch() + delta;
            Landmark {
                native,
                workspace: expected.to_workspace(native),
            }
        });
        let fit = fit_clock(&endpoints, 1.0).unwrap();
        assert!((fit.scale() - expected.scale()).abs() < 1e-9);
        assert_eq!(
            fit.to_workspace(expected.native_epoch()),
            expected.to_workspace(expected.native_epoch())
        );
        let native = 1_000_000_501.125;
        assert_eq!(fit.to_native(fit.to_workspace(native)), native);
    }

    #[test]
    fn inconsistent_middle_landmark_produces_visible_signed_residuals() {
        let points = [
            Landmark {
                native: 0.0,
                workspace: 0.5,
            },
            Landmark {
                native: 5.0,
                workspace: 6.0,
            },
            Landmark {
                native: 10.0,
                workspace: 10.5,
            },
        ];
        let fit = fit_clock(&points, 1.0).unwrap();
        let residuals = points.map(|point| fit.to_workspace(point.native) - point.workspace);
        assert!((residuals[0] - 1.0 / 6.0).abs() < 1e-12);
        assert!((residuals[1] + 1.0 / 3.0).abs() < 1e-12);
        assert!((residuals[2] - 1.0 / 6.0).abs() < 1e-12);
    }

    #[test]
    fn refuses_coincident_reversed_and_non_finite_landmarks() {
        let first = Landmark {
            native: 1.0,
            workspace: 2.0,
        };
        for second in [
            Landmark {
                native: 1.0,
                workspace: 3.0,
            },
            Landmark {
                native: 2.0,
                workspace: 2.0,
            },
            Landmark {
                native: 2.0,
                workspace: 1.0,
            },
            Landmark {
                native: f64::NAN,
                workspace: 5.0,
            },
        ] {
            assert!(fit_clock(&[first, second], 1.0).is_err());
        }
    }
}
