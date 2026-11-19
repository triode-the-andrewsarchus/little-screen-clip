use super::{BrushPreset, BrushTipKind, DabCurve, ParametricShape};
use crate::ImageThumbnail;
use std::f32::consts::PI;

/// Draws a sample stroke and optional isolated tip in ink on a transparent background.
pub fn render_brush_preview(
    preset: &BrushPreset,
    width: u32,
    height: u32,
    fg_rgb: [u8; 3],
    include_tip_stamp: bool,
) -> ImageThumbnail {
    let w = width.max(8) as usize;
    let h = height.max(8) as usize;
    let mut coverage = vec![0.0_f32; w * h];

    let stroke_x0 = if include_tip_stamp && w >= h * 2 {
        let center = h as f32 * 0.5;
        stamp_tip(
            &mut coverage,
            (w, h),
            &preset.tip,
            Dab {
                x: center,
                y: center,
                radius: h as f32 * 0.34,
                angle: preset.params.angle_deg,
                opacity: 1.0,
                hardness: None,
                aspect: 1.0,
                spacing: 0.0,
            },
        );
        h as f32 + 4.0
    } else {
        w as f32 * 0.08
    };
    let radius = preview_radius(preset.params.size_px, h as f32);
    let inset = (radius + 1.0).max(3.0);
    let path = StrokePath {
        x0: stroke_x0 + inset,
        x1: (w as f32 - inset).max(stroke_x0 + inset),
        y: h as f32 * 0.5,
        amplitude: (h as f32 * 0.21).min(h as f32 * 0.5 - radius - 1.0),
    };
    draw_stroke(&mut coverage, (w, h), preset, path, radius);

    let mut premultiplied_rgba = Vec::with_capacity(w * h * 4);
    for cov in coverage {
        let a = (cov.clamp(0.0, 1.0) * 255.0).round() as u8;
        let af = f32::from(a) / 255.0;
        premultiplied_rgba.push((f32::from(fg_rgb[0]) * af).round() as u8);
        premultiplied_rgba.push((f32::from(fg_rgb[1]) * af).round() as u8);
        premultiplied_rgba.push((f32::from(fg_rgb[2]) * af).round() as u8);
        premultiplied_rgba.push(a);
    }

    ImageThumbnail {
        width: w as u32,
        height: h as u32,
        premultiplied_rgba,
    }
}

// Fit large brushes in the cell and keep small brushes legible.
fn preview_radius(size_px: f32, height: f32) -> f32 {
    let fraction = (size_px.max(1.0) / 128.0).sqrt().clamp(0.06, 1.0);
    (height * 0.28 * fraction).max(1.0)
}

struct StrokePath {
    x0: f32,
    x1: f32,
    y: f32,
    amplitude: f32,
}

impl StrokePath {
    fn point(&self, t: f32) -> (f32, f32) {
        (
            self.x0 + (self.x1 - self.x0) * t,
            self.y - self.amplitude * (t * 2.0 * PI).sin(),
        )
    }
}

#[derive(Clone, Copy)]
struct Dab {
    x: f32,
    y: f32,
    radius: f32,
    angle: f32,
    opacity: f32,
    hardness: Option<f32>,
    aspect: f32,
    spacing: f32,
}

fn draw_stroke(
    coverage: &mut [f32],
    dimensions: (usize, usize),
    preset: &BrushPreset,
    path: StrokePath,
    radius: f32,
) {
    const PATH_SEGMENTS: usize = 64;
    const MAX_DABS: usize = 320;
    let mut distances = [0.0_f32; PATH_SEGMENTS + 1];
    let mut previous = path.point(0.0);
    for i in 1..=PATH_SEGMENTS {
        let point = path.point(i as f32 / PATH_SEGMENTS as f32);
        distances[i] = distances[i - 1] + (point.0 - previous.0).hypot(point.1 - previous.1);
        previous = point;
    }
    let length = distances[PATH_SEGMENTS];
    let dynamics = &preset.params.dynamics;
    let count = dynamics.scatter_count.clamp(1, 16);
    let max_steps = MAX_DABS / usize::from(count);
    let step_floor = (length / (max_steps - 1) as f32).max(0.8);
    let seed = fnv1a(preset.name.as_bytes());
    let mut distance = 0.0;
    for index in 0..max_steps {
        if distance > length {
            break;
        }
        let end = distances
            .partition_point(|&d| d <= distance)
            .clamp(1, PATH_SEGMENTS);
        let fraction = (distance - distances[end - 1])
            / (distances[end] - distances[end - 1]).max(f32::EPSILON);
        let t = (end as f32 - 1.0 + fraction) / PATH_SEGMENTS as f32;
        let sample = StrokeSample {
            t,
            direction: (-path.amplitude * 2.0 * PI * (t * 2.0 * PI).cos()).atan2(path.x1 - path.x0),
            seed: mix_hash(seed ^ (index as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15)),
            start_seed: seed,
            position: path.point(t),
        };
        let dab = sample_dab(preset, radius, sample);
        let tip = dynamics
            .tip_sequence
            .as_ref()
            .map_or(&preset.tip, |sequence| {
                sequence.tip_for_dab(index, pen_pressure(t), sample.seed)
            });
        stamp_tip(coverage, dimensions, tip, dab);
        let count = (f32::from(count)
            * (1.0 - (signed_random(sample.seed, 8) + 1.0) * 0.5 * dynamics.count_jitter))
            .round() as u8;
        for copy in 1..count {
            let sample = StrokeSample {
                seed: mix_hash(sample.seed ^ u64::from(copy)),
                ..sample
            };
            stamp_tip(
                coverage,
                dimensions,
                tip,
                sample_dab(preset, radius, sample),
            );
        }
        distance += (dab.radius * 2.0 * dab.spacing).max(step_floor);
    }
}

