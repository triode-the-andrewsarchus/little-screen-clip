use super::{BrushPreset, BrushTipKind, StrokePath, sample_mask};

const SEGMENTS: usize = 64;

pub(super) fn draw(
    coverage: &mut [f32],
    (w, h): (usize, usize),
    preset: &BrushPreset,
    path: StrokePath,
    max_radius: f32,
) {
    let BrushTipKind::AlphaMask {
        width,
        height,
        alpha,
    } = &preset.tip
    else {
        return;
    };
    let points: [_; SEGMENTS + 1] =
        std::array::from_fn(|i| path.geometry(i as f32 / SEGMENTS as f32).0);
    let segments: Vec<_> = points
        .windows(2)
        .filter_map(|pair| Segment::between(pair[0], pair[1]))
        .collect();
    let length: f32 = segments.iter().map(|segment| segment.length).sum();
    let radius = (0.5 * length * *height as f32 / (*width).max(1) as f32)
        .min(max_radius)
        .max(0.75);
    let opacity = (preset.params.opacity * preset.params.flow).clamp(0.0, 1.0);
    let mut nearest = vec![f32::INFINITY; w * h];
    let mut traveled = 0.0;
    for (i, segment) in segments.iter().enumerate() {
        let first = i == 0;
        let last = i + 1 == segments.len();
        let (xs, ys) = segment.bounds(radius, w, h);
        for y in ys {
            for x in xs.clone() {
                let index = y * w + x;
                let Some(projection) =
                    segment.project((x as f32 + 0.5, y as f32 + 0.5), first, last)
                else {
                    continue;
                };
                if projection.distance_sq >= nearest[index]
                    || projection.distance_sq > radius * radius
                {
                    continue;
                }
                nearest[index] = projection.distance_sq;
                let longitudinal = 2.0 * (traveled + projection.along) / length - 1.0;
                let normal = projection.across / radius;
                coverage[index] =
                    sample_mask(longitudinal, normal, *width, *height, alpha) * opacity;
            }
        }
        traveled += segment.length;
    }
}

struct Segment {
    start: (f32, f32),
    end: (f32, f32),
    tangent: (f32, f32),
    length: f32,
}

struct Projection {
    along: f32,
    across: f32,
    distance_sq: f32,
}

impl Segment {
    fn between(start: (f32, f32), end: (f32, f32)) -> Option<Self> {
        let (dx, dy) = (end.0 - start.0, end.1 - start.1);
        let length = dx.hypot(dy);
        (length > f32::EPSILON).then(|| Self {
            start,
            end,
            tangent: (dx / length, dy / length),
            length,
        })
    }

    fn bounds(
        &self,
        radius: f32,
        w: usize,
        h: usize,
    ) -> (std::ops::Range<usize>, std::ops::Range<usize>) {
        let clipped_range = |a: f32, b: f32, edge: usize| {
            let min = ((a.min(b) - radius).floor() as usize).min(edge);
            let max = ((a.max(b) + radius).ceil() as usize).min(edge);
            min..max
        };
        (
            clipped_range(self.start.0, self.end.0, w),
            clipped_range(self.start.1, self.end.1, h),
        )
    }

    fn project(&self, point: (f32, f32), first: bool, last: bool) -> Option<Projection> {
        let dx = point.0 - self.start.0;
        let dy = point.1 - self.start.1;
        let (tx, ty) = self.tangent;
        let axial = dx * tx + dy * ty;
        if (first && axial < 0.0) || (last && axial > self.length) {
            return None;
        }
        let along = axial.clamp(0.0, self.length);
        let across = dy * tx - dx * ty;
        let overhang = axial - along;
        Some(Projection {
            along,
            across,
            distance_sq: across * across + overhang * overhang,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brushes::BrushStrokeParams;

    #[test]
    fn pattern_follows_a_curved_path_once() {
        let mut preset = BrushPreset {
            name: "Pattern".into(),
            // The left half carries ink, and the right half is blank.
            tip: BrushTipKind::AlphaMask {
                width: 8,
                height: 2,
                alpha: [255, 255, 255, 255, 0, 0, 0, 0].repeat(2).into(),
            },
            params: BrushStrokeParams::default(),
            detail: String::new(),
        };
        preset.params.dynamics.vector_texture = true;
        let mut coverage = vec![0.0; 100 * 50];
        super::super::draw_stroke(
            &mut coverage,
            (100, 50),
            &preset,
            StrokePath::Wave {
                x0: 10.0,
                x1: 90.0,
                y: 25.0,
                amplitude: 10.0,
            },
            8.0,
        );
        assert!(coverage[15 * 100 + 30] > 0.9);
        assert_eq!(coverage[35 * 100 + 70], 0.0);
        assert_eq!(coverage[25 * 100 + 2], 0.0);
    }
}
