//! Visible image rows share gallery decoding and Image Studio analysis.
pub(in crate::file_manager) mod hash_art;
mod hover;
mod ui;
use super::media::PreviewData;
use crate::{file_source::FileSource, icons::Icon};
pub(super) use hover::Hover;
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
pub(super) use ui::show;

pub(super) struct Analysis {
    pub dimensions: (u32, u32),
    pub orientation: u8,
    pub palette: Vec<egui::Color32>,
    pub histogram: [[u32; 256]; 3],
    pub exact: Option<String>,
    pub perceptual: String,
    pub hash_art: [Option<little_clip_core::ImageThumbnail>; 2],
    pub metadata: little_clip_ops::metadata::ImageMetadataSummary,
}

pub(super) fn load(path: &Path, revision: u64, cancelled: &AtomicBool) -> PreviewData {
    let mut data = PreviewData::new(
        super::display_path(path),
        "Image preview unavailable".into(),
        Icon::Image,
    );
    match analyze(path, revision, cancelled) {
        Ok((image, analysis)) => {
            data.detail = format!("{} × {}", analysis.dimensions.0, analysis.dimensions.1);
            data.image = Some(image);
            data.photo = Some(Arc::new(analysis));
        }
        Err(error) => data.detail = error,
    }
    data
}

fn analyze(
    path: &Path,
    revision: u64,
    cancelled: &AtomicBool,
) -> Result<(little_clip_core::ImageThumbnail, Analysis), String> {
    let source = FileSource::capture(path)?;
    let dimensions = super::gallery::metadata::probe(path, revision)?;
    if cancelled.load(Ordering::Relaxed) {
        return Err("Cancelled".into());
    }
    let thumbnail = super::gallery::thumbnail::load(path, 256, dimensions.orientation, cancelled)
        .ok_or("Image preview unavailable")?;
    let rgba = straight_pixels(&thumbnail).ok_or("Invalid image thumbnail")?;
    let palette = crate::image_preview::extract_palette(&rgba, 9)
        .0
        .into_iter()
        .map(|color| color.color)
        .collect();
    let mut histogram = [[0; 256]; 3];
    for pixel in rgba.pixels().filter(|pixel| pixel[3] >= 64) {
        for channel in 0..3 {
            histogram[channel][usize::from(pixel[channel])] += 1;
        }
    }
    let metadata = crate::file_source::read_file_bytes(path, 2 * 1024 * 1024, false, true)
        .map(|bytes| little_clip_ops::metadata::extract_image_metadata(&bytes))
        .unwrap_or_default();
    let perceptual = difference_hash(&rgba);
    if cancelled.load(Ordering::Relaxed) {
        return Err("Cancelled".into());
    }
    let exact = exact_hash(path, cancelled);
    source.check()?;
    super::filesystem::check_revision(path, revision)?;
    if cancelled.load(Ordering::Relaxed) {
        return Err("Cancelled".into());
    }
    let hash_art = [
        exact.as_deref().and_then(hash_art::exact),
        hash_art::perceptual(&perceptual),
    ];
    let size = if (5..=8).contains(&dimensions.orientation) {
        (dimensions.height, dimensions.width)
    } else {
        (dimensions.width, dimensions.height)
    };
    Ok((
        thumbnail,
        Analysis {
            dimensions: size,
            orientation: dimensions.orientation,
            palette,
            histogram,
            exact,
            perceptual,
            metadata,
            hash_art,
        },
    ))
}

fn straight_pixels(thumbnail: &little_clip_core::ImageThumbnail) -> Option<image::RgbaImage> {
    let mut pixels = thumbnail.premultiplied_rgba.clone();
    for pixel in pixels.chunks_exact_mut(4) {
        let alpha = u32::from(pixel[3]);
        if alpha > 0 {
            for channel in &mut pixel[..3] {
                *channel = ((u32::from(*channel) * 255 + alpha / 2) / alpha).min(255) as u8;
            }
        }
    }
    image::RgbaImage::from_raw(thumbnail.width, thumbnail.height, pixels)
}

// Version 1: four format bits, twelve coarse mean-color bits, then 64 dHash bits.
// Color bins and neighboring luminance changes describe appearance, not file identity.
fn difference_hash(image: &image::RgbaImage) -> String {
    let gray = image::DynamicImage::ImageRgba8(image.clone()).into_luma8();
    let small = image::imageops::resize(&gray, 9, 8, image::imageops::FilterType::Triangle);
    let mut hash = 0_u64;
    for y in 0..8 {
        for x in 0..8 {
            hash = (hash << 1) | u64::from(small[(x, y)][0] > small[(x + 1, y)][0]);
        }
    }
    let mut sums = [0_u64; 3];
    let mut weight = 0_u64;
    for pixel in image.pixels() {
        let alpha = u64::from(pixel[3]);
        weight += alpha;
        for channel in 0..3 {
            sums[channel] += u64::from(pixel[channel]) * alpha;
        }
    }
    let color = sums.map(|sum| (sum / weight.max(1)) as u8 >> 4);
    format!("1{:x}{:x}{:x}{hash:016x}", color[0], color[1], color[2])
}

// The same bounded helper used by the file SHA-256 pipeline, never a shell.
fn exact_hash(path: &Path, cancelled: &AtomicBool) -> Option<String> {
    let output = little_clip_core::process::CommandRequest::new("sha256sum")
        .arg("--")
        .arg(path.as_os_str())
        .timeout(Duration::from_secs(10))
        .run_cancellable(cancelled)
        .ok()?;
    let text = output.stdout_lossy();
    // sha256sum prefixes a backslash when escaping unusual filenames.
    let hash = text.trim_start_matches('\\').split_whitespace().next()?;
    (hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit())).then(|| hash.to_owned())
}
