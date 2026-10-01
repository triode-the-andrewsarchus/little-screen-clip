use super::*;

#[test]
fn test_binary_diff_view_state_creation_and_recompute() {
    let mut state = BinaryDiffViewState::new(
        "file_a.bin".to_string(),
        b"HELLO WORLD BINARY TEST".to_vec(),
        "file_b.bin".to_string(),
        b"HELLO RUSTY BINARY TEST".to_vec(),
    );
    assert_eq!(state.bytes_per_row, 32);

    // Wait for worker thread result
    if let Some(rx) = state.receiver.take() {
        let (_gen, res) = rx.recv().expect("job result").expect("computed result");
        state.result = Some(res);
        state.is_computing = false;
        state.compute_ticket = None;
    }

    assert!(state.result.is_some());
    let res = state.result.as_ref().unwrap();
    assert_eq!(res.left_label, "file_a.bin");
    assert_eq!(res.right_label, "file_b.bin");
    assert!(!res.rows.is_empty());
    assert!(!res.diff_blocks.is_empty());

    // Test search
    state.search_query = "RUSTY".to_string();
    state.update_search();
    assert!(!state.search_matches.is_empty());

    // Test navigation
    state.next_diff();
    assert!(state.scroll_target_row.is_some());
    state.prev_diff();

    // Test swap operands
    state.swap_operands();
    assert_eq!(state.left_label, "file_b.bin");
    assert_eq!(state.right_label, "file_a.bin");

    // Drain recomputed worker thread
    if let Some(rx) = state.receiver.take() {
        let (_gen, res) = rx.recv().expect("job result").expect("swapped result");
        state.result = Some(res);
        state.is_computing = false;
        state.compute_ticket = None;
    }
    let res = state.result.as_ref().unwrap();
    assert_eq!(res.left_label, "file_b.bin");
    assert_eq!(res.right_label, "file_a.bin");

    // Test toggle mode
    assert_eq!(state.mode, BinaryDiffMode::Aligned);
    state.toggle_mode();
    assert_eq!(state.mode, BinaryDiffMode::Raw);
}

