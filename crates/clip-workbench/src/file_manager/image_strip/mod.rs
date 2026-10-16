//! Visible image rows share gallery decoding and Image Studio analysis.
pub(in crate::file_manager) mod hash_art;
pub(in crate::file_manager) mod hilbert;
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
    pub hilbert_strip: Arc<hilbert::HilbertStrip>,
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
    let hilbert_strip = Arc::new(
        hilbert::HilbertStrip::read(&rgba, hilbert::STRIP_SEGMENTS, cancelled)
            .ok_or("Image preview unavailable")?,
    );
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
            hilbert_strip,
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
        .arg_content_file(path)
        .ok()?
        .timeout(Duration::from_secs(10))
        .run_cancellable(cancelled)
        .ok()?;
    let text = output.stdout_lossy();
    // sha256sum prefixes a backslash when escaping unusual filenames.
    let hash = text.trim_start_matches('\\').split_whitespace().next()?;
    (hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit())).then(|| hash.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_manager::filesystem::Entry;
    use crate::file_manager::shelf::preview::{Data, Ready};
    use crate::ui_appearance::WorkbenchColors;
    use egui::{Pos2, Rect, Vec2, pos2, vec2};

    fn test_analysis() -> Analysis {
        let mut img = image::RgbaImage::new(4, 4);
        for pixel in img.pixels_mut() {
            *pixel = image::Rgba([128, 64, 32, 255]);
        }
        let cancelled = AtomicBool::new(false);
        let strip = Arc::new(hilbert::HilbertStrip::read(&img, 8, &cancelled).unwrap());
        Analysis {
            dimensions: (100, 100),
            orientation: 1,
            hilbert_strip: strip,
            histogram: [[0; 256]; 3],
            exact: None,
            perceptual: "10000000000000000".into(),
            hash_art: [None, None],
            metadata: Default::default(),
        }
    }

    fn test_entry() -> Entry {
        Entry {
            path: std::path::PathBuf::from("/tmp/test.png"),
            name: "test.png".into(),
            directory: false,
            regular_file: true,
            system: None,
            typescript: false,
            symlink: false,
            hidden: false,
            size: "100 B".into(),
            size_bytes: 100,
            modified: "".into(),
            modified_time: None,
            warning: None,
            revision: 1,
            identity: None,
        }
    }

    #[test]
    fn hilbert_strip_hover_and_click_interaction() {
        let ctx = egui::Context::default();
        let analysis = test_analysis();
        let ready = Ready {
            data: Data {
                photo: Some(Arc::new(analysis)),
                ..Data::new("test.png".into(), "detail".into(), Icon::Image)
            },
            texture: None,
            spectrum_texture: None,
            hash_textures: [None, None],
        };
        let entry = test_entry();
        let identity = uuid::Uuid::new_v4();
        let mut hover = Hover::default();
        let colors =
            WorkbenchColors::from_palette(&little_clip_core::palette::AppPalette::default());

        let screen = Rect::from_min_size(Pos2::ZERO, Vec2::new(1200.0, 800.0));
        let row = Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 108.0));
        let body = Rect::from_min_max(row.min + vec2(6.0, 32.0), row.max - vec2(6.0, 10.0));
        let thumb = Rect::from_min_size(body.min, vec2(body.height() * 16.0 / 9.0, body.height()));
        let palette = ui::palette_rect(body, thumb);

        // Hover over the palette rect
        let strip_center = palette.center();
        let input = egui::RawInput {
            screen_rect: Some(screen),
            events: vec![egui::Event::PointerMoved(strip_center)],
            ..Default::default()
        };

        let _ = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.set_clip_rect(screen);
                show(
                    ui,
                    row,
                    &entry,
                    None,
                    identity,
                    Some(&ready),
                    &mut hover,
                    colors,
                );
            });
        });

        // Hover veil should be active!
        assert!(
            hover.is_veil_active(),
            "Veil texture should be active when hovering hilbert strip"
        );

        // Now simulate click on the palette strip to copy color
        let click_input = egui::RawInput {
            screen_rect: Some(screen),
            events: vec![
                egui::Event::PointerMoved(strip_center),
                egui::Event::PointerButton {
                    pos: strip_center,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: Default::default(),
                },
                egui::Event::PointerButton {
                    pos: strip_center,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: Default::default(),
                },
            ],
            ..Default::default()
        };

        let click_output = ctx.run(click_input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.set_clip_rect(screen);
                show(
                    ui,
                    row,
                    &entry,
                    None,
                    identity,
                    Some(&ready),
                    &mut hover,
                    colors,
                );
            });
        });

        assert!(
            click_output.platform_output.copied_text.starts_with('#'),
            "Clicking hilbert strip should copy hex color, got {:?}",
            click_output.platform_output.copied_text
        );

        // Now hover over thumb: veil should NOT be active
        let thumb_input = egui::RawInput {
            screen_rect: Some(screen),
            events: vec![egui::Event::PointerMoved(thumb.center())],
            ..Default::default()
        };
        let _ = ctx.run(thumb_input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.set_clip_rect(screen);
                show(
                    ui,
                    row,
                    &entry,
                    None,
                    identity,
                    Some(&ready),
                    &mut hover,
                    colors,
                );
            });
        });

        assert!(
            !hover.is_veil_active(),
            "Veil texture should NOT be active when hovering thumbnail"
        );
    }

    #[test]
    fn dimmed_color_transitions_across_frames_when_switching_segment_lightness() {
        let ctx = egui::Context::default();
        let mut img = image::RgbaImage::new(10, 2);
        // Left half (segment 0) is white (OKLab L >= 0.6 -> INK: [24, 20, 15])
        for x in 0..5 {
            for y in 0..2 {
                img.put_pixel(x, y, image::Rgba([255, 255, 255, 255]));
            }
        }
        // Right half (segment 1) is black (OKLab L < 0.6 -> PAPER: [247, 241, 230])
        for x in 5..10 {
            for y in 0..2 {
                img.put_pixel(x, y, image::Rgba([0, 0, 0, 255]));
            }
        }
        let cancelled = AtomicBool::new(false);
        let strip = Arc::new(hilbert::HilbertStrip::read(&img, 2, &cancelled).unwrap());
        let analysis = Analysis {
            dimensions: (10, 2),
            orientation: 1,
            hilbert_strip: strip,
            histogram: [[0; 256]; 3],
            exact: None,
            perceptual: "10000000000000000".into(),
            hash_art: [None, None],
            metadata: Default::default(),
        };
        let ready = Ready {
            data: Data {
                photo: Some(Arc::new(analysis)),
                ..Data::new("test.png".into(), "detail".into(), Icon::Image)
            },
            texture: None,
            spectrum_texture: None,
            hash_textures: [None, None],
        };
        let entry = test_entry();
        let identity = uuid::Uuid::new_v4();
        let mut hover = Hover::default();
        let colors =
            WorkbenchColors::from_palette(&little_clip_core::palette::AppPalette::default());

        let screen = Rect::from_min_size(Pos2::ZERO, Vec2::new(1200.0, 800.0));
        let row = Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 108.0));
        let body = Rect::from_min_max(row.min + vec2(6.0, 32.0), row.max - vec2(6.0, 10.0));
        let thumb = Rect::from_min_size(body.min, vec2(body.height() * 16.0 / 9.0, body.height()));
        let palette = ui::palette_rect(body, thumb);

        // Frame 1: Hover over left segment (INK: [24, 20, 15])
        let left_pos = pos2(palette.left() + palette.width() * 0.25, palette.center().y);
        let input1 = egui::RawInput {
            screen_rect: Some(screen),
            events: vec![egui::Event::PointerMoved(left_pos)],
            ..Default::default()
        };
        let _ = ctx.run(input1, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.set_clip_rect(screen);
                show(
                    ui,
                    row,
                    &entry,
                    None,
                    identity,
                    Some(&ready),
                    &mut hover,
                    colors,
                );
            });
        });

        let c1 = hover.current_dimmed_color().expect("dimmed color active");
        assert_eq!(c1, [24.0, 20.0, 15.0]);

        // Frame 2: Switch preview to right segment (PAPER: [247, 241, 230])
        let right_pos = pos2(palette.left() + palette.width() * 0.75, palette.center().y);
        let input2 = egui::RawInput {
            screen_rect: Some(screen),
            events: vec![egui::Event::PointerMoved(right_pos)],
            ..Default::default()
        };
        let _ = ctx.run(input2, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.set_clip_rect(screen);
                show(
                    ui,
                    row,
                    &entry,
                    None,
                    identity,
                    Some(&ready),
                    &mut hover,
                    colors,
                );
            });
        });

        let c2 = hover.current_dimmed_color().expect("dimmed color active");
        // Must NOT snap immediately to 247! Should transition smoothly over ~5-6 frames.
        assert!(
            c2[0] > 30.0 && c2[0] < 120.0,
            "Frame 2 should make moderate progress, got {:?}",
            c2
        );

        let mut prev_c = c2;
        // Verify progressive smoothing across frames 3 through 6
        for frame in 3..=6 {
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(screen),
                    events: vec![egui::Event::PointerMoved(right_pos)],
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        ui.set_clip_rect(screen);
                        show(
                            ui,
                            row,
                            &entry,
                            None,
                            identity,
                            Some(&ready),
                            &mut hover,
                            colors,
                        );
                    });
                },
            );
            let curr_c = hover.current_dimmed_color().expect("dimmed color active");
            assert!(
                curr_c[0] > prev_c[0],
                "Frame {frame} should increase towards target: prev {:?}, curr {:?}",
                prev_c,
                curr_c
            );
            prev_c = curr_c;
        }
        // At frame 6, color is well into the transition towards 247
        assert!(
            prev_c[0] > 150.0 && prev_c[0] < 240.0,
            "By frame 6 color should be smoothly approaching 247, got {:?}",
            prev_c
        );
    }
}
