use super::{
    Dab, StrokePath, crossed_paths, overlap_radius, tip_geometry, visit_dabs, visit_tip_pixels,
};
use crate::brushes::{
    BrushPreset, BrushTipKind,
    krita::{LegacySmudge, SmudgeMode},
};

pub(super) fn render(
    preset: &BrushPreset,
    settings: &LegacySmudge,
    dimensions: (usize, usize),
    colors: [[u8; 3]; 2],
) -> Vec<[u8; 4]> {
    let (w, h) = dimensions;
    let inks = colors.map(Paint::opaque);
    let mut canvas = Canvas::new(w, h);
    let radius = overlap_radius(preset, h);
    if settings.color.strength == 0.0 {
        canvas.seed_swatch(inks);
        let path = StrokePath::Wave {
            x0: w as f32 * 0.21,
            x1: w as f32 * 0.81,
            y: h as f32 * 0.5,
            amplitude: h as f32 * 0.07,
        };
        canvas.stroke(preset, settings, path, radius, inks[0]);
    } else {
        for (path, ink) in crossed_paths(w, h, radius).into_iter().zip(inks) {
            canvas.stroke(preset, settings, path, radius, ink);
        }
    }
    canvas.pixels.into_iter().map(Paint::rgba8).collect()
}

/// Premultiplied color in the preview's sRGB paint space.
#[derive(Clone, Copy, Default)]
struct Paint([f32; 4]);

impl Paint {
    fn opaque([r, g, b]: [u8; 3]) -> Self {
        Self([r, g, b, 255].map(|value| f32::from(value) / 255.0))
    }

    fn mix(self, sample: Self, amount: f32) -> Self {
        Self(std::array::from_fn(|i| {
            self.0[i] + (sample.0[i] - self.0[i]) * amount
        }))
    }

    fn over(self, ink: Self, amount: f32) -> Self {
        let remaining = 1.0 - ink.0[3] * amount;
        Self(std::array::from_fn(|i| {
            self.0[i] * remaining + ink.0[i] * amount
        }))
    }

    fn rgba8(self) -> [u8; 4] {
        self.0
            .map(|value| (value.clamp(0.0, 1.0) * 255.0).round() as u8)
    }
}

struct Canvas {
    w: usize,
    h: usize,
    pixels: Vec<Paint>,
    pending: Vec<(usize, Paint)>,
}

impl Canvas {
    fn new(w: usize, h: usize) -> Self {
        Self {
            w,
            h,
            pixels: vec![Paint::default(); w * h],
            pending: Vec::new(),
        }
    }

    fn seed_swatch(&mut self, inks: [Paint; 2]) {
        let (w, h) = (self.w as f32, self.h as f32);
        let half_width = w * 0.39;
        let half_height = h * 0.31;
        let corner = h * 0.08;
        for (index, pixel) in self.pixels.iter_mut().enumerate() {
            let (x, y) = ((index % self.w) as f32 + 0.5, (index / self.w) as f32 + 0.5);
            let edge_x = ((x - w * 0.5).abs() - (half_width - corner)).max(0.0);
            let edge_y = ((y - h * 0.5).abs() - (half_height - corner)).max(0.0);
            let coverage = (corner + 0.5 - edge_x.hypot(edge_y)).clamp(0.0, 1.0);
            let ink = inks[usize::from(x >= w * 0.5)];
            *pixel = Paint(ink.0.map(|channel| channel * coverage));
        }
    }

    fn stroke(
        &mut self,
        preset: &BrushPreset,
        settings: &LegacySmudge,
        path: StrokePath,
        radius: f32,
        ink: Paint,
    ) {
        let mut previous = None;
        visit_dabs(preset, path, radius, |tip, dab, pressure| {
            // The first position initializes the pickup location for the stroke.
            let center = (dab.x, dab.y);
            if let Some(previous) = previous.replace(center) {
                let rates = DabRates {
                    transfer: settings.length.at(pressure) * dab.opacity,
                    pigment: settings.color.at(pressure)
                        * dab.opacity
                        * (1.0 - settings.maximum_length).max(0.2),
                    radius: settings.radius.at(pressure),
                };
                self.paint_dab(tip, dab, previous, settings, rates, ink);
            }
        });
    }

