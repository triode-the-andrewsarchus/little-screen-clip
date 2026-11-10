//! Explicit CPU measurements of the real prepared-audio views; no device or content I/O.
use super::*;
use egui::{Pos2, Rect, Vec2};
use little_clip_core::audio::{FullAudioAnalysis, build_peak_pyramid};
use std::hash::{Hash, Hasher};
use std::time::Instant;

fn prepared_analysis() -> Arc<FullAudioAnalysis> {
    let samples: Vec<f32> = (0..1_048_576)
        .map(|index| (index as f32 * 0.071).sin() * 0.8)
        .collect();
    let second: Vec<f32> = samples.iter().map(|sample| sample * -0.7).collect();
    let (a, b, c) = build_peak_pyramid(&samples, 44_100.0);
    let (d, e, f) = build_peak_pyramid(&second, 44_100.0);
    Arc::new(FullAudioAnalysis {
        channels: 2,
        sample_rate: 44_100,
        total_frames: samples.len(),
        duration_ms: samples.len() as u64 * 1000 / 44_100,
        ch1_samples: samples.into(),
        ch2_samples: Some(second.into()),
        peaks_lvl0_ch1: a,
        peaks_lvl1_ch1: b,
        peaks_lvl2_ch1: c,
        peaks_lvl0_ch2: Some(d),
        peaks_lvl1_ch2: Some(e),
        peaks_lvl2_ch2: Some(f),
        is_spectrogram_ready: true,
        is_transients_ready: true,
        ..Default::default()
    })
}

fn paint_stats(primitives: &[egui::ClippedPrimitive]) -> (u64, usize, usize) {
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    let (mut vertices, mut indices) = (0, 0);
    for primitive in primitives {
        for coordinate in [
            primitive.clip_rect.min.x,
            primitive.clip_rect.min.y,
            primitive.clip_rect.max.x,
            primitive.clip_rect.max.y,
        ] {
            coordinate.to_bits().hash(&mut hash);
        }
        let egui::epaint::Primitive::Mesh(mesh) = &primitive.primitive else {
            panic!("prepared audio measurement must not submit a GPU callback");
        };
        mesh.texture_id.hash(&mut hash);
        mesh.indices.hash(&mut hash);
        for vertex in &mesh.vertices {
            vertex.pos.x.to_bits().hash(&mut hash);
            vertex.pos.y.to_bits().hash(&mut hash);
            vertex.uv.x.to_bits().hash(&mut hash);
            vertex.uv.y.to_bits().hash(&mut hash);
            vertex.color.to_array().hash(&mut hash);
        }
        vertices += mesh.vertices.len();
        indices += mesh.indices.len();
    }
    (hash.finish(), vertices, indices)
}

#[test]
#[ignore = "explicit prepared-audio CPU frame/memory comparison; use scripts/waveform_frame_comparison.py"]
fn measure_prepared_audio_frames() {
    let analysis = prepared_analysis();
    let mut studio = AudioPreviewState::from_completed_source(
        PathBuf::from("__prepared_frame_fixture__.wav"),
        "Prepared waveform".into(),
        None,
        Arc::clone(&analysis),
    );
    let mut rolling = RollingAudioStripState::empty_test_strip();
    rolling.duration_sec = studio.duration_sec;
    rolling.sample_rate = analysis.sample_rate;
    rolling.channels = 1;
    rolling.mono_samples = analysis.ch1_samples.to_vec();
    rolling.peaks_lvl0 = analysis.peaks_lvl0_ch1.clone();
    rolling.peaks_lvl1 = analysis.peaks_lvl1_ch1.clone();
    rolling.peaks_lvl2 = analysis.peaks_lvl2_ch1.clone();
    rolling.is_loaded = true;
    let palette = AppPalette::default();
    for width in [900.0, 1800.0] {
        for density in [1.0, 2.0] {
            // The strip always shows its complete snapshot; studio exercises peak
            // summaries and the sample-detail path at distinct zoom levels.
            for (kind, seconds) in [
                ("rolling", studio.duration_sec),
                ("studio", studio.duration_sec),
                ("studio", 2.0),
                ("studio", 0.001),
            ] {
                let ctx = egui::Context::default();
                crate::ui_ext::configure_workbench_style(&ctx);
                egui_extras::install_image_loaders(&ctx);
                ctx.set_pixels_per_point(density);
                studio.view_start_sec = 0.0;
                studio.view_duration_sec = seconds;
                let mut times = Vec::new();
                let mut stats = None;
                for frame in 0..110 {
                    let started = Instant::now();
                    let output = ctx.run(
                        egui::RawInput {
                            screen_rect: Some(Rect::from_min_size(
                                Pos2::ZERO,
                                Vec2::new(width, 760.0),
                            )),
                            time: Some(1.0),
                            ..Default::default()
                        },
                        |ctx| {
                            egui::CentralPanel::default().show(ctx, |ui| {
                                if kind == "studio" {
                                    studio.render_ui(ui, ctx, &palette, &mut false);
                                } else {
                                    rolling.render_ui(ui, ctx, &palette, &mut None);
                                }
                            });
                        },
                    );
                    let primitives = ctx.tessellate(output.shapes, output.pixels_per_point);
                    let elapsed = started.elapsed().as_secs_f64() * 1000.0;
                    if frame >= 10 {
                        times.push(elapsed);
                    }
                    if frame == 10 {
                        stats = Some(paint_stats(&primitives));
                    }
                    if frame == 109 {
                        assert_eq!(stats.unwrap(), paint_stats(&primitives));
                    }
                }
                if kind == "studio" {
                    assert_eq!(studio.view_duration_sec, seconds, "fixture zoom changed");
                }
                times.sort_by(f64::total_cmp);
                let (fingerprint, vertices, indices) = stats.unwrap();
                println!(
                    "WAVEFORM_FRAME_COST {}",
                    serde_json::json!({
                        "kind": kind, "width": width, "density": density, "seconds": seconds,
                        "frames": times.len(), "median_ms": times[50], "p95_ms": times[94],
                        "fingerprint": fingerprint, "vertices": vertices, "indices": indices,
                        "geometry_bytes": vertices * std::mem::size_of::<egui::epaint::Vertex>() + indices * 4,
                        "debug_assertions": cfg!(debug_assertions),
                    })
                );
            }
        }
    }
}
