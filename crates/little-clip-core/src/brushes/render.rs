use super::{BrushPreset, BrushTipKind, DabCurve, ParametricShape, mask};
use crate::{
    ImageThumbnail,
    palette::{linear_to_srgb, srgb_to_linear},
};
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
                flow: 1.0,
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
    let path = StrokePath::Wave {
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

pub const BRUSH_HOVER_ASPECT_RATIO: f32 = 4.5;

/// Draws an isolated tip, a white stroke, and two overlapping colored strokes.
pub fn render_brush_hover_preview(
    preset: &BrushPreset,
    edge: u32,
    colors: [[u8; 3]; 2],
) -> ImageThumbnail {
    let h = edge.clamp(8, 280);
    let w = (h as f32 * BRUSH_HOVER_ASPECT_RATIO).round() as u32;
    let white = render_brush_preview(preset, h * 3, h, [242, 242, 245], true);
    let overlap_w = w - white.width;
    let [lower, upper] = crossed_coverage(preset, overlap_w as usize, h as usize);
    let linear = colors.map(|rgb| rgb.map(|channel| srgb_to_linear(f32::from(channel) / 255.0)));

    let mut premultiplied_rgba = Vec::with_capacity(w as usize * h as usize * 4);
    for ((white, lower), upper) in white
        .premultiplied_rgba
        .chunks_exact(white.width as usize * 4)
        .zip(lower.chunks_exact(overlap_w as usize))
        .zip(upper.chunks_exact(overlap_w as usize))
    {
        premultiplied_rgba.extend_from_slice(white);
        for (&lower, &upper) in lower.iter().zip(upper) {
            premultiplied_rgba.extend_from_slice(&overlap_pixel(colors, linear, lower, upper));
        }
    }
    ImageThumbnail {
        width: w,
        height: h,
        premultiplied_rgba,
    }
}

fn crossed_coverage(preset: &BrushPreset, w: usize, h: usize) -> [Vec<f32>; 2] {
    let (width, height) = (w as f32, h as f32);
    let radius = preview_radius(preset.params.size_px, height).min(height * 0.18);
    let y_start = (radius + 2.0).max(height * 0.13);
    let center = width * 0.5;
    let span = width * 0.24;
    let bow = width * 0.09;
    [1.0, -1.0].map(|direction| {
        let path = StrokePath::Bow {
            start: (center - (span + bow) * direction, y_start),
            end: (center + (span - bow) * direction, height - y_start),
            bend: bow * direction,
        };
        let mut coverage = vec![0.0; w * h];
        draw_stroke(&mut coverage, (w, h), preset, path, radius);
        coverage
    })
}

fn overlap_pixel(colors: [[u8; 3]; 2], linear: [[f32; 3]; 2], lower: f32, upper: f32) -> [u8; 4] {
    let transmitted = lower * (1.0 - upper);
    let alpha = upper + transmitted;
    let rgb = if transmitted == 0.0 {
        colors[1]
    } else if upper == 0.0 {
        colors[0]
    } else {
        std::array::from_fn(|i| {
            let mixed = (linear[0][i] * transmitted + linear[1][i] * upper) / alpha;
            (linear_to_srgb(mixed) * 255.0).round() as u8
        })
    };
    let a = (alpha * 255.0).round() as u8;
    let af = f32::from(a) / 255.0;
    let [r, g, b] = rgb.map(|channel| (f32::from(channel) * af).round() as u8);
    [r, g, b, a]
}

// Fit large brushes in the cell and keep small brushes legible.
fn preview_radius(size_px: f32, height: f32) -> f32 {
    let fraction = (size_px.max(1.0) / 128.0).sqrt().clamp(0.06, 1.0);
    (height * 0.28 * fraction).max(1.0)
}

enum StrokePath {
    Wave {
        x0: f32,
        x1: f32,
        y: f32,
        amplitude: f32,
    },
    Bow {
        start: (f32, f32),
        end: (f32, f32),
        bend: f32,
    },
}

impl StrokePath {
    fn geometry(&self, t: f32) -> ((f32, f32), (f32, f32)) {
        match *self {
            Self::Wave {
                x0,
                x1,
                y,
                amplitude,
            } => {
                let phase = t * 2.0 * PI;
                let dx = x1 - x0;
                (
                    (x0 + dx * t, y - amplitude * phase.sin()),
                    (dx, -amplitude * 2.0 * PI * phase.cos()),
                )
            }
            Self::Bow { start, end, bend } => {
                let dx = end.0 - start.0;
                let dy = end.1 - start.1;
                (
                    (
                        start.0 + dx * t + bend * 4.0 * t * (1.0 - t),
                        start.1 + dy * t,
                    ),
                    (dx + bend * 4.0 * (1.0 - 2.0 * t), dy),
                )
            }
        }
    }
}

#[derive(Clone, Copy)]
struct Dab {
    x: f32,
    y: f32,
    radius: f32,
    angle: f32,
    opacity: f32,
    flow: f32,
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
    visit_dabs(preset, path, radius, |tip, dab, _| {
        stamp_tip(coverage, dimensions, tip, dab);
    });
}

fn visit_dabs(
    preset: &BrushPreset,
    path: StrokePath,
    radius: f32,
    mut visit: impl FnMut(&BrushTipKind, Dab, f32),
) {
    const PATH_SEGMENTS: usize = 64;
    const MAX_DABS: usize = 320;
    let mut distances = [0.0_f32; PATH_SEGMENTS + 1];
    let mut previous = path.geometry(0.0).0;
    for i in 1..=PATH_SEGMENTS {
        let point = path.geometry(i as f32 / PATH_SEGMENTS as f32).0;
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
        let (position, tangent) = path.geometry(t);
        let sample = StrokeSample {
            index,
            t,
            direction: tangent.1.atan2(tangent.0),
            seed: mix_hash(seed ^ (index as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15)),
            start_seed: seed,
            position,
        };
        let dab = sample_dab(preset, radius, sample);
        let pressure = pen_pressure(t);
        let tip = dynamics
            .tip_sequence
            .as_ref()
            .map_or(&preset.tip, |sequence| {
                sequence.tip_for_dab(index, pressure, sample.direction, sample.seed)
            });
        visit(tip, dab, pressure);
        let count = (f32::from(count)
            * (1.0 - (signed_random(sample.seed, 8) + 1.0) * 0.5 * dynamics.count_jitter))
            .round() as u8;
        for copy in 1..count {
            let sample = StrokeSample {
                seed: mix_hash(sample.seed ^ u64::from(copy)),
                ..sample
            };
            visit(tip, sample_dab(preset, radius, sample), pressure);
        }
        distance += dab_gap(preset, dab, radius, sample.direction).max(step_floor);
    }
}

fn dab_gap(preset: &BrushPreset, dab: Dab, base_radius: f32, direction: f32) -> f32 {
    let dynamics = &preset.params.dynamics;
    let diameter = 2.0 * dab.radius;
    if !dynamics.axis_spacing && dynamics.auto_spacing.is_none() {
        return diameter * dab.spacing;
    }
    let axis_scale = if dynamics.axis_spacing {
        match &preset.tip {
            BrushTipKind::AlphaMask { width, height, .. } => {
                let longest = (*width).max(*height).max(1) as f32;
                (*width as f32 / longest, *height as f32 / longest)
            }
            BrushTipKind::Parametric { aspect_ratio, .. } => {
                (1.0, (aspect_ratio * dab.aspect).max(1.0).recip())
            }
        }
    } else {
        (1.0, 1.0)
    };
    let axis_gap = |axis: f32| {
        if let Some(coefficient) = dynamics.auto_spacing {
            // Compute Krita's automatic gap at source size, then scale it
            // together with the current dab into preview pixels.
            let source_diameter = preset.params.size_px * dab.radius / base_radius;
            let source_axis = source_diameter * axis;
            coefficient * source_axis.min(source_axis.sqrt()) * diameter / source_diameter
        } else {
            diameter * axis * dab.spacing
        }
    };
    let (sin, cos) = (direction - dab.angle.to_radians()).sin_cos();
    let x_gap = axis_gap(axis_scale.0).max(f32::EPSILON);
    let y_gap = axis_gap(axis_scale.1).max(f32::EPSILON);
    (cos / x_gap).hypot(sin / y_gap).recip()
}

#[derive(Clone, Copy)]
struct StrokeSample {
    index: usize,
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
    let pressure_opacity = response(&dynamics.opacity, 1.0);
    let opacity_response = dynamics
        .opacity_fade
        .as_ref()
        .map_or(pressure_opacity, |fade| {
            fade.at(sample.index, pressure_opacity)
        });
    let opacity = params.opacity
        * opacity_response
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
        opacity: opacity.clamp(0.0, 1.0),
        flow: flow.clamp(0.0, 1.0),
        hardness: dynamics.hardness.as_ref().map(|curve| curve.at(pressure)),
        aspect: response(&dynamics.aspect, 1.0),
        spacing: response(&dynamics.spacing, params.spacing).clamp(0.01, 5.0),
    }
}

