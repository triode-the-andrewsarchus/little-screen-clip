//! Offline CPU plot benchmark for a small and a large waveform.
//! Set SCOPE_BENCH_SMALL and SCOPE_BENCH_LARGE to supported test captures.
//! Optionally set SCOPE_BENCH_MATH to an expression such as `> 1V`.
//! Run cargo test -p clip-file-manager --lib scope_frame_benchmark -- --ignored --nocapture

use super::*;
use std::time::Instant;

#[test]
#[ignore = "manual frame-time benchmark using explicit capture paths"]
fn scope_frame_benchmark() {
    let math = std::env::var("SCOPE_BENCH_MATH").unwrap_or_default();
    for key in ["SCOPE_BENCH_SMALL", "SCOPE_BENCH_LARGE"] {
        let path = std::env::var_os(key).expect("set the waveform benchmark input paths");
        let path = Path::new(&path);
        let cancel = AtomicBool::new(false);
        let start = Instant::now();
        let mut viewer = Viewer::load(path, &cancel).unwrap();
        viewer.math.clone_from(&math);
        let opened = start.elapsed();
        let start = Instant::now();
        viewer.index = Some(viewer.recording.index(&cancel).unwrap());
        let indexed = start.elapsed();
        viewer.index_started = true;
        let mut copies = Copies::default();
        let ctx = Context::default();
        let domain = viewer.domain.end - viewer.domain.start;
        let step = viewer.overview.channels[0].interval_seconds;
        for (label, width) in [("wide", domain * 0.75), ("detail", step * 512.0)] {
            let mut latencies = Vec::new();
            let mut painting = Vec::new();
            let mut tessellation = Vec::new();
            for frame in 0..130 {
                let offset = (domain - width) * f64::from(frame) / 130.0;
                let left = viewer.domain.start + offset;
                viewer.span = viewer.clamp_span(left..left + width);
                if label == "detail" {
                    // Make each measured frame show the new capture window.
                    // The native viewer queues this read off its UI thread.
                    viewer.detail = Some((
                        viewer.span.clone(),
                        PlotData::Raw(
                            reads::detail(&viewer.recording, viewer.span.clone(), &cancel).unwrap(),
                        ),
                    ));
                }
                let now = Instant::now();
                let output = clip_ui::test_support::run_frame(
                    &ctx,
                    egui::RawInput {
                        screen_rect: Some(Rect::from_min_size(
                            pos2(0.0, 0.0),
                            egui::vec2(1280.0, 800.0),
                        )),
                        ..Default::default()
                    },
                    |ctx| {
                        ctx.root_ui(|ui| {
                            egui::CentralPanel::default().show(ui, |ui| {
                                viewer.content(ui, &mut copies, None, true);
                            });
                        });
                    },
                );
                let draw = now.elapsed();
                let tessellate = Instant::now();
                let shapes = ctx.tessellate(output.shapes, output.pixels_per_point);
                assert!(!shapes.is_empty());
                std::hint::black_box(shapes);
                if frame >= 10 {
                    latencies.push(now.elapsed());
                    painting.push(draw);
                    tessellation.push(tessellate.elapsed());
                }
            }
            for series in [&mut latencies, &mut painting, &mut tessellation] {
                series.sort_unstable();
            }
            let p95 = latencies.len() * 95 / 100;
            println!(
                "{key} {label} math={math:?}: preview {opened:?}, index {indexed:?}, CPU frame p95 {:?}, paint p95 {:?}, tessellate p95 {:?}",
                latencies[p95], painting[p95], tessellation[p95]
            );
        }
    }
}
