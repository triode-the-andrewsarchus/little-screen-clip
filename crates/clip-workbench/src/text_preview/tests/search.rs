use super::*;

#[test]
fn async_search_coalesces_queries_and_publishes_only_latest_revision() {
    let mut state = TextPreviewState::from_text(
        "old target; newest target; newest target".to_owned(),
        None,
        &AppConfig::default(),
    );
    state.find_query = "old".to_owned();
    state.update_search_matches();
    let first_revision = state.search_revision;
    state.find_query = "newest".to_owned();
    state.update_search_matches();
    assert!(state.search_revision > first_revision);
    assert!(state.search_pending);
    assert_eq!(state.match_count(), 0);

    update_search_matches_and_wait(&mut state);
    assert_eq!(state.match_count(), 2);
    for index in 0..state.match_count() {
        let matched = state.search_match(index).unwrap();
        assert_eq!(
            &state.content[matched.start_byte..matched.end_byte],
            "newest"
        );
    }
}

#[test]
fn changing_search_query_cooperatively_cancels_previous_job() {
    let jobs = BackgroundJobs::new(1, 1);
    let (ticket, receiver) = jobs.submit(Duration::MAX, || {
        Ok::<_, String>(SearchBuildOutput::Cancelled)
    });
    let cancelled = Arc::new(AtomicBool::new(false));
    let mut state = TextPreviewState::from_text("target".to_owned(), None, &AppConfig::default());
    state.search_job = Some(SearchMatchJob {
        revision: state.search_revision,
        ticket,
        cancelled: Arc::clone(&cancelled),
        receiver,
    });

    state.find_query = "updated".to_owned();
    state.update_search_matches();

    assert!(cancelled.load(Ordering::Acquire));
    assert!(state.search_pending);
    assert!(state.search_job.is_none());
}

#[test]
fn completed_stale_search_revision_is_never_published() {
    let jobs = BackgroundJobs::new(1, 1);
    let (ticket, _ticket_receiver) = jobs.submit(Duration::MAX, || ());
    let mut spool = SearchMatchSpool::new().unwrap();
    spool.append(0, 6).unwrap();
    let (sender, receiver) = std::sync::mpsc::channel();
    sender
        .send(Ok(Ok(SearchBuildOutput::Matches(spool))))
        .unwrap();

    let mut state = TextPreviewState::from_text("target".to_owned(), None, &AppConfig::default());
    state.search_revision = 2;
    state.search_pending = true;
    state.search_job = Some(SearchMatchJob {
        revision: 1,
        ticket,
        cancelled: Arc::new(AtomicBool::new(false)),
        receiver,
    });
    state.poll_search_job(Instant::now());

    assert!(state.search_pending);
    assert_eq!(state.match_count(), 0);
    assert!(state.matches.is_none());
}

#[test]
fn pending_search_completes_when_find_toolbar_is_hidden() {
    let mut state =
        TextPreviewState::from_text("needle here".to_owned(), None, &AppConfig::default());
    state.find_query = "needle".to_owned();
    state.update_search_matches();
    state.search_debounce_until = Some(Instant::now());
    assert!(!state.show_find);

    let context = egui::Context::default();
    let palette = AppPalette::default();
    let mut close = false;
    let deadline = Instant::now() + Duration::from_secs(2);
    while state.search_pending && Instant::now() < deadline {
        let _ = context.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                state.render_ui(ui, ctx, &palette, &mut close);
            });
        });
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(!state.search_pending);
    assert_eq!(state.match_count(), 1);
}

