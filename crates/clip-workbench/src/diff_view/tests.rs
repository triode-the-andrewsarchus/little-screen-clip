use super::handoff::*;
use super::*;

fn staged_files_with_bytes(expected: &[u8]) -> Vec<std::path::PathBuf> {
    std::fs::read_dir(std::env::temp_dir())
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("clip_diff_"))
                && std::fs::read(path).is_ok_and(|bytes| bytes == expected)
        })
        .collect()
}

fn assert_staged_bytes_cleaned(bytes: &[u8]) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
    while !staged_files_with_bytes(bytes).is_empty() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(
        staged_files_with_bytes(bytes).is_empty(),
        "temporary staged file containing {:?} was not removed",
        String::from_utf8_lossy(bytes)
    );
}

#[test]
fn image_diff_labels_meet_contrast_on_the_view_surface() {
    for appearance in [
        little_clip_core::config::WorkbenchAppearance::Dark,
        little_clip_core::config::WorkbenchAppearance::Light,
    ] {
        let palette = crate::ui_appearance::palette(appearance);
        let surface = crate::hex_to_c32(&palette.colors.canvas);
        for preferred in [
            crate::hex_to_c32(&palette.diff_del),
            crate::hex_to_c32(&palette.harmonies.base),
        ] {
            let foreground =
                crate::ui_ext::contrast_text_c32_on(surface, surface, Some(preferred), &palette);
            assert!(
                little_clip_core::palette::contrast_ratio(
                    (surface.r(), surface.g(), surface.b()),
                    (foreground.r(), foreground.g(), foreground.b()),
                ) >= 4.5,
                "{appearance:?} image-diff custom label lacks contrast"
            );
        }
    }
}

#[test]
fn modified_s_does_not_swap_diff_operands() {
    fn send_modified_s(state: &mut DiffViewerState, modifiers: egui::Modifiers) {
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
            |ctx| state.handle_shortcuts(ctx, &mut close),
        );
        assert!(!close);
    }

    let left = DiffTarget::ClipboardText {
        label: "Left".to_string(),
        content: "old text\n".to_string(),
        ext_hint: Some("txt".to_string()),
    };
    let right = DiffTarget::ClipboardText {
        label: "Right".to_string(),
        content: "new text\n".to_string(),
        ext_hint: Some("txt".to_string()),
    };
    let mut state = DiffViewerState::new(left, right, DiffDisplayMode::Inline);
    state.wait_for_completion();
    let original_result = state.result.raw_ansi.clone();

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
        send_modified_s(&mut state, modifiers);
        assert_eq!(state.lhs.label(), "Left");
        assert_eq!(state.rhs.label(), "Right");
        assert!(!state.is_computing);
        assert_eq!(state.result.raw_ansi, original_result);
    }
}

#[test]
fn unmodified_s_swaps_operands_and_recomputes_text_diff() {
    let left = DiffTarget::ClipboardText {
        label: "Left".to_string(),
        content: "old text\n".to_string(),
        ext_hint: Some("txt".to_string()),
    };
    let right = DiffTarget::ClipboardText {
        label: "Right".to_string(),
        content: "new text\n".to_string(),
        ext_hint: Some("txt".to_string()),
    };
    let mut state = DiffViewerState::new(left, right, DiffDisplayMode::Inline);
    state.wait_for_completion();

    let ctx = egui::Context::default();
    let mut close = false;
    let _ = ctx.run(
        egui::RawInput {
            events: vec![egui::Event::Key {
                key: egui::Key::S,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        },
        |ctx| state.handle_shortcuts(ctx, &mut close),
    );

    assert!(!close);
    assert_eq!(state.lhs.label(), "Right");
    assert_eq!(state.rhs.label(), "Left");
    assert!(state.is_computing);
    state.wait_for_completion();
    let plain_result = strip_ansi_codes(&state.result.raw_ansi);
    assert!(plain_result.contains("+ old text"));
    assert!(plain_result.contains("- new text"));
}

#[test]
fn test_strip_ansi_codes() {
    let text = "\x1b[91m-old\x1b[0m\n\x1b[92m+new\x1b[0m";
    assert_eq!(strip_ansi_codes(text), "-old\n+new");
}

#[test]
fn text_diff_ctrl_c_publishes_exact_ansi_stripped_result() {
    let left = DiffTarget::ClipboardText {
        label: "Left".to_string(),
        content: "left".to_string(),
        ext_hint: Some("txt".to_string()),
    };
    let right = DiffTarget::ClipboardText {
        label: "Right".to_string(),
        content: "right".to_string(),
        ext_hint: Some("txt".to_string()),
    };
    let mut state = DiffViewerState::new(left, right, DiffDisplayMode::Inline);
    state.wait_for_completion();
    let ansi_result = "\x1b[1;91m--- Left\x1b[0m\n\x1b[91m- old\x1b[0m\n\x1b[92m+ new\x1b[0m\n";
    let expected = "--- Left\n- old\n+ new\n";
    state.result = DiffResult {
        engine_name: "synthetic fixture".to_string(),
        raw_ansi: ansi_result.to_string(),
        is_identical: false,
        mode: DiffDisplayMode::Inline,
    };

    let ctx = egui::Context::default();
    crate::ui_ext::configure_workbench_style(&ctx);
    let palette =
        crate::ui_appearance::palette(little_clip_core::config::WorkbenchAppearance::Dark);
    let output = ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(900.0, 620.0),
            )),
            events: vec![egui::Event::Key {
                key: egui::Key::C,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::CTRL,
            }],
            modifiers: egui::Modifiers::CTRL,
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                state.render_ui(ui, ctx, &palette, &mut false);
            });
        },
    );

    assert_eq!(output.platform_output.copied_text, expected);
    assert!(
        state
            .status_toast
            .as_ref()
            .is_some_and(|(message, _)| message == "Diff text copied to clipboard!")
    );
}

