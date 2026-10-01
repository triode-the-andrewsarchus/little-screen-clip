use super::*;

fn staged_files_with_bytes(expected: &[u8]) -> Vec<std::path::PathBuf> {
    std::fs::read_dir(std::env::temp_dir())
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("clip_bin_"))
                && std::fs::read(path).is_ok_and(|bytes| bytes == expected)
        })
        .collect()
}

fn assert_staged_bytes_cleaned(bytes: &[u8]) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
    while !staged_files_with_bytes(bytes).is_empty() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(staged_files_with_bytes(bytes).is_empty());
}

#[test]
fn binary_handoff_stages_exact_left_and_right_bytes_as_bin_files() {
    let left_bytes = [0, 255, 0, 0x7f, 13, 10];
    let right_bytes = [0xfe, 0x00, 0x01, 0x80, 0x42];
    let left_path = stage_binary_handoff(&left_bytes, "lhs").unwrap();
    let right_path = stage_binary_handoff(&right_bytes, "rhs").unwrap();

    assert_eq!(
        left_path.extension().and_then(|ext| ext.to_str()),
        Some("bin")
    );
    assert_eq!(
        right_path.extension().and_then(|ext| ext.to_str()),
        Some("bin")
    );
    assert_eq!(std::fs::read(&left_path).unwrap(), left_bytes);
    assert_eq!(std::fs::read(&right_path).unwrap(), right_bytes);

    std::fs::remove_file(left_path).unwrap();
    std::fs::remove_file(right_path).unwrap();
}

#[test]
fn binary_handoff_write_failures_skip_external_tool_lookup_and_spawn() {
    use std::cell::Cell;

    let fixture_dir = std::env::temp_dir().join(format!(
        "clip-bin-stage-error-{}",
        uuid::Uuid::new_v4().simple()
    ));
    std::fs::create_dir_all(&fixture_dir).unwrap();
    let blocked_parent = fixture_dir.join("not-a-directory");
    std::fs::write(&blocked_parent, b"fixture blocker").unwrap();
    let cancelled = std::sync::atomic::AtomicBool::new(false);
    let lookups = Cell::new(0);
    let launches = Cell::new(0);

    let left_failure = run_binary_handoff_with(
        b"left bytes",
        b"right bytes",
        &cancelled,
        |_, side| {
            stage_binary_handoff_at(
                b"left bytes",
                side,
                blocked_parent.join("left-stage-fail.bin"),
            )
        },
        |_| {
            lookups.set(lookups.get() + 1);
            Some(std::path::PathBuf::from("/fake/imhex"))
        },
        |_, _, _| {
            launches.set(launches.get() + 1);
            true
        },
    );
    assert!(
        left_failure
            .unwrap_err()
            .contains("Failed to write lhs temp file")
    );
    assert_eq!(lookups.get(), 0);
    assert_eq!(launches.get(), 0);

    let staged_left = fixture_dir.join("successful-left.bin");
    let right_failure = run_binary_handoff_with(
        b"left bytes",
        b"right bytes",
        &cancelled,
        |bytes, side| {
            let path = if side == "lhs" {
                staged_left.clone()
            } else {
                blocked_parent.join("right-stage-fail.bin")
            };
            stage_binary_handoff_at(bytes, side, path)
        },
        |_| {
            lookups.set(lookups.get() + 1);
            Some(std::path::PathBuf::from("/fake/imhex"))
        },
        |_, _, _| {
            launches.set(launches.get() + 1);
            true
        },
    );
    assert!(
        right_failure
            .unwrap_err()
            .contains("Failed to write rhs temp file")
    );
    assert_eq!(lookups.get(), 0);
    assert_eq!(launches.get(), 0);
    assert!(
        !staged_left.exists(),
        "partial staging output should be cleaned"
    );
    std::fs::remove_dir_all(fixture_dir).unwrap();
}

