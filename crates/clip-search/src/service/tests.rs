use super::super::store::TestFixture;
use super::*;
use little_clip_core::file_inventory as filesystem;

fn test_worker(path: &Path, store: Option<Store>) -> Worker {
    Worker {
        paths: Paths {
            data: path.join("unused-search-cache"),
            helpers: PathBuf::new(),
        },
        config: FileSearchConfig::default(),
        exclusions: Vec::new(),
        store,
        model: None,
        dispatch: Default::default(),
        snapshot: Arc::new(Mutex::new(Snapshot::default())),
        stop: Arc::new(AtomicBool::new(false)),
        work: VecDeque::new(),
        pipeline: "model".into(),
        last_snapshot: Instant::now(),
        roots_dirty: false,
        finishing: Default::default(),
        pruning: None,
        pruning_dirty: false,
        answers_until: None,
        monitor: super::super::monitor::Monitor::default(),
        capacity_blocked: false,
        readers: Default::default(),
        staged_files: Vec::new(),
        next_probe: 0,
    }
}

#[test]
fn failed_files_wait_without_queued_work_and_manual_rebuild_resets_retries() {
    let fixture = TestFixture::new();
    let root = SourceRoot::from_root(&fixture.root);
    let id = fixture.store.roots().unwrap()[0].id;
    let entry = fixture.entry;
    let file = fixture.file;
    let mut worker = test_worker(&root.path, Some(fixture.store));
    worker.monitor.register(id, root.clone());
    worker
        .file_failure(
            FileWork::new(root.clone(), id, 1, entry.clone(), file),
            "decoder failed".into(),
        )
        .unwrap();
    worker.observe_entry(&root, id, 1, entry, "model").unwrap();
    assert!(worker.work.is_empty(), "backoff retained runnable work");
    assert!(matches!(
        worker.store.as_ref().unwrap().file_retry(file).unwrap(),
        super::super::store::Retry::After(_)
    ));
    let refresh = worker
        .monitor
        .due(&HashSet::new(), Instant::now() + Duration::from_secs(61));
    assert_eq!(refresh.len(), 1);
    assert_eq!(
        refresh[0].directories.as_ref().unwrap(),
        &vec![root.path.clone()]
    );
    worker.command(Command::Rebuild(root.path.clone())).unwrap();
    assert_eq!(
        worker.store.as_ref().unwrap().file_retry(file).unwrap(),
        super::super::store::Retry::Ready
    );
    assert!(matches!(worker.work.front(), Some(Work::Probe { .. })));
}

#[test]
fn committed_quantum_releases_payload_and_admits_a_third_root_before_long_files_finish() {
    let fixture = TestFixture::new();
    let root = SourceRoot::from_root(&fixture.root);
    let mut current = FileWork::new(root.clone(), 1, 1, fixture.entry.clone(), fixture.file);
    current.source_check = SourceCheck::Extract(None);
    current.cursor = 2;
    current.text_cursor = 256;
    current.prepared = Some(Prepared {
        text: "ordinary source text ".repeat(128),
        ..Default::default()
    });
    current.segments = (0..8)
        .map(|i| Segment {
            modality: "text".into(),
            location: format!("passage {i}"),
            text: format!("text {i}"),
            input_hash: vec![i as u8; 32],
            vector: Some(vec![1.0; super::super::store::DIMENSION]),
        })
        .collect();
    let entry = fixture.entry.clone();
    let mut worker = test_worker(&root.path, Some(fixture.store));
    assert!(worker.reserve_file_staging(&mut current));
    let mut second = FileWork::new(root.clone(), 2, 1, entry.clone(), 20);
    assert!(worker.reserve_file_staging(&mut second));
    worker.work.push_back(Work::File(Box::new(FileWork::new(
        root.clone(),
        1,
        1,
        entry.clone(),
        10,
    ))));
    worker.work.push_back(Work::File(Box::new(FileWork::new(
        root.clone(),
        3,
        1,
        entry.clone(),
        30,
    ))));
    worker.work.push_back(Work::File(Box::new(second)));
    worker.readers.completed(
        1,
        1,
        &entry.path,
        Request::Verify {
            root: root.clone(),
            entry: entry.clone(),
        },
        Reply::Verified,
    );
    assert!(
        !worker.checkpoint_file(&mut current).unwrap(),
        "a committed eight-segment quantum must yield"
    );
    assert!(current.prepared.is_none() && current.segments.is_empty());
    assert!(
        !worker
            .database()
            .unwrap()
            .file_complete(current.id, current.entry.revision, "model")
            .unwrap()
    );
    assert!(
        current.staging.is_none(),
        "yield must release decoded-payload admission"
    );
    worker.work.push_back(Work::File(Box::new(current)));
    let third = worker
        .next_work()
        .expect("the third root must become runnable");
    assert_eq!(third.root_id(), 3);
    assert!(matches!(&third, Work::File(file) if file.staging.is_some()));
    // Release this admitted quantum, then reconstruct the parked original
    // from the actual canonical checkpoint without re-encoding its prefix.
    drop(third);
    let Work::File(mut resumed) = worker.work.pop_back().unwrap() else {
        panic!("parked file");
    };
    assert!(worker.reserve_file_staging(&mut resumed));
    worker.resume_file(&mut resumed).unwrap();
    assert_eq!(
        (resumed.cursor, resumed.text_cursor, resumed.published),
        (2, 256, 8)
    );
    assert_eq!(resumed.segments.len(), 8);
    assert!(resumed.resume_hash.is_some());
    assert!(
        worker.checkpoint_file(&mut resumed).unwrap(),
        "resuming a prefix must not yield it again without new segments"
    );
    assert!(resumed.staging.is_some());
    assert_eq!(
        worker
            .database()
            .unwrap()
            .connection
            .query_row("SELECT count(*) FROM segments", [], |r| r.get::<_, u64>(0))
            .unwrap(),
        8
    );
}

#[test]
fn overlapping_roots_share_one_active_canonical_file_owner() {
    let fixture = TestFixture::new();
    let root = SourceRoot::from_root(&fixture.root);
    let mut worker = test_worker(&root.path, None);
    let mut owner = FileWork::new(root.clone(), 1, 1, fixture.entry.clone(), fixture.file);
    let mut follower = FileWork::new(root, 2, 1, fixture.entry, fixture.file);
    assert!(worker.reserve_file_staging(&mut owner));
    assert!(!worker.reserve_file_staging(&mut follower));
    worker.work.push_back(Work::File(Box::new(follower)));
    assert!(
        worker.ready_work().is_none(),
        "an alias cannot race the owner's checkpoint cursor"
    );
    drop(owner);
    assert!(
        matches!(worker.next_work(), Some(Work::File(file)) if file.root_id == 2 && file.staging.is_some())
    );
}

