//! Real directory events may stall watch workers, never the coordinator's event drain.
use super::*;

#[test]
#[ignore = "requires tmpfs, /dev/fuse and the packaged RAM helper; stops only its owned server"]
fn stopped_directory_registration_keeps_controls_sync_and_shutdown_owned() {
    let fixture = TestDrives::new(0);
    let (backing, id, mut native, view) = fixture.native_source("partition");
    let service = fixture.start_observed_reads(Arc::new(|| {}));
    let small = fixture.pair(&service, "small", "");
    let large = fixture.pair(&service, "large", "");
    let pair = status(&service)
        .pairs
        .into_iter()
        .find(|pair| pair.drive == "part:large")
        .unwrap();
    finish(
        &service,
        DriveRequest::Pause {
            pair: pair.id,
            paused: true,
        },
    );
    // More directories than one worker batch can register before the server is
    // stopped. These are actual kernel create events, not injected descriptors.
    for index in 0..128 {
        fs::create_dir(view.join(format!("new-{index}"))).unwrap();
    }
    let mut closing = None;
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        native.with_suspended(|| {
            let (tid, wait) = wait_fuse_request(|| {
                let mut tids = worker_threads("drive-watch-");
                tids.extend(worker_threads("portable-drive-"));
                tids
            });
            let name = fs::read_to_string(format!("/proc/self/task/{tid}/comm")).unwrap();
            eprintln!(
                "WATCH_REGISTRATION_FUSE_BLOCKED tid={tid} name={} wchan={wait}",
                name.trim()
            );
            assert_configuration_responsive(&service);
            fs::write(
                small.join("independent"),
                b"sync during stalled watch registration",
            )
            .unwrap();
            wait_contents(
                &fixture.temp.path().join("small/independent"),
                b"sync during stalled watch registration",
            );
            assert!(
                name.trim().starts_with("drive-watch-"),
                "Filesystem request ran on {name}"
            );
            closing = Some(shutdown_while_held(&service));
        });
    }));
    if let Some(closing) = closing {
        finish_held_shutdown(closing);
    } else {
        service.shutdown().unwrap();
    }
    assert_eq!(
        fs::read(large.join("edit")).unwrap(),
        b"original portable content"
    );
    finish_native_fixture(backing, id, native, result);
}

#[test]
#[ignore = "requires tmpfs, /dev/fuse and the packaged RAM helper; stops only its owned server"]
fn cancelled_watch_registration_retains_its_pins_and_does_not_hide_other_events() {
    let fixture = TestDrives::new(0);
    let (backing, id, mut native, view) = fixture.native_source("partition");
    let root = Arc::new(Root::open(&view).unwrap());
    let ordinary = Arc::new(Root::open(&fixture.temp.path().join("small")).unwrap());
    let fresh = Arc::new(Root::create(&fixture.temp.path().join("new-observer")).unwrap());
    let mut watch = Watch::new().unwrap();
    watch.add(ordinary.clone());
    crate::watch::overflow_tests::settle(&mut watch);
    let (ready, returned) = mpsc::channel();
    let worker_root = root.clone();
    let mut joining = None;
    let mut retired = None;
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        native.with_suspended(|| {
            joining = Some(std::thread::spawn(move || {
                watch.add(worker_root);
                assert!(ready.send(watch).is_ok());
            }));
            let mut watch = returned
                .recv_timeout(Duration::from_secs(2))
                .expect("Watch::add entered the stopped filesystem");
            watch.changes(false);
            wait_fuse_request(|| worker_threads("drive-watch-"));
            watch.add(fresh.clone());
            let started = Instant::now();
            loop {
                fs::write(fresh.path.join("new-root-event"), b"newly registered root").unwrap();
                let changes = watch.changes(false);
                if changes.paths.contains(&fresh.path.join("new-root-event"))
                    && !watch.busy_scope(&fresh.scope())
                {
                    break;
                }
                assert!(
                    started.elapsed() < Duration::from_secs(3),
                    "Stopped root prevented independent registration"
                );
                std::thread::sleep(Duration::from_millis(5));
            }
            fs::write(ordinary.path.join("event"), b"unrelated event").unwrap();
            assert!(
                watch
                    .changes(false)
                    .paths
                    .contains(&ordinary.path.join("event"))
            );
            watch.remove_scope(&root.scope());
            assert!(watch.busy_scope(&root.scope()));
            assert!(
                Arc::strong_count(&root) > 1,
                "Cancelled worker lost its source pin"
            );
            // Rebuilding cannot resurrect the retired source when its old
            // registration acknowledgement or child inventory arrives later.
            watch.rebuild().unwrap();
            retired = Some(std::thread::spawn(move || drop(watch)));
            std::thread::sleep(Duration::from_millis(50));
            assert!(
                !retired.as_ref().unwrap().is_finished(),
                "Watch shutdown abandoned a blocked worker"
            );
        });
    }));
    if let Some(joining) = joining {
        joining.join().unwrap();
    }
    if let Some(retired) = retired {
        retired.join().unwrap();
    }
    drop(returned);
    assert_eq!(Arc::strong_count(&root), 1);
    drop(root);
    finish_native_fixture(backing, id, native, result);
}