#[test]
fn binary_external_tool_prefers_imhex_then_uses_xdg_open_fallback() {
    let left = std::path::PathBuf::from("/private/left.bin");
    let right = std::path::PathBuf::from("/private/right.bin");
    let not_cancelled = std::sync::atomic::AtomicBool::new(false);

    let mut lookups = Vec::new();
    let mut launches = Vec::new();
    let result = launch_binary_diff_tool_with(
        &left,
        &right,
        &not_cancelled,
        |command| {
            lookups.push(command.to_string());
            Some(std::path::PathBuf::from(format!("/fake/{command}")))
        },
        |app, left_arg, right_arg| {
            launches.push((
                app.file_name().unwrap().to_string_lossy().into_owned(),
                left_arg.to_path_buf(),
                right_arg.map(std::path::Path::to_path_buf),
            ));
            true
        },
    );
    assert_eq!(result, Ok(BinaryToolLaunch::ImHex));
    assert_eq!(lookups, vec!["imhex"]);
    assert_eq!(
        launches,
        vec![("imhex".to_string(), left.clone(), Some(right.clone()))]
    );

    let mut lookups = Vec::new();
    let mut launches = Vec::new();
    let result = launch_binary_diff_tool_with(
        &left,
        &right,
        &not_cancelled,
        |command| {
            lookups.push(command.to_string());
            (command == "xdg-open").then(|| std::path::PathBuf::from("/fake/xdg-open"))
        },
        |app, left_arg, right_arg| {
            launches.push((
                app.file_name().unwrap().to_string_lossy().into_owned(),
                left_arg.to_path_buf(),
                right_arg.map(std::path::Path::to_path_buf),
            ));
            true
        },
    );
    assert_eq!(result, Ok(BinaryToolLaunch::XdgOpen));
    assert_eq!(lookups, vec!["imhex", "xdg-open"]);
    assert_eq!(launches, vec![("xdg-open".to_string(), left.clone(), None)]);

    let mut launches = Vec::new();
    let result = launch_binary_diff_tool_with(
        &left,
        &right,
        &not_cancelled,
        |command| Some(std::path::PathBuf::from(format!("/fake/{command}"))),
        |app, left_arg, right_arg| {
            launches.push(app.file_name().unwrap().to_string_lossy().into_owned());
            let _ = (left_arg, right_arg);
            app.file_name().is_some_and(|name| name == "xdg-open")
        },
    );
    assert_eq!(result, Ok(BinaryToolLaunch::XdgOpen));
    assert_eq!(launches, vec!["imhex", "xdg-open"]);

    let mut attempts = Vec::new();
    let result = launch_binary_diff_tool_with(
        &left,
        &right,
        &not_cancelled,
        |command| (command == "xdg-open").then(|| std::path::PathBuf::from("/fake/xdg-open")),
        |app, _, _| {
            attempts.push(app.file_name().unwrap().to_string_lossy().into_owned());
            false
        },
    );
    assert_eq!(
        result,
        Err("Failed to launch available external tool(s): xdg-open".to_string())
    );
    assert_eq!(attempts, vec!["xdg-open"]);

    let mut attempts = Vec::new();
    let result = launch_binary_diff_tool_with(
        &left,
        &right,
        &not_cancelled,
        |command| Some(std::path::PathBuf::from(format!("/fake/{command}"))),
        |app, _, _| {
            attempts.push(app.file_name().unwrap().to_string_lossy().into_owned());
            false
        },
    );
    assert_eq!(
        result,
        Err("Failed to launch available external tool(s): imhex, xdg-open".to_string())
    );
    assert_eq!(attempts, vec!["imhex", "xdg-open"]);

    let mut lookups = Vec::new();
    let result = launch_binary_diff_tool_with(
        &left,
        &right,
        &not_cancelled,
        |command| {
            lookups.push(command.to_string());
            None
        },
        |_, _, _| panic!("spawn must not run when no opener is installed"),
    );
    assert_eq!(result, Ok(BinaryToolLaunch::Unavailable));
    assert_eq!(lookups, vec!["imhex", "xdg-open"]);
}