#[test]
fn spool_read_failure_clears_completed_search_results() {
    let directory = tempfile::tempdir().unwrap();
    let mut state = TextPreviewState::from_text("needle".to_owned(), None, &AppConfig::default());
    let mut spool = SearchMatchSpool::new_in(directory.path()).unwrap();
    let records_at_limit = SEARCH_MATCH_MEMORY_LIMIT_BYTES / SEARCH_MATCH_RECORD_BYTES;
    for index in 0..=records_at_limit as u64 {
        spool.append(index * 2, index * 2 + 1).unwrap();
    }
    assert!(spool.is_rolled());
    #[cfg(target_os = "linux")]
    assert_eq!(open_spool_files_in(directory.path()), 1);
    spool.file.set_len(0).unwrap();
    state.matches = Some(spool);
    state.match_count = 1;
    state.active_match_idx = 0;
    state.search_match_at(0);

    assert!(
        state
            .search_error
            .as_deref()
            .is_some_and(|error| { error.starts_with("could not read search results:") })
    );
    assert_eq!(state.match_count(), 0);
    assert_eq!(state.active_match_idx, 0);
    assert!(state.matches.is_none());
    #[cfg(target_os = "linux")]
    assert_eq!(open_spool_files_in(directory.path()), 0);
}

#[test]
fn dropping_preview_releases_completed_search_spool() {
    let directory = tempfile::tempdir().unwrap();
    let mut state = TextPreviewState::from_text("target".to_owned(), None, &AppConfig::default());
    let mut spool = SearchMatchSpool::new_in(directory.path()).unwrap();
    let records_at_limit = SEARCH_MATCH_MEMORY_LIMIT_BYTES / SEARCH_MATCH_RECORD_BYTES;
    for index in 0..=records_at_limit as u64 {
        spool.append(index * 2, index * 2 + 1).unwrap();
    }
    assert!(spool.is_rolled());
    state.match_count = spool.len();
    state.matches = Some(spool);

    #[cfg(target_os = "linux")]
    assert_eq!(open_spool_files_in(directory.path()), 1);
    drop(state);
    #[cfg(target_os = "linux")]
    assert_eq!(open_spool_files_in(directory.path()), 0);
}

#[test]
fn replacing_preview_releases_previous_search_spool() {
    let directory = tempfile::tempdir().unwrap();
    let mut previous =
        TextPreviewState::from_text("target".to_owned(), None, &AppConfig::default());
    let mut spool = SearchMatchSpool::new_in(directory.path()).unwrap();
    let records_at_limit = SEARCH_MATCH_MEMORY_LIMIT_BYTES / SEARCH_MATCH_RECORD_BYTES;
    for index in 0..=records_at_limit as u64 {
        spool.append(index * 2, index * 2 + 1).unwrap();
    }
    previous.match_count = spool.len();
    previous.matches = Some(spool);

    let mut active_preview = Some(previous);
    #[cfg(target_os = "linux")]
    assert_eq!(open_spool_files_in(directory.path()), 1);
    let replaced = active_preview.replace(TextPreviewState::from_text(
        "new preview".to_owned(),
        None,
        &AppConfig::default(),
    ));
    drop(replaced);
    #[cfg(target_os = "linux")]
    assert_eq!(open_spool_files_in(directory.path()), 0);
}

#[test]
fn closing_preview_cancels_running_search_and_releases_spilled_file() {
    let directory = tempfile::tempdir().unwrap();
    let jobs = BackgroundJobs::new(1, 1);
    let cancelled = Arc::new(AtomicBool::new(false));
    let worker_cancelled = Arc::clone(&cancelled);
    let worker_directory = directory.path().to_path_buf();
    let (spooled_tx, spooled_rx) = std::sync::mpsc::channel();
    let (dropped_tx, dropped_rx) = std::sync::mpsc::channel();
    let (ticket, receiver) = jobs.submit(Duration::MAX, move || {
        let mut spool = SearchMatchSpool::new_in(&worker_directory).unwrap();
        let records_at_limit = SEARCH_MATCH_MEMORY_LIMIT_BYTES / SEARCH_MATCH_RECORD_BYTES;
        for index in 0..=records_at_limit as u64 {
            spool.append(index * 2, index * 2 + 1).unwrap();
        }
        assert!(spool.is_rolled());
        spooled_tx.send(()).unwrap();
        while !worker_cancelled.load(Ordering::Acquire) {
            std::thread::sleep(Duration::from_millis(1));
        }
        drop(spool);
        dropped_tx.send(()).unwrap();
        Ok::<_, String>(SearchBuildOutput::Cancelled)
    });
    let mut state = TextPreviewState::from_text("target".to_owned(), None, &AppConfig::default());
    state.search_job = Some(SearchMatchJob {
        revision: state.search_revision,
        ticket,
        cancelled: Arc::clone(&cancelled),
        receiver,
    });

    spooled_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    #[cfg(target_os = "linux")]
    assert_eq!(open_spool_files_in(directory.path()), 1);
    drop(state);

    assert!(cancelled.load(Ordering::Acquire));
    dropped_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    #[cfg(target_os = "linux")]
    assert_eq!(open_spool_files_in(directory.path()), 0);
}

