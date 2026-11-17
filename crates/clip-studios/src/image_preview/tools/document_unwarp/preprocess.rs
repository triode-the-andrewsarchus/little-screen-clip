use super::DeskewMethod;
use image::{Rgb, RgbImage, RgbaImage};

pub(super) fn prepare_input(image: &RgbaImage, method: DeskewMethod) -> Result<Vec<f32>, String> {
    if image.width() < 2 || image.height() < 2 {
        return Err("Document image must be at least 2 × 2 pixels".into());
    }
    let (width, height) = method.input_size();
    let rgb = RgbImage::from_fn(image.width(), image.height(), |x, y| {
        let p = image.get_pixel(x, y);
        let alpha = p[3] as f32 / 255.0;
        Rgb(std::array::from_fn(|c| {
            (p[c] as f32 * alpha + 255.0 * (1.0 - alpha)).round() as u8
        }))
    });
    let count = width * height;
    let mut output = vec![0.0; count * 3];
    if method == DeskewMethod::SyntheticDoc {
        // PIL/torchvision's antialiased bilinear resize, followed by [-1, 1].
        let horizontal = resize_axis(&rgb, width as u32, true);
        let resized = resize_axis(&horizontal, height as u32, false);
        for (i, p) in resized.pixels().enumerate() {
            for c in 0..3 {
                output[c * count + i] = p[c] as f32 / 127.5 - 1.0;
            }
        }
    } else {
        // OpenCV INTER_LINEAR on float RGB, without antialiasing or u8 rounding.
        let sample = |channel: usize, sx: f32, sy: f32| {
            let sx = sx.clamp(0.0, (rgb.width() - 1) as f32);
            let sy = sy.clamp(0.0, (rgb.height() - 1) as f32);
            let x0 = sx.floor() as u32;
            let y0 = sy.floor() as u32;
            let x1 = (x0 + 1).min(rgb.width() - 1);
            let y1 = (y0 + 1).min(rgb.height() - 1);
            let fx = sx - x0 as f32;
            let fy = sy - y0 as f32;
            let top = rgb.get_pixel(x0, y0)[channel] as f32 * (1.0 - fx)
                + rgb.get_pixel(x1, y0)[channel] as f32 * fx;
            let bottom = rgb.get_pixel(x0, y1)[channel] as f32 * (1.0 - fx)
                + rgb.get_pixel(x1, y1)[channel] as f32 * fx;
            (top * (1.0 - fy) + bottom * fy) / 255.0
        };
        for y in 0..height {
            for x in 0..width {
                let sx = (x as f32 + 0.5) * rgb.width() as f32 / width as f32 - 0.5;
                let sy = (y as f32 + 0.5) * rgb.height() as f32 / height as f32 - 0.5;
                for c in 0..3 {
                    output[c * count + y * width + x] = sample(c, sx, sy);
                }
            }
        }
    }
    Ok(output)
}

// Match Pillow's antialiased bilinear kernel and its per-pass u8 rounding.
// image::resize keeps a float intermediate and reverses the pass order, which
// changes the neural input on camera-sized images. Coefficients use 22 bits,
// as in Pillow's 8-bit resampler; intermediate storage stays width × source H.
fn resize_axis(source: &RgbImage, length: u32, horizontal: bool) -> RgbImage {
    let old_length = if horizontal {
        source.width()
    } else {
        source.height()
    };
    if old_length == length {
        return source.clone();
    }
    let weights = resize_weights(old_length, length);
    let (width, height) = if horizontal {
        (length, source.height())
    } else {
        (source.width(), length)
    };
    RgbImage::from_fn(width, height, |x, y| {
        let (index, other) = if horizontal { (x, y) } else { (y, x) };
        let (start, coefficients) = &weights[index as usize];
        let mut sums = [1_i64 << 21; 3];
        for (offset, &weight) in coefficients.iter().enumerate() {
            let along = *start + offset as u32;
            let pixel = if horizontal {
                source.get_pixel(along, other)
            } else {
                source.get_pixel(other, along)
            };
            for c in 0..3 {
                sums[c] += pixel[c] as i64 * weight;
            }
        }
        Rgb(sums.map(|sum| (sum >> 22).clamp(0, 255) as u8))
    })
}

fn resize_weights(source: u32, target: u32) -> Vec<(u32, Vec<i64>)> {
    let scale = source as f64 / target as f64;
    let support = scale.max(1.0);
    (0..target)
        .map(|index| {
            let center = (index as f64 + 0.5) * scale;
            let first = ((center - support + 0.5) as i64).max(0) as u32;
            let last = (((center + support + 0.5) as i64).max(0) as u32).min(source);
            let mut coefficients: Vec<_> = (first..last)
                .map(|pixel| (1.0 - ((pixel as f64 - center + 0.5) / support).abs()).max(0.0))
                .collect();
            let total: f64 = coefficients.iter().sum();
            for value in &mut coefficients {
                *value /= total;
            }
            (
                first,
                coefficients
                    .into_iter()
                    .map(|value| (value * (1_u32 << 22) as f64 + 0.5) as i64)
                    .collect(),
            )
        })
        .collect()
}
