use super::{BrushPreset, BrushTipKind, ParametricShape};
use crate::ImageThumbnail;
use std::f32::consts::PI;

/// Renders a cohesive S-curve pressure-tapered stroke (with optional left-side tip stamp)
/// using `fg_rgb` ink on a transparent background.
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
        let stamp_size = ((h as f32) * 0.72).max(8.0);
        let cx = h as f32 * 0.5;
        let cy = h as f32 * 0.5;
        stamp_tip(
            &mut coverage,
            w,
            h,
            &preset.tip,
            cx,
            cy,
            stamp_size * 0.5,
            0.0,
            1.0,
        );
        (h as f32) + 4.0
    } else {
        (w as f32) * 0.08
    };
    let stroke_x1 = (w as f32) * 0.92;
    let stroke_w = (stroke_x1 - stroke_x0).max(8.0);

    let max_radius = ((h as f32) * 0.28).max(2.5);
    let min_radius = (max_radius * 0.20).max(1.0);
    let spacing = preset.params.spacing.clamp(0.06, 0.85);
    let flow = (preset.params.opacity * preset.params.flow).clamp(0.25, 1.0);
    let scatter_amp = preset.params.scatter.clamp(0.0, 1.5) * max_radius * 0.45;
    let seed = fnv1a(preset.name.as_bytes());

    let steps = ((stroke_w / (max_radius * spacing).max(1.2)).ceil() as usize).clamp(16, 260);
    for i in 0..=steps {
        let t = i as f32 / steps as f32;
        let pressure = (t * PI).sin().max(0.0).powf(0.65);
        let radius = min_radius + (max_radius - min_radius) * pressure;
        let x = stroke_x0 + stroke_w * t;
        let y = (h as f32) * 0.5 - (h as f32) * 0.22 * (t * 2.0 * PI).sin();

        let (jx, jy) = if scatter_amp > 0.01 {
            let h1 = mix_hash(seed ^ (i as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15));
            let h2 = mix_hash(h1 ^ 0xbf58_476d_1ce4_e5b9);
            let rx = ((h1 & 0xffff) as f32 / 32767.5) - 1.0;
            let ry = ((h2 & 0xffff) as f32 / 32767.5) - 1.0;
            (rx * scatter_amp, ry * scatter_amp)
        } else {
            (0.0, 0.0)
        };

        stamp_tip(
            &mut coverage,
            w,
            h,
            &preset.tip,
            x + jx,
            y + jy,
            radius,
            preset.params.angle_deg,
            flow,
        );
    }

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

#[expect(clippy::too_many_arguments)]
fn stamp_tip(
    coverage: &mut [f32],
    w: usize,
    h: usize,
    tip: &BrushTipKind,
    cx: f32,
    cy: f32,
    radius: f32,
    angle_deg: f32,
    flow: f32,
) {
    let bound = radius.ceil() as i32 + 1;
    let x0 = (cx.floor() as i32 - bound).max(0) as usize;
    let x1 = (cx.ceil() as i32 + bound).min(w as i32 - 1) as usize;
    let y0 = (cy.floor() as i32 - bound).max(0) as usize;
    let y1 = (cy.ceil() as i32 + bound).min(h as i32 - 1) as usize;
    if x0 > x1 || y0 > y1 {
        return;
    }

    let rad = angle_deg.to_radians();
    let (sin_a, cos_a) = rad.sin_cos();

    for py in y0..=y1 {
        let dy = py as f32 + 0.5 - cy;
        let row_off = py * w;
        for px in x0..=x1 {
            let dx = px as f32 + 0.5 - cx;
            let lx = (dx * cos_a + dy * sin_a) / radius;
            let ly = (-dx * sin_a + dy * cos_a) / radius;

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
                } => sample_parametric(lx, ly, *shape, *hardness, *aspect_ratio, *spikes),
            };

            if sample > 0.001 {
                let dst = &mut coverage[row_off + px];
                let src = sample * flow;
                *dst = 1.0 - (1.0 - *dst) * (1.0 - src);
            }
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

fn mix_hash(mut x: u64) -> u64 {
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}