#[test]
fn out_of_range_search_lookup_preserves_valid_results() {
    let mut state = TextPreviewState::from_text("needle".to_owned(), None, &AppConfig::default());
    let mut spool = SearchMatchSpool::new().unwrap();
    spool.append(0, 6).unwrap();
    state.matches = Some(spool);
    state.match_count = 1;

    assert!(state.search_match_at(1).is_none());
    assert_eq!(state.match_count(), 1);
    assert!(state.search_error.is_none());
    assert!(state.matches.is_some());
}

#[test]
fn recomputing_search_preserves_or_resets_active_match_ordinal() {
    let mut state = TextPreviewState::from_text(
        "one two two two two".to_owned(),
        None,
        &AppConfig::default(),
    );
    state.is_regex = true;
    state.find_query = "two|one".to_owned();
    update_search_matches_and_wait(&mut state);
    assert_eq!(state.match_count(), 5);
    state.active_match_idx = 2;
    state.find_query = "two".to_owned();
    update_search_matches_and_wait(&mut state);
    assert_eq!(state.match_count(), 4);
    assert_eq!(state.active_match_idx, 2);

    state.active_match_idx = 4;
    state.find_query = "one".to_owned();
    update_search_matches_and_wait(&mut state);
    assert_eq!(state.match_count(), 1);
    assert_eq!(state.active_match_idx, 0);

    state.is_regex = true;
    state.find_query = "(".to_owned();
    update_search_matches_and_wait(&mut state);
    assert!(state.regex_error.is_some());
    assert_eq!(state.active_match_idx, 0);
}

