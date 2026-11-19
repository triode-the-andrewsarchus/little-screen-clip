/// A sampled dab property as a function of pen pressure.
#[derive(Clone, Debug)]
pub struct PressureCurve(Vec<(f32, f32)>);

impl PressureCurve {
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

    pub(super) fn at(&self, pressure: f32) -> f32 {
        let end = self.0.partition_point(|point| point.0 < pressure);
        match end {
            0 => self.0[0].1,
            end if end == self.0.len() => self.0[end - 1].1,
            end => {
                let (x0, y0) = self.0[end - 1];
                let (x1, y1) = self.0[end];
                y0 + (y1 - y0) * ((pressure - x0) / (x1 - x0))
            }
        }
    }
}

/// Optional dab behavior recovered from a brush preset for preview rendering.
#[derive(Clone, Debug, Default)]
pub struct BrushDynamics {
    pub(super) size: Option<PressureCurve>,
    pub(super) opacity: Option<PressureCurve>,
    pub(super) flow: Option<PressureCurve>,
    pub(super) angle: Option<PressureCurve>,
    pub(super) hardness: Option<PressureCurve>,
    pub(super) aspect: Option<PressureCurve>,
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