#[test]
fn binary_diff_tab_shortcut_recomputes_raw_result() {
    let mut state = BinaryDiffViewState::new(
        "left.bin".to_string(),
        b"A B C D".to_vec(),
        "right.bin".to_string(),
        b"A X B C D".to_vec(),
    );
    let initial_rx = state.receiver.take().expect("initial compute receiver");
    let (initial_generation, initial_result) = initial_rx
        .recv()
        .expect("initial job result")
        .expect("initial diff result");
    assert_eq!(initial_generation, state.current_generation);
    assert_eq!(initial_result.mode, BinaryDiffMode::Aligned);
    state.result = Some(initial_result);
    state.is_computing = false;
    state.compute_ticket = None;

    let previous_generation = state.current_generation;
    let ctx = egui::Context::default();
    let mut close = false;
    let _ = ctx.run(
        egui::RawInput {
            events: vec![egui::Event::Key {
                key: egui::Key::Tab,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        },
        |ctx| state.handle_global_shortcuts(ctx, &mut close),
    );

    assert!(!close);
    assert_eq!(state.mode, BinaryDiffMode::Raw);
    assert_eq!(state.current_generation, previous_generation + 1);
    assert!(state.is_computing);
    assert_eq!(state.result.as_ref().unwrap().mode, BinaryDiffMode::Aligned);

    let recompute_rx = state.receiver.take().expect("raw recompute receiver");
    let (generation, recomputed) = recompute_rx
        .recv()
        .expect("recompute job result")
        .expect("raw diff result");
    assert_eq!(generation, state.current_generation);
    assert_eq!(recomputed.mode, BinaryDiffMode::Raw);
    assert_eq!(recomputed.left_label, "left.bin");
    assert_eq!(recomputed.right_label, "right.bin");
    state.result = Some(recomputed);
    state.is_computing = false;
    state.compute_ticket = None;
    assert_eq!(state.result.as_ref().unwrap().mode, BinaryDiffMode::Raw);
}

#[test]
fn binary_diff_width_shortcuts_recompute_each_requested_width() {
    let left_bytes: Vec<u8> = (0..48).collect();
    let mut right_bytes = left_bytes.clone();
    right_bytes[17] = 250;
    let mut state = BinaryDiffViewState::new(
        "left.bin".to_string(),
        left_bytes,
        "right.bin".to_string(),
        right_bytes,
    );
    let initial_rx = state.receiver.take().expect("initial compute receiver");
    let (initial_generation, initial_result) = initial_rx
        .recv()
        .expect("initial job result")
        .expect("initial diff result");
    assert_eq!(initial_generation, state.current_generation);
    assert_eq!(initial_result.bytes_per_row, 32);
    state.result = Some(initial_result);
    state.is_computing = false;
    state.compute_ticket = None;

    for (key, expected_width) in [
        (egui::Key::Num0, 8),
        (egui::Key::Num1, 16),
        (egui::Key::Num2, 32),
    ] {
        let previous_generation = state.current_generation;
        let previous_width = state.result.as_ref().unwrap().bytes_per_row;
        trigger_global_shortcut(&mut state, key);
        assert_eq!(state.bytes_per_row, expected_width);
        assert_eq!(state.current_generation, previous_generation + 1);
        assert!(state.is_computing);
        assert_eq!(state.result.as_ref().unwrap().bytes_per_row, previous_width);

        let rx = state.receiver.take().expect("width recompute receiver");
        let (generation, result) = rx
            .recv()
            .expect("width recompute job result")
            .expect("width recompute result");
        assert_eq!(generation, state.current_generation);
        assert_eq!(result.bytes_per_row, expected_width);
        assert_eq!(result.mode, BinaryDiffMode::Aligned);
        state.result = Some(result);
        state.is_computing = false;
        state.compute_ticket = None;
    }
}

#[test]
fn binary_diff_s_shortcut_swaps_operands_and_recomputes_result() {
    let left_bytes = vec![0xA1, 0xA2, 0xA3];
    let right_bytes = vec![0xB1, 0xB2, 0xB3];
    let mut state = BinaryDiffViewState::new(
        "left.bin".to_string(),
        left_bytes.clone(),
        "right.bin".to_string(),
        right_bytes.clone(),
    );
    let initial_rx = state.receiver.take().expect("initial compute receiver");
    let (initial_generation, initial_result) = initial_rx
        .recv()
        .expect("initial job result")
        .expect("initial diff result");
    assert_eq!(initial_generation, state.current_generation);
    state.result = Some(initial_result);
    state.is_computing = false;
    state.compute_ticket = None;

    let previous_generation = state.current_generation;
    trigger_global_shortcut(&mut state, egui::Key::S);
    assert_eq!(state.left_label, "right.bin");
    assert_eq!(state.right_label, "left.bin");
    assert_eq!(&*state.left_bytes, &right_bytes);
    assert_eq!(&*state.right_bytes, &left_bytes);
    assert_eq!(state.current_generation, previous_generation + 1);
    assert!(state.is_computing);
    assert_eq!(state.result.as_ref().unwrap().left_label, "left.bin");

    let recompute_rx = state.receiver.take().expect("swapped compute receiver");
    let (generation, result) = recompute_rx
        .recv()
        .expect("recompute job result")
        .expect("swapped diff result");
    assert_eq!(generation, state.current_generation);
    assert_eq!(result.left_label, "right.bin");
    assert_eq!(result.right_label, "left.bin");
    assert_eq!(result.bytes_per_row, state.bytes_per_row);
    assert_eq!(
        result
            .rows
            .first()
            .unwrap()
            .left_cells
            .first()
            .unwrap()
            .byte_val,
        Some(right_bytes[0])
    );
    assert_eq!(
        result
            .rows
            .first()
            .unwrap()
            .right_cells
            .first()
            .unwrap()
            .byte_val,
        Some(left_bytes[0])
    );
    state.result = Some(result);
    state.is_computing = false;
    state.compute_ticket = None;
}

#[test]
fn modified_s_does_not_swap_binary_diff_operands() {
    let left_bytes = vec![0xA1, 0xA2, 0xA3];
    let right_bytes = vec![0xB1, 0xB2, 0xB3];
    let mut state = BinaryDiffViewState::new(
        "left.bin".to_string(),
        left_bytes.clone(),
        "right.bin".to_string(),
        right_bytes.clone(),
    );
    let initial_rx = state.receiver.take().expect("initial compute receiver");
    let (generation, result) = initial_rx
        .recv()
        .expect("initial job result")
        .expect("initial diff result");
    assert_eq!(generation, state.current_generation);
    state.result = Some(result);
    state.is_computing = false;
    state.compute_ticket = None;

    let baseline_generation = state.current_generation;
    let baseline_result = state.result.as_ref().unwrap();
    let baseline_left_label = baseline_result.left_label.clone();
    let baseline_right_label = baseline_result.right_label.clone();
    for modifiers in [
        egui::Modifiers::CTRL,
        egui::Modifiers::COMMAND,
        egui::Modifiers::MAC_CMD,
        egui::Modifiers {
            shift: true,
            ..egui::Modifiers::NONE
        },
        egui::Modifiers::ALT,
    ] {
        let ctx = egui::Context::default();
        let mut close = false;
        let _ = ctx.run(
            egui::RawInput {
                modifiers,
                events: vec![egui::Event::Key {
                    key: egui::Key::S,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers,
                }],
                ..Default::default()
            },
            |ctx| state.handle_global_shortcuts(ctx, &mut close),
        );

        assert!(!close);
        assert_eq!(state.left_label, "left.bin");
        assert_eq!(state.right_label, "right.bin");
        assert_eq!(&*state.left_bytes, &left_bytes);
        assert_eq!(&*state.right_bytes, &right_bytes);
        assert_eq!(state.current_generation, baseline_generation);
        assert!(!state.is_computing);
        let result = state.result.as_ref().unwrap();
        assert_eq!(result.left_label, baseline_left_label);
        assert_eq!(result.right_label, baseline_right_label);
    }
}

#[test]
fn binary_diff_queue_saturation_is_visible_and_recovers() {
    use std::sync::mpsc;
    use std::time::Duration;

    let jobs = BackgroundJobs::new(1, 1);
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let _blocker = jobs.submit(Duration::MAX, move || {
        let _ = started_tx.send(());
        let _ = release_rx.recv();
    });
    started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    let _queued = jobs.submit(Duration::MAX, || ());

    let mut state = BinaryDiffViewState::new_with_jobs(
        "left.bin".into(),
        b"left".to_vec(),
        "right.bin".into(),
        b"right".to_vec(),
        &jobs,
    );
    assert!(state.is_computing);

    let ctx = egui::Context::default();
    let palette = AppPalette::default();
    let expected = "Binary diff failed: background job queue is full";
    let output = ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                Vec2::new(1000.0, 700.0),
            )),
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                state.render_ui(ui, ctx, &palette, &mut false);
            });
        },
    );

    assert!(!state.is_computing);
    assert!(state.receiver.is_none());
    assert!(output.shapes.iter().any(|shape| {
        matches!(&shape.shape, egui::Shape::Text(text) if text.galley.text().contains(expected))
    }));
    assert_eq!(
        state.status_toast.message(0.0),
        Some(&format!("❌ {expected}")[..])
    );

    let _ = release_tx.send(());
}