    fn paint_dab(
        &mut self,
        tip: &BrushTipKind,
        dab: Dab,
        previous: (f32, f32),
        settings: &LegacySmudge,
        rates: DabRates,
        ink: Paint,
    ) {
        if rates.transfer <= 0.0 || dab.flow <= 0.0 {
            return;
        }
        let offset = (
            previous.0.round() as i32 - dab.x.round() as i32,
            previous.1.round() as i32 - dab.y.round() as i32,
        );
        let average = match settings.mode {
            SmudgeMode::Smear => None,
            SmudgeMode::Dull => {
                let (rx, ry) = tip_geometry(tip, dab).half_extent;
                Some(self.average(previous, (rx * rates.radius, ry * rates.radius)))
            }
        };
        let mut pending = std::mem::take(&mut self.pending);
        let (w, h) = (self.w, self.h);
        visit_tip_pixels((w, h), tip, dab, |x, y, coverage| {
            if coverage <= 0.0 {
                return;
            }
            let index = y * w + x;
            let underneath = self.pixels[index];
            let pickup =
                average.unwrap_or_else(|| self.sample(x as i32 + offset.0, y as i32 + offset.1));
            let loaded = pickup.over(ink, rates.pigment);
            let strength = coverage * rates.transfer * dab.flow;
            let result = if settings.smear_alpha {
                underneath.mix(loaded, strength)
            } else {
                underneath.over(loaded, strength)
            };
            pending.push((index, result));
        });
        // Every source pixel is read before the dab changes the canvas.
        for (index, color) in pending.drain(..) {
            self.pixels[index] = color;
        }
        self.pending = pending;
    }

    fn sample(&self, x: i32, y: i32) -> Paint {
        if x < 0 || x >= self.w as i32 || y < 0 || y >= self.h as i32 {
            return Paint::default();
        }
        self.pixels[y as usize * self.w + x as usize]
    }

    fn average(&self, (x, y): (f32, f32), (rx, ry): (f32, f32)) -> Paint {
        fn span(center: f32, radius: f32) -> (i32, i32) {
            let first = (center - radius).floor() as i32;
            let width = ((center + radius).ceil() as i32 - first).max(1);
            (first, width)
        }
        let (x0, width) = span(x, rx);
        let (y0, height) = span(y, ry);
        // An evenly spaced bounded sample keeps a large pickup radius cheap.
        let nx = width.min(16);
        let ny = height.min(16);
        let mut sum = [0.0; 4];
        for row in 0..ny {
            let y = y0 + (2 * row + 1) * height / (2 * ny);
            for column in 0..nx {
                let x = x0 + (2 * column + 1) * width / (2 * nx);
                for (sum, sample) in sum.iter_mut().zip(self.sample(x, y).0) {
                    *sum += sample;
                }
            }
        }
        Paint(sum.map(|channel| channel / (nx * ny) as f32))
    }
}

struct DabRates {
    transfer: f32,
    pigment: f32,
    radius: f32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dulling_averages_color_and_alpha_across_the_pickup_area() {
        let mut canvas = Canvas::new(2, 1);
        canvas.pixels[0] = Paint::opaque([200, 40, 0]);
        assert_eq!(
            canvas.average((1.0, 0.5), (1.0, 0.5)).rgba8(),
            [100, 20, 0, 128]
        );
        canvas.pixels[1] = Paint::opaque([0, 0, 220]);
        assert_eq!(
            canvas.average((1.0, 0.5), (1.0, 0.5)).rgba8(),
            [100, 20, 110, 255]
        );
    }

    #[test]
    fn smearing_alpha_can_lift_paint_that_over_compositing_keeps() {
        let pigment = Paint::opaque([200, 90, 30]);
        let empty = Paint::default();
        assert_eq!(pigment.mix(empty, 0.5).rgba8(), [100, 45, 15, 128]);
        assert_eq!(pigment.over(empty, 0.5).rgba8(), [200, 90, 30, 255]);
    }
}