#[test]
fn async_search_retries_after_background_queue_saturation() {
    let jobs = BackgroundJobs::new(1, 1);
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let (_blocker_ticket, _blocker_rx) = jobs.submit(Duration::MAX, move || {
        entered_tx.send(()).unwrap();
        release_rx.recv().unwrap();
    });
    entered_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    let (_queued_ticket, queued_receiver) = jobs.submit(Duration::MAX, || ());

    let mut state =
        TextPreviewState::from_text("target target".to_owned(), None, &AppConfig::default());
    state.find_query = "target".to_owned();
    state.update_search_matches();
    let now = Instant::now();
    state.search_debounce_until = Some(now);
    let context = egui::Context::default();
    state.service_search_jobs(&context, &jobs, now);
    state.service_search_jobs(&context, &jobs, now);
    assert!(state.search_pending);
    assert_eq!(state.match_count(), 0);
    let retry_at = state
        .search_retry_at
        .expect("saturated queue schedules retry");

    release_tx.send(()).unwrap();
    queued_receiver
        .recv_timeout(Duration::from_secs(1))
        .unwrap()
        .unwrap();
    state.service_search_jobs(&context, &jobs, retry_at + SEARCH_RETRY_DELAY);
    let deadline = Instant::now() + Duration::from_secs(2);
    while state.search_pending && Instant::now() < deadline {
        state.service_search_jobs(&context, &jobs, Instant::now());
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(
        !state.search_pending,
        "retry should publish completed results"
    );
    assert_eq!(state.match_count(), 2);
}

#[test]
fn search_match_spool_uses_the_application_staging_directory() {
    let mut spool = SearchMatchSpool::new().unwrap();
    spool.append(10, 20).unwrap();
    assert_eq!(
        spool.get(0).unwrap(),
        SearchMatchRange {
            start_byte: 10,
            end_byte: 20
        }
    );
}

#[test]
fn search_match_spool_round_trips_records_by_index_after_reads() {
    let directory = tempfile::tempdir().unwrap();
    let mut spool = SearchMatchSpool::new_in(directory.path()).unwrap();
    spool.append(3, 8).unwrap();
    assert_eq!(
        spool.get(0).unwrap(),
        SearchMatchRange {
            start_byte: 3,
            end_byte: 8
        }
    );

    spool
        .append(u32::MAX as u64 + 9, u32::MAX as u64 + 19)
        .unwrap();
    assert_eq!(spool.len(), 2);
    assert_eq!(
        spool.get(1).unwrap(),
        SearchMatchRange {
            start_byte: u32::MAX as u64 + 9,
            end_byte: u32::MAX as u64 + 19,
        }
    );
    assert_eq!(
        spool.get(0).unwrap(),
        SearchMatchRange {
            start_byte: 3,
            end_byte: 8
        }
    );
    assert_eq!(
        spool.get(2).unwrap_err().kind(),
        io::ErrorKind::InvalidInput
    );
}

#[test]
fn search_match_spool_preserves_zero_width_records() {
    let directory = tempfile::tempdir().unwrap();
    let mut spool = SearchMatchSpool::new_in(directory.path()).unwrap();
    spool.append(42, 42).unwrap();

    assert_eq!(
        spool.get(0).unwrap(),
        SearchMatchRange {
            start_byte: 42,
            end_byte: 42,
        }
    );
}

#[test]
fn search_match_spool_fragment_lookup_includes_crossing_ranges() {
    let directory = tempfile::tempdir().unwrap();
    let mut spool = SearchMatchSpool::new_in(directory.path()).unwrap();
    for range in [(0, 2), (3, 6), (6, 8), (10, 10), (13, 15)] {
        spool.append(range.0, range.1).unwrap();
    }

    let ranges = collect_fragment_ranges(&mut spool, 4, 7);
    assert_eq!(
        ranges,
        vec![
            SearchMatchRange {
                start_byte: 3,
                end_byte: 6,
            },
            SearchMatchRange {
                start_byte: 6,
                end_byte: 8,
            },
        ]
    );
}

#[test]
fn search_match_spool_fragment_lookup_observes_half_open_and_zero_width_edges() {
    let directory = tempfile::tempdir().unwrap();
    let mut spool = SearchMatchSpool::new_in(directory.path()).unwrap();
    for range in [(2, 4), (4, 4), (5, 5), (6, 9), (9, 9)] {
        spool.append(range.0, range.1).unwrap();
    }

    let ranges = collect_fragment_ranges(&mut spool, 4, 9);
    assert_eq!(
        ranges,
        vec![
            SearchMatchRange {
                start_byte: 5,
                end_byte: 5,
            },
            SearchMatchRange {
                start_byte: 6,
                end_byte: 9,
            },
        ]
    );
    assert!(
        spool
            .ranges_for_fragment(9, 10)
            .unwrap()
            .map(|result| result.map(|(_, range)| range))
            .collect::<io::Result<Vec<_>>>()
            .unwrap()
            .is_empty()
    );
    assert!(spool.ranges_for_fragment(5, 5).unwrap().next().is_none());
}

#[test]
fn search_match_spool_fragment_lookup_handles_empty_and_outside_bounds() {
    let directory = tempfile::tempdir().unwrap();
    let mut spool = SearchMatchSpool::new_in(directory.path()).unwrap();
    spool.append(5, 7).unwrap();
    spool.append(10, 12).unwrap();

    for (start, end) in [(0, 5), (12, 20), (8, 8)] {
        assert!(
            spool
                .ranges_for_fragment(start, end)
                .unwrap()
                .map(|result| result.map(|(_, range)| range))
                .collect::<io::Result<Vec<_>>>()
                .unwrap()
                .is_empty()
        );
    }
}

#[test]
fn search_match_spool_requires_ordered_ranges_and_recovers_after_partial_iteration() {
    let directory = tempfile::tempdir().unwrap();
    let mut spool = SearchMatchSpool::new_in(directory.path()).unwrap();
    spool.append(2, 4).unwrap();
    spool.append(5, 7).unwrap();
    assert!(spool.append(4, 6).is_err(), "starts must be ordered");
    assert!(spool.append(8, 6).is_err(), "ranges cannot be reversed");
    assert!(spool.append(6, 6).is_err(), "ends must be ordered");

    let mut ranges = spool.ranges_for_fragment(0, 10).unwrap();
    assert_eq!(ranges.next().unwrap().unwrap().1.start_byte, 2);
    drop(ranges);

    spool.append(9, 10).unwrap();
    assert_eq!(spool.len(), 3);
    assert_eq!(spool.get(2).unwrap().start_byte, 9);
}

#[test]
fn search_match_spool_rolls_over_only_after_the_memory_threshold_and_cleans_up() {
    let directory = tempfile::tempdir().unwrap();
    let mut spool = SearchMatchSpool::new_in(directory.path()).unwrap();
    let records_at_limit = SEARCH_MATCH_MEMORY_LIMIT_BYTES / SEARCH_MATCH_RECORD_BYTES;
    for index in 0..records_at_limit as u64 {
        spool.append(index * 2, index * 2 + 1).unwrap();
    }
    assert!(!spool.is_rolled());
    assert_eq!(spool.len(), records_at_limit as u64);

    spool.append(100_000, 100_001).unwrap();
    assert!(spool.is_rolled());
    assert_eq!(
        spool.get(0).unwrap(),
        SearchMatchRange {
            start_byte: 0,
            end_byte: 1
        }
    );
    assert_eq!(
        spool.get(records_at_limit as u64).unwrap(),
        SearchMatchRange {
            start_byte: 100_000,
            end_byte: 100_001,
        }
    );
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);

    drop(spool);
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
}