#[test]
fn image_diff_state_uses_generated_fixture_images() {
    let temp_dir = tempfile::tempdir().unwrap();
    let left_path = temp_dir.path().join("left.png");
    let right_path = temp_dir.path().join("right.png");
    let left = image::RgbaImage::from_pixel(3, 2, image::Rgba([0, 0, 0, 255]));
    let mut right = left.clone();
    right.put_pixel(1, 1, image::Rgba([255, 255, 255, 255]));
    left.save(&left_path).unwrap();
    right.save(&right_path).unwrap();

    let left = little_clip_core::ClipItem::from_single_file(&left_path);
    let right = little_clip_core::ClipItem::from_single_file(&right_path);
    let left = little_clip_ops::diff::resolve_diff_target(&left).unwrap();
    let right = little_clip_ops::diff::resolve_diff_target(&right).unwrap();
    let mut state = DiffViewerState::new(left, right, DiffDisplayMode::SideBySide);
    state.wait_for_completion();

    let result = state.image_result.as_ref().unwrap();
    assert_eq!((result.width, result.height), (3, 2));
    assert_eq!(result.diff_count, 1);
}

#[test]
fn image_diff_copy_request_contains_left_hand_mask_png() {
    let mask_rgba = vec![
        0, 0, 0, 255, // unchanged pixel
        255, 0, 0, 255, // highlighted difference
    ];
    let result = ImageDiffResult {
        width: 2,
        height: 1,
        diff_count: 1,
        total_pixels: 2,
        diff_percentage: 50.0,
        is_identical: false,
        base_rgba: vec![0; 8],
        comp_rgba: vec![0; 8],
        mask_rgba,
        overlay_rgba: vec![0; 8],
    };

    let request = image_mask_push_request(&result).expect("encode mask request");
    let IpcRequest::PushImage { bytes, mime, hand } = request else {
        panic!("image mask must use PushImage");
    };
    assert_eq!(hand, HandSide::Left);
    assert_eq!(mime, "image/png");
    let decoded = image::load_from_memory(&bytes)
        .expect("decode encoded mask")
        .to_rgba8();
    assert_eq!((decoded.width(), decoded.height()), (2, 1));
    assert_eq!(decoded.as_raw(), &[0, 0, 0, 255, 255, 0, 0, 255]);
}

#[test]
fn text_diff_state_computes_in_background() {
    let left = DiffTarget::ClipboardText {
        label: "Left".to_string(),
        content: "hello world\nalpha\nbravo".to_string(),
        ext_hint: Some("txt".to_string()),
    };
    let right = DiffTarget::ClipboardText {
        label: "Right".to_string(),
        content: "hello world\ncharlie\nbravo".to_string(),
        ext_hint: Some("txt".to_string()),
    };

    let mut state = DiffViewerState::new(left, right, DiffDisplayMode::Inline);
    assert!(state.is_computing);
    state.wait_for_completion();
    assert!(!state.is_computing);
    assert!(!state.result.raw_ansi.is_empty());

    for appearance in [
        little_clip_core::config::WorkbenchAppearance::Dark,
        little_clip_core::config::WorkbenchAppearance::Light,
    ] {
        let ctx = egui::Context::default();
        crate::ui_ext::configure_workbench_style(&ctx);
        let palette = crate::ui_appearance::palette(appearance);
        let console = crate::hex_to_c32(&palette.colors.console_surface);
        let mut close = false;
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                state.render_ui(ui, ctx, &palette, &mut close);
            });
        });
        assert!(output.shapes.iter().any(|clipped| {
            matches!(&clipped.shape, egui::Shape::Rect(rect) if rect.fill == console)
        }));
    }
}

