//! Fixed-size hash artwork generated once on the image preview worker.
use little_clip_core::{
    ImageThumbnail,
    palette::{Oklab, Oklch},
};
use std::f32::consts::TAU;
use tiny_skia::{Color, FillRule, Paint, PathBuilder, Pixmap, Transform};
const EDGE: u32 = 128;
type Point = (f32, f32);

struct Bits {
    bytes: Vec<u8>,
    position: usize,
}
impl Bits {
    fn parse(hash: &str) -> Option<Self> {
        let digits: Option<Vec<_>> = hash.chars().map(|c| c.to_digit(16)).collect();
        let digits = digits?;
        if digits.is_empty() || digits.len() % 2 != 0 {
            return None;
        }
        Some(Self {
            bytes: digits
                .chunks_exact(2)
                .map(|pair| (pair[0] * 16 + pair[1]) as u8)
                .collect(),
            position: 0,
        })
    }
    fn take(&mut self, count: usize) -> u32 {
        let mut value = 0;
        for _ in 0..count {
            let bit = self.position % (self.bytes.len() * 8);
            value = (value << 1) | u32::from((self.bytes[bit / 8] >> (7 - bit % 8)) & 1);
            self.position += 1;
        }
        value
    }
    fn unit(&mut self, count: usize) -> f32 {
        self.take(count) as f32 / ((1_u32 << count) - 1) as f32
    }
}

struct Canvas {
    pixels: Pixmap,
    colors: [Color; 5],
}
impl Canvas {
    fn new(hue: f32, separation: f32, lightness: f32, chroma: f32) -> Option<Self> {
        let base = Oklch {
            l: lightness,
            c: chroma,
            h: hue,
        };
        let colors = [
            Oklch {
                l: 0.94,
                c: 0.025,
                ..base
            },
            Oklch {
                l: 0.24,
                c: 0.045,
                ..base
            },
            base,
            base.rotate_hue(separation),
            Oklch {
                l: 0.82,
                c: 0.085,
                ..base.rotate_hue(separation)
            },
        ]
        .map(|color| {
            let (r, g, b) = color.to_rgb();
            Color::from_rgba8(r, g, b, 255)
        });
        let mut pixels = Pixmap::new(EDGE, EDGE)?;
        pixels.fill(colors[0]);
        Some(Self { pixels, colors })
    }
    fn fill(&mut self, builder: PathBuilder, color: usize) {
        if let Some(path) = builder.finish() {
            let mut paint = Paint::default();
            paint.set_color(self.colors[color % self.colors.len()]);
            self.pixels.fill_path(
                &path,
                &paint,
                FillRule::Winding,
                Transform::from_scale(EDGE as f32, EDGE as f32),
                None,
            );
        }
    }
    fn polygon(&mut self, points: &[Point], color: usize) {
        let Some(first) = points.first() else {
            return;
        };
        let mut path = PathBuilder::new();
        path.move_to(first.0, first.1);
        for point in &points[1..] {
            path.line_to(point.0, point.1);
        }
        path.close();
        self.fill(path, color);
    }
    fn finish(self) -> ImageThumbnail {
        ImageThumbnail {
            width: EDGE,
            height: EDGE,
            premultiplied_rgba: self.pixels.take(),
        }
    }
}

pub(in crate::file_manager) fn exact(hash: &str) -> Option<ImageThumbnail> {
    let mut bits = Bits::parse(hash)?;
    let family = bits.take(2);
    let hue = bits.unit(8) * 360.0;
    let separation = [30.0, 120.0, 180.0, 150.0][bits.take(2) as usize];
    let lightness = 0.60 + bits.unit(2) * 0.12;
    let chroma = 0.10 + bits.unit(2) * 0.06;
    let mut canvas = Canvas::new(hue, separation, lightness, chroma)?;
    match family {
        0 => rings(&mut canvas, &mut bits),
        1 => stripes(&mut canvas, &mut bits),
        2 => radial(&mut canvas, &mut bits),
        _ => angular(&mut canvas, &mut bits),
    }
    Some(canvas.finish())
}