#[test]
fn search_match_spool_reports_directory_and_rollover_io_errors() {
    let parent = tempfile::tempdir().unwrap();
    let not_a_directory = parent.path().join("regular-file");
    std::fs::write(&not_a_directory, b"file").unwrap();
    assert!(SearchMatchSpool::new_in(&not_a_directory).is_err());

    let spill_directory = parent.path().join("spill");
    let mut spool = SearchMatchSpool::new_in(&spill_directory).unwrap();
    let records_at_limit = SEARCH_MATCH_MEMORY_LIMIT_BYTES / SEARCH_MATCH_RECORD_BYTES;
    for index in 0..records_at_limit as u64 {
        spool.append(index, index + 1).unwrap();
    }
    std::fs::remove_dir(&spill_directory).unwrap();
    assert!(spool.append(10_000, 10_000).is_err());
    assert!(!spool.is_rolled());
    assert_eq!(spool.len(), records_at_limit as u64);
    assert_eq!(
        spool.get(0).unwrap(),
        SearchMatchRange {
            start_byte: 0,
            end_byte: 1
        }
    );
    std::fs::create_dir(&spill_directory).unwrap();
    spool.append(10_000, 10_000).unwrap();
    assert!(spool.is_rolled());
    assert_eq!(spool.len(), records_at_limit as u64 + 1);
    assert_eq!(
        spool.get(records_at_limit as u64).unwrap(),
        SearchMatchRange {
            start_byte: 10_000,
            end_byte: 10_000,
        }
    );
}