#[test]
fn text_diff_tab_and_c_recompute_the_selected_mode() {
    fn send_shortcut(state: &mut DiffViewerState, key: egui::Key) {
        let ctx = egui::Context::default();
        let mut close = false;
        let _ = ctx.run(
            egui::RawInput {
                events: vec![egui::Event::Key {
                    key,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
                ..Default::default()
            },
            |ctx| state.handle_shortcuts(ctx, &mut close),
        );
        assert!(!close);
    }

    let left = DiffTarget::ClipboardText {
        label: "Left".to_string(),
        content: "alpha\nold line\nomega\n".to_string(),
        ext_hint: Some("txt".to_string()),
    };
    let right = DiffTarget::ClipboardText {
        label: "Right".to_string(),
        content: "alpha\nnew line\nomega\n".to_string(),
        ext_hint: Some("txt".to_string()),
    };
    let mut state = DiffViewerState::new(left, right, DiffDisplayMode::Inline);
    state.wait_for_completion();
    let inline_result = state.result.raw_ansi.clone();

    send_shortcut(&mut state, egui::Key::Tab);
    assert_eq!(state.mode, DiffDisplayMode::SideBySide);
    assert!(state.is_computing, "Tab should schedule a new text diff");
    assert_eq!(state.result.mode, DiffDisplayMode::Inline);
    state.wait_for_completion();
    assert_eq!(state.result.mode, DiffDisplayMode::SideBySide);
    let split_result = state.result.raw_ansi.clone();
    assert_ne!(split_result, inline_result);

    send_shortcut(&mut state, egui::Key::C);
    assert_eq!(state.mode, DiffDisplayMode::Inline);
    assert!(state.is_computing, "C should schedule a new text diff");
    assert_eq!(state.result.mode, DiffDisplayMode::SideBySide);
    state.wait_for_completion();
    assert_eq!(state.result.mode, DiffDisplayMode::Inline);
    assert_eq!(state.result.raw_ansi, inline_result);
}

#[test]
fn image_diff_tab_and_c_cycle_modes_without_recomputing() {
    fn send_shortcut(state: &mut DiffViewerState, key: egui::Key) {
        let ctx = egui::Context::default();
        let mut close = false;
        let _ = ctx.run(
            egui::RawInput {
                events: vec![egui::Event::Key {
                    key,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
                ..Default::default()
            },
            |ctx| state.handle_shortcuts(ctx, &mut close),
        );
        assert!(!close);
    }

    let image_target = |label: &str, color: [u8; 4]| {
        let rgba = image::RgbaImage::from_pixel(2, 2, image::Rgba(color));
        let mut bytes = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(rgba)
            .write_to(&mut bytes, image::ImageFormat::Png)
            .expect("encode tiny PNG fixture");
        DiffTarget::Image {
            label: label.to_owned(),
            path: None,
            bytes: bytes.into_inner(),
            width: 2,
            height: 2,
            mime: "image/png".to_owned(),
        }
    };
    let mut state = DiffViewerState::new(
        image_target("Left", [255, 0, 0, 255]),
        image_target("Right", [0, 0, 255, 255]),
        DiffDisplayMode::SideBySide,
    );
    state.wait_for_completion();
    let initial_result = state.image_result.as_ref().expect("image diff result");
    let result_address = std::ptr::from_ref(initial_result);
    let compute_started = state.compute_start_time;

    for (key, expected_mode) in [
        (egui::Key::Tab, ImageDiffViewMode::Overlay),
        (egui::Key::C, ImageDiffViewMode::MaskOnly),
        (egui::Key::Tab, ImageDiffViewMode::Split),
        (egui::Key::C, ImageDiffViewMode::ThreeUp),
    ] {
        send_shortcut(&mut state, key);
        assert_eq!(state.image_mode, expected_mode);
        assert!(!state.is_computing, "presentation change must not compute");
        assert!(state.image_receiver.is_none());
        assert_eq!(state.compute_start_time, compute_started);
        assert_eq!(
            std::ptr::from_ref(state.image_result.as_ref().unwrap()),
            result_address,
            "presentation change must retain the computed image result"
        );
    }
}

#[test]
fn saturated_diff_executor_reports_recoverable_error() {
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

    let left = DiffTarget::ClipboardText {
        label: "Left".to_string(),
        content: "left".to_string(),
        ext_hint: Some("txt".to_string()),
    };
    let right = DiffTarget::ClipboardText {
        label: "Right".to_string(),
        content: "right".to_string(),
        ext_hint: Some("txt".to_string()),
    };
    let mut state = DiffViewerState::new_with_jobs(left, right, DiffDisplayMode::Inline, &jobs);
    assert!(state.is_computing);
    state.wait_for_completion();

    assert!(!state.is_computing);
    assert!(state.text_receiver.is_none());
    assert_eq!(
        state.decode_error.as_deref(),
        Some("Diff computation failed: background job queue is full")
    );

    let ctx = egui::Context::default();
    crate::ui_ext::configure_workbench_style(&ctx);
    let palette =
        crate::ui_appearance::palette(little_clip_core::config::WorkbenchAppearance::Dark);
    let error_text = "Diff unavailable: Diff computation failed: background job queue is full";
    let output = ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(800.0, 500.0),
            )),
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                state.render_ui(ui, ctx, &palette, &mut false);
            });
        },
    );
    assert!(output.shapes.iter().any(|shape| {
        matches!(&shape.shape, egui::Shape::Text(text) if text.galley.text() == error_text)
    }));

    let _ = release_tx.send(());
}