pub(super) fn perceptual(hash: &str) -> Option<ImageThumbnail> {
    let mut bits = Bits::parse(hash)?;
    if bits.bytes.len() != 10 || bits.take(4) != 1 {
        return None;
    }
    // Color the grid from the prefix; each remaining bit maps to one cell.
    let rgb: [u8; 3] = std::array::from_fn(|_| (bits.take(4) * 16 + 8) as u8);
    let base = Oklab::from_rgb(rgb[0], rgb[1], rgb[2]).to_oklch();
    let mut canvas = Canvas::new(base.h, 0.0, 0.78, base.c.clamp(0.045, 0.16))?;
    for y in 0..8 {
        for x in 0..8 {
            let color = if bits.take(1) == 0 { 1 } else { 2 };
            let cell = tiny_skia::Rect::from_xywh(x as f32 * 16.0, y as f32 * 16.0, 16.0, 16.0)?;
            let mut paint = Paint::default();
            paint.set_color(canvas.colors[color]);
            paint.anti_alias = false;
            canvas
                .pixels
                .fill_rect(cell, &paint, Transform::identity(), None);
        }
    }
    Some(canvas.finish())
}

fn polar(center: Point, radius: f32, angle: f32) -> Point {
    (
        center.0 + radius * angle.cos(),
        center.1 + radius * angle.sin(),
    )
}

fn rings(canvas: &mut Canvas, bits: &mut Bits) {
    for _ in 0..6 {
        let center = (bits.unit(7), bits.unit(7));
        let radius = 0.18 + bits.unit(7) * 0.62;
        let width = 0.035 + bits.unit(5) * 0.16;
        let start = bits.unit(6) * TAU;
        let span = (0.25 + bits.unit(6) * 0.75) * TAU;
        let color = 1 + bits.take(3) as usize % 4;
        let mut points: Vec<_> = (0..=48)
            .map(|i| polar(center, radius, start + span * i as f32 / 48.0))
            .collect();
        points.extend(
            (0..=48)
                .rev()
                .map(|i| polar(center, radius - width, start + span * i as f32 / 48.0)),
        );
        canvas.polygon(&points, color);
    }
}

fn stripes(canvas: &mut Canvas, bits: &mut Bits) {
    let angle = bits.unit(8) * TAU;
    let transform = |x: f32, y: f32| {
        (
            0.5 + x * angle.cos() - y * angle.sin(),
            0.5 + x * angle.sin() + y * angle.cos(),
        )
    };
    for index in 0..10 {
        let x = -0.9 + index as f32 * 0.18 + (bits.unit(6) - 0.5) * 0.12;
        let width = 0.075 + bits.unit(5) * 0.13;
        let wave = (bits.unit(6) - 0.5) * 0.28;
        let taper = (bits.unit(5) - 0.5) * 0.05;
        let color = 1 + bits.take(3) as usize % 4;
        let a = transform(x, -1.0);
        let b = transform(x + width, -1.0);
        let c = transform(x + width + taper, 1.0);
        let d = transform(x + taper, 1.0);
        let bend_a = transform(x + wave, 0.0);
        let bend_b = transform(x + width + wave, 0.0);
        let mut path = PathBuilder::new();
        path.move_to(a.0, a.1);
        path.line_to(b.0, b.1);
        path.quad_to(bend_b.0, bend_b.1, c.0, c.1);
        path.line_to(d.0, d.1);
        path.quad_to(bend_a.0, bend_a.1, a.0, a.1);
        path.close();
        canvas.fill(path, color);
    }
}

fn radial(canvas: &mut Canvas, bits: &mut Bits) {
    let phase = bits.unit(8) * TAU;
    for index in 0..12 {
        let angle = phase + index as f32 * TAU / 12.0;
        let radius = 0.52 + bits.unit(6) * 0.4;
        let inner = 0.03 + bits.unit(5) * 0.32;
        let bend = bits.unit(5) * 0.08;
        let color = 1 + bits.take(3) as usize % 4;
        let twist = (bits.unit(3) - 0.5) * 0.16;
        canvas.polygon(
            &[
                polar((0.5, 0.5), inner, angle + twist),
                polar((0.5, 0.5), radius, angle),
                polar((0.5, 0.5), radius, angle + TAU / 12.0),
                polar((0.5, 0.5), bend, angle),
            ],
            color,
        );
    }
}

fn angular(canvas: &mut Canvas, bits: &mut Bits) {
    let center = (0.15 + bits.unit(4) * 0.7, 0.15 + bits.unit(4) * 0.7);
    for index in 0..12 {
        let radius = 0.45 + bits.unit(7) * 0.7;
        let offset = (bits.unit(6) - 0.5) * 0.24;
        let inset = bits.unit(4) * 0.25;
        let color = 1 + bits.take(3) as usize % 4;
        let angle = index as f32 * TAU / 12.0;
        canvas.polygon(
            &[
                polar(center, inset, angle + offset),
                polar(center, radius, angle + offset),
                polar(center, radius, angle + TAU / 12.0 + offset),
            ],
            color,
        );
    }
}