#[test]
fn dropping_binary_diff_viewer_cancels_queued_compute() {
    use std::sync::mpsc;
    use std::time::Duration;

    let jobs = BackgroundJobs::new(1, 1);
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let _blocker = jobs.submit(Duration::MAX, move || {
        let _ = started_tx.send(());
        let _ = release_rx.recv();
    });
    started_rx.recv_timeout(Duration::from_secs(1)).unwrap();

    let mut state = BinaryDiffViewState::new_with_jobs(
        "left.bin".into(),
        b"left".to_vec(),
        "right.bin".into(),
        b"right".to_vec(),
        &jobs,
    );
    let result_rx = state.receiver.take().unwrap();
    drop(state);

    let _ = release_tx.send(());
    assert!(matches!(
        result_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
        Err(little_clip_core::JobError::Cancelled)
    ));
}

#[test]
fn test_global_shortcuts_navigate_diffs_and_switch_widths() {
    let left_bytes = vec![0; 24];
    let mut right_bytes = left_bytes.clone();
    right_bytes[0] = 1;
    right_bytes[16] = 1;
    let mut state = BinaryDiffViewState::new(
        "file_a.bin".to_string(),
        left_bytes.clone(),
        "file_b.bin".to_string(),
        right_bytes.clone(),
    );
    state.result = Some(compute_binary_diff(
        "file_a.bin",
        &left_bytes,
        "file_b.bin",
        &right_bytes,
        BinaryDiffMode::Aligned,
        8,
    ));
    state.is_computing = false;
    let diff_blocks = state.result.as_ref().unwrap().diff_blocks.clone();
    assert!(diff_blocks.len() >= 2);

    for key in [egui::Key::J, egui::Key::ArrowRight, egui::Key::ArrowDown] {
        state.nav.current_diff_idx = 0;
        state.current_diff_idx = 0;
        state.nav.selected_diff_block = None;
        state.nav.scroll_target_row = None;
        trigger_global_shortcut(&mut state, key);
        assert_eq!(state.current_diff_idx, 1);
        assert_eq!(state.nav.selected_diff_block, Some(diff_blocks[1].id));
        assert_eq!(state.selected_diff_block, Some(diff_blocks[1].id));
        assert_eq!(state.nav.scroll_target_row, Some(diff_blocks[1].start_row));
        assert_eq!(state.scroll_target_row, Some(diff_blocks[1].start_row));
    }
    for key in [egui::Key::K, egui::Key::ArrowLeft, egui::Key::ArrowUp] {
        state.nav.current_diff_idx = 1;
        state.current_diff_idx = 1;
        state.nav.selected_diff_block = None;
        state.nav.scroll_target_row = None;
        trigger_global_shortcut(&mut state, key);
        assert_eq!(state.current_diff_idx, 0);
        assert_eq!(state.nav.selected_diff_block, Some(diff_blocks[0].id));
        assert_eq!(state.selected_diff_block, Some(diff_blocks[0].id));
        assert_eq!(state.nav.scroll_target_row, Some(diff_blocks[0].start_row));
        assert_eq!(state.scroll_target_row, Some(diff_blocks[0].start_row));
    }
    for (key, expected_width) in [
        (egui::Key::Num0, 8),
        (egui::Key::Num1, 16),
        (egui::Key::Num2, 32),
    ] {
        trigger_global_shortcut(&mut state, key);
        assert_eq!(state.bytes_per_row, expected_width);
    }
}