#[test]
fn diff_external_tool_prefers_meld_then_falls_back_to_kdiff3() {
    let left_path = std::path::PathBuf::from("/tmp/diff-left.txt");
    let right_path = std::path::PathBuf::from("/tmp/diff-right.txt");
    let not_cancelled = std::sync::atomic::AtomicBool::new(false);

    let mut lookups = Vec::new();
    let mut launches = Vec::new();
    let result = launch_diff_tool_with(
        &left_path,
        &right_path,
        &not_cancelled,
        |command| {
            lookups.push(command.to_string());
            Some(std::path::PathBuf::from(format!("/fake/{command}")))
        },
        |app, left, right| {
            launches.push((
                app.file_name().unwrap().to_string_lossy().into_owned(),
                left.to_path_buf(),
                right.to_path_buf(),
            ));
            true
        },
    );
    assert_eq!(result, Ok(Some("🚀 Launched Meld comparison".to_string())));
    assert_eq!(lookups, vec!["meld"]);
    assert_eq!(
        launches,
        vec![("meld".to_string(), left_path.clone(), right_path.clone())]
    );

    let mut lookups = Vec::new();
    let mut launches = Vec::new();
    let result = launch_diff_tool_with(
        &left_path,
        &right_path,
        &not_cancelled,
        |command| {
            lookups.push(command.to_string());
            (command == "kdiff3").then(|| std::path::PathBuf::from("/fake/kdiff3"))
        },
        |app, left, right| {
            launches.push((
                app.file_name().unwrap().to_string_lossy().into_owned(),
                left.to_path_buf(),
                right.to_path_buf(),
            ));
            true
        },
    );
    assert_eq!(
        result,
        Ok(Some("🚀 Launched KDiff3 comparison".to_string()))
    );
    assert_eq!(lookups, vec!["meld", "kdiff3"]);
    assert_eq!(
        launches,
        vec![("kdiff3".to_string(), left_path.clone(), right_path.clone())]
    );

    let mut launches = Vec::new();
    let result = launch_diff_tool_with(
        &left_path,
        &right_path,
        &not_cancelled,
        |command| Some(std::path::PathBuf::from(format!("/fake/{command}"))),
        |app, left, right| {
            launches.push(app.file_name().unwrap().to_string_lossy().into_owned());
            let _ = (left, right);
            app.file_name().is_some_and(|name| name == "kdiff3")
        },
    );
    assert_eq!(
        result,
        Ok(Some("🚀 Launched KDiff3 comparison".to_string()))
    );
    assert_eq!(launches, vec!["meld", "kdiff3"]);
}

