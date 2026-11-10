use super::*;

#[test]
fn python_minimap_ends_comment_scope_at_each_source_newline() {
    let text = Arc::new(
        "# module comment\nimport os\ndef greet(name):\n    return \"hello\"\n".to_owned(),
    );
    let syntax = find_syntax("py");
    let MinimapResult::Lines(lines) =
        build_minimap_lines(&Arc::downgrade(&text), syntax, &AtomicBool::new(false))
    else {
        panic!("Python minimap should complete")
    };
    let mut highlighter = HighlightLines::new(syntax, &THEME_SET.themes["base16-eighties.dark"]);
    for ((_, actual), source) in lines.iter().zip(text.split_inclusive('\n')) {
        let expected = build_minimap_line(source, &mut highlighter);
        assert_eq!(actual.segments.len(), expected.segments.len());
        for (actual, expected) in actual.segments.iter().zip(&expected.segments) {
            assert_eq!(actual.color, expected.color, "syntax colors for {source:?}");
        }
    }
    assert!(
        lines[1]
            .1
            .segments
            .iter()
            .any(|segment| segment.color != lines[0].1.segments[0].color)
    );
}

#[test]
fn rust_minimap_handles_unicode_before_line_ending_whitespace() {
    let text = Arc::new("//界\n/*\n界\n*/\nfn main() {}\n".to_owned());
    let MinimapResult::Lines(lines) = build_minimap_lines(
        &Arc::downgrade(&text),
        find_syntax("rs"),
        &AtomicBool::new(false),
    ) else {
        panic!("Unicode Rust minimap should complete")
    };
    for index in [0, 1, 2, 3, 4] {
        assert!(
            !lines[index].1.segments.is_empty(),
            "source line {index} needs visible blocks"
        );
    }
    assert_eq!(lines[2].1.segments[0].start_column, 0);
    assert_eq!(lines[2].1.segments[0].end_column, 1);

    // A worker panic disconnects the receiver and leaves only the scrollbar.
    // Verify that this source reaches the published cache as well.
    let jobs = BackgroundJobs::new(1, 1);
    let mut cache = MinimapCache::default();
    cache.start(Arc::clone(&text), "rs".to_owned(), &jobs);
    let deadline = Instant::now() + Duration::from_secs(5);
    while !cache.poll() {
        assert!(
            Instant::now() < deadline,
            "Unicode minimap worker did not finish"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(cache.ready);
    assert!(!cache.failed);
    assert_eq!(cache.lines[2].1.segments, lines[2].1.segments);
}

#[test]
fn test_minimap_cache_uses_syntax_colored_blocks() {
    let text = Arc::new("fn main() {\n    let answer = 42;\n}\n".to_owned());
    let lines = build_minimap_lines(
        &Arc::downgrade(&text),
        find_syntax("rs"),
        &AtomicBool::new(false),
    );
    let MinimapResult::Lines(lines) = lines else {
        panic!("small source should build minimap lines")
    };

    assert_eq!(lines.len(), 4, "one minimap row per source line");
    assert!(
        lines
            .iter()
            .take(3)
            .all(|(_, line)| !line.segments.is_empty())
    );
    assert!(
        lines
            .iter()
            .flat_map(|(_, line)| &line.segments)
            .all(|segment| {
                segment.start_column < segment.end_column && segment.end_column <= MINIMAP_COLUMNS
            })
    );
}

#[test]
fn test_minimap_builder_cancellation_returns_no_partial_samples() {
    let text = Arc::new("fn main() {}\n".repeat(10_000));
    let cancelled = AtomicBool::new(true);
    assert!(matches!(
        build_minimap_lines(&Arc::downgrade(&text), find_syntax("rs"), &cancelled),
        MinimapResult::Cancelled
    ));
}

#[test]
fn test_minimap_builder_preserves_complete_long_lines() {
    let text = Arc::new(format!("{}\nlet after = 1;", "x".repeat(300_000)));
    let MinimapResult::Lines(lines) = build_minimap_lines(
        &Arc::downgrade(&text),
        find_syntax("rs"),
        &AtomicBool::new(false),
    ) else {
        panic!("long logical lines remain supported")
    };
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0].0, 0);
    assert_eq!(lines[1].0, 1);
}