#[test]
fn test_cell_formatting() {
    let palette = AppPalette::default();
    let match_cell = AlignedCell {
        offset: Some(0),
        byte_val: Some(0x41),
        kind: DiffKind::Match,
    };
    // Digits mode: text is colored, no background pill
    let (hex, color, bg) = format_cell_hex(&match_cell, true, ByteColorMode::Digits, &palette);
    assert_eq!(hex, "41");
    assert_eq!(color, byte_subtle_color(0x41));
    assert!(bg.is_none());

    // Background mode: background pill is colored, text is light neutral
    let (hex_bg, color_bg, pill) =
        format_cell_hex(&match_cell, true, ByteColorMode::Background, &palette);
    assert_eq!(hex_bg, "41");
    assert_eq!(color_bg, hex_to_c32(&palette.colors.text_primary));
    assert!(pill.is_some());

    let (ascii, a_color) = format_cell_ascii(&match_cell, true, ByteColorMode::Digits, &palette);
    assert_eq!(ascii, 'A');
    assert_eq!(a_color, byte_subtle_color(0x41));

    let gap_cell = AlignedCell {
        offset: None,
        byte_val: None,
        kind: DiffKind::LeftOnly,
    };
    let (gap_hex, _, _) = format_cell_hex(&gap_cell, false, ByteColorMode::Digits, &palette);
    assert_eq!(gap_hex, "--");
    let (gap_ascii, _) = format_cell_ascii(&gap_cell, false, ByteColorMode::Digits, &palette);
    assert_eq!(gap_ascii, '·');
}

#[test]
fn test_layout_fits_window() {
    let palette = AppPalette::default();
    for bpr in [8, 16, 32] {
        let mut state = BinaryDiffViewState::new(
            "file_a.bin".to_string(),
            vec![0u8; 1000],
            "file_b.bin".to_string(),
            vec![1u8; 1000],
        );
        let res = little_clip_ops::binary_diff::compute_binary_diff(
            "file_a.bin",
            &[0u8; 1000],
            "file_b.bin",
            &[1u8; 1000],
            BinaryDiffMode::Aligned,
            bpr,
        );
        state.result = Some(res);
        state.is_computing = false;
        state.bytes_per_row = bpr;

        let ctx = egui::Context::default();
        // Frame 1
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::Vec2::new(1180.0, 680.0),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default()
                    .frame(egui::Frame::none())
                    .show(ctx, |ui| {
                        let mut close_req = false;
                        state.render_ui(ui, ctx, &palette, &mut close_req);
                    });
            },
        );

        // Frame 2
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::Vec2::new(1180.0, 680.0),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default()
                    .frame(egui::Frame::none())
                    .show(ctx, |ui| {
                        let mut close_req = false;
                        state.render_ui(ui, ctx, &palette, &mut close_req);
                        println!("[TEST BPR={}] min_rect: {:?}", bpr, ui.min_rect());
                        assert!(
                            ui.min_rect().max.x <= 1180.5,
                            "bpr={} overflows width: max.x={}",
                            bpr,
                            ui.min_rect().max.x
                        );
                    });
            },
        );
    }
}