#[test]
fn external_handoff_reports_unavailable_launch_and_staging_outcomes() {
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    let fixture_dir = std::env::temp_dir().join(format!(
        "clip-diff-handoff-test-{}",
        uuid::Uuid::new_v4().simple()
    ));
    std::fs::create_dir_all(&fixture_dir).unwrap();
    let left_path = fixture_dir.join("left.txt");
    let right_path = fixture_dir.join("right.txt");
    std::fs::write(&left_path, b"left fixture").unwrap();
    std::fs::write(&right_path, b"right fixture").unwrap();
    let left = DiffHandoffTarget::Existing(left_path.clone());
    let right = DiffHandoffTarget::Existing(right_path.clone());
    let cancelled = std::sync::atomic::AtomicBool::new(false);

    let jobs = BackgroundJobs::new(1, 2);
    let unavailable_lookups = Arc::new(Mutex::new(Vec::new()));
    let lookup_log = unavailable_lookups.clone();
    let (ticket, unavailable_rx) = jobs.submit(Duration::MAX, move || {
        run_diff_handoff_with(
            left,
            right,
            &cancelled,
            move |command| {
                lookup_log.lock().unwrap().push(command.to_string());
                None
            },
            |_, _, _| panic!("spawn must not run when neither tool is installed"),
        )
    });
    let unavailable = unavailable_rx
        .recv_timeout(Duration::from_secs(1))
        .unwrap()
        .unwrap();
    assert_eq!(
        unavailable,
        Ok("⚠️ Neither Meld nor KDiff3 is installed".to_string())
    );
    assert_eq!(*unavailable_lookups.lock().unwrap(), vec!["meld", "kdiff3"]);
    drop(ticket);

    let left = DiffHandoffTarget::Existing(left_path.clone());
    let right = DiffHandoffTarget::Existing(right_path.clone());
    let cancelled = std::sync::atomic::AtomicBool::new(false);
    let attempts = Arc::new(Mutex::new(Vec::new()));
    let attempt_log = attempts.clone();
    let passed_paths = Arc::new(Mutex::new(Vec::new()));
    let path_log = passed_paths.clone();
    let (ticket, launch_rx) = jobs.submit(Duration::MAX, move || {
        run_diff_handoff_with(
            left,
            right,
            &cancelled,
            |command| Some(std::path::PathBuf::from(format!("/fake/{command}"))),
            move |app, left, right| {
                attempt_log
                    .lock()
                    .unwrap()
                    .push(app.file_name().unwrap().to_string_lossy().into_owned());
                path_log
                    .lock()
                    .unwrap()
                    .push((left.to_path_buf(), right.to_path_buf()));
                false
            },
        )
    });
    let launch_failure = launch_rx
        .recv_timeout(Duration::from_secs(1))
        .unwrap()
        .unwrap();
    assert_eq!(
        launch_failure,
        Err("Failed to launch available comparison tool(s): meld, kdiff3".to_string())
    );
    assert_eq!(*attempts.lock().unwrap(), vec!["meld", "kdiff3"]);
    assert!(
        passed_paths
            .lock()
            .unwrap()
            .iter()
            .all(|(lhs, rhs)| { lhs == &left_path && rhs == &right_path })
    );
    drop(ticket);

    let partial_stage_bytes =
        format!("diff-partial-stage-{}", uuid::Uuid::new_v4().simple()).into_bytes();
    let good_left = DiffHandoffTarget::Shared(
        Arc::new(DiffTarget::ClipboardText {
            label: "left stage fixture".into(),
            content: String::from_utf8(partial_stage_bytes.clone()).unwrap(),
            ext_hint: Some("txt".into()),
        }),
        "lhs".into(),
    );
    let bad_right = DiffHandoffTarget::Shared(
        Arc::new(DiffTarget::ClipboardText {
            label: "bad extension fixture".into(),
            content: "not written".into(),
            ext_hint: Some("missing-parent/file".into()),
        }),
        "rhs".into(),
    );
    let cancelled = std::sync::atomic::AtomicBool::new(false);
    let lookup_count = Arc::new(Mutex::new(0));
    let lookup_count_worker = lookup_count.clone();
    let (ticket, staging_rx) = jobs.submit(Duration::MAX, move || {
        run_diff_handoff_with(
            good_left,
            bad_right,
            &cancelled,
            move |_| {
                *lookup_count_worker.lock().unwrap() += 1;
                Some(std::path::PathBuf::from("/fake/meld"))
            },
            |_, _, _| true,
        )
    });
    let staging_failure = staging_rx
        .recv_timeout(Duration::from_secs(1))
        .unwrap()
        .unwrap();
    assert!(
        staging_failure
            .unwrap_err()
            .contains("Failed to stage diff rhs file")
    );
    assert_eq!(*lookup_count.lock().unwrap(), 0);
    drop(ticket);
    assert_staged_bytes_cleaned(&partial_stage_bytes);

    std::fs::remove_dir_all(fixture_dir).unwrap();
}

