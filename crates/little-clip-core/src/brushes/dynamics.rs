/// A piecewise-linear response to a normalized brush input.
#[derive(Clone, Debug)]
pub(super) struct DabCurve(Vec<(f32, f32)>);

impl DabCurve {
    pub(super) fn linear(min: f32, max: f32) -> Self {
        Self(vec![(0.0, min), (1.0, max)])
    }

    pub(super) fn sampled(f: impl Fn(f32) -> f32) -> Self {
        Self::lookup(32, f)
    }

    pub(super) fn lookup(segments: u16, f: impl Fn(f32) -> f32) -> Self {
        Self(
            (0..=segments)
                .map(|index| {
                    let input = f32::from(index) / f32::from(segments);
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
    pub(super) vector_texture: bool,
    pub(super) opacity: Option<DabCurve>,
    pub(super) opacity_fade: Option<super::krita::OpacityFade>,
    pub(super) krita_smudge: Option<super::krita::LegacySmudge>,
    pub(super) flow: Option<DabCurve>,
    pub(super) spacing: Option<DabCurve>,
    pub(super) axis_spacing: bool,
    pub(super) auto_spacing: Option<f32>,
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
    pub(super) tip_sequence: Option<super::gimp::TipSequence>,
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