#[test]
fn minimap_syntax_limit_is_inclusive_and_counts_utf8_bytes() {
    let at_limit = "x".repeat(MINIMAP_MAX_SYNTAX_LINE_BYTES);
    assert_eq!(minimap_source_shape(&at_limit, || false), Some((1, false)));
    let exact_source = Arc::new(at_limit.clone());
    let MinimapResult::Lines(exact_lines) = build_minimap_lines(
        &Arc::downgrade(&exact_source),
        find_syntax("txt"),
        &AtomicBool::new(false),
    ) else {
        panic!("the inclusive boundary should retain syntax minimap output")
    };
    assert_ne!(exact_lines[0].1.segments[0].color, MINIMAP_NEUTRAL_COLOR);
    assert_eq!(
        minimap_source_shape(&(at_limit.clone() + "x"), || false),
        Some((1, true))
    );
    assert_eq!(
        minimap_source_shape(&(at_limit + "\nafter"), || false),
        Some((2, false))
    );
    let multibyte = "界".repeat(MINIMAP_MAX_SYNTAX_LINE_BYTES / 3);
    assert_eq!(minimap_source_shape(&multibyte, || false), Some((1, false)));
    assert_eq!(
        minimap_source_shape(&(multibyte + "界"), || false),
        Some((1, true))
    );
}

#[test]
fn minimap_shape_scan_checks_cancellation_within_64_kib() {
    let text = "x".repeat(3 * MINIMAP_CANCELLATION_CHECK_BYTES);
    let mut checks = 0;
    assert_eq!(
        minimap_source_shape(&text, || {
            checks += 1;
            checks == 3
        }),
        None
    );
    assert_eq!(checks, 3);
}

#[test]
fn oversized_line_makes_the_entire_minimap_neutral_and_keeps_later_rows() {
    let text = Arc::new(format!(
        "  let before = 1;\n/*{}\nlet after = 2;\n",
        "x".repeat(MINIMAP_MAX_SYNTAX_LINE_BYTES)
    ));
    let MinimapResult::Lines(lines) = build_minimap_lines(
        &Arc::downgrade(&text),
        find_syntax("rs"),
        &AtomicBool::new(false),
    ) else {
        panic!("oversized source should publish a neutral minimap")
    };
    assert_eq!(lines.len(), 4);
    assert_eq!(
        lines.iter().map(|(row, _)| *row).collect::<Vec<_>>(),
        (0..4).collect::<Vec<_>>()
    );
    assert!(lines[3].1.segments.is_empty());
    for (_, line) in lines.iter().take(3) {
        assert_eq!(line.segments.len(), 1);
        let segment = &line.segments[0];
        assert_eq!(segment.color, MINIMAP_NEUTRAL_COLOR);
        assert!(segment.start_column < segment.end_column);
        assert!(segment.end_column <= MINIMAP_COLUMNS);
    }
    assert_eq!(lines[0].1.segments[0].start_column, 2);
    assert_eq!(lines[2].1.segments[0].start_column, 0);

    let mut state = TextPreviewState::from_text((*text).clone(), None, &AppConfig::default());
    assert_eq!(state.content.as_str(), text.as_str());
    state.find_query = "let after = 2".to_owned();
    update_search_matches_and_wait(&mut state);
    assert_eq!(state.match_count(), 1);
    let matched = state.search_match(0).unwrap();
    assert_eq!(
        &state.content[matched.start_byte..matched.end_byte],
        "let after = 2"
    );
}

#[test]
fn neutral_minimap_retains_bounded_source_position_samples_and_cancels() {
    let text = Arc::new(format!(
        "{}\n{}",
        "x".repeat(MINIMAP_MAX_SYNTAX_LINE_BYTES + 1),
        "  row\n".repeat(5_000)
    ));
    let cancelled = AtomicBool::new(true);
    assert!(matches!(
        build_minimap_lines(&Arc::downgrade(&text), find_syntax("rs"), &cancelled),
        MinimapResult::Cancelled
    ));
    let MinimapResult::Lines(lines) = build_minimap_lines(
        &Arc::downgrade(&text),
        find_syntax("rs"),
        &AtomicBool::new(false),
    ) else {
        panic!("neutral minimap should complete")
    };
    let source_lines: usize = 5_002;
    let stride = source_lines.div_ceil(MINIMAP_RETAINED_LINES);
    assert_eq!(lines.len(), source_lines.div_ceil(stride));
    assert!(lines.len() <= MINIMAP_RETAINED_LINES);
    assert!(
        lines
            .iter()
            .enumerate()
            .all(|(sample, (row, line))| *row == sample * stride && line.segments.len() <= 1)
    );
    assert_eq!(lines.last().unwrap().0, 5_000);
}

#[test]
fn test_minimap_parses_unsampled_lines_to_preserve_syntax_state() {
    let mut text = String::from("let first = 1;\n/*\ncomment body\n*/\n");
    text.push_str(&"let filler = 0;\n".repeat(MINIMAP_RETAINED_LINES));
    let text = Arc::new(text);
    let MinimapResult::Lines(lines) = build_minimap_lines(
        &Arc::downgrade(&text),
        find_syntax("rs"),
        &AtomicBool::new(false),
    ) else {
        panic!("fixture should complete")
    };
    assert_eq!(
        lines[1].0, 2,
        "line 2 should be sampled while line 1 is not"
    );

    let mut expected_highlighter =
        HighlightLines::new(find_syntax("rs"), &THEME_SET.themes["base16-eighties.dark"]);
    let _ = expected_highlighter.highlight_line("let first = 1;", &SYNTAX_SET);
    let _ = expected_highlighter.highlight_line("/*", &SYNTAX_SET);
    let expected = build_minimap_line("comment body", &mut expected_highlighter);
    assert_eq!(
        lines[1].1.segments, expected.segments,
        "the unsampled block-comment opener must update syntect state"
    );
}

