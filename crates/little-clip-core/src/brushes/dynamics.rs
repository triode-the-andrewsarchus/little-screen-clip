/// A piecewise-linear response to a normalized brush input.
#[derive(Clone, Debug)]
pub(super) struct DabCurve(Vec<(f32, f32)>);

impl DabCurve {
    pub(super) fn linear(min: f32, max: f32) -> Self {
        Self(vec![(0.0, min), (1.0, max)])
    }

    pub(super) fn from_points(points: impl Iterator<Item = (f32, f32)>) -> Option<Self> {
        let points: Vec<_> = points.take(65).collect();
        if !(2..=64).contains(&points.len())
            || !points
                .iter()
                .all(|(x, y)| (0.0..=1.0).contains(x) && y.is_finite())
            || !points.windows(2).all(|pair| pair[0].0 < pair[1].0)
        {
            return None;
        }
        Some(Self(points))
    }

    pub(super) fn sampled(f: impl Fn(f32) -> f32) -> Self {
        Self(
            (0..=32)
                .map(|index| {
                    let input = index as f32 / 32.0;
                    (input, f(input))
                })
                .collect(),
        )
    }

    pub(super) fn at(&self, input: f32) -> f32 {
        let end = self.0.partition_point(|point| point.0 < input);
        match end {
            0 => self.0[0].1,
            end if end == self.0.len() => self.0[end - 1].1,
            end => {
                let (x0, y0) = self.0[end - 1];
                let (x1, y1) = self.0[end];
                y0 + (y1 - y0) * ((input - x0) / (x1 - x0))
            }
        }
    }
}

/// Optional dab behavior recovered from a brush preset for preview rendering.
#[derive(Clone, Debug, Default)]
pub struct BrushDynamics {
    pub(super) size: Option<DabCurve>,
    pub(super) opacity: Option<DabCurve>,
    pub(super) flow: Option<DabCurve>,
    pub(super) spacing: Option<DabCurve>,
    pub(super) scatter: Option<DabCurve>,
    pub(super) angle: Option<DabCurve>,
    pub(super) random_angle: Option<DabCurve>,
    pub(super) hardness: Option<DabCurve>,
    pub(super) aspect: Option<DabCurve>,
    pub(super) size_jitter: f32,
    pub(super) opacity_jitter: f32,
    pub(super) flow_jitter: f32,
    pub(super) angle_jitter_deg: f32,
    pub(super) follow_direction: f32,
    pub(super) random_start_angle: bool,
    pub(super) scatter_along_stroke: bool,
    pub(super) scatter_count: u8,
    pub(super) count_jitter: f32,
    pub(super) taper_start: f32,
    pub(super) taper_end: f32,
    pub(super) taper_size: f32,
    pub(super) taper_opacity: f32,
}

impl BrushDynamics {
    pub(super) fn taper(&self, progress: f32) -> f32 {
        let start = progress / self.taper_start.max(f32::EPSILON);
        let end = (1.0 - progress) / self.taper_end.max(f32::EPSILON);
        let start = if self.taper_start > 0.0 { start } else { 1.0 };
        let end = if self.taper_end > 0.0 { end } else { 1.0 };
        1.0 - start.min(end).min(1.0)
    }
}