#[test]
fn test_text_preview_creation_and_search() {
    let config = AppConfig::default();
    let content = "fn main() {\n    let answer = 42;\n    println!(\"{}\", answer);\n}".to_string();
    let mut state = TextPreviewState::from_text(content, None, &config);

    assert_eq!(state.language, "rs");
    assert!(state.show_line_numbers);
    assert_eq!(state.view_mode, TextViewMode::Raw);

    // Test Find query
    state.find_query = "answer".to_string();
    update_search_matches_and_wait(&mut state);
    assert_eq!(state.match_count(), 2);
    assert_eq!(state.active_match_idx, 0);

    state.next_match();
    assert_eq!(state.active_match_idx, 1);

    state.next_match();
    assert_eq!(state.active_match_idx, 0);

    state.prev_match();
    assert_eq!(state.active_match_idx, 1);
}

#[test]
fn text_preview_strips_only_a_leading_bom() {
    let config = AppConfig::default();
    let plain = "clipboard content".to_owned();
    let state = TextPreviewState::from_text(plain.clone(), None, &config);
    assert_eq!(state.content.as_str(), plain);

    let bom_prefixed = format!("\u{feff}{}", plain);
    let state = TextPreviewState::from_text(bom_prefixed, None, &config);
    assert_eq!(state.content.as_str(), plain);
}

#[test]
fn find_editor_enter_navigates_once_without_closing_the_preview() {
    let ctx = egui::Context::default();
    let mut state = TextPreviewState::from_text(
        "match one\nmatch two\nmatch three".to_owned(),
        None,
        &AppConfig::default(),
    );
    state.show_find = true;
    state.find_query = "match".to_owned();
    update_search_matches_and_wait(&mut state);
    state.active_match_idx = 0;

    let render = |ctx: &egui::Context, state: &mut TextPreviewState, events| {
        ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(900.0, 400.0),
                )),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    state.render_search_toolbar(ui, &AppPalette::default());
                });
            },
        )
    };

    state.focus_find_input = true;
    render(&ctx, &mut state, Vec::new());
    assert!(
        ctx.wants_keyboard_input(),
        "Find editor should own keyboard focus"
    );

    render(
        &ctx,
        &mut state,
        vec![egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }],
    );
    assert_eq!(state.active_match_idx, 1, "Enter advances one match");
    assert!(state.show_find, "Enter must leave the Find toolbar open");

    render(&ctx, &mut state, Vec::new());
    assert_eq!(
        state.active_match_idx, 1,
        "one Enter must not advance twice"
    );
}

#[test]
fn find_editor_shift_enter_navigates_to_previous_match_once() {
    let ctx = egui::Context::default();
    let mut state = TextPreviewState::from_text(
        "match one\nmatch two\nmatch three".to_owned(),
        None,
        &AppConfig::default(),
    );
    state.show_find = true;
    state.find_query = "match".to_owned();
    update_search_matches_and_wait(&mut state);
    state.active_match_idx = 1;
    state.focus_find_input = true;

    let mut render = |events, modifiers| {
        ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(900.0, 400.0),
                )),
                events,
                modifiers,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    state.render_search_toolbar(ui, &AppPalette::default());
                });
            },
        )
    };

    render(Vec::new(), egui::Modifiers::NONE);
    render(
        vec![egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::SHIFT,
        }],
        egui::Modifiers::SHIFT,
    );
    assert_eq!(
        state.active_match_idx, 0,
        "Shift+Enter moves to the prior match"
    );
    assert!(
        state.show_find,
        "Shift+Enter must leave the Find toolbar open"
    );
}