#[test]
fn wide_rows_fit_without_horizontal_scrolling() {
    let left: Vec<u8> = (0x41..0x61).collect();
    let mut right = left.clone();
    right[31] = b'z';
    let mut state = BinaryDiffViewState::new(
        "lhs.bin".into(),
        left.clone(),
        "rhs.bin".into(),
        right.clone(),
    );
    state.result = Some(compute_binary_diff(
        "lhs.bin",
        &left,
        "rhs.bin",
        &right,
        BinaryDiffMode::Aligned,
        32,
    ));
    state.is_computing = false;
    state.receiver = None;
    state.panel_state.right_collapsed = true;

    let palette = AppPalette::default();
    let ctx = egui::Context::default();
    let screen_rect = egui::Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(1100.0, 760.0));
    let font_size = state.layout_metrics(32, 1050.0).2;
    assert!(font_size < crate::ui_ext::MONO_FONT_SIZE);

    let mut time = 0.0;
    let mut output = None;
    for _ in 0..4 {
        time += 0.05;
        output = Some(ctx.run(
            egui::RawInput {
                screen_rect: Some(screen_rect),
                time: Some(time),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    state.render_ui(ui, ctx, &palette, &mut false);
                });
            },
        ));
    }
    let output = output.unwrap();
    assert!(
        output.shapes.iter().any(|clipped| match &clipped.shape {
            egui::Shape::Text(text) if text.galley.text() == "Offset R" => clipped
                .clip_rect
                .contains_rect(egui::Rect::from_min_size(text.pos, text.galley.size())),
            _ => false,
        }),
        "the right offset header should fit beside the table data"
    );
    let final_byte = output
        .shapes
        .iter()
        .find_map(|clipped| match &clipped.shape {
            egui::Shape::Text(text)
                if text.galley.text() == "z"
                    && clipped
                        .clip_rect
                        .contains_rect(egui::Rect::from_min_size(text.pos, text.galley.size())) =>
            {
                Some((text.pos, text.galley.size()))
            }
            _ => None,
        })
        .expect("the final right-file ASCII byte should fit in the viewport");
    assert!(final_byte.0.x + final_byte.1.x <= screen_rect.max.x);
    assert!(
        output.shapes.iter().any(|clipped| match &clipped.shape {
            egui::Shape::Text(text)
                if text.galley.text() == "00000000"
                    && text.pos.x > final_byte.0.x
                    && clipped
                        .clip_rect
                        .contains_rect(
                            egui::Rect::from_min_size(text.pos, text.galley.size(),)
                        ) =>
                true,
            _ => false,
        }),
        "the right file's row offset should remain visible after its final byte"
    );
}

#[test]
fn alternating_row_bands_follow_row_positions() {
    let bytes = vec![0x55; 128];
    let mut state = BinaryDiffViewState::new(
        "left.bin".into(),
        bytes.clone(),
        "right.bin".into(),
        bytes.clone(),
    );
    state.result = Some(compute_binary_diff(
        "left.bin",
        &bytes,
        "right.bin",
        &bytes,
        BinaryDiffMode::Aligned,
        32,
    ));
    state.is_computing = false;
    state.receiver = None;
    state.panel_state.right_collapsed = true;
    state.bytes_per_row = 32;

    let palette = AppPalette::default();
    let stripe = hex_to_c32_alpha(&palette.colors.text_muted, 4);
    let ctx = egui::Context::default();
    let screen_rect = egui::Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(1100.0, 760.0));
    let output = ctx.run(
        egui::RawInput {
            screen_rect: Some(screen_rect),
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                state.render_ui(ui, ctx, &palette, &mut false);
            });
        },
    );
    let mut bands: Vec<_> = output
        .shapes
        .iter()
        .filter_map(|clipped| match &clipped.shape {
            egui::Shape::Rect(rect) if rect.fill == stripe && rect.rect.width() > 500.0 => {
                Some(rect.rect)
            }
            _ => None,
        })
        .collect();
    bands.sort_by(|a, b| a.top().total_cmp(&b.top()));
    assert_eq!(bands.len(), 2, "only odd-numbered rows should be striped");
    assert!(bands[0].height() > 0.0);
    assert!((bands[1].top() - bands[0].top() - 2.0 * bands[0].height()).abs() < 0.1);
}

