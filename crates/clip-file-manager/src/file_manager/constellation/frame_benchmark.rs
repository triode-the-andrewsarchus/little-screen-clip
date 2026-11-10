//! Manual CPU frame benchmark. Run with the workbench optimized, e.g.:
//! cargo test -p clip-workbench --lib constellation_frame_benchmark \
//!   --config 'profile.dev.package.clip-workbench.opt-level=3' -- --ignored --nocapture
//! Optional CONSTELLATION_BENCH_CACHE_COPY must name a disposable cache snapshot:
//! the normal DB loader may migrate it. Source audio is never opened by this test.
use super::*;
use std::time::Instant;

#[test]
#[ignore = "manual optimized frame-time benchmark"]
fn constellation_frame_benchmark() {
    let samples = if let Ok(path) = std::env::var("CONSTELLATION_BENCH_CACHE_COPY") {
        super::super::db::ConstellationDb::open_path(path.into())
            .unwrap()
            .load_samples(std::path::Path::new("/mnt/rust/music/Music/Samples"))
            .unwrap()
    } else {
        (0..72_000)
            .map(|index| {
                let mut sample = super::tests::sample_fixture(index);
                sample.x = (index % 317) as f32 / 158.0 - 1.0;
                sample.y = (index / 317) as f32 / 113.0 - 1.0;
                sample.pitch_hz = note_to_hz(36.0 + (index % 61) as f32);
                sample.pitch_clarity = 1.0;
                sample.duration_seconds = 0.02 + (index % 200) as f32 * 0.025;
                sample.source_revision =
                    Some(super::super::classifier::PITCH_ANALYSIS_REVISION.to_vec());
                sample
            })
            .collect()
    };
    let mut state = ConstellationState::default();
    state.samples = samples;
    state.sample_sets = super::super::multisample::SampleSets::build(&state.samples);
    state.pack_shelf_open = false;
    let size = Vec2::new(1920.0, 1080.0);
    let palette = little_clip_core::palette::AppPalette::default();
    let iterations = std::env::var("CONSTELLATION_BENCH_FRAMES")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(30)
        .max(1);
    for case in [
        "flat",
        "pitch",
        "orbit",
        "trail",
        "filtered",
        "pitch_range",
        "multisamples",
    ] {
        let ctx = egui::Context::default();
        state.pitch_height_enabled = case != "flat";
        state.max_duration = if case == "filtered" { 0.25 } else { 0.0 };
        state.pitch_range = (case == "pitch_range").then_some((48.0, 72.0));
        state.multisamples_only = case == "multisamples";
        state.camera = Default::default();
        state.history_trail.clear();
        if case == "trail" {
            for index in (0..state.samples.len()).step_by((state.samples.len() / 32).max(1)) {
                state.record_played_sample(&state.samples[index].clone());
            }
        }
        let mut count_times = Vec::new();
        let mut layout_times = Vec::new();
        let mut build = Vec::new();
        let mut tessellate = Vec::new();
        let mut vertices = 0;
        let mut indices = 0;
        for frame in 0..iterations + 5 {
            if case == "orbit" {
                state.camera.orbit(Vec2::new(2.0, 0.2));
            }
            let start = Instant::now();
            let output = super::tests::render_frame(&ctx, &mut state, &palette, size);
            let built = start.elapsed().as_secs_f64() * 1000.0;
            let start = Instant::now();
            let primitives = ctx.tessellate(output.shapes, output.pixels_per_point);
            let tessellated = start.elapsed().as_secs_f64() * 1000.0;
            vertices = 0;
            indices = 0;
            for primitive in &primitives {
                if let egui::epaint::Primitive::Mesh(mesh) = &primitive.primitive {
                    vertices += mesh.vertices.len();
                    indices += mesh.indices.len();
                }
            }
            std::hint::black_box(primitives);
            let start = Instant::now();
            std::hint::black_box(state.sample_count_label());
            let counted = start.elapsed().as_secs_f64() * 1000.0;
            let viewport = state.map_viewport.unwrap();
            let start = Instant::now();
            std::hint::black_box(state.layout_map_points(
                viewport,
                viewport,
                map_point_radius(viewport, state.samples.len(), state.zoom),
                "",
            ));
            let laid_out = start.elapsed().as_secs_f64() * 1000.0;
            if frame >= 5 {
                count_times.push(counted);
                layout_times.push(laid_out);
                build.push(built);
                tessellate.push(tessellated);
            }
        }
        count_times.sort_by(f64::total_cmp);
        layout_times.sort_by(f64::total_cmp);
        build.sort_by(f64::total_cmp);
        tessellate.sort_by(f64::total_cmp);
        println!(
            "{}",
            serde_json::json!({
                "case": case, "samples": state.samples.len(), "sets": state.sample_sets.sets.len(),
                "frames": iterations, "build_p50_ms": build[iterations / 2],
                "build_p95_ms": build[(iterations * 95 / 100).min(iterations - 1)],
                "tessellate_p50_ms": tessellate[iterations / 2],
                "status_p50_ms": count_times[iterations / 2],
                "layout_probe_p50_ms": layout_times[iterations / 2],
                "vertices": vertices, "indices": indices,
                "mesh_mb": (vertices * std::mem::size_of::<egui::epaint::Vertex>() + indices * 4) as f64 / 1_000_000.0,
            })
        );
    }
}