#[test]
fn binary_handoff_cleans_staged_files_unless_an_opener_needs_them() {
    use std::sync::mpsc;
    use std::time::Duration;

    let left_bytes =
        format!("binary-lifecycle-left-{}", uuid::Uuid::new_v4().simple()).into_bytes();
    let right_bytes =
        format!("binary-lifecycle-right-{}", uuid::Uuid::new_v4().simple()).into_bytes();
    let jobs = BackgroundJobs::new(1, 1);

    let left = left_bytes.clone();
    let right = right_bytes.clone();
    let cancelled = std::sync::atomic::AtomicBool::new(false);
    let (ticket, failed_rx) = jobs.submit(Duration::MAX, move || {
        run_binary_handoff_with(
            &left,
            &right,
            &cancelled,
            stage_binary_handoff,
            |command| Some(std::path::PathBuf::from(format!("/fake/{command}"))),
            |_, _, _| false,
        )
    });
    assert!(
        failed_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .unwrap()
            .is_err()
    );
    drop(ticket);
    assert_staged_bytes_cleaned(&left_bytes);
    assert_staged_bytes_cleaned(&right_bytes);

    let left = left_bytes.clone();
    let right = right_bytes.clone();
    let cancelled = std::sync::atomic::AtomicBool::new(false);
    let (ticket, unavailable_rx) = jobs.submit(Duration::MAX, move || {
        run_binary_handoff_with(
            &left,
            &right,
            &cancelled,
            stage_binary_handoff,
            |_| None,
            |_, _, _| panic!("spawn must not run when no opener is installed"),
        )
    });
    assert_eq!(
        unavailable_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .unwrap(),
        Ok("⚠️ Neither ImHex nor xdg-open is installed".to_string())
    );
    drop(ticket);
    assert_staged_bytes_cleaned(&left_bytes);
    assert_staged_bytes_cleaned(&right_bytes);

    let partial_left =
        format!("binary-partial-left-{}", uuid::Uuid::new_v4().simple()).into_bytes();
    let left = partial_left.clone();
    let cancelled = std::sync::atomic::AtomicBool::new(false);
    let (ticket, partial_rx) = jobs.submit(Duration::MAX, move || {
        run_binary_handoff_with(
            &left,
            b"right unused",
            &cancelled,
            |bytes, side| {
                if side == "rhs" {
                    Err("Failed to write rhs temp file: forced test error".to_string())
                } else {
                    stage_binary_handoff(bytes, side)
                }
            },
            |_| panic!("staging failure must skip opener lookup"),
            |_, _, _| panic!("staging failure must skip opener spawn"),
        )
    });
    assert!(
        partial_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .unwrap()
            .unwrap_err()
            .contains("Failed to write rhs temp file")
    );
    drop(ticket);
    assert_staged_bytes_cleaned(&partial_left);

    let cancelled_left =
        format!("binary-cancel-left-{}", uuid::Uuid::new_v4().simple()).into_bytes();
    let cancelled_right =
        format!("binary-cancel-right-{}", uuid::Uuid::new_v4().simple()).into_bytes();
    let worker_left = cancelled_left.clone();
    let worker_right = cancelled_right.clone();
    let cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let worker_cancelled = cancelled.clone();
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let (spawn_tx, spawn_rx) = mpsc::channel();
    let (ticket, cancelled_rx) = jobs.submit(Duration::MAX, move || {
        run_binary_handoff_with(
            &worker_left,
            &worker_right,
            &worker_cancelled,
            stage_binary_handoff,
            move |_| {
                let _ = entered_tx.send(());
                let _ = release_rx.recv();
                Some(std::path::PathBuf::from("/fake/imhex"))
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
        cancelled_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .unwrap(),
        Err("External handoff cancelled".to_string())
    );
    assert!(spawn_rx.try_recv().is_err());
    drop(ticket);
    assert_staged_bytes_cleaned(&cancelled_left);
    assert_staged_bytes_cleaned(&cancelled_right);

    let (imhex_paths_tx, imhex_paths_rx) = mpsc::channel();
    let left = left_bytes.clone();
    let right = right_bytes.clone();
    let cancelled = std::sync::atomic::AtomicBool::new(false);
    let (ticket, imhex_rx) = jobs.submit(Duration::MAX, move || {
        run_binary_handoff_with(
            &left,
            &right,
            &cancelled,
            stage_binary_handoff,
            |command| Some(std::path::PathBuf::from(format!("/fake/{command}"))),
            move |_, left, right| {
                let _ = imhex_paths_tx.send((left.to_path_buf(), right.unwrap().to_path_buf()));
                true
            },
        )
    });
    assert_eq!(
        imhex_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .unwrap(),
        Ok("🚀 Launched ImHex hex editor".to_string())
    );
    let (imhex_left_path, imhex_right_path) =
        imhex_paths_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    assert_eq!(std::fs::read(&imhex_left_path).unwrap(), left_bytes);
    assert_eq!(std::fs::read(&imhex_right_path).unwrap(), right_bytes);
    std::fs::remove_file(imhex_left_path).unwrap();
    std::fs::remove_file(imhex_right_path).unwrap();
    drop(ticket);

    let (xdg_left_tx, xdg_left_rx) = mpsc::channel();
    let left = left_bytes.clone();
    let right = right_bytes.clone();
    let cancelled = std::sync::atomic::AtomicBool::new(false);
    let (ticket, xdg_rx) = jobs.submit(Duration::MAX, move || {
        run_binary_handoff_with(
            &left,
            &right,
            &cancelled,
            stage_binary_handoff,
            |command| (command == "xdg-open").then(|| std::path::PathBuf::from("/fake/xdg-open")),
            move |_, left, right| {
                let _ = xdg_left_tx.send((left.to_path_buf(), right.is_none()));
                true
            },
        )
    });
    assert_eq!(
        xdg_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .unwrap(),
        Ok("🚀 Opened left binary file with system default".to_string())
    );
    let (xdg_left_path, xdg_received_only_left) =
        xdg_left_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    assert!(xdg_received_only_left);
    assert_eq!(std::fs::read(&xdg_left_path).unwrap(), left_bytes);
    assert_staged_bytes_cleaned(&right_bytes);
    std::fs::remove_file(xdg_left_path).unwrap();
    drop(ticket);
}

#[test]
fn replacing_and_dropping_binary_viewer_cancel_queued_handoffs() {
    use std::sync::{Arc, atomic::AtomicUsize, mpsc};
    use std::time::Duration;

    let mut state = BinaryDiffViewState::new(
        "left".into(),
        b"left bytes".to_vec(),
        "right".into(),
        b"right bytes".to_vec(),
    );
    let (generation, result) = state
        .receiver
        .take()
        .unwrap()
        .recv_timeout(Duration::from_secs(1))
        .unwrap()
        .unwrap();
    assert_eq!(generation, state.current_generation);
    state.result = Some(result);
    state.is_computing = false;
    state.compute_ticket = None;

    let jobs = BackgroundJobs::new(1, 3);
    let (blocker_started_tx, blocker_started_rx) = mpsc::channel();
    let (blocker_release_tx, blocker_release_rx) = mpsc::channel();
    let (blocker_ticket, blocker_rx) = jobs.submit(Duration::MAX, move || {
        let _ = blocker_started_tx.send(());
        let _ = blocker_release_rx.recv();
    });
    blocker_started_rx
        .recv_timeout(Duration::from_secs(1))
        .unwrap();

    let stage_calls = Arc::new(AtomicUsize::new(0));
    let lookup_calls = Arc::new(AtomicUsize::new(0));
    let first_stages = stage_calls.clone();
    let first_lookups = lookup_calls.clone();
    state.trigger_escape_hatch_on(
        0.0,
        &jobs,
        move |_, _| {
            first_stages.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            Ok(std::path::PathBuf::from("/private/first.bin"))
        },
        move |_| {
            first_lookups.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            None
        },
        |_, _, _| false,
    );
    let first_flag = state.handoff_cancel.as_ref().unwrap().0.clone();

    let second_stages = stage_calls.clone();
    let second_lookups = lookup_calls.clone();
    state.trigger_escape_hatch_on(
        0.1,
        &jobs,
        move |_, _| {
            second_stages.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            Ok(std::path::PathBuf::from("/private/second.bin"))
        },
        move |_| {
            second_lookups.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            None
        },
        |_, _, _| false,
    );
    assert!(first_flag.load(std::sync::atomic::Ordering::Acquire));
    let close_flag = state.handoff_cancel.as_ref().unwrap().0.clone();
    drop(state);
    assert!(close_flag.load(std::sync::atomic::Ordering::Acquire));

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
    assert_eq!(stage_calls.load(std::sync::atomic::Ordering::Relaxed), 0);
    assert_eq!(lookup_calls.load(std::sync::atomic::Ordering::Relaxed), 0);
    drop(sentinel_ticket);
    drop(blocker_ticket);
}

#[test]
fn dropping_handoff_guard_cancels_stale_publication() {
    let cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    drop(HandoffCancelGuard(cancelled.clone()));
    assert!(cancelled.load(std::sync::atomic::Ordering::Acquire));
}