#[derive(Clone, Copy)]
struct StrokeSample {
    t: f32,
    direction: f32,
    seed: u64,
    start_seed: u64,
    position: (f32, f32),
}

fn pen_pressure(t: f32) -> f32 {
    (t * PI).sin().max(0.0).powf(0.65)
}

fn sample_dab(preset: &BrushPreset, radius: f32, sample: StrokeSample) -> Dab {
    let params = &preset.params;
    let dynamics = &params.dynamics;
    let pressure = pen_pressure(sample.t);
    let response = |curve: &Option<DabCurve>, fallback| {
        curve.as_ref().map_or(fallback, |curve| curve.at(pressure))
    };
    let random = |channel| signed_random(sample.seed, channel);
    let taper = dynamics.taper(sample.t);
    let size = response(&dynamics.size, 1.0)
        * (1.0 + random(1) * dynamics.size_jitter)
        * (1.0 - taper * dynamics.taper_size);
    let opacity = params.opacity
        * response(&dynamics.opacity, 1.0)
        * (1.0 - (random(2) + 1.0) * 0.5 * dynamics.opacity_jitter)
        * (1.0 - taper * dynamics.taper_opacity);
    let flow = params.flow
        * response(&dynamics.flow, 1.0)
        * (1.0 - (random(3) + 1.0) * 0.5 * dynamics.flow_jitter);
    let angle = params.angle_deg
        + response(&dynamics.angle, 0.0)
        + sample.direction.to_degrees() * dynamics.follow_direction
        + random(4) * dynamics.angle_jitter_deg
        + dynamics
            .random_angle
            .as_ref()
            .map_or(0.0, |curve| curve.at((random(9) + 1.0) * 0.5))
        + signed_random(sample.start_seed, 7) * 180.0 * f32::from(dynamics.random_start_angle);
    let (sin, cos) = sample.direction.sin_cos();
    let scatter = response(&dynamics.scatter, params.scatter).clamp(0.0, 4.0) * radius;
    let across = random(5) * scatter;
    let along = random(6) * scatter * f32::from(dynamics.scatter_along_stroke);

    Dab {
        x: sample.position.0 + along * cos - across * sin,
        y: sample.position.1 + along * sin + across * cos,
        radius: (radius * size.clamp(0.0, 2.0)).max(0.4),
        angle,
        opacity: (opacity * flow).clamp(0.0, 1.0),
        hardness: dynamics.hardness.as_ref().map(|curve| curve.at(pressure)),
        aspect: response(&dynamics.aspect, 1.0),
        spacing: response(&dynamics.spacing, params.spacing).clamp(0.01, 5.0),
    }
}

fn stamp_tip(coverage: &mut [f32], (w, h): (usize, usize), tip: &BrushTipKind, dab: Dab) {
    if dab.opacity <= 0.0 {
        return;
    }
    let (rx, ry) = match tip {
        BrushTipKind::AlphaMask { width, height, .. } => {
            let scale = dab.radius / (*width).max(*height).max(1) as f32;
            (scale * *width as f32, scale * *height as f32)
        }
        BrushTipKind::Parametric { .. } => (dab.radius, dab.radius),
    };
    let (sin, cos) = dab.angle.to_radians().sin_cos();
    let extent_x = rx * cos.abs() + ry * sin.abs();
    let extent_y = rx * sin.abs() + ry * cos.abs();
    let x0 = ((dab.x - extent_x).floor() as i32).clamp(0, w as i32) as usize;
    let x1 = ((dab.x + extent_x).ceil() as i32).clamp(0, w as i32) as usize;
    let y0 = ((dab.y - extent_y).floor() as i32).clamp(0, h as i32) as usize;
    let y1 = ((dab.y + extent_y).ceil() as i32).clamp(0, h as i32) as usize;

    for py in y0..y1 {
        let dy = py as f32 + 0.5 - dab.y;
        for px in x0..x1 {
            let dx = px as f32 + 0.5 - dab.x;
            let lx = (dx * cos + dy * sin) / rx.max(0.5);
            let ly = (-dx * sin + dy * cos) / ry.max(0.5);
            let sample = match tip {
                BrushTipKind::AlphaMask {
                    width,
                    height,
                    alpha,
                } => sample_mask(lx, ly, *width, *height, alpha),
                BrushTipKind::Parametric {
                    shape,
                    hardness,
                    aspect_ratio,
                    spikes,
                    ..
                } => sample_parametric(
                    lx,
                    ly,
                    *shape,
                    dab.hardness.unwrap_or(*hardness),
                    (aspect_ratio * dab.aspect).max(1.0),
                    *spikes,
                ),
            };
            let dst = &mut coverage[py * w + px];
            *dst += (1.0 - *dst) * sample * dab.opacity;
        }
    }
}