#[test]
fn handoff_failure_and_running_cancellation_remove_owned_staging_files() {
    use std::sync::mpsc;
    use std::time::Duration;

    let left_bytes =
        format!("diff-launch-fail-left-{}", uuid::Uuid::new_v4().simple()).into_bytes();
    let right_bytes =
        format!("diff-launch-fail-right-{}", uuid::Uuid::new_v4().simple()).into_bytes();
    let target = |bytes: &[u8], side: &str| {
        DiffHandoffTarget::Shared(
            std::sync::Arc::new(DiffTarget::ClipboardText {
                label: side.into(),
                content: String::from_utf8(bytes.to_vec()).unwrap(),
                ext_hint: Some("txt".into()),
            }),
            side.into(),
        )
    };

    let jobs = BackgroundJobs::new(1, 1);
    let cancelled = std::sync::atomic::AtomicBool::new(false);
    let left_target = target(&left_bytes, "lhs");
    let right_target = target(&right_bytes, "rhs");
    let (ticket, rx) = jobs.submit(Duration::MAX, move || {
        run_diff_handoff_with(
            left_target,
            right_target,
            &cancelled,
            |command| Some(std::path::PathBuf::from(format!("/fake/{command}"))),
            |_, _, _| false,
        )
    });
    assert!(
        rx.recv_timeout(Duration::from_secs(1))
            .unwrap()
            .unwrap()
            .is_err()
    );
    drop(ticket);
    assert_staged_bytes_cleaned(&left_bytes);
    assert_staged_bytes_cleaned(&right_bytes);

    let success_left_bytes =
        format!("diff-success-left-{}", uuid::Uuid::new_v4().simple()).into_bytes();
    let success_right_bytes =
        format!("diff-success-right-{}", uuid::Uuid::new_v4().simple()).into_bytes();
    let left_target = target(&success_left_bytes, "lhs");
    let right_target = target(&success_right_bytes, "rhs");
    let cancelled = std::sync::atomic::AtomicBool::new(false);
    let (paths_tx, paths_rx) = mpsc::channel();
    let (ticket, rx) = jobs.submit(Duration::MAX, move || {
        run_diff_handoff_with(
            left_target,
            right_target,
            &cancelled,
            |_| Some(std::path::PathBuf::from("/fake/meld")),
            move |_, left, right| {
                let _ = paths_tx.send((left.to_path_buf(), right.to_path_buf()));
                true
            },
        )
    });
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(1)).unwrap().unwrap(),
        Ok("🚀 Launched Meld comparison".to_string())
    );
    let (left_path, right_path) = paths_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    assert_eq!(std::fs::read(&left_path).unwrap(), success_left_bytes);
    assert_eq!(std::fs::read(&right_path).unwrap(), success_right_bytes);
    std::fs::remove_file(left_path).unwrap();
    std::fs::remove_file(right_path).unwrap();
    drop(ticket);

    let left_bytes = format!("diff-cancel-left-{}", uuid::Uuid::new_v4().simple()).into_bytes();
    let right_bytes = format!("diff-cancel-right-{}", uuid::Uuid::new_v4().simple()).into_bytes();
    let cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let worker_cancelled = cancelled.clone();
    let left_target = target(&left_bytes, "lhs");
    let right_target = target(&right_bytes, "rhs");
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let (spawn_tx, spawn_rx) = mpsc::channel();
    let (ticket, rx) = jobs.submit(Duration::MAX, move || {
        run_diff_handoff_with(
            left_target,
            right_target,
            &worker_cancelled,
            move |_| {
                let _ = entered_tx.send(());
                let _ = release_rx.recv();
                Some(std::path::PathBuf::from("/fake/meld"))
            },
            move |_, _, _| {
                let _ = spawn_tx.send(());
                true
            },
        )
    });
    entered_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    cancelled.store(true, std::sync::atomic::Ordering::Release);
    release_tx.send(()).unwrap();
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(1)).unwrap().unwrap(),
        Err("External handoff cancelled".to_string())
    );
    assert!(spawn_rx.try_recv().is_err());
    drop(ticket);
    assert_staged_bytes_cleaned(&left_bytes);
    assert_staged_bytes_cleaned(&right_bytes);
}

#[test]
fn replacing_and_dropping_diff_viewer_cancel_queued_handoffs() {
    use std::sync::{Arc, atomic::AtomicUsize, mpsc};
    use std::time::Duration;

    let jobs = BackgroundJobs::new(1, 3);
    let (blocker_started_tx, blocker_started_rx) = mpsc::channel();
    let (blocker_release_tx, blocker_release_rx) = mpsc::channel();
    let (_blocker_ticket, blocker_rx) = jobs.submit(Duration::MAX, move || {
        let _ = blocker_started_tx.send(());
        let _ = blocker_release_rx.recv();
    });
    blocker_started_rx
        .recv_timeout(Duration::from_secs(1))
        .unwrap();

    let mut state = DiffViewerState::new(
        DiffTarget::ClipboardText {
            label: "left".into(),
            content: "left fixture".into(),
            ext_hint: Some("txt".into()),
        },
        DiffTarget::ClipboardText {
            label: "right".into(),
            content: "right fixture".into(),
            ext_hint: Some("txt".into()),
        },
        DiffDisplayMode::Inline,
    );
    state.wait_for_completion();

    let lookups = Arc::new(AtomicUsize::new(0));
    let first_lookups = lookups.clone();
    state.trigger_escape_hatch_on(
        0.0,
        &jobs,
        move |_| {
            first_lookups.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            None
        },
        |_, _, _| false,
    );
    let second_lookups = lookups.clone();
    state.trigger_escape_hatch_on(
        0.1,
        &jobs,
        move |_| {
            second_lookups.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            None
        },
        |_, _, _| false,
    );
    let closing_flag = state.handoff_cancel.as_ref().unwrap().0.clone();
    drop(state);
    assert!(closing_flag.load(std::sync::atomic::Ordering::Acquire));

    let (sentinel_ticket, sentinel_rx) = jobs.submit(Duration::MAX, || ());
    blocker_release_tx.send(()).unwrap();
    blocker_rx
        .recv_timeout(Duration::from_secs(1))
        .unwrap()
        .unwrap();
    sentinel_rx
        .recv_timeout(Duration::from_secs(1))
        .unwrap()
        .unwrap();
    assert_eq!(lookups.load(std::sync::atomic::Ordering::Relaxed), 0);
    drop(sentinel_ticket);
}