#[test]
fn test_dropping_queued_minimap_releases_source_before_worker_runs() {
    let jobs = BackgroundJobs::new(1, 2);
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let (_blocker_ticket, _blocker_rx) = jobs.submit(std::time::Duration::MAX, move || {
        entered_tx.send(()).unwrap();
        release_rx.recv().unwrap();
    });
    entered_rx
        .recv_timeout(std::time::Duration::from_secs(1))
        .unwrap();

    let source = Arc::new("let queued = true;\n".repeat(100));
    let weak_source = Arc::downgrade(&source);
    let mut cache = MinimapCache::default();
    cache.start(Arc::clone(&source), "rs".to_owned(), &jobs);
    drop(source);
    drop(cache);
    assert!(
        weak_source.upgrade().is_none(),
        "queued closure must retain only Weak"
    );

    release_tx.send(()).unwrap();
    let (finished_tx, finished_rx) = std::sync::mpsc::channel();
    let (_ticket, _receiver) = jobs.submit(std::time::Duration::MAX, move || {
        finished_tx.send(()).unwrap();
    });
    finished_rx
        .recv_timeout(std::time::Duration::from_secs(1))
        .unwrap();
}

#[test]
fn test_minimap_worker_publishes_bounded_samples_without_changing_navigation() {
    let text = "let x = 1;\n".repeat(MINIMAP_RETAINED_LINES * 2 + 1);
    let mut state = TextPreviewState::from_text(text, None, &AppConfig::default());
    let byte = state.content.len() / 2;
    let expected_row = state.display_row_for_byte(byte);
    let mut cache = MinimapCache::default();
    let jobs = BackgroundJobs::new(1, 1);
    cache.start(Arc::clone(&state.content), "rs".to_owned(), &jobs);
    while !cache.poll() {
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert!(!cache.failed);
    assert!(cache.lines.len() <= MINIMAP_RETAINED_LINES);
    assert_eq!(cache.lines.first().map(|(line, _)| *line), Some(0));
    assert!(cache.lines.windows(2).all(|pair| pair[0].0 < pair[1].0));
    state.minimap_lines = cache;
    assert_eq!(state.display_row_for_byte(byte), expected_row);
}

#[test]
fn test_minimap_job_cancellation_prevents_stale_result_publication() {
    let jobs = BackgroundJobs::new(1, 1);
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let (_blocker_ticket, _blocker_rx) = jobs.submit(std::time::Duration::MAX, move || {
        entered_tx.send(()).unwrap();
        release_rx.recv().unwrap();
    });
    entered_rx
        .recv_timeout(std::time::Duration::from_secs(1))
        .unwrap();

    let mut cache = MinimapCache::default();
    cache.start(Arc::new("fn main() {}".to_owned()), "rs".to_owned(), &jobs);
    cache.cancel();
    release_tx.send(()).unwrap();
    while !cache.poll() {
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert!(cache.failed);
    assert!(cache.lines.is_empty());
}

#[test]
fn test_minimap_job_retries_after_background_queue_saturation() {
    let jobs = BackgroundJobs::new(1, 1);
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let (_blocker_ticket, _blocker_rx) = jobs.submit(std::time::Duration::MAX, move || {
        entered_tx.send(()).unwrap();
        release_rx.recv().unwrap();
    });
    entered_rx
        .recv_timeout(std::time::Duration::from_secs(1))
        .unwrap();
    let (_queued_ticket, queued_receiver) = jobs.submit(std::time::Duration::MAX, || ());

    let mut cache = MinimapCache::default();
    let text = Arc::new("fn main() {}".to_owned());
    cache.start(Arc::clone(&text), "rs".to_owned(), &jobs);
    assert!(!cache.poll(), "a full queue should remain retryable");
    assert!(!cache.started);

    release_tx.send(()).unwrap();
    queued_receiver
        .recv_timeout(std::time::Duration::from_secs(1))
        .unwrap()
        .unwrap();
    cache.start(Arc::clone(&text), "rs".to_owned(), &jobs);
    while !cache.poll() {
        std::thread::sleep(std::time::Duration::from_millis(1));
    }

    assert!(cache.ready);
    assert!(!cache.failed);
    assert!(!cache.lines.is_empty());
}