#[test]
fn byte_density_shrinks_to_fit_only_when_needed() {
    let mut state =
        BinaryDiffViewState::new("left".to_owned(), vec![0], "right".to_owned(), vec![1]);
    for bpr in [8, 16, 32] {
        state.bytes_per_row = bpr;
        let (cell_w, _, font_size, ascii_w, offset_w, row_height) =
            state.layout_metrics(bpr, 3600.0);
        assert_eq!(font_size, crate::ui_ext::MONO_FONT_SIZE);
        assert!(cell_w >= font_size * 1.2);
        assert!(ascii_w >= font_size * 0.6);
        assert!(offset_w >= font_size * 5.4);
        assert!(row_height >= font_size + 10.0);
    }
    state.bytes_per_row = 32;
    assert!(state.layout_metrics(32, 1100.0).2 < crate::ui_ext::MONO_FONT_SIZE);
}

#[test]
fn test_binary_diff_navigation_cycles_and_bounds() {
    use little_clip_ops::binary_diff::BinaryDiffBlock;

    let mut nav = BinaryDiffNavigation::default();
    let blocks = vec![
        BinaryDiffBlock {
            id: 10,
            start_row: 5,
            end_row: 6,
            left_offset: None,
            right_offset: None,
            byte_count: 2,
            kind: DiffKind::Mismatch,
            kind_label: "Mismatch".into(),
            preview: String::new(),
        },
        BinaryDiffBlock {
            id: 20,
            start_row: 15,
            end_row: 15,
            left_offset: None,
            right_offset: None,
            byte_count: 1,
            kind: DiffKind::RightOnly,
            kind_label: "Insertion".into(),
            preview: String::new(),
        },
        BinaryDiffBlock {
            id: 30,
            start_row: 25,
            end_row: 27,
            left_offset: None,
            right_offset: None,
            byte_count: 3,
            kind: DiffKind::LeftOnly,
            kind_label: "Replacement".into(),
            preview: String::new(),
        },
    ];

    // Empty block safeguard
    nav.next_diff(&[]);
    assert_eq!(nav.current_diff_idx, 0);
    assert_eq!(nav.selected_diff_block, None);

    // Next forward
    nav.next_diff(&blocks);
    assert_eq!(nav.current_diff_idx, 1);
    assert_eq!(nav.selected_diff_block, Some(20));
    assert_eq!(nav.scroll_target_row, Some(15));

    nav.next_diff(&blocks);
    assert_eq!(nav.current_diff_idx, 2);
    assert_eq!(nav.selected_diff_block, Some(30));
    assert_eq!(nav.scroll_target_row, Some(25));

    // Wrap forward
    nav.next_diff(&blocks);
    assert_eq!(nav.current_diff_idx, 0);
    assert_eq!(nav.selected_diff_block, Some(10));
    assert_eq!(nav.scroll_target_row, Some(5));

    // Wrap backward
    nav.prev_diff(&blocks);
    assert_eq!(nav.current_diff_idx, 2);
    assert_eq!(nav.selected_diff_block, Some(30));

    nav.prev_diff(&blocks);
    assert_eq!(nav.current_diff_idx, 1);
    assert_eq!(nav.selected_diff_block, Some(20));
}

#[test]
fn test_binary_diff_search_navigation() {
    let mut nav = BinaryDiffNavigation::default();
    nav.set_search_matches(vec![3, 7, 12]);
    assert_eq!(nav.current_search_idx, 0);
    assert_eq!(nav.scroll_target_row, Some(3));

    nav.next_search_match();
    assert_eq!(nav.current_search_idx, 1);
    assert_eq!(nav.scroll_target_row, Some(7));

    nav.next_search_match();
    assert_eq!(nav.current_search_idx, 2);
    assert_eq!(nav.scroll_target_row, Some(12));

    // Wrap forward
    nav.next_search_match();
    assert_eq!(nav.current_search_idx, 0);
    assert_eq!(nav.scroll_target_row, Some(3));

    // Wrap backward
    nav.prev_search_match();
    assert_eq!(nav.current_search_idx, 2);
    assert_eq!(nav.scroll_target_row, Some(12));
}