#[test]
fn dropping_diff_viewer_cancels_queued_compute() {
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

    let left = DiffTarget::ClipboardText {
        label: "Left".to_string(),
        content: "left".to_string(),
        ext_hint: Some("txt".to_string()),
    };
    let right = DiffTarget::ClipboardText {
        label: "Right".to_string(),
        content: "right".to_string(),
        ext_hint: Some("txt".to_string()),
    };
    let mut state = DiffViewerState::new_with_jobs(left, right, DiffDisplayMode::Inline, &jobs);
    let result_rx = state.text_receiver.take().unwrap();
    drop(state);

    let _ = release_tx.send(());
    assert!(matches!(
        result_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
        Err(little_clip_core::JobError::Cancelled)
    ));
}

#[test]
fn ansi_long_line_scrolls_to_full_tail_with_readable_colors_in_both_appearances() {
    use little_clip_core::config::WorkbenchAppearance;
    use little_clip_core::palette::contrast_ratio;

    let prefix = "x".repeat(5_000);
    let ansi = format!(
        "{prefix}\x1b[31mSTANDARD_END\x1b[0m \x1b[38;2;14;43;61mTRUECOLOR_END\x1b[0m \x1b[38;5;196m256COLOR_END\x1b[0m"
    );
    let layout_job = ansi_to_layout_job(&ansi, crate::ui_ext::MONO_FONT_SIZE);
    let plain = layout_job.text.clone();
    let marker = "256COLOR_END";
    let marker_start = plain.find(marker).expect("plain ANSI tail marker");

    for appearance in [WorkbenchAppearance::Light, WorkbenchAppearance::Dark] {
        let palette = crate::ui_appearance::palette(appearance);
        let console = crate::hex_to_c32(&palette.colors.console_surface);
        for token in ["STANDARD_END", "TRUECOLOR_END", "256COLOR_END"] {
            let section = layout_job
                .sections
                .iter()
                .find(|section| layout_job.text[section.byte_range.clone()].contains(token))
                .unwrap_or_else(|| panic!("missing ANSI section for {token}"));
            let foreground = section.format.color;
            assert!(
                contrast_ratio(
                    (console.r(), console.g(), console.b()),
                    (foreground.r(), foreground.g(), foreground.b()),
                ) >= 4.5,
                "{appearance:?}: {token} foreground lacks contrast against console surface"
            );
        }

        let lhs = DiffTarget::ClipboardText {
            label: "Left".to_string(),
            content: "left placeholder".to_string(),
            ext_hint: Some("txt".to_string()),
        };
        let rhs = DiffTarget::ClipboardText {
            label: "Right".to_string(),
            content: "right placeholder".to_string(),
            ext_hint: Some("txt".to_string()),
        };
        let mut state = DiffViewerState::new(lhs, rhs, DiffDisplayMode::Inline);
        state.result = DiffResult {
            engine_name: "ANSI fixture".to_string(),
            raw_ansi: ansi.clone(),
            is_identical: false,
            mode: DiffDisplayMode::Inline,
        };
        state.layout_job = layout_job.clone();
        state.is_computing = false;
        state.text_receiver = None;

        let ctx = egui::Context::default();
        crate::ui_ext::configure_workbench_style(&ctx);
        let screen_rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(900.0, 620.0));
        let mut time = 0.0;
        let mut render = |events: Vec<egui::Event>| {
            time += 0.05;
            ctx.run(
                egui::RawInput {
                    screen_rect: Some(screen_rect),
                    time: Some(time),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        state.render_ui(ui, ctx, &palette, &mut false);
                    });
                },
            )
        };

        let initial = render(Vec::new());
        let find_line = |output: &egui::FullOutput| {
            output
                .shapes
                .iter()
                .find_map(|clipped| match &clipped.shape {
                    egui::Shape::Text(text) if text.galley.text() == plain => {
                        Some((text.pos, text.galley.size(), clipped.clip_rect))
                    }
                    _ => None,
                })
        };
        let find_console_rect = |output: &egui::FullOutput| {
            output
                .shapes
                .iter()
                .filter_map(|clipped| match &clipped.shape {
                    egui::Shape::Rect(rect) if rect.fill == console => Some(rect.rect),
                    _ => None,
                })
                .max_by(|a, b| (a.width() * a.height()).total_cmp(&(b.width() * b.height())))
        };
        let (initial_pos, initial_size, viewport) =
            find_line(&initial).expect("full ANSI line should render without elision");
        assert!(
            initial_size.x > viewport.width() * 10.0,
            "long-line size {} should greatly exceed viewport width {}",
            initial_size.x,
            viewport.width()
        );
        assert!(plain.ends_with(marker));

        let console_rect = find_console_rect(&initial).expect("console surface should render");
        let content_viewport = console_rect.shrink(crate::ui_ext::SPACE_SM);
        let scroll_point = content_viewport.center();
        let _ = render(vec![
            egui::Event::PointerMoved(scroll_point),
            egui::Event::Scroll(egui::vec2(-100_000.0, 0.0)),
        ]);
        let mut scrolled = None;
        for _ in 0..24 {
            scrolled = Some(render(Vec::new()));
        }
        let scrolled = scrolled.unwrap();
        let (scrolled_pos, scrolled_size, scrolled_viewport) =
            find_line(&scrolled).expect("scrolled ANSI line should retain its complete galley");
        let scrolled_console_rect =
            find_console_rect(&scrolled).expect("scrolled console surface should render");
        let scrolled_content_viewport = scrolled_console_rect.shrink(crate::ui_ext::SPACE_SM);
        assert_eq!(scrolled_size, initial_size);
        assert!(
            scrolled_pos.x < initial_pos.x - viewport.width(),
            "horizontal pan should advance across the long line: {initial_pos:?} -> {scrolled_pos:?}"
        );
        let glyph_width = scrolled_size.x / plain.chars().count() as f32;
        let marker_rect = egui::Rect::from_min_size(
            egui::pos2(
                scrolled_pos.x + marker_start as f32 * glyph_width,
                scrolled_pos.y,
            ),
            egui::vec2(marker.chars().count() as f32 * glyph_width, scrolled_size.y),
        );
        assert!(
            scrolled_viewport.contains_rect(marker_rect)
                && scrolled_content_viewport.contains_rect(marker_rect),
            "{appearance:?}: far ANSI marker should be brought fully into the console viewport: {marker_rect:?} in {scrolled_content_viewport:?}"
        );
        assert_eq!(
            find_line(&scrolled).unwrap().1.x,
            initial_size.x,
            "long ANSI line must not be elided after scrolling"
        );
    }
}

