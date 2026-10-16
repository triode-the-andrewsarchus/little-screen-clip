//! Bounded FFT windows across a track, rendered by Audio Studio's spectral engine.
use little_clip_core::ImageThumbnail;
use little_clip_ops::hf_audio::{HfSpectrogramConfig, render_hf_viewport_spectrogram};
use rodio::Source;
use std::{
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

pub(super) fn render(
    path: &Path,
    fallback_duration: f32,
    cancelled: &AtomicBool,
) -> Result<ImageThumbnail, String> {
    let image = render_fft(path, fallback_duration, cancelled)?;
    let width = image.width as usize;
    let source_height = image.height as usize;
    let height = 64;
    let mut pixels = vec![0; width * height * 4];
    // Preserve narrow spectral lines when compressing hundreds of FFT bins into
    // a short strip. Bilinear GPU sampling can skip a tone's bin entirely.
    for row in 0..height {
        let first = row * source_height / height;
        let end = ((row + 1) * source_height / height).max(first + 1);
        for column in 0..width {
            if let Some(peak) = (first..end)
                .map(|source_row| {
                    let offset = (source_row * width + column) * 4;
                    &image.premultiplied_rgba[offset..offset + 4]
                })
                .max_by_key(|pixel| u32::from(pixel[0]) + u32::from(pixel[1]) + u32::from(pixel[2]))
            {
                let offset = (row * width + column) * 4;
                pixels[offset..offset + 4].copy_from_slice(peak);
            }
        }
    }
    Ok(ImageThumbnail {
        width: image.width,
        height: height as u32,
        premultiplied_rgba: pixels,
    })
}

fn render_fft(
    path: &Path,
    fallback_duration: f32,
    cancelled: &AtomicBool,
) -> Result<ImageThumbnail, String> {
    let mut decoder = super::decode_file(path)?;
    let rate = decoder.sample_rate().get();
    let channels = usize::from(decoder.channels().get());
    let duration = decoder
        .total_duration()
        .map_or(fallback_duration, |duration| duration.as_secs_f32());
    if !duration.is_finite() || duration <= 0.0 {
        return Err("Unknown track duration".into());
    }
    let columns = 512;
    let config = HfSpectrogramConfig {
        fft_size: if duration >= 600.0 { 512 } else { 1024 },
        ..Default::default()
    };
    let started = Instant::now();
    // Retain only one FFT window per output column, regardless of track length.
    let mut windows = vec![0.0; columns * config.fft_size];
    for (column, window) in windows.chunks_exact_mut(config.fft_size).enumerate() {
        check_budget(cancelled, started)?;
        let center = (column as f64 / columns as f64 * duration as f64 * rate as f64) as usize;
        let first = center.saturating_sub(config.fft_size / 2);
        decoder
            .try_seek(Duration::from_secs_f64(first as f64 / rate as f64))
            .map_err(|error| error.to_string())?;
        let padding = (config.fft_size / 2).saturating_sub(center);
        for frame in &mut window[padding..] {
            let mut sum = 0.0;
            for _ in 0..channels {
                sum += decoder.next().unwrap_or(0.0);
            }
            *frame = sum / channels as f32;
        }
    }
    check_budget(cancelled, started)?;
    // Packed windows have evenly spaced centers. The shared renderer applies
    // the same Hann window, FFT normalization, dB range and Inferno colors as Studio.
    let (width, height, rgba) = render_hf_viewport_spectrogram(
        &windows,
        rate,
        config.fft_size as f32 * 0.5 / rate as f32,
        windows.len() as f32 / rate as f32,
        columns,
        config,
    );
    check_budget(cancelled, started)?;
    Ok(ImageThumbnail {
        width: width as u32,
        height: height as u32,
        premultiplied_rgba: rgba,
    })
}

fn check_budget(cancelled: &AtomicBool, started: Instant) -> Result<(), String> {
    if cancelled.load(Ordering::Relaxed) {
        return Err("Music preview cancelled".into());
    }
    if started.elapsed() > Duration::from_secs(10) {
        return Err("Spectrum exceeded the ten-second preview budget".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_matches_studio_fft_and_preserves_time_and_stereo_content() {
        let (_directory, path) = crate::audio_preview::tests::generate_fixture_wav("spectrum", 0.4);
        let mut bytes = std::fs::read(&path).unwrap();
        // Silence the first half and leave only the right channel in the second.
        let frames = (bytes.len() - 44) / 4;
        for (index, frame) in bytes[44..].chunks_exact_mut(4).enumerate() {
            frame[..2].fill(0);
            if index < frames / 2 {
                frame.fill(0);
            }
        }
        std::fs::write(&path, bytes).unwrap();
        let image = render_fft(&path, 0.4, &AtomicBool::new(false)).unwrap();
        assert_eq!((image.width, image.height), (512, 512));
        let samples: Vec<_> = super::super::decode_file(&path).unwrap().collect();
        let mono: Vec<_> = samples
            .chunks_exact(2)
            .map(|frame| (frame[0] + frame[1]) * 0.5)
            .collect();
        let (_, _, reference) = render_hf_viewport_spectrogram(
            &mono,
            44100,
            0.0,
            0.4,
            512,
            HfSpectrogramConfig::default(),
        );
        let mean_error = image
            .premultiplied_rgba
            .iter()
            .zip(&reference)
            .map(|(&a, &b)| f64::from(a.abs_diff(b)))
            .sum::<f64>()
            / reference.len() as f64;
        assert!(
            mean_error < 2.0,
            "Studio FFT mean pixel difference: {mean_error}"
        );
        let pixel = |column: usize, bin: usize| {
            let offset = ((511 - bin) * 512 + column) * 4;
            &image.premultiplied_rgba[offset..offset + 3]
        };
        assert!(
            pixel(384, 20).iter().map(|&v| u32::from(v)).sum::<u32>()
                > pixel(128, 20).iter().map(|&v| u32::from(v)).sum::<u32>()
        );
        let strip = render(&path, 0.4, &AtomicBool::new(false)).unwrap();
        assert_eq!((strip.width, strip.height), (512, 64));
        let bin_row = (511 - 20) / 8;
        let offset = (bin_row * 512 + 384) * 4;
        assert!(
            strip.premultiplied_rgba[offset] > 100,
            "narrow tone must survive strip reduction"
        );
        assert!(
            render(&path, 0.4, &AtomicBool::new(true))
                .unwrap_err()
                .contains("cancelled")
        );
    }
}