fn sample_mask(lx: f32, ly: f32, width: u32, height: u32, alpha: &[u8]) -> f32 {
    if lx < -1.0 || lx > 1.0 || ly < -1.0 || ly > 1.0 || width == 0 || height == 0 {
        return 0.0;
    }
    let u = (lx * 0.5 + 0.5) * (width - 1) as f32;
    let v = (ly * 0.5 + 0.5) * (height - 1) as f32;
    let x0 = (u.floor() as u32).min(width - 1);
    let y0 = (v.floor() as u32).min(height - 1);
    let x1 = (x0 + 1).min(width - 1);
    let y1 = (y0 + 1).min(height - 1);
    let fx = u - x0 as f32;
    let fy = v - y0 as f32;

    let get = |x: u32, y: u32| -> f32 {
        alpha
            .get((y * width + x) as usize)
            .map_or(0.0, |&v| f32::from(v) / 255.0)
    };
    let a = get(x0, y0);
    let b = get(x1, y0);
    let c = get(x0, y1);
    let d = get(x1, y1);
    let top = a + (b - a) * fx;
    let bot = c + (d - c) * fx;
    top + (bot - top) * fy
}

fn sample_parametric(
    lx: f32,
    ly: f32,
    shape: ParametricShape,
    hardness: f32,
    aspect_ratio: f32,
    spikes: u8,
) -> f32 {
    let sx = lx;
    let sy = ly * aspect_ratio.max(1.0);
    let mut dist = match shape {
        ParametricShape::Circle => (sx * sx + sy * sy).sqrt(),
        ParametricShape::Square => sx.abs().max(sy.abs()),
        ParametricShape::Diamond => sx.abs() + sy.abs(),
    };
    if spikes > 2 {
        let theta = sy.atan2(sx);
        let mod_factor = 0.72 + 0.28 * (theta * f32::from(spikes)).cos();
        dist /= mod_factor;
    }
    if dist >= 1.0 {
        return 0.0;
    }
    let hard = hardness.clamp(0.02, 0.99);
    if dist <= hard {
        1.0
    } else {
        let t = (dist - hard) / (1.0 - hard);
        (1.0 - t * t * (3.0 - 2.0 * t)).clamp(0.0, 1.0)
    }
}

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325_u64;
    for &b in bytes {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

fn signed_random(seed: u64, channel: u64) -> f32 {
    let mixed = mix_hash(seed ^ channel.wrapping_mul(0xd6e8_feb8_6659_fd93));
    (mixed & 0xffff) as f32 / 32767.5 - 1.0
}

fn mix_hash(mut x: u64) -> u64 {
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotated_bitmaps_keep_their_proportions() {
        let tip = BrushTipKind::AlphaMask {
            width: 8,
            height: 2,
            alpha: [255; 16].into(),
        };
        let mut coverage = vec![0.0; 32 * 32];
        stamp_tip(
            &mut coverage,
            (32, 32),
            &tip,
            Dab {
                x: 16.0,
                y: 16.0,
                radius: 8.0,
                angle: 90.0,
                opacity: 1.0,
                hardness: None,
                aspect: 1.0,
                spacing: 0.0,
            },
        );
        assert_eq!(coverage[22 * 32 + 16], 1.0);
        assert_eq!(coverage[16 * 32 + 22], 0.0);
    }

    #[test]
    fn taper_and_pressure_only_apply_when_the_preset_enables_them() {
        let mut preset = BrushPreset {
            name: "Test".into(),
            tip: BrushTipKind::Parametric {
                shape: ParametricShape::Circle,
                hardness: 1.0,
                aspect_ratio: 1.0,
                angle_deg: 0.0,
                spikes: 2,
            },
            params: Default::default(),
            detail: String::new(),
        };
        let sample = |t| StrokeSample {
            t,
            direction: 0.0,
            seed: 1,
            start_seed: 2,
            position: (0.0, 0.0),
        };
        assert_eq!(sample_dab(&preset, 10.0, sample(0.0)).radius, 10.0);
        preset.params.dynamics.size = Some(DabCurve::linear(0.3, 1.0));
        preset.params.dynamics.taper_start = 0.25;
        preset.params.dynamics.taper_opacity = 1.0;
        let start = sample_dab(&preset, 10.0, sample(0.0));
        let middle = sample_dab(&preset, 10.0, sample(0.5));
        assert_eq!(start.radius, 3.0);
        assert_eq!(start.opacity, 0.0);
        assert_eq!(middle.radius, 10.0);
        assert_eq!(middle.opacity, 1.0);
    }
}