#[test]
fn external_handoff_stages_both_exact_operands_with_expected_extensions() {
    let lhs = std::sync::Arc::new(DiffTarget::ClipboardText {
        label: "left fixture".into(),
        content: "left exact\nclipboard bytes\n".into(),
        ext_hint: Some("md".into()),
    });
    let rhs = std::sync::Arc::new(DiffTarget::Image {
        label: "right fixture".into(),
        path: None,
        bytes: b"right encoded image bytes".to_vec(),
        width: 1,
        height: 1,
        mime: "image/png".into(),
    });

    let lhs_path = DiffHandoffTarget::from_target(&lhs, "lhs").stage().unwrap();
    let rhs_path = DiffHandoffTarget::from_target(&rhs, "rhs").stage().unwrap();
    assert_eq!(
        lhs_path.path.extension().and_then(|ext| ext.to_str()),
        Some("md")
    );
    assert_eq!(
        rhs_path.path.extension().and_then(|ext| ext.to_str()),
        Some("png")
    );
    assert_eq!(
        std::fs::read(&lhs_path.path).unwrap(),
        b"left exact\nclipboard bytes\n"
    );
    assert_eq!(
        std::fs::read(&rhs_path.path).unwrap(),
        b"right encoded image bytes"
    );
}

#[test]
fn external_handoff_reports_temp_write_failure() {
    let target = std::sync::Arc::new(DiffTarget::ClipboardText {
        label: "fixture".into(),
        content: "data".into(),
        ext_hint: Some("missing-parent/file".into()),
    });
    assert!(
        DiffHandoffTarget::from_target(&target, "lhs")
            .stage()
            .unwrap_err()
            .contains("Failed to stage diff lhs file")
    );
}