#[test]
fn binary_diff_search_field_updates_matches_and_navigates_while_focused() {
    let mut bytes = vec![b'x'; 96];
    bytes[1..4].copy_from_slice(b"ABC");
    bytes[65..68].copy_from_slice(b"ABC");
    let mut state = BinaryDiffViewState::new(
        "left.bin".to_string(),
        bytes.clone(),
        "right.bin".to_string(),
        bytes,
    );
    let initial_rx = state.receiver.take().expect("initial compute receiver");
    let (generation, result) = initial_rx
        .recv()
        .expect("initial job result")
        .expect("initial diff result");
    assert_eq!(generation, state.current_generation);
    state.result = Some(result);
    state.is_computing = false;
    state.compute_ticket = None;

    let ctx = egui::Context::default();
    crate::ui_ext::configure_workbench_style(&ctx);
    let search_id = egui::Id::new("binary_diff_search_field");
    ctx.memory_mut(|memory| memory.request_focus(search_id));
    let palette = AppPalette::default();
    let screen_rect = egui::Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(1200.0, 800.0));
    let _ = ctx.run(
        egui::RawInput {
            screen_rect: Some(screen_rect),
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                state.render_ui(ui, ctx, &palette, &mut false);
            });
        },
    );
    assert_eq!(ctx.memory(|memory| memory.focused()), Some(search_id));

    let mut keyboard_focus_kept = false;
    let _ = ctx.run(
        egui::RawInput {
            screen_rect: Some(screen_rect),
            events: vec![egui::Event::Text("ABC".to_string())],
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                state.render_ui(ui, ctx, &palette, &mut false);
            });
            keyboard_focus_kept = ctx.wants_keyboard_input();
        },
    );
    assert!(keyboard_focus_kept);
    assert_eq!(ctx.memory(|memory| memory.focused()), Some(search_id));
    assert_eq!(state.search_query, "ABC");
    assert_eq!(state.search_matches, vec![0, 2]);
    assert_eq!(state.current_search_idx, 0);
    assert_eq!(state.nav.scroll_target_row, Some(0));

    state.next_search_match();
    assert_eq!(state.current_search_idx, 1);
    assert_eq!(state.scroll_target_row, Some(2));
    assert_eq!(ctx.memory(|memory| memory.focused()), Some(search_id));
    state.prev_search_match();
    assert_eq!(state.current_search_idx, 0);
    assert_eq!(state.scroll_target_row, Some(0));
    assert_eq!(ctx.memory(|memory| memory.focused()), Some(search_id));
}

#[test]
fn focused_binary_diff_search_escape_clears_query_before_close() {
    let mut left_bytes = vec![b'x'; 96];
    left_bytes[1..4].copy_from_slice(b"ABC");
    let mut right_bytes = left_bytes.clone();
    right_bytes[20] = b'y';
    let mut state = BinaryDiffViewState::new(
        "left.bin".to_string(),
        left_bytes,
        "right.bin".to_string(),
        right_bytes,
    );
    let initial_rx = state.receiver.take().expect("initial compute receiver");
    let (generation, result) = initial_rx
        .recv()
        .expect("initial job result")
        .expect("initial diff result");
    assert_eq!(generation, state.current_generation);
    state.result = Some(result);
    state.is_computing = false;
    state.compute_ticket = None;
    state.search_query = "ABC".to_string();
    state.update_search();
    assert!(!state.search_matches.is_empty());

    let ctx = egui::Context::default();
    crate::ui_ext::configure_workbench_style(&ctx);
    let search_id = egui::Id::new("binary_diff_search_field");
    ctx.memory_mut(|memory| memory.request_focus(search_id));
    let palette = AppPalette::default();
    let screen_rect = egui::Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(1200.0, 800.0));
    let _ = ctx.run(
        egui::RawInput {
            screen_rect: Some(screen_rect),
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                state.render_ui(ui, ctx, &palette, &mut false);
            });
        },
    );
    assert_eq!(ctx.memory(|memory| memory.focused()), Some(search_id));
    assert!(ctx.wants_keyboard_input());

    let mut close_requested = false;
    let _ = ctx.run(
        egui::RawInput {
            screen_rect: Some(screen_rect),
            events: vec![egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                state.render_ui(ui, ctx, &palette, &mut close_requested);
            });
        },
    );

    assert!(state.search_query.is_empty());
    assert!(state.search_matches.is_empty());
    assert!(!close_requested, "Escape clears the focused search first");
}