#[test]
fn completed_alias_verifies_without_decoding_or_republishing_the_canonical_prefix() {
    let fixture = TestFixture::new();
    let root = SourceRoot::from_root(&fixture.root);
    let entry = fixture.entry.clone();
    let file = fixture.file;
    let revision = entry.revision;
    let before: Vec<u8> = fixture
        .store
        .connection
        .query_row(
            "SELECT vector FROM segments WHERE file_id=?1",
            [file],
            |r| r.get(0),
        )
        .unwrap();
    let mut worker = test_worker(&root.path, Some(fixture.store));
    assert!(
        worker
            .database()
            .unwrap()
            .file_complete(file, revision, "model")
            .unwrap()
    );
    assert!(
        !worker
            .database()
            .unwrap()
            .file_complete(file, revision.wrapping_add(1), "model")
            .unwrap()
    );
    assert!(
        !worker
            .database()
            .unwrap()
            .file_complete(file, revision, "other-model")
            .unwrap()
    );
    let mut alias = FileWork::new(root.clone(), 2, 1, entry.clone(), file);
    assert!(worker.reserve_file_staging(&mut alias));
    worker.readers.completed(
        2,
        1,
        &entry.path,
        Request::Verify {
            root,
            entry: entry.clone(),
        },
        Reply::Verified,
    );
    worker.index_file(alias).unwrap();
    assert!(worker.work.is_empty() && worker.readers.is_empty() && worker.model.is_none());
    let after: Vec<u8> = worker
        .database()
        .unwrap()
        .connection
        .query_row(
            "SELECT vector FROM segments WHERE file_id=?1",
            [file],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(before, after);
    assert!(
        worker
            .staged_files
            .iter()
            .all(|slot| slot.strong_count() == 0)
    );
}

#[test]
fn queued_file_payloads_keep_staging_admission_until_their_owner_is_dropped() {
    let fixture = TestFixture::new();
    let root = SourceRoot::from_root(&fixture.root);
    let mut worker = test_worker(&root.path, None);
    for id in 1..=MAX_STAGED_FILES as i64 {
        let mut file = FileWork::new(root.clone(), id, 1, fixture.entry.clone(), id);
        assert!(worker.reserve_file_staging(&mut file));
        assert!(
            worker.reserve_file_staging(&mut file),
            "Existing reservation is reused"
        );
        worker.work.push_back(Work::File(Box::new(file)));
    }
    let mut waiting = FileWork::new(root.clone(), 99, 1, fixture.entry.clone(), 99);
    assert!(!worker.reserve_file_staging(&mut waiting));
    worker.work.push_front(Work::File(Box::new(waiting)));
    assert_eq!(
        worker.ready_work(),
        Some(1),
        "A staged file can still make progress"
    );
    worker.work.push_front(Work::Probe {
        id: -1,
        path: root.path.clone(),
        persist: false,
    });
    assert!(
        matches!(worker.next_work(), Some(Work::Probe { .. })),
        "Metadata probes do not retain content buffers"
    );
    // Cancelling/removing an owner releases only its retained-file permit;
    // existing reader retirement keeps its independent two-slot admission.
    worker.work.retain(|work| work.root_id() != 1);
    assert_eq!(worker.ready_work(), Some(0));
    let Some(Work::File(mut waiting)) = worker.next_work() else {
        panic!("Expected the waiting file");
    };
    assert!(
        waiting.staging.is_some(),
        "Scheduling reserves before any source/cache read"
    );
    assert!(worker.reserve_file_staging(&mut waiting));
    assert_eq!(worker.staged_files.len(), MAX_STAGED_FILES);
    let mut another = FileWork::new(root.clone(), 100, 1, fixture.entry.clone(), 100);
    assert!(!worker.reserve_file_staging(&mut another));
    worker.work.clear();
    drop(waiting);
    assert!(worker.reserve_file_staging(&mut another));
    assert_eq!(worker.staged_files.len(), 1);
}

fn inventory_fixture(count: usize) -> (TestFixture, Worker, InventoryWork) {
    let mut fixture = TestFixture::new();
    let root = SourceRoot::from_root(&fixture.root);
    let id = fixture.store.roots().unwrap()[0].id;
    let generation = fixture.store.begin_scan(id).unwrap();
    // Synthetic aliases exercise catalog admission, not source I/O.
    let mut entries = VecDeque::from([fixture.entry.clone()]);
    entries.extend((0..count).map(|index| {
        let mut entry = fixture.entry.clone();
        entry.name = format!("alias-{index:06}.txt");
        entry.path = root.path.join(&entry.name);
        entry
    }));
    let remaining_bytes = entries
        .iter()
        .map(reader::Inventory::entry_storage_bytes)
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
        .into_iter()
        .sum();
    let inventory = InventoryWork {
        remaining_bytes,
        _permit: None,
        path: root.path.clone(),
        root,
        id,
        generation,
        recursive: true,
        invalidated: false,
        entries,
        pipeline: "model".into(),
    };
    let worker = test_worker(&inventory.path, None);
    (fixture, worker, inventory)
}

#[test]
fn inventory_application_yields_without_completing_or_retiring_partial_coverage() {
    let (fixture, mut worker, inventory) = inventory_fixture(513);
    worker.store = Some(fixture.store);
    let id = inventory.id;
    let generation = inventory.generation;
    let path = inventory.path.clone();
    let cancel = worker.dispatch.lock().unwrap().cancel.clone();
    cancel.store(true, Ordering::Relaxed);
    worker.apply_inventory(inventory).unwrap();
    assert_eq!(
        worker
            .store
            .as_ref()
            .unwrap()
            .connection
            .query_row("SELECT COUNT(*) FROM paths", [], |row| row
                .get::<_, usize>(0))
            .unwrap(),
        1
    );
    assert!(worker.dispatch.lock().unwrap().begin_background());
    let mut observed = 1;
    for _ in 0..2000 {
        let Some(Work::Inventory(inventory)) = worker.work.pop_front() else {
            break;
        };
        worker.apply_inventory(*inventory).unwrap();
        let store = worker.store.as_ref().unwrap();
        let count = store
            .connection
            .query_row("SELECT COUNT(*) FROM paths", [], |row| {
                row.get::<_, usize>(0)
            })
            .unwrap();
        assert!(
            count.saturating_sub(observed) <= 64,
            "unbounded metadata application"
        );
        observed = count;
        assert_eq!(
            store.known_directory(id, &path).unwrap(),
            worker.work.is_empty()
        );
    }
    assert!(worker.work.is_empty());
    assert_eq!(observed, 514);
    let store = worker.store.as_ref().unwrap();
    assert_eq!(
        store
            .connection
            .query_row(
                "SELECT COUNT(*) FROM memberships WHERE generation=?1 AND verified=1",
                [generation],
                |row| row.get::<_, usize>(0)
            )
            .unwrap(),
        514
    );
}

#[test]
fn events_abandon_buffered_inventory_and_restart_with_a_new_generation() {
    for whole_root in [false, true] {
        let (fixture, mut worker, inventory) = inventory_fixture(100);
        worker.store = Some(fixture.store);
        let id = inventory.id;
        let generation = inventory.generation;
        let path = inventory.path.clone();
        worker.apply_inventory(inventory).unwrap();
        let Some(Work::Inventory(inventory)) = worker.work.front() else {
            panic!("inventory did not yield")
        };
        let before = inventory.entries.len();
        worker.invalidate_reads(vec![(id, Some(path.join("unrelated")))]);
        assert!(
            matches!(worker.work.front(), Some(Work::Inventory(i)) if !i.invalidated),
            "unrelated notifications must not restart this directory"
        );
        let root = SourceRoot::from_root(&fixture.root);
        let pending = FileWork::new(
            root.clone(),
            id,
            generation,
            fixture.entry.clone(),
            fixture.file,
        );
        worker.work.push_back(Work::File(Box::new(pending)));
        let mut other = fixture.entry.clone();
        other.path = path.join("unrelated/note.txt");
        worker.work.push_back(Work::File(Box::new(FileWork::new(
            root,
            id,
            generation,
            other.clone(),
            fixture.file,
        ))));
        worker.invalidate_reads(vec![(id, (!whole_root).then_some(path.clone()))]);
        let Some(Work::Inventory(inventory)) = worker.work.pop_front() else {
            unreachable!()
        };
        worker.apply_inventory(*inventory).unwrap();
        let Some(Work::Directory {
            generation: replacement,
            remaining: None,
            ..
        }) = worker.work.front()
        else {
            panic!("old snapshot was not replaced")
        };
        assert!(*replacement > generation);
        assert_eq!(
            worker.work.len(),
            2,
            "restart drops pending files only in this directory"
        );
        assert!(
            matches!(worker.work.back(), Some(Work::File(file)) if file.entry.path == other.path)
        );
        let store = worker.store.as_ref().unwrap();
        let total = store
            .connection
            .query_row("SELECT COUNT(*) FROM paths", [], |row| {
                row.get::<_, usize>(0)
            })
            .unwrap();
        assert_eq!(
            total,
            101 - before,
            "no old entries applied after invalidation"
        );
        assert_eq!(
            store
                .connection
                .query_row(
                    "SELECT COUNT(*) FROM memberships WHERE verified=1",
                    [],
                    |row| row.get::<_, usize>(0)
                )
                .unwrap(),
            0
        );
        assert!(!store.known_directory(id, &path).unwrap());
        assert!(
            !store
                .unseen_children(id, *replacement, &path)
                .unwrap()
                .is_empty(),
            "fresh generation must reconsider previously observed removals"
        );
    }
}

#[test]
fn unsupported_indexing_queues_only_final_validation_without_digest_or_decoder() {
    let mut fixture = TestFixture::new();
    let path = fixture.root.path.join("archive.zip");
    std::fs::write(&path, b"owned archive-shaped fixture").unwrap();
    let entry = filesystem::read_directory(&fixture.root.path, &AtomicBool::new(false))
        .unwrap()
        .entries
        .into_iter()
        .find(|entry| entry.path == path)
        .unwrap();
    let root = SourceRoot::from_root(&fixture.root);
    let id = fixture.store.roots().unwrap()[0].id;
    let generation = fixture.store.begin_scan(id).unwrap();
    let (file_id, _) = fixture
        .store
        .observe_object(
            id,
            generation,
            &entry,
            entry.identity.unwrap().object(),
            "model",
        )
        .unwrap();
    let file = FileWork::new(root.clone(), id, generation, entry.clone(), file_id);
    let mut worker = test_worker(&root.path, Some(fixture.store));
    worker.index_file(file).unwrap();
    let Some(Work::File(pending)) = worker.work.front() else {
        panic!("validation was not retained")
    };
    assert!(pending.publishing);
    assert_eq!(pending.coverage, reader::NAME_ONLY_UNSUPPORTED);
    assert!(pending.segments.is_empty() && pending.prepared.is_none());
    assert!(matches!(pending.source_check, SourceCheck::Extract(None)));
    assert!(
        worker
            .readers
            .request(
                id,
                generation,
                &entry.path,
                Request::Fingerprint {
                    root: root.clone(),
                    entry: entry.clone(),
                }
            )
            .is_err(),
        "a fingerprint must not replace the owned final-validation request"
    );
    assert!(
        worker
            .readers
            .request(
                id,
                generation,
                &entry.path,
                Request::Verify {
                    root,
                    entry: entry.clone()
                }
            )
            .unwrap()
            .is_none()
    );
    worker.cancel_reads(None);
    let deadline = Instant::now() + Duration::from_secs(3);
    while !worker.readers.is_empty() {
        assert!(Instant::now() < deadline);
        worker.readers.poll();
        std::thread::yield_now();
    }
    assert_eq!(worker.snapshot.lock().unwrap().generated_vectors, 0);
    let job = AnswerJob {
        id: 1,
        root: SourceRoot::from_root(&fixture.root),
        entry,
        question: "What is the score?".into(),
        kind: super::super::query::AnswerType::Number,
        cancel: Arc::new(AtomicBool::new(false)),
        plans: Default::default(),
    };
    let answer = worker.answer_file(&job).unwrap();
    assert!(matches!(answer.state, State::Unsupported));
    assert!(
        worker.model.is_none() && worker.readers.is_empty(),
        "unsupported questions must not read a header or start inference"
    );
    worker.exclusions.push(job.entry.path.clone());
    assert!(matches!(
        worker.answer_file(&job).unwrap().state,
        State::Excluded
    ));
}

#[test]
fn pending_enrollment_is_removable_while_indexing_is_paused_without_creating_a_catalog() {
    let fixture = tempfile::tempdir().unwrap();
    let mut worker = test_worker(fixture.path(), None);
    worker.config.paused = true;
    worker.capacity_blocked = true;
    worker.answers_until = Some(Instant::now() + Duration::from_secs(60));
    worker.enroll(fixture.path(), false).unwrap();
    worker.enroll(fixture.path(), true).unwrap();
    assert_eq!(worker.work.len(), 1);
    let id = worker.work[0].root_id();
    assert!(id < 0 && worker.background_ready());
    assert!(matches!(worker.work[0], Work::Probe { persist: true, .. }));
    assert_eq!(worker.snapshot.lock().unwrap().roots[0].state, "checking");
    worker
        .command(Command::Remove(fixture.path().into()))
        .unwrap();
    assert!(worker.work.is_empty());
    assert!(worker.snapshot.lock().unwrap().roots.is_empty());
    assert!(worker.config.roots.is_empty() && worker.store.is_none() && worker.model.is_none());
    assert!(!worker.paths.data.exists());
}

#[test]
fn removal_resolves_the_stored_root_after_a_temporary_probe_has_completed() {
    let fixture = TestFixture::new();
    let path = fixture.root.path.clone();
    let stored = fixture.store.root_id(&path).unwrap().unwrap();
    let mut worker = test_worker(&path, None);
    worker.enroll(&path, true).unwrap();
    let temporary = worker.enrollment_id(&path).unwrap().unwrap();
    assert!(temporary < 0);
    // The UI still holds its old temporary snapshot while the coordinator
    // completes enrollment. A path-based removal resolves the actual root.
    worker.work.clear();
    worker.store = Some(fixture.store);
    assert_eq!(worker.snapshot.lock().unwrap().roots[0].id, temporary);
    assert_eq!(worker.enrollment_id(&path).unwrap(), Some(stored));
}

#[test]
fn queued_reprobe_invalidates_membership_without_retiring_canonical_vectors() {
    let fixture = TestFixture::new();
    let path = fixture.root.path.clone();
    let id = fixture.store.root_id(&path).unwrap().unwrap();
    let file = fixture.file;
    let vector: Vec<u8> = fixture
        .store
        .connection
        .query_row(
            "SELECT vector FROM segments WHERE file_id=?1",
            [file],
            |row| row.get(0),
        )
        .unwrap();
    let mut worker = test_worker(&path, Some(fixture.store));
    worker.enroll(&path, false).unwrap();
    assert_eq!(worker.work[0].root_id(), id);
    let store = worker.store.as_ref().unwrap();
    let retained: Vec<u8> = store
        .connection
        .query_row(
            "SELECT vector FROM segments WHERE file_id=?1",
            [file],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(retained, vector);
    let verified: i64 = store
        .connection
        .query_row(
            "SELECT COUNT(*) FROM memberships WHERE root_id=?1 AND verified=1",
            [id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(verified, 0);
    assert_eq!(store.roots().unwrap()[0].state, "checking");
    assert!(worker.background_ready());
}

#[test]
fn excluded_enrollment_never_queues_a_probe_or_creates_a_catalog() {
    let fixture = tempfile::tempdir().unwrap();
    let mut worker = test_worker(fixture.path(), None);
    assert!(worker.enroll(Path::new("/proc"), true).is_err());
    worker.exclusions.push(fixture.path().into());
    assert!(worker.enroll(fixture.path(), true).is_err());
    assert!(worker.work.is_empty() && worker.store.is_none());
    assert!(!worker.paths.data.exists());
}

#[test]
fn shutdown_during_indexing_preserves_pending_and_checkpointed_files() {
    for partial in [false, true] {
        let mut fixture = TestFixture::new();
        let root = SourceRoot::from_root(&fixture.root);
        let root_id = fixture.store.roots().unwrap()[0].id;
        let segments: Vec<Segment> = if partial {
            (0..8)
                .map(|index| Segment {
                    modality: "text".into(),
                    location: format!("part {index}"),
                    text: format!("text {index}"),
                    input_hash: vec![index as u8; 32],
                    vector: Some(vec![1.0; super::super::store::DIMENSION]),
                })
                .collect()
        } else {
            Vec::new()
        };
        if partial {
            let checkpoint = Checkpoint {
                cursor: 0,
                text_cursor: 5,
                coverage: "complete".into(),
                format: super::super::store::source_format(&fixture.entry),
                digest: None,
                prepared_hash: Some([9; 32]),
                segments: 0,
            };
            assert!(
                fixture
                    .store
                    .checkpoint(
                        fixture.file,
                        fixture.entry.revision,
                        "model",
                        &segments,
                        &checkpoint
                    )
                    .unwrap()
            );
        } else {
            fixture
                .store
                .connection
                .execute(
                    "UPDATE files SET state='pending' WHERE id=?1",
                    [fixture.file],
                )
                .unwrap();
        }
        let file = FileWork {
            published: segments.len(),
            segments,
            ..FileWork::new(
                root.clone(),
                root_id,
                1,
                fixture.entry.clone(),
                fixture.file,
            )
        };
        let mut worker = test_worker(&root.path, Some(fixture.store));
        worker.stop.store(true, Ordering::Relaxed);
        worker.advance(Work::File(Box::new(file)));
        let store = worker.store.as_ref().unwrap();
        let (state, error): (String, Option<String>) = store
            .connection
            .query_row(
                "SELECT state,error FROM files WHERE id=?1",
                [fixture.file],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(state, if partial { "ready" } else { "pending" });
        assert!(error.is_none());
        let root_state = &store.roots().unwrap()[0];
        assert_eq!(root_state.state, "building");
        assert!(root_state.error.is_none());
        let count: i64 = store
            .connection
            .query_row(
                "SELECT COUNT(*) FROM file_checkpoints WHERE file_id=?1",
                [fixture.file],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, i64::from(partial));
        if !partial {
            let fresh_file = || {
                FileWork::new(
                    root.clone(),
                    root_id,
                    1,
                    fixture.entry.clone(),
                    fixture.file,
                )
            };
            let failure = worker.file_failure(fresh_file(), "decoder failed".into());
            assert_eq!(failure.unwrap_err(), "decoder failed");
            assert!(!worker.shutdown_cancelled("decoder failed"));
            worker.stop.store(false, Ordering::Relaxed);
            worker
                .file_failure(fresh_file(), "decoder failed".into())
                .unwrap();
            let (state, error): (String, Option<String>) = worker
                .store
                .as_ref()
                .unwrap()
                .connection
                .query_row(
                    "SELECT state,error FROM files WHERE id=?1",
                    [fixture.file],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .unwrap();
            assert_eq!(state, "failed");
            assert_eq!(error.as_deref(), Some("decoder failed"));
        }
    }
}
#[test]
fn media_reuse_separates_vision_token_budgets_but_preserves_compatible_inputs() {
    let bytes = b"same canonical PNG bytes";
    assert_ne!(
        media_hash(bytes, "image", 140),
        media_hash(bytes, "image", 280)
    );
    assert_eq!(
        media_hash(bytes, "image", 280),
        Sha256::digest(bytes).to_vec()
    );
    assert_eq!(
        media_hash(bytes, "audio", 280),
        Sha256::digest(bytes).to_vec()
    );
}
use super::test_support::queued_service;

#[test]
fn graceful_close_rejects_new_admission_but_preserves_accepted_maintenance() {
    let (mut service, receiver) = queued_service();
    service.send(Command::Clear).unwrap();
    assert!(!service.allow_close());
    assert!(
        !service.stop.load(Ordering::Relaxed),
        "accepted maintenance was cancelled"
    );
    assert!(service.send(Command::Setup).is_err());
    assert!(matches!(receiver.recv().unwrap(), Command::Clear));
    service.dispatch.lock().unwrap().queued -= 1;
    assert!(!service.allow_close(), "active maintenance was discarded");
    service.snapshot.lock().unwrap().maintenance = false;
    assert!(service.allow_close());
    assert!(service.stop.load(Ordering::Relaxed));
}

#[test]
fn graceful_close_waits_for_the_coordinator_to_reap_its_model() {
    let fixture = tempfile::tempdir().unwrap();
    let mut worker = test_worker(fixture.path(), None);
    model_preemption_fixture(&mut worker, "unused");
    let model = worker.model(Role::Answer, &AtomicBool::new(false)).unwrap();
    let pid = model.info["pid"].as_u64().unwrap();
    let process = PathBuf::from(format!("/proc/{pid}"));
    assert!(process.exists());
    let (mut service, receiver) = queued_service();
    worker.dispatch = service.dispatch.clone();
    worker.snapshot = service.snapshot.clone();
    worker.stop = service.stop.clone();
    service.worker = Some(std::thread::spawn(move || worker.run(receiver)));
    let deadline = Instant::now() + Duration::from_secs(3);
    while !service.allow_close() {
        assert!(
            Instant::now() < deadline,
            "coordinator did not finish shutdown"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(
        !process.exists(),
        "close was approved before the helper was reaped"
    );
    assert!(service.worker.is_none());
}
fn interrupt_catalog(
    worker: &mut Worker,
    table: &str,
) -> (mpsc::Receiver<Command>, Arc<AtomicBool>) {
    assert!(matches!(table, "roots" | "files" | "paths"));
    let (service, receiver) = queued_service();
    worker.dispatch = service.dispatch.clone();
    worker.snapshot = service.snapshot.clone();
    worker.stop = service.stop.clone();
    worker.update_roots(true);
    let service = Arc::new(service);
    let admitted = service.clone();
    let read_completed = Arc::new(AtomicBool::new(false));
    let completed = read_completed.clone();
    worker
        .store
        .as_ref()
        .unwrap()
        .connection
        .create_scalar_function(
            "clip_test_read_finished",
            1,
            rusqlite::functions::FunctionFlags::SQLITE_UTF8,
            move |context| {
                completed.store(true, Ordering::Relaxed);
                context.get::<i64>(0)
            },
        )
        .unwrap();
    worker
        .store
        .as_ref()
        .unwrap()
        .connection
        .create_scalar_function(
            "clip_test_accept_command",
            0,
            rusqlite::functions::FunctionFlags::SQLITE_UTF8,
            move |_| {
                admitted.send(Command::Setup).unwrap();
                Ok(true)
            },
        )
        .unwrap();
    // Only this owned fixture changes its schema. The callback admits an
    // actual command before a deliberately expensive read reaches a result.
    worker
            .store
            .as_ref()
            .unwrap()
            .connection
            .execute_batch(&format!(
                "ALTER TABLE {table} RENAME TO clip_test_records;
             CREATE VIEW {table} AS SELECT * FROM clip_test_records
             WHERE (WITH RECURSIVE cost(n) AS (VALUES(clip_test_accept_command()) UNION ALL SELECT n+1 FROM cost WHERE n<1000000)
                SELECT clip_test_read_finished(sum(n)) FROM cost)>0;"
            ))
            .unwrap();
    (receiver, read_completed)
}

#[test]
fn accepted_command_interrupts_root_summary_and_retry_preserves_snapshot_and_writes() {
    let fixture = TestFixture::new();
    let mut worker = test_worker(&fixture.root.path, Some(fixture.store));
    let (receiver, read_completed) = interrupt_catalog(&mut worker, "roots");
    assert_eq!(worker.snapshot.lock().unwrap().roots[0].vectors, 1);
    worker.update_roots(true);
    assert!(
        !read_completed.load(Ordering::Relaxed),
        "accepted command must interrupt before the expensive read completes"
    );
    assert!(worker.roots_dirty);
    assert!(worker.snapshot.lock().unwrap().error.is_none());
    assert_eq!(worker.snapshot.lock().unwrap().roots[0].vectors, 1);
    assert!(matches!(receiver.try_recv().unwrap(), Command::Setup));
    assert_eq!(worker.dispatch.lock().unwrap().queued, 1);
    // The guard must already be gone: accepted writes cannot inherit the
    // cancelled read's progress handler, even while its token remains set.
    worker
        .store
        .as_ref()
        .unwrap()
        .connection
        .execute_batch(
            "UPDATE clip_test_records SET generation=(WITH RECURSIVE cost(n) AS
               (VALUES(1) UNION ALL SELECT n+1 FROM cost WHERE n<10000) SELECT sum(n) FROM cost);
             DROP VIEW roots; ALTER TABLE clip_test_records RENAME TO roots;",
        )
        .unwrap();
    worker.update_roots(true);
    assert!(
        worker.roots_dirty,
        "queued command must keep summary work deferred"
    );
    worker.dispatch.lock().unwrap().queued -= 1;
    worker.last_snapshot = Instant::now() - Duration::from_secs(1);
    worker.update_roots(false);
    assert!(!worker.roots_dirty);
    let snapshot = worker.snapshot.lock().unwrap();
    assert!(snapshot.error.is_none());
    assert_eq!((snapshot.roots[0].files, snapshot.roots[0].vectors), (1, 1));
}

#[test]
fn accepted_command_interrupts_completion_without_publishing_or_losing_retry() {
    let fixture = TestFixture::new();
    let id = fixture.store.roots().unwrap()[0].id;
    let other = tempfile::tempdir().unwrap();
    let extra = fixture
        .store
        .enroll(&super::super::policy::Root::open(other.path()).unwrap())
        .unwrap();
    fixture
        .store
        .connection
        .execute("UPDATE files SET state='pending'", [])
        .unwrap();
    let mut worker = test_worker(&fixture.root.path, Some(fixture.store));
    let (receiver, completed) = interrupt_catalog(&mut worker, "files");
    worker.finishing.extend([id, extra]);
    worker.finish_root();
    assert!(!completed.load(Ordering::Relaxed));
    assert_eq!(worker.finishing, VecDeque::from([extra, id]));
    assert!(worker.snapshot.lock().unwrap().error.is_none());
    assert!(matches!(receiver.try_recv().unwrap(), Command::Setup));
    worker.store.as_ref().unwrap().connection.execute_batch("UPDATE clip_test_records SET state='ready',revision=revision+(WITH RECURSIVE cost(n) AS
            (VALUES(1) UNION ALL SELECT n+1 FROM cost WHERE n<10000) SELECT sum(n)*0 FROM cost);
            DROP VIEW files; ALTER TABLE clip_test_records RENAME TO files;").unwrap();
    worker.finish_root();
    assert_eq!(
        worker.finishing,
        VecDeque::from([extra, id]),
        "queued control did not defer completion"
    );
    worker.dispatch.lock().unwrap().queued -= 1;
    worker.finish_root();
    assert_eq!(worker.finishing, VecDeque::from([id]));
    worker.finish_root();
    assert!(worker.finishing.is_empty());
    assert_eq!(
        worker.store.as_ref().unwrap().roots().unwrap()[0].state,
        "ready"
    );
}

#[test]
fn orphan_cleanup_progresses_while_queued_indexing_is_paused_or_capacity_blocked() {
    for capacity in [false, true] {
        let fixture = TestFixture::new().without_memberships();
        let root = SourceRoot::from_root(&fixture.root);
        let mut worker = test_worker(&root.path, Some(fixture.store));
        worker.config.paused = !capacity;
        worker.capacity_blocked = capacity;
        worker.work.push_back(Work::File(Box::new(FileWork::new(
            root,
            1,
            1,
            fixture.entry.clone(),
            999,
        ))));
        worker.pruning = Some(Default::default());
        for _ in 0..12 {
            worker.background_tick();
        }
        assert_eq!(
            worker
                .store
                .as_ref()
                .unwrap()
                .connection
                .query_row(
                    "SELECT COUNT(*) FROM segments WHERE vector IS NOT NULL",
                    [],
                    |row| row.get::<_, usize>(0)
                )
                .unwrap(),
            0,
            "reclaimable orphan vectors must not require the work queue to drain"
        );
        assert!(worker.pruning.is_none());
        assert_eq!(worker.work.len(), 1, "cleanup must preserve queued jobs");
        assert_eq!(
            std::fs::read(&fixture.entry.path).unwrap(),
            b"original content"
        );
    }
}

#[test]
fn busy_cleanup_keeps_its_cursor_and_revisits_new_orphans_behind_it() {
    let fixture = TestFixture::new().without_memberships();
    let root = SourceRoot::from_root(&fixture.root);
    let mut worker = test_worker(&root.path, Some(fixture.store));
    worker.work.push_back(Work::File(Box::new(FileWork::new(
        root.clone(),
        1,
        1,
        fixture.entry.clone(),
        999,
    ))));
    for (phase, steps) in [2, 8].into_iter().enumerate() {
        for _ in 0..steps {
            worker.advance(Work::Directory {
                root: root.clone(),
                id: 1,
                generation: 1,
                path: root.path.clone(),
                recursive: false,
                remaining: Some(VecDeque::new()),
            });
            worker.prune_background();
        }
        assert_eq!(
            worker
                .database()
                .unwrap()
                .connection
                .query_row("SELECT COUNT(*) FROM files", [], |row| row
                    .get::<_, usize>(0))
                .unwrap(),
            0,
            "ongoing jobs must advance cleanup and revisit new orphans"
        );
        if phase == 0 {
            // Reuse a low ID behind the active file seek position.
            let (file, _) = worker
                .database()
                .unwrap()
                .observe_object(
                    1,
                    1,
                    &fixture.entry,
                    fixture.entry.identity.unwrap().object(),
                    "model",
                )
                .unwrap();
            assert!(
                worker
                    .database()
                    .unwrap()
                    .publish(
                        file,
                        fixture.entry.revision,
                        "model",
                        &[Segment {
                            modality: "text".into(),
                            location: "part 1".into(),
                            text: "original content".into(),
                            input_hash: vec![1; 32],
                            vector: Some(vec![1.0; super::super::store::DIMENSION]),
                        }],
                        "complete"
                    )
                    .unwrap()
            );
            worker
                .database()
                .unwrap()
                .connection
                .execute("DELETE FROM memberships", [])
                .unwrap();
        }
    }
    assert_eq!(worker.work.len(), 1);
    worker.work.clear();
    for _ in 0..8 {
        worker.prune_background();
    }
    assert!(
        worker.pruning.is_none(),
        "cleanup must stop after finite quiet passes"
    );
}

#[test]
fn queued_metadata_preserves_move_reuse_until_pressure_requires_retirement() {
    let fixture = TestFixture::new().without_memberships();
    fixture
        .store
        .remember_source(
            fixture.file,
            fixture.entry.revision,
            "model",
            &fixture.entry,
            &[9; 32],
        )
        .unwrap();
    let root = SourceRoot::from_root(&fixture.root);
    let mut worker = test_worker(&root.path, Some(fixture.store));
    worker.work.push_back(Work::Directory {
        root: root.clone(),
        id: 1,
        generation: 1,
        path: root.path.clone(),
        recursive: true,
        remaining: None,
    });
    worker.pruning = Some(Default::default());
    for _ in 0..4 {
        worker.prune_background();
    }
    assert!(
        worker
            .database()
            .unwrap()
            .reusable_source(&fixture.entry, "model", &[9; 32])
            .unwrap()
            .is_some(),
        "source-before-destination scans must retain reusable canonical inputs"
    );
    worker.capacity_blocked = true;
    worker.pruning = Some(Default::default());
    for _ in 0..4 {
        worker.prune_background();
    }
    assert!(
        !worker.capacity_blocked,
        "committed vector reclamation must permit an admission retry"
    );
    assert!(
        worker
            .database()
            .unwrap()
            .reusable_source(&fixture.entry, "model", &[9; 32])
            .unwrap()
            .is_none()
    );
    assert_eq!(worker.work.len(), 1);
    assert_eq!(
        std::fs::read(&fixture.entry.path).unwrap(),
        b"original content"
    );
}

#[test]
fn accepted_command_interrupts_prune_selection_and_writes_do_not_inherit_guard() {
    let fixture = TestFixture::new().without_memberships();
    let mut worker = test_worker(&fixture.root.path, Some(fixture.store));
    let (receiver, completed) = interrupt_catalog(&mut worker, "paths");
    worker.pruning = Some(Default::default());
    worker.prune_background();
    assert!(!completed.load(Ordering::Relaxed));
    assert!(worker.pruning.is_some());
    assert!(worker.snapshot.lock().unwrap().error.is_none());
    assert!(matches!(receiver.try_recv().unwrap(), Command::Setup));
    worker.store.as_ref().unwrap().connection.execute_batch("UPDATE clip_test_records SET parent=parent WHERE
            (WITH RECURSIVE cost(n) AS (VALUES(1) UNION ALL SELECT n+1 FROM cost WHERE n<10000) SELECT sum(n) FROM cost)>0;
            DROP VIEW paths; ALTER TABLE clip_test_records RENAME TO paths;").unwrap();
    worker.prune_background();
    assert!(worker.pruning.is_some());
    worker.dispatch.lock().unwrap().queued -= 1;
    for _ in 0..10 {
        worker.prune_background();
    }
    assert!(worker.pruning.is_none());
    let rows: usize = worker
        .store
        .as_ref()
        .unwrap()
        .connection
        .query_row("SELECT count(*) FROM segments", [], |row| row.get(0))
        .unwrap();
    assert_eq!(rows, 0);
}

#[test]
fn accepted_commands_preempt_background_but_queue_rejection_does_not() {
    let (service, receiver) = queued_service();
    let token = service.dispatch.lock().unwrap().cancel.clone();
    assert!(service.dispatch.lock().unwrap().begin_background());
    service.send(Command::Setup).unwrap();
    assert!(token.load(Ordering::Relaxed));
    assert!(!service.dispatch.lock().unwrap().begin_background());
    assert!(token.load(Ordering::Relaxed));
    receiver.recv().unwrap();
    service.dispatch.lock().unwrap().queued -= 1;
    assert!(service.dispatch.lock().unwrap().begin_background());
    assert!(!token.load(Ordering::Relaxed));
    service.send(Command::Setup).unwrap();
    // Rejection must neither add phantom pending work nor interrupt again.
    token.store(false, Ordering::Relaxed);
    assert!(service.send(Command::Setup).is_err());
    assert!(!token.load(Ordering::Relaxed));
    assert_eq!(service.dispatch.lock().unwrap().queued, 1);
    let dispatch = service.dispatch.clone();
    drop(service);
    assert!(token.load(Ordering::Relaxed));
    dispatch.lock().unwrap().queued = 0;
    assert!(!dispatch.lock().unwrap().begin_background());
    assert!(
        token.load(Ordering::Relaxed),
        "shutdown cancellation must not be reset"
    );
}

#[test]
fn rejected_controls_preserve_interactive_work_and_accepted_controls_cancel_it() {
    let (service, receiver) = queued_service();
    let cancel = Arc::new(AtomicBool::new(false));
    service
        .send(Command::Search {
            id: 1,
            path: "/ordinary".into(),
            query: Query::parse(":emb:\"ordinary\"").unwrap(),
            hidden: false,
            cancel: cancel.clone(),
        })
        .unwrap();
    assert!(service.send(Command::Clear).is_err());
    assert!(!cancel.load(Ordering::Relaxed));
    assert!(!service.maintenance_busy());
    receiver.recv().unwrap();
    service.send(Command::Clear).unwrap();
    assert!(cancel.load(Ordering::Relaxed));
    assert!(service.maintenance_busy());
    assert!(service.dispatch.lock().unwrap().interactive.is_none());
}

fn model_preemption_fixture(worker: &mut Worker, stage: &str) {
    worker.pipeline = "search-v2:\"fixture\"".into();
    let paths = &mut worker.paths;
    paths.helpers = paths.data.join("helpers");
    std::fs::create_dir_all(&paths.helpers).unwrap();
    std::fs::create_dir_all(paths.runtime().join("lib")).unwrap();
    std::fs::create_dir_all(paths.models()).unwrap();
    for name in [
        "embeddinggemma-2-Q8_0.gguf",
        "mmproj-embeddinggemma-2-Q8_0.gguf",
        "Qwen_Qwen3.5-9B-Q8_0.gguf",
    ] {
        std::fs::write(paths.models().join(name), b"").unwrap();
    }
    std::fs::write(paths.runtime().join("lib/libclip-search.so"), b"").unwrap();
    std::fs::write(paths.runtime().join("lib/libclip-answer.so"), b"").unwrap();
    std::fs::write(paths.helpers.join("setup.py"), b"").unwrap();
    std::fs::write(paths.helpers.join("stage"), stage).unwrap();
    std::fs::write(paths.helpers.join("worker.py"), r#"import base64,json,os,struct,sys,time
from pathlib import Path
root=Path(__file__).parent
stage=(root/'stage').read_text()
def hold(name):
    if stage==name and not (root/'release').exists():
        (root/'held.tmp').write_text(str(os.getpid()))
        (root/'held.tmp').replace(root/'held')
        time.sleep(30)
hold('load')
backend=sys.argv[sys.argv.index('--device')+1]
role=sys.argv[sys.argv.index('--role')+1]
pipeline={'provider':sys.argv[sys.argv.index('--answer-provider')+1]} if role=='answer' else ((root/'pipeline').read_text() if (root/'pipeline').exists() else 'fixture')
print(json.dumps({'ready':True,'role':role,'dimension':768,'backend':backend,'device':'test','pipeline':pipeline,'pid':os.getpid()}),flush=True)
for line in sys.stdin:
    request=json.loads(line)
    hold(request['op'])
    result={'id':request['id'],'ok':True}
    if request['op']=='passage':
        size=len(request['text'].encode())
        result.update(end=size,next=size)
    else:
        vector=struct.pack('<f',768**-0.5)*768
        result.update(dimension=768,count=1,vectors=base64.b64encode(vector).decode())
    print(json.dumps(result),flush=True)
"#).unwrap();
}

#[test]
fn changed_embedding_pipeline_retires_work_and_aborts_before_new_inference() {
    let (fixture, mut worker, mut inventory) = inventory_fixture(3);
    let root = SourceRoot::from_root(&fixture.root);
    worker.config.roots = vec![little_clip_core::path_address::display_path(&root.path)];
    worker.store = Some(fixture.store);
    model_preemption_fixture(&mut worker, "unused");
    let cancel = AtomicBool::new(false);
    worker.model(Role::Embedding, &cancel).unwrap();
    inventory.pipeline = worker.pipeline.clone();
    worker.work.push_back(Work::Inventory(Box::new(inventory)));
    worker.work.push_back(Work::File(Box::new(FileWork::new(
        root.clone(),
        1,
        1,
        fixture.entry.clone(),
        fixture.file,
    ))));
    // A real helper restart with the same pipeline must preserve all work.
    worker.model = None;
    worker.model(Role::Embedding, &cancel).unwrap();
    assert_eq!(worker.work.len(), 2);
    assert!(worker.snapshot.lock().unwrap().clear_results.is_none());

    let mut owned_file = FileWork::new(root.clone(), 1, 1, fixture.entry, fixture.file);
    owned_file.prepared = Some(Prepared {
        modality: "text".into(),
        text: "new passage".into(),
        location: "part 2".into(),
        ..Default::default()
    });
    owned_file.segments.push(Segment {
        modality: "text".into(),
        location: "part 1".into(),
        text: "old prefix".into(),
        input_hash: vec![1; 32],
        vector: Some(vec![1.0; super::super::store::DIMENSION]),
    });
    std::fs::write(worker.paths.helpers.join("pipeline"), "updated").unwrap();
    worker.model = None;
    let error = worker.text_segment(&mut owned_file).err().unwrap();
    assert!(error.contains("pipeline changed"));
    assert_eq!(owned_file.text_cursor, 0);
    assert_eq!(owned_file.segments.len(), 1);
    assert_eq!(owned_file.segments[0].text, "old prefix");
    assert!(
        worker.model.is_none(),
        "changed helper must be retired before returning"
    );
    assert_eq!(worker.pipeline, "search-v2:\"updated\"");
    assert_eq!(worker.work.len(), 1);
    assert!(matches!(worker.work.front(), Some(Work::Probe { path, .. }) if *path == root.path));
    assert!(matches!(
        worker.snapshot.lock().unwrap().clear_results,
        Some(ResultReset::Semantic)
    ));
    assert_eq!(worker.snapshot.lock().unwrap().generated_vectors, 0);
    // The next load uses the new pipeline without repeatedly restarting it.
    worker.model(Role::Embedding, &cancel).unwrap();
    assert_eq!(worker.work.len(), 1);
    assert_eq!(worker.snapshot.lock().unwrap().generated_vectors, 0);
}

#[test]
fn initial_pipeline_selection_and_answer_roles_preserve_queued_index_work() {
    let (_fixture, mut worker, inventory) = inventory_fixture(1);
    model_preemption_fixture(&mut worker, "unused");
    worker.pipeline.clear();
    worker.work.push_back(Work::Inventory(Box::new(inventory)));
    let cancel = AtomicBool::new(false);
    worker.model(Role::Embedding, &cancel).unwrap();
    let pipeline = worker.pipeline.clone();
    worker.model(Role::Answer, &cancel).unwrap();
    assert_eq!(worker.pipeline, pipeline);
    worker.model(Role::Embedding, &cancel).unwrap();
    assert_eq!(worker.work.len(), 1);
    assert!(worker.snapshot.lock().unwrap().clear_results.is_none());
}

#[test]
fn idle_model_release_ignores_unrelated_queued_index_work() {
    for role in [Role::Embedding, Role::Answer] {
        let (_fixture, mut worker, inventory) = inventory_fixture(1);
        model_preemption_fixture(&mut worker, "unused");
        let cancel = AtomicBool::new(false);
        let model = worker.model(role, &cancel).unwrap();
        let pid = model.info["pid"].as_u64().unwrap();
        let process = PathBuf::from(format!("/proc/{pid}"));
        assert!(process.exists());
        worker.work.push_back(Work::Inventory(Box::new(inventory)));
        // An admitted command prevents indexing from consuming this queue.
        worker.dispatch.lock().unwrap().queued = 1;

        worker.model.as_mut().unwrap().last_used = Instant::now();
        worker.background_tick();
        assert!(worker.model.is_some(), "recently used model was released");

        worker.model.as_mut().unwrap().last_used = Instant::now() - Duration::from_secs(301);
        worker.config.keep_model_loaded = true;
        worker.background_tick();
        assert!(worker.model.is_some(), "keep-loaded preference was ignored");
        assert!(process.exists());

        worker.config.keep_model_loaded = false;
        worker.background_tick();
        assert!(
            worker.model.is_none(),
            "queued metadata retained an idle model"
        );
        assert!(worker.snapshot.lock().unwrap().backend.is_none());
        assert!(!process.exists(), "idle helper was not killed and reaped");
        assert_eq!(worker.work.len(), 1, "model release discarded queued work");
    }
}

#[test]
fn command_preempts_model_load_and_inference_without_losing_passages_or_media() {
    for stage in ["load", "text", "image"] {
        let fixture = TestFixture::new();
        let root = SourceRoot::from_root(&fixture.root);
        let root_id = fixture.store.roots().unwrap()[0].id;
        let mut file = FileWork::new(root.clone(), root_id, 1, fixture.entry, fixture.file);
        let prepared = Prepared {
            text: "first passage must survive interruption".into(),
            data: (stage == "image").then(|| STANDARD.encode(b"owned prepared image")),
            modality: "image".into(),
            location: "fixture".into(),
            next: None,
            coverage: "complete".into(),
            vision_tokens: Some(280),
        };
        file.prepared = Some(prepared);
        let mut worker = test_worker(&root.path, Some(fixture.store));
        model_preemption_fixture(&mut worker, stage);
        let (mut service, receiver) = queued_service();
        service.dispatch = worker.dispatch.clone();
        let helpers = worker.paths.helpers.clone();
        let command = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(5);
            while !helpers.join("held").exists() {
                assert!(Instant::now() < deadline, "model did not reach {helpers:?}");
                std::thread::sleep(Duration::from_millis(5));
            }
            let pid = std::fs::read_to_string(helpers.join("held")).unwrap();
            service.send(Command::Setup).unwrap();
            (service, pid)
        });
        let started = Instant::now();
        if stage == "image" {
            let prepared = file.prepared.take().unwrap();
            worker.finish_file(file, prepared).unwrap();
        } else {
            worker.extract_file(file).unwrap();
        }
        let (service, pid) = command.join().unwrap();
        assert!(started.elapsed() < Duration::from_secs(3));
        assert!(
            !Path::new(&format!("/proc/{pid}")).exists(),
            "child must be reaped before yielding"
        );
        assert!(matches!(receiver.recv().unwrap(), Command::Setup));
        worker.dispatch.lock().unwrap().queued -= 1;
        assert!(worker.dispatch.lock().unwrap().begin_background());
        let Some(Work::File(mut retry)) = worker.work.pop_front() else {
            panic!("lost interrupted file")
        };
        assert_eq!(retry.text_cursor, 0);
        assert!(retry.segments.is_empty());
        assert!(retry.prepared.is_some());
        assert!(worker.snapshot.lock().unwrap().error.is_none());
        let retries: usize = worker
            .store
            .as_ref()
            .unwrap()
            .connection
            .query_row("SELECT count(*) FROM file_retries", [], |row| row.get(0))
            .unwrap();
        assert_eq!(retries, 0, "command preemption consumed a file retry");
        std::fs::write(worker.paths.helpers.join("release"), b"").unwrap();
        let segment = if stage == "image" {
            worker
                .media_segment(retry.prepared.as_ref().unwrap())
                .unwrap()
        } else {
            let segment = worker.text_segment(&mut retry).unwrap();
            assert_eq!(
                retry.text_cursor,
                retry.prepared.as_ref().unwrap().text.len()
            );
            assert_eq!(segment.text, "first passage must survive interruption");
            segment
        };
        assert_eq!(
            segment.vector.unwrap().len(),
            super::super::store::DIMENSION
        );
        assert_eq!(worker.snapshot.lock().unwrap().generated_vectors, 1);
        drop(service);
    }
}

#[test]
fn maintenance_admission_interrupts_a_foreground_model_and_reaps_before_return() {
    let fixture = tempfile::tempdir().unwrap();
    let mut worker = test_worker(fixture.path(), None);
    model_preemption_fixture(&mut worker, "answer");
    let (mut service, receiver) = queued_service();
    service.dispatch = worker.dispatch.clone();
    let cancel = Arc::new(AtomicBool::new(false));
    service
        .send(Command::Search {
            id: 1,
            path: fixture.path().into(),
            query: Query::parse(":emb:\"ordinary\"").unwrap(),
            hidden: false,
            cancel: cancel.clone(),
        })
        .unwrap();
    receiver.recv().unwrap();
    worker.dispatch.lock().unwrap().queued -= 1;
    let helpers = worker.paths.helpers.clone();
    let admission = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !helpers.join("held").exists() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        let pid = std::fs::read_to_string(helpers.join("held")).unwrap();
        service.send(Command::Clear).unwrap();
        (service, pid)
    });
    let start = Instant::now();
    let error = worker
        .model(Role::Answer, &cancel)
        .unwrap()
        .answer(
            serde_json::json!({"op":"answer"}),
            Duration::from_secs(30),
            &cancel,
        )
        .unwrap_err();
    let (service, pid) = admission.join().unwrap();
    assert_eq!(error, "Search model request cancelled");
    assert!(start.elapsed() < Duration::from_secs(3));
    assert!(!Path::new(&format!("/proc/{pid}")).exists());
    assert!(matches!(receiver.recv().unwrap(), Command::Clear));
    assert!(
        service.maintenance_busy(),
        "accepted mutation retains its completion lifecycle"
    );
}

#[test]
fn search_publication_wakes_after_unlock_and_registration_cannot_miss_a_result() {
    let (service, _receiver) = queued_service();
    let calls = Arc::new(AtomicU64::new(0));
    let observed = calls.clone();
    let snapshot = service.snapshot.clone();
    let notify: crate::ChangeNotifier = Arc::new(move || {
        assert!(
            snapshot.try_lock().is_ok(),
            "callback ran under snapshot lock"
        );
        observed.fetch_add(1, Ordering::Relaxed);
    });
    service.snapshot.lock().unwrap().progress = Some(SearchProgress {
        id: 7,
        checked: Arc::new(AtomicU64::new(2048)),
    });
    let result = |complete| SearchResult {
        id: 7,
        complete,
        result: Ok(super::super::Results {
            entries: Vec::new(),
            sources: Vec::new(),
            message: String::new(),
            explanations: Default::default(),
        }),
    };
    // Completion before registration must still wake the first consumer.
    publish_search_result(&service.snapshot, result(false));
    assert_eq!(calls.load(Ordering::Relaxed), 0);
    assert_eq!(
        service.snapshot.lock().unwrap().checked_segments(7),
        Some(2048)
    );
    service.set_notifier(notify);
    assert_eq!(calls.load(Ordering::Relaxed), 1);
    let previous = calls.load(Ordering::Relaxed);
    // Publication after registration clears final progress and wakes too.
    publish_search_result(&service.snapshot, result(true));
    assert!(calls.load(Ordering::Relaxed) > previous);
    let replacement = Arc::new(AtomicU64::new(0));
    let observed = replacement.clone();
    let old_calls = calls.load(Ordering::Relaxed);
    service.set_notifier(Arc::new(move || {
        observed.fetch_add(1, Ordering::Relaxed);
    }));
    assert_eq!(replacement.load(Ordering::Relaxed), 1);
    publish_search_result(&service.snapshot, result(true));
    assert_eq!(replacement.load(Ordering::Relaxed), 2);
    assert_eq!(calls.load(Ordering::Relaxed), old_calls);
    let mut snapshot = service.snapshot.lock().unwrap();
    assert!(snapshot.result.as_ref().unwrap().complete);
    assert!(snapshot.progress.is_none());
    snapshot.notifier = None;
}
#[test]
fn progress_is_request_scoped_and_replaced_only_after_queue_admission() {
    let (service, receiver) = queued_service();
    let checked = Arc::new(AtomicU64::new(2048));
    service.snapshot.lock().unwrap().progress = Some(SearchProgress {
        id: 4,
        checked: checked.clone(),
    });
    service.send(Command::Setup).unwrap();
    let next = || Command::Search {
        id: 5,
        path: PathBuf::from("/unused"),
        query: Query::parse(":image").unwrap(),
        hidden: false,
        cancel: Arc::new(AtomicBool::new(false)),
    };
    assert!(service.send(next()).is_err());
    {
        let snapshot = service.snapshot.lock().unwrap();
        assert_eq!(snapshot.checked_segments(4), Some(2048));
        assert_eq!(snapshot.checked_segments(5), None);
    }
    receiver.try_recv().unwrap();
    service.send(next()).unwrap();
    checked.store(8192, Ordering::Relaxed); // Retired worker may still own its counter.
    assert_eq!(service.snapshot.lock().unwrap().checked_segments(4), None);
    assert_eq!(service.snapshot.lock().unwrap().checked_segments(5), None);
}

#[test]
fn rejected_maintenance_does_not_leave_close_blocked() {
    let (service, receiver) = queued_service();
    service.snapshot.lock().unwrap().maintenance_outcome = Some(Ok("Previous backup saved".into()));
    service.send(Command::Setup).unwrap();
    assert!(service.send(Command::Clear).is_err());
    assert!(!service.maintenance_busy());
    assert_eq!(
        service.snapshot.lock().unwrap().maintenance_outcome,
        Some(Ok("Previous backup saved".into()))
    );
    assert!(matches!(receiver.try_recv().unwrap(), Command::Setup));
}

#[test]
fn dropping_service_with_poisoned_dispatch_still_cancels_without_panicking() {
    let interactive = Arc::new(AtomicBool::new(false));
    let dispatch = Arc::new(Mutex::new(Dispatch {
        interactive: Some(interactive.clone()),
        ..Default::default()
    }));
    let background = dispatch.lock().unwrap().cancel.clone();
    let stop = Arc::new(AtomicBool::new(false));
    let (commands, _incoming) = mpsc::sync_channel(1);
    let service = Service {
        commands,
        dispatch: dispatch.clone(),
        snapshot: Default::default(),
        stop: stop.clone(),
        worker: None,
    };
    let poison = std::panic::catch_unwind(|| {
        let _guard = dispatch.lock().unwrap();
        panic!("simulate a failed dispatch update");
    });
    assert!(poison.is_err());
    assert!(dispatch.is_poisoned());

    assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(service))).is_ok());
    assert!(stop.load(Ordering::Relaxed));
    assert!(background.load(Ordering::Relaxed));
    assert!(interactive.load(Ordering::Relaxed));
    assert!(dispatch.lock().err().unwrap().into_inner().stopped);
}