fn stamp_tip(coverage: &mut [f32], dimensions: (usize, usize), tip: &BrushTipKind, dab: Dab) {
    if dab.opacity <= 0.0 || dab.flow <= 0.0 {
        return;
    }
    visit_tip_pixels(dimensions, tip, dab, |x, y, sample| {
        let dst = &mut coverage[y * dimensions.0 + x];
        // Repeated dabs build up toward the current opacity at the flow rate.
        *dst += (dab.opacity - *dst).max(0.0) * sample * dab.flow;
    });
}

fn visit_tip_pixels(
    (w, h): (usize, usize),
    tip: &BrushTipKind,
    dab: Dab,
    mut visit: impl FnMut(usize, usize, f32),
) {
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
            visit(px, py, sample);
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
    let mut dist = mask::shape_distance(shape, sx, sy);
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
    fn hover_strokes_keep_the_white_sample_and_blend_using_brush_opacity() {
        let mut preset = BrushPreset {
            name: "Circle".into(),
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
        preset.params.opacity = 0.5;
        let colors = [[240, 0, 0], [0, 0, 240]];
        let hover = render_brush_hover_preview(&preset, 100, colors);
        let white = render_brush_preview(&preset, 300, 100, [242, 242, 245], true);
        for (hover_row, white_row) in hover
            .premultiplied_rgba
            .chunks_exact(450 * 4)
            .zip(white.premultiplied_rgba.chunks_exact(300 * 4))
        {
            assert_eq!(&hover_row[..300 * 4], white_row);
        }
        let pixel = (50 * 450 + 375) * 4;
        let [red, green, blue, alpha] = hover.premultiplied_rgba[pixel..pixel + 4] else {
            unreachable!()
        };
        assert!(blue > red && red > 40 && green == 0);
        assert!((185..=193).contains(&alpha));

        preset.params.opacity = 1.0;
        let opaque = render_brush_hover_preview(&preset, 100, colors);
        assert_eq!(
            &opaque.premultiplied_rgba[pixel..pixel + 4],
            &[0, 0, 240, 255]
        );
    }

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
                flow: 1.0,
                hardness: None,
                aspect: 1.0,
                spacing: 0.0,
            },
        );
        assert_eq!(coverage[22 * 32 + 16], 1.0);
        assert_eq!(coverage[16 * 32 + 22], 0.0);
    }

    #[test]
    fn automatic_spacing_is_calculated_on_source_axes_before_preview_scaling() {
        let mut preset = BrushPreset {
            name: "Flat tip".into(),
            tip: BrushTipKind::AlphaMask {
                width: 4,
                height: 1,
                alpha: [255; 4].into(),
            },
            params: super::super::BrushStrokeParams {
                size_px: 100.0,
                ..Default::default()
            },
            detail: String::new(),
        };
        preset.params.dynamics.axis_spacing = true;
        let dab = Dab {
            x: 0.0,
            y: 0.0,
            radius: 10.0,
            angle: 0.0,
            opacity: 1.0,
            flow: 1.0,
            hardness: None,
            aspect: 1.0,
            spacing: 0.5,
        };
        assert_eq!(dab_gap(&preset, dab, 10.0, 0.0), 10.0);
        assert_eq!(dab_gap(&preset, dab, 10.0, PI / 2.0), 2.5);

        preset.params.dynamics.auto_spacing = Some(2.0);
        assert_eq!(dab_gap(&preset, dab, 10.0, 0.0), 4.0);
        assert_eq!(dab_gap(&preset, dab, 10.0, PI / 2.0), 2.0);
    }

    #[test]
    fn opacity_caps_buildup_while_flow_controls_each_dab() {
        let tip = BrushTipKind::AlphaMask {
            width: 1,
            height: 1,
            alpha: [255].into(),
        };
        let mut coverage = [0.0; 8 * 8];
        let dab = Dab {
            x: 4.0,
            y: 4.0,
            radius: 2.0,
            angle: 0.0,
            opacity: 0.5,
            flow: 0.5,
            hardness: None,
            aspect: 1.0,
            spacing: 0.0,
        };
        stamp_tip(&mut coverage, (8, 8), &tip, dab);
        assert_eq!(coverage[4 * 8 + 4], 0.25);
        for _ in 0..20 {
            stamp_tip(&mut coverage, (8, 8), &tip, dab);
        }
        assert!((coverage[4 * 8 + 4] - 0.5).abs() < 1.0e-6);
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
            index: 0,
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
