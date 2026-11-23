use egui::{Color32, ColorImage, Pos2, Rect};
use little_clip_core::engineering::iq::{Spectrum, Waterfall};

pub(super) struct Image {
    pub data: Waterfall,
    pub pixels: ColorImage,
    pub low_dbfs: f32,
    pub high_dbfs: f32,
}

impl Image {
    pub(super) fn prepare(data: Waterfall) -> Self {
        let limits = contrast(&data);
        let time = data.rows.len();
        let frequencies = data.rows[0].bins_dbfs.len();
        let mut pixels = Vec::with_capacity(time * frequencies);
        for frequency in (0..frequencies).rev() {
            for row in &data.rows {
                let value = row.bins_dbfs[frequency];
                pixels.push(tone(value, limits));
            }
        }
        Self {
            data,
            pixels: ColorImage::new([time, frequencies], pixels),
            low_dbfs: limits.0,
            high_dbfs: limits.1,
        }
    }
}

pub(super) struct Point<'a> {
    pub spectrum: &'a Spectrum,
    pub bin: usize,
    pub frequency_fraction: f32,
}

pub(super) fn point(frame: &Waterfall, rect: Rect, position: Pos2) -> Point<'_> {
    let time = ((position.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
    let row = ((time * frame.rows.len() as f32) as usize).min(frame.rows.len() - 1);
    let spectrum = &frame.rows[row];
    let frequency_fraction = ((rect.bottom() - position.y) / rect.height()).clamp(0.0, 1.0);
    let bin = ((frequency_fraction * spectrum.bins_dbfs.len() as f32) as usize)
        .min(spectrum.bins_dbfs.len() - 1);
    Point {
        spectrum,
        bin,
        frequency_fraction,
    }
}

fn contrast(data: &Waterfall) -> (f32, f32) {
    let mut histogram = [0_u32; 221];
    let mut count = 0;
    for value in data.rows.iter().flat_map(|row| row.bins_dbfs.iter()) {
        if value.is_finite() {
            let bin = (value + 180.0).round().clamp(0.0, 220.0) as usize;
            histogram[bin] += 1;
            count += 1;
        }
    }
    let low = percentile(&histogram, count / 5).min(-25.0);
    let high = percentile(&histogram, count.saturating_sub(count / 400)).max(low + 25.0);
    (low, high)
}

fn percentile(histogram: &[u32], target: u32) -> f32 {
    let mut total = 0;
    for (index, count) in histogram.iter().enumerate() {
        total += count;
        if total >= target.max(1) {
            return index as f32 - 180.0;
        }
    }
    -65.0
}

fn tone(value: f32, (low, high): (f32, f32)) -> Color32 {
    if !value.is_finite() {
        return Color32::from_rgb(20, 21, 29);
    }
    const COLORS: [[u8; 3]; 5] = [
        [8, 12, 26],
        [24, 35, 76],
        [23, 99, 145],
        [56, 196, 179],
        [255, 196, 86],
    ];
    let value = (((value - low) / (high - low)).clamp(0.0, 1.0) * 4.0).min(3.999);
    let floor = value.floor() as usize;
    let fraction = value - floor as f32;
    let [red, green, blue] = std::array::from_fn(|channel| {
        let from = f32::from(COLORS[floor][channel]);
        let to = f32::from(COLORS[floor + 1][channel]);
        (from + (to - from) * fraction).round() as u8
    });
    Color32::from_rgb(red, green, blue)
}