#[test]
fn test_text_preview_regex_search() {
    let config = AppConfig::default();
    let content = "const VALUE_A = 100;\nconst VALUE_B = 200;\nconst OTHER = 300;".to_string();
    let mut state = TextPreviewState::from_text(content, None, &config);

    state.find_query = r"VALUE_[A-Z]".to_string();
    state.is_regex = true;
    update_search_matches_and_wait(&mut state);

    assert!(state.regex_error.is_none());
    assert_eq!(state.match_count(), 2);

    // Invalid regex
    state.find_query = r"[invalid".to_string();
    update_search_matches_and_wait(&mut state);
    assert!(state.regex_error.is_some());
    assert_eq!(state.match_count(), 0);
}

#[test]
fn literal_search_preserves_unicode_offsets_and_escapes_metacharacters() {
    let mut state =
        TextPreviewState::from_text("İX\nCAFÉ café [x]".into(), None, &AppConfig::default());
    state.find_query = "x".into();
    update_search_matches_and_wait(&mut state);
    let matched = state.search_match(0).unwrap();
    assert_eq!(&state.content[matched.start_byte..matched.end_byte], "X");
    assert_eq!(matched.start_byte, "İ".len());
    state.find_query = "CAFÉ".into();
    update_search_matches_and_wait(&mut state);
    assert_eq!(state.match_count(), 2);
    for index in 0..state.match_count() {
        let matched = state.search_match(index).unwrap();
        assert!(state.content.is_char_boundary(matched.start_byte));
        assert!(state.content.is_char_boundary(matched.end_byte));
        assert_eq!(matched.line, 2);
    }
    state.find_query = "[x]".into();
    update_search_matches_and_wait(&mut state);
    assert_eq!(state.match_count(), 1);
    let matched = state.search_match(0).unwrap();
    assert_eq!(&state.content[matched.start_byte..matched.end_byte], "[x]");
}

#[test]
fn search_line_numbers_derive_from_first_fragments_for_empty_and_trailing_lines() {
    let mut empty = TextPreviewState::from_text(String::new(), None, &AppConfig::default());
    empty.find_query = "(?m)^".into();
    empty.is_regex = true;
    update_search_matches_and_wait(&mut empty);
    assert_eq!(empty.line_first_fragment.len(), 1);
    assert_eq!(empty.search_match(0).unwrap().line, 1);

    let mut trailing =
        TextPreviewState::from_text("alpha\nbeta\n".into(), None, &AppConfig::default());
    trailing.find_query = "(?m)^".into();
    trailing.is_regex = true;
    update_search_matches_and_wait(&mut trailing);
    assert_eq!(trailing.line_first_fragment.len(), 3);
    assert_eq!(
        trailing.search_match(2).unwrap().start_byte,
        trailing.content.len()
    );
    assert_eq!(trailing.search_match(2).unwrap().line, 3);
}

#[test]
fn highlights_project_active_and_multiline_ranges_to_source_lines() {
    let matches = vec![
        SearchMatch {
            start_byte: 1,
            end_byte: 3,
            line: 1,
        },
        SearchMatch {
            start_byte: 6,
            end_byte: 12,
            line: 2,
        },
    ];
    assert_eq!(
        line_highlight_ranges(&matches, 1, 5, 10),
        vec![(1, 5, true)]
    );
    assert_eq!(
        line_highlight_ranges(&matches, 1, 11, 15),
        vec![(0, 1, true)]
    );
    assert!(line_highlight_ranges(&matches, 1, 15, 20).is_empty());
}

#[test]
fn search_background_covers_only_the_matched_part_of_a_token() {
    let palette = AppPalette::default();
    let mut job = egui::text::LayoutJob::default();
    append_highlighted_token(
        &mut job,
        "identifier",
        0,
        Color32::WHITE,
        &palette,
        &[(2, 5, true)],
    );
    assert_eq!(job.text, "identifier");
    let highlighted: Vec<_> = job
        .sections
        .iter()
        .filter(|section| section.format.background != Color32::TRANSPARENT)
        .collect();
    assert_eq!(highlighted.len(), 1);
    assert_eq!(highlighted[0].byte_range, 2..5);
}
