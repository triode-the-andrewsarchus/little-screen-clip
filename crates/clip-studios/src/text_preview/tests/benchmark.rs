use super::*;

#[test]
#[ignore = "headless CPU benchmark; run explicitly with --release and --nocapture"]
fn benchmark_large_text_preview_frames_across_width_dpi_appearance() {
    use little_clip_core::config::WorkbenchAppearance;

    const LOGICAL_LINES: usize = 12_000;
    const TIMED_FRAMES: usize = 60;
    let mut fixture = String::new();
    for line in 0..LOGICAL_LINES {
        if line % 257 == 0 {
            fixture.push_str(&format!(
                "fn row_{line}() {{ let value = {line}; let padding = \"{}\"; }}\n",
                "x".repeat(8 * 1024)
            ));
        } else {
            fixture.push_str(&format!("fn row_{line}() {{ let value = {line}; }}\n"));
        }
    }
    let fixture_bytes = fixture.len();

    let cases = [
        (egui::vec2(640.0, 520.0), 1.0, WorkbenchAppearance::Light),
        (egui::vec2(640.0, 520.0), 1.5, WorkbenchAppearance::Light),
        (egui::vec2(640.0, 520.0), 1.0, WorkbenchAppearance::Dark),
        (egui::vec2(640.0, 520.0), 1.5, WorkbenchAppearance::Dark),
        (egui::vec2(1180.0, 720.0), 1.0, WorkbenchAppearance::Light),
        (egui::vec2(1180.0, 720.0), 1.5, WorkbenchAppearance::Light),
        (egui::vec2(1180.0, 720.0), 1.0, WorkbenchAppearance::Dark),
        (egui::vec2(1180.0, 720.0), 1.5, WorkbenchAppearance::Dark),
    ];
    let render_frame = |state: &mut TextPreviewState,
                        ctx: &egui::Context,
                        palette: &AppPalette,
                        time: &mut f64,
                        size: Vec2,
                        pixels_per_point: f32,
                        events: Vec<egui::Event>| {
        *time += 1.0 / 60.0;
        let started = std::time::Instant::now();
        let mut input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
            time: Some(*time),
            events,
            ..Default::default()
        };
        input
            .viewports
            .get_mut(&input.viewport_id)
            .expect("root viewport exists")
            .native_pixels_per_point = Some(pixels_per_point);
        let output = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let mut close = false;
                state.render_ui(ui, ctx, palette, &mut close);
            });
        });
        let shape_count = output.shapes.len();
        (started.elapsed().as_secs_f64() * 1000.0, shape_count)
    };
    let summarize = |values: &mut Vec<f64>| {
        values.sort_by(f64::total_cmp);
        let at = |fraction: f64| {
            let index = ((values.len() - 1) as f64 * fraction).ceil() as usize;
            values[index]
        };
        (at(0.50), at(0.95), values[values.len() - 1])
    };

    for (size, pixels_per_point, appearance) in cases {
        let construction_started = std::time::Instant::now();
        let mut state = TextPreviewState::from_text(fixture.clone(), None, &AppConfig::default());
        let construction_ms = construction_started.elapsed().as_secs_f64() * 1000.0;

        // Provide deterministic, ready minimap samples so timed production
        // frames include minimap painting/index lookups but exclude worker
        // scheduling and syntax preparation.
        let source_lines = state.line_first_fragment.len();
        let minimap_stride = source_lines.div_ceil(MINIMAP_RETAINED_LINES).max(1);
        state.minimap_lines.lines = (0..source_lines)
            .step_by(minimap_stride)
            .map(|source_line| {
                (
                    source_line,
                    MinimapLine {
                        segments: vec![MinimapSegment {
                            start_column: 0,
                            end_column: MINIMAP_COLUMNS,
                            color: syntect::highlighting::Color {
                                r: 128,
                                g: 160,
                                b: 192,
                                a: 255,
                            },
                        }],
                    },
                )
            })
            .collect();
        state.minimap_lines.started = true;
        state.minimap_lines.ready = true;
        state.minimap_overflows = true;

        let palette = crate::ui_appearance::palette(appearance);
        let ctx = egui::Context::default();
        crate::ui_ext::configure_workbench_style(&ctx);
        crate::ui_appearance::apply_visuals(&ctx, &palette);
        let mut time = 0.0;
        let (cold_ms, cold_shapes) = render_frame(
            &mut state,
            &ctx,
            &palette,
            &mut time,
            size,
            pixels_per_point,
            Vec::new(),
        );

        let warm_frame_limit = state.code_fragments.len().div_ceil(8) + 32;
        let mut warm_frames = 0;
        while state.code_rows.next_measurement < state.code_fragments.len()
            && warm_frames < warm_frame_limit
        {
            let _ = render_frame(
                &mut state,
                &ctx,
                &palette,
                &mut time,
                size,
                pixels_per_point,
                Vec::new(),
            );
            warm_frames += 1;
        }
        assert_eq!(
            state.code_rows.next_measurement,
            state.code_fragments.len(),
            "geometry warm-up must finish before steady-state samples"
        );

        let mut idle_ms = Vec::with_capacity(TIMED_FRAMES);
        let mut idle_shapes = 0;
        for _ in 0..TIMED_FRAMES {
            let (elapsed, shapes) = render_frame(
                &mut state,
                &ctx,
                &palette,
                &mut time,
                size,
                pixels_per_point,
                Vec::new(),
            );
            idle_ms.push(elapsed);
            idle_shapes = shapes;
        }
        let idle = summarize(&mut idle_ms);

        let pointer = egui::pos2(size.x * 0.45, size.y * 0.5);
        let mut scroll_ms = Vec::with_capacity(TIMED_FRAMES);
        let mut scroll_shapes = 0;
        for frame in 0..TIMED_FRAMES {
            let delta = if frame % 2 == 0 { -240.0 } else { 240.0 };
            let (elapsed, shapes) = render_frame(
                &mut state,
                &ctx,
                &palette,
                &mut time,
                size,
                pixels_per_point,
                vec![
                    egui::Event::PointerMoved(pointer),
                    egui::Event::Scroll(egui::vec2(0.0, delta)),
                ],
            );
            scroll_ms.push(elapsed);
            scroll_shapes = shapes;
        }
        let scroll = summarize(&mut scroll_ms);

        let reflow_size = egui::vec2(size.x - 80.0, size.y);
        let reflow_pixels_per_point = if pixels_per_point == 1.0 { 1.5 } else { 1.0 };
        let (reflow_first_ms, _) = render_frame(
            &mut state,
            &ctx,
            &palette,
            &mut time,
            reflow_size,
            reflow_pixels_per_point,
            Vec::new(),
        );
        let settle_started = std::time::Instant::now();
        let mut reflow_frames = 1;
        while state.code_rows.next_measurement < state.code_fragments.len()
            && reflow_frames < warm_frame_limit
        {
            let _ = render_frame(
                &mut state,
                &ctx,
                &palette,
                &mut time,
                reflow_size,
                reflow_pixels_per_point,
                Vec::new(),
            );
            reflow_frames += 1;
        }
        assert_eq!(state.code_rows.next_measurement, state.code_fragments.len());
        let reflow_settle_ms = settle_started.elapsed().as_secs_f64() * 1000.0;

        println!(
            "text-preview appearance={appearance:?} host={}x{} ppp={pixels_per_point:.1} source_bytes={fixture_bytes} fragments={} samples={} construct_ms={construction_ms:.3} cold_ms={cold_ms:.3} cold_shapes={cold_shapes} warm_frames={warm_frames} idle_ms[p50/p95/max]={:.3}/{:.3}/{:.3} idle_shapes={idle_shapes} scroll_ms[p50/p95/max]={:.3}/{:.3}/{:.3} scroll_shapes={scroll_shapes} reflow_first_ms={reflow_first_ms:.3} reflow_settle_frames={reflow_frames} reflow_settle_ms={reflow_settle_ms:.3}",
            size.x as usize,
            size.y as usize,
            state.code_fragments.len(),
            state.minimap_lines.lines.len(),
            idle.0,
            idle.1,
            idle.2,
            scroll.0,
            scroll.1,
            scroll.2,
        );
    }
}
