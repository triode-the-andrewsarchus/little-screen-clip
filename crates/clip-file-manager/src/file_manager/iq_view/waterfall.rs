use egui::{Color32, ColorImage, Pos2, Rect};
use little_clip_core::engineering::iq::{Spectrum, Waterfall};

pub(super) struct Image {
    pub data: Waterfall,
    pub pixels: ColorImage,
    pub low_dbfs: f32,
    pub high_dbfs: f32,
    pub has_absolute_axis: bool,
}

impl Image {
    pub(super) fn prepare(data: Waterfall, has_absolute_axis: bool) -> Self {
        let limits = contrast(&data);
        let time = data.rows.len();
        let frequencies = data.rows[0].bins_dbfs.len();
        let mut pixels = Vec::with_capacity(time * frequencies);
        for row in &data.rows {
            pixels.extend(row.bins_dbfs.iter().map(|&value| tone(value, limits)));
        }
        Self {
            data,
            pixels: ColorImage::new([frequencies, time], pixels),
            low_dbfs: limits.0,
            high_dbfs: limits.1,
            has_absolute_axis,
        }
    }
}

pub(super) struct Point<'a> {
    pub spectrum: &'a Spectrum,
    pub bin: usize,
    pub frequency_fraction: f32,
}

pub(super) fn point(frame: &Waterfall, rect: Rect, position: Pos2) -> Point<'_> {
    let time = time_fraction(rect, position);
    let row = ((time * frame.rows.len() as f32) as usize).min(frame.rows.len() - 1);
    let spectrum = &frame.rows[row];
    let frequency_fraction = ((position.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
    let bin = super::plot::bin_at_fraction(frequency_fraction, spectrum.bins_dbfs.len());
    Point {
        spectrum,
        bin,
        frequency_fraction,
    }
}

pub(super) fn time_fraction(rect: Rect, position: Pos2) -> f32 {
    ((position.y - rect.top()) / rect.height()).clamp(0.0, 1.0)
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

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{pos2, vec2};

    #[test]
    fn texture_and_pointer_share_frequency_columns_and_time_rows() {
        let row = |start, levels: [f32; 3]| Spectrum {
            samples: start..start + 8,
            sample_rate_hz: Some(250_000.0),
            center_hz: None,
            crosses_tuning: false,
            finite_samples: 8,
            noise_bandwidth_hz: None,
            bins_dbfs: levels.into(),
        };
        let waterfall = Waterfall {
            samples: 0..16,
            rows: vec![row(0, [-45.0, -90.0, -52.0]), row(8, [-76.0, -79.0, -16.0])],
            sampled: false,
        };
        let image = Image::prepare(waterfall, false);
        assert_eq!(image.pixels.size, [3, 2]);
        let rect = Rect::from_min_size(pos2(20.0, 30.0), vec2(300.0, 200.0));
        for (position, time_row, bin) in [(pos2(70.0, 80.0), 0, 0), (pos2(270.0, 180.0), 1, 2)] {
            let point = point(&image.data, rect, position);
            assert_eq!(point.spectrum.samples.start, time_row as u64 * 8);
            assert_eq!(point.bin, bin);
            assert_eq!(
                image.pixels.pixels[time_row * 3 + bin],
                tone(
                    point.spectrum.bins_dbfs[bin],
                    (image.low_dbfs, image.high_dbfs)
                )
            );
        }
    }
}