#[test]
fn focused_binary_diff_search_preempts_global_shortcuts() {
    fn render_frame(
        state: &mut BinaryDiffViewState,
        ctx: &egui::Context,
        screen_rect: egui::Rect,
        events: Vec<egui::Event>,
    ) -> bool {
        let mut wants_keyboard_before_render = false;
        let palette = AppPalette::default();
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(screen_rect),
                events,
                ..Default::default()
            },
            |ctx| {
                wants_keyboard_before_render = ctx.wants_keyboard_input();
                egui::CentralPanel::default().show(ctx, |ui| {
                    state.render_ui(ui, ctx, &palette, &mut false);
                });
            },
        );
        wants_keyboard_before_render
    }

    let mut left_bytes = vec![b'x'; 96];
    left_bytes[1..4].copy_from_slice(b"ABC");
    left_bytes[65..68].copy_from_slice(b"ABC");
    let mut right_bytes = left_bytes.clone();
    right_bytes[20] = b'y';
    right_bytes[80] = b'z';
    let mut state = BinaryDiffViewState::new(
        "left.bin".to_string(),
        left_bytes,
        "right.bin".to_string(),
        right_bytes,
    );
    let initial_rx = state.receiver.take().expect("initial compute receiver");
    let (generation, result) = initial_rx
        .recv()
        .expect("initial job result")
        .expect("initial diff result");
    assert_eq!(generation, state.current_generation);
    state.result = Some(result);
    state.is_computing = false;
    state.compute_ticket = None;
    state.search_query = "ABC".to_string();
    state.update_search();
    assert_eq!(state.search_matches, vec![0, 2]);
    state.next_diff();
    assert!(state.selected_diff_block.is_some());

    let ctx = egui::Context::default();
    crate::ui_ext::configure_workbench_style(&ctx);
    let search_id = egui::Id::new("binary_diff_search_field");
    let screen_rect = egui::Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(1200.0, 800.0));
    ctx.memory_mut(|memory| memory.request_focus(search_id));
    let _ = render_frame(&mut state, &ctx, screen_rect, Vec::new());
    assert_eq!(ctx.memory(|memory| memory.focused()), Some(search_id));

    let baseline = (
        state.mode,
        state.left_label.clone(),
        state.right_label.clone(),
        (*state.left_bytes).clone(),
        (*state.right_bytes).clone(),
        state.bytes_per_row,
        state.current_diff_idx,
        state.selected_diff_block,
        state.scroll_target_row,
    );
    let mut keys = vec![
        egui::Key::Tab,
        egui::Key::S,
        egui::Key::J,
        egui::Key::ArrowRight,
        egui::Key::ArrowDown,
        egui::Key::K,
        egui::Key::ArrowLeft,
        egui::Key::ArrowUp,
        egui::Key::Num0,
        egui::Key::Num1,
        egui::Key::Num2,
    ];

    for key in keys.drain(..) {
        ctx.memory_mut(|memory| memory.request_focus(search_id));
        let _ = render_frame(&mut state, &ctx, screen_rect, Vec::new());
        assert_eq!(ctx.memory(|memory| memory.focused()), Some(search_id));

        let wants_keyboard_before_render = render_frame(
            &mut state,
            &ctx,
            screen_rect,
            vec![egui::Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        assert!(
            wants_keyboard_before_render,
            "{key:?} frame must enter with search focused"
        );
        assert_eq!(
            (
                state.mode,
                state.left_label.clone(),
                state.right_label.clone(),
                (*state.left_bytes).clone(),
                (*state.right_bytes).clone(),
                state.bytes_per_row,
                state.current_diff_idx,
                state.selected_diff_block,
                state.scroll_target_row,
            ),
            baseline,
            "{key:?} must not run a global shortcut while search owns keyboard input"
        );
    }
}

#[test]
fn test_stale_generation_discarded() {
    let mut state = BinaryDiffViewState::new(
        "file_a.bin".to_string(),
        b"A".to_vec(),
        "file_b.bin".to_string(),
        b"B".to_vec(),
    );

    let initial_gen = state.current_generation;
    assert_eq!(initial_gen, 1);

    state.recompute();
    assert_eq!(state.current_generation, 2);

    // Inject an artificial stale message from generation 1
    let (tx, rx) =
        std::sync::mpsc::channel::<Result<(u64, BinaryDiffResult), little_clip_core::JobError>>();
    let dummy_res = compute_binary_diff("stale", b"A", "stale", b"B", BinaryDiffMode::Raw, 16);
    tx.send(Ok((1, dummy_res))).unwrap();
    state.receiver = Some(rx);

    // Poll receiver with generation 2 active
    if let Some(ref rx) = state.receiver {
        if let Ok(Ok((req_gen, res))) = rx.try_recv() {
            if req_gen == state.current_generation {
                state.result = Some(res);
            }
        }
    }

    // Stale result should NOT have been applied
    assert!(state.result.is_none());
}
