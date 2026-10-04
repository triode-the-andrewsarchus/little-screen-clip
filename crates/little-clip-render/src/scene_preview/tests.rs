use super::*;
use std::sync::atomic::AtomicUsize;
use std::time::SystemTime;

fn key(name: &str) -> Key {
    Key {
        path: PathBuf::from(name),
        modified: Some(SystemTime::UNIX_EPOCH),
        file_bytes: 1,
        size: [97, 64],
        appearance: "dark".into(),
        triangle_budget: 100,
    }
}

fn wait_for(
    cache: &mut PreviewCache,
    key: Key,
    bake: impl Fn(Arc<AtomicBool>) -> Result<OrbitLoop, String> + Clone + Send + 'static,
) -> Preview {
    let started = Instant::now();
    loop {
        let result = cache.request(key.clone(), bake.clone());
        if !matches!(result, Preview::Loading) {
            return result;
        }
        assert!(started.elapsed() < Duration::from_secs(10));
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn mirror_reuses_exported_orbit_without_loading_scene_or_owning_daemon_files() {
    let sources = tempfile::tempdir().unwrap();
    let path = sources.path().join("unloadable.sog");
    std::fs::write(&path, b"not a scene").unwrap();
    let config = little_clip_core::AppConfig::default();
    let daemon = crate::ScreenRenderer::fallback(config.clone());
    let metadata = path.metadata().unwrap();
    let identity = Key {
        path: path.clone(),
        modified: metadata.modified().ok(),
        file_bytes: metadata.len(),
        size: [97, 64],
        appearance: daemon.palette.colors.surface_recessed.clone(),
        triangle_budget: config.model3d.preview_triangle_limit,
    };
    let orbit = OrbitLoop::bake_frames(97, 64, |_| {
        Ok(image::RgbaImage::from_pixel(
            97,
            64,
            image::Rgba([32, 48, 64, 255]),
        ))
    })
    .unwrap();
    let directory = orbit.frames()[0].parent().unwrap().to_owned();
    daemon.scene_previews.lock().unwrap().entries.insert(
        identity,
        Entry {
            value: Value::Ready(Arc::new(orbit)),
            access: 0,
        },
    );
    let exported = daemon.scene_previews();
    let mut mirror = crate::ScreenRenderer::fallback(config);
    mirror.set_scene_preview_source(move || exported.clone());
    let started = Instant::now();
    let acquired = loop {
        match request(&path, [97, 64], &mirror) {
            Preview::Ready(orbit) => break orbit,
            Preview::Error(error) => {
                panic!("Sharing must avoid loading invalid source bytes: {error}")
            }
            Preview::Loading => {}
        }
        assert!(started.elapsed() < Duration::from_secs(5));
        std::thread::sleep(Duration::from_millis(1));
    };
    let reader_directory = acquired.frames()[0].parent().unwrap().to_owned();
    drop(daemon);
    assert!(!directory.exists());
    assert_eq!(
        acquired.read_frame(0).unwrap()[(0, 0)],
        image::Rgba([32, 48, 64, 255])
    );
    drop(acquired);
    mirror
        .render_pixmap(&little_clip_core::SlotManager::default())
        .unwrap();
    assert!(!reader_directory.exists());
    // The retired descriptor cannot resurrect deleted files; fallback reports the bad source.
    let started = Instant::now();
    loop {
        match request(&path, [97, 64], &mirror) {
            Preview::Error(_) => break,
            Preview::Ready(_) => panic!("Retired frame files must not be reused"),
            Preview::Loading => {}
        }
        assert!(started.elapsed() < Duration::from_secs(5));
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn slow_scene_bake_does_not_block_general_preview_workers() {
    let mut cache = PreviewCache::default();
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    cache.request(key("slow.sog"), move |_| {
        started_tx.send(()).unwrap();
        release_rx.recv().unwrap();
        Err("finished fixture".into())
    });
    started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    // Four bakes would exhaust the ordinary four-worker executor.
    let mut extra_releases = Vec::new();
    for index in 0..3 {
        let (release, wait) = mpsc::channel();
        extra_releases.push(release);
        cache.request(key(&format!("queued-{index}.sog")), move |_| {
            wait.recv().unwrap();
            Err("finished queued fixture".into())
        });
    }
    let (_, result) = BackgroundJobs::global().submit(Duration::from_secs(5), || {
        let renderer = crate::ScreenRenderer::fallback(little_clip_core::AppConfig::default());
        let manager = little_clip_core::SlotManager {
            left_hand: Some(little_clip_core::ClipItem::from_text("fast text preview")),
            ..Default::default()
        };
        renderer.render_pixmap(&manager).unwrap().width()
    });
    let rendered = result.recv_timeout(Duration::from_secs(2));
    release_tx.send(()).unwrap();
    for release in extra_releases {
        release.send(()).unwrap();
    }
    assert_eq!(rendered.unwrap().unwrap(), crate::SCREEN_WIDTH);
}

#[test]
fn unchanged_preview_failure_is_cached_and_a_source_revision_restarts_once() {
    let mut cache = PreviewCache::default();
    let starts = Arc::new(AtomicUsize::new(0));
    let worker_starts = Arc::clone(&starts);
    let bake = move |_| {
        worker_starts.fetch_add(1, Ordering::SeqCst);
        Err("Invalid fixture scene".into())
    };
    let original = key("invalid.splat");
    assert!(matches!(
        wait_for(&mut cache, original.clone(), bake.clone()),
        Preview::Error(_)
    ));
    for _ in 0..100 {
        assert!(matches!(
            cache.request(original.clone(), bake.clone()),
            Preview::Error(_)
        ));
    }
    assert_eq!(starts.load(Ordering::SeqCst), 1);
    let mut replacement = original;
    replacement.file_bytes = 2;
    wait_for(&mut cache, replacement, bake);
    assert_eq!(starts.load(Ordering::SeqCst), 2);
    assert_eq!(cache.entries.len(), 1);
}

#[test]
fn changing_appearance_cancels_a_running_old_preview() {
    let mut cache = PreviewCache::default();
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let original = key("changing.splat");
    cache.request(original.clone(), move |cancelled| {
        started_tx.send(cancelled).unwrap();
        release_rx.recv().unwrap();
        Err("Old preview finished".into())
    });
    let cancelled = started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let mut replacement = original;
    replacement.appearance = "light".into();
    cache.request(replacement, |_| Err("New fixture".into()));
    assert!(cancelled.load(Ordering::Acquire));
    release_tx.send(()).unwrap();
    assert_eq!(cache.entries.len(), 1);
}

#[test]
fn invisible_ready_preview_removes_tmp_frames_and_pending_preview_is_cancelled() {
    let orbit = OrbitLoop::bake_frames(1, 1, |_| {
        Ok(image::RgbaImage::from_pixel(
            1,
            1,
            image::Rgba([0, 0, 0, 255]),
        ))
    })
    .unwrap();
    let directory = orbit.frames()[0].parent().unwrap().to_owned();
    let mut cache = PreviewCache::default();
    cache.entries.insert(
        key("ready.sog"),
        Entry {
            value: Value::Ready(Arc::new(orbit)),
            access: 0,
        },
    );
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    cache.request(key("pending.sog"), move |cancelled| {
        started_tx.send(cancelled).unwrap();
        release_rx.recv().unwrap();
        Err("Cancelled fixture".into())
    });
    let cancelled = started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    cache.retain_visible(&HashSet::new());
    assert!(cache.entries.is_empty());
    assert!(!directory.exists());
    assert!(cancelled.load(Ordering::Acquire));
    release_tx.send(()).unwrap();
}

#[test]
fn renderer_retires_hidden_slots_and_drop_removes_remaining_tmp_files() {
    let source_directory = tempfile::tempdir().unwrap();
    let source = source_directory.path().join("visible.sog");
    std::fs::write(&source, b"fixture").unwrap();
    let mut config = little_clip_core::AppConfig::default();
    config.slots.left_priors = 2;
    let renderer = crate::ScreenRenderer::fallback(config);
    let make_orbit = || {
        OrbitLoop::bake_frames(1, 1, |_| {
            Ok(image::RgbaImage::from_pixel(
                1,
                1,
                image::Rgba([0, 0, 0, 255]),
            ))
        })
        .unwrap()
    };
    let orbit = make_orbit();
    let directory = orbit.frames()[0].parent().unwrap().to_owned();
    let mut identity = key("visible.sog");
    identity.path = source.clone();
    renderer.scene_previews.lock().unwrap().entries.insert(
        identity.clone(),
        Entry {
            value: Value::Ready(Arc::new(orbit)),
            access: 0,
        },
    );
    let mut manager = little_clip_core::SlotManager::default();
    manager.left_prior3 = Some(little_clip_core::ClipItem::from_text(
        source.to_string_lossy(),
    ));
    // The configured layout includes only two left-history cards.
    renderer.render_pixmap(&manager).unwrap();
    assert!(!directory.exists());
    let orbit = make_orbit();
    let directory = orbit.frames()[0].parent().unwrap().to_owned();
    renderer.scene_previews.lock().unwrap().entries.insert(
        identity,
        Entry {
            value: Value::Ready(Arc::new(orbit)),
            access: 0,
        },
    );
    drop(renderer);
    assert!(!directory.exists());
}

#[test]
#[ignore = "requires a native compute-capable GPU adapter"]
fn successful_orbit_is_baked_once_and_repeated_playback_reuses_cpu_frames() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("fixture.splat");
    let mut bytes = Vec::new();
    for position in [[-0.5f32, 0.0, 0.0], [0.5, 0.3, 0.2]] {
        for value in position.into_iter().chain([0.1, 0.2, 0.15]) {
            bytes.extend(value.to_le_bytes());
        }
        bytes.extend([255, 30, 20, 240, 255, 128, 128, 128]);
    }
    std::fs::write(&path, bytes).unwrap();
    let starts = Arc::new(AtomicUsize::new(0));
    let worker_starts = Arc::clone(&starts);
    let card_path = path.clone();
    let bake = move |cancelled: Arc<AtomicBool>| {
        worker_starts.fetch_add(1, Ordering::SeqCst);
        let scene = SplatScene::load(&path).unwrap();
        let mut renderer = SplatRenderer::new(Arc::new(scene)).unwrap();
        renderer
            .bake_orbit(97, 64, || cancelled.load(Ordering::Acquire))
            .map_err(|error| error.to_string())
    };
    let mut cache = PreviewCache::default();
    let identity = key("fixture.splat");
    let Preview::Ready(original) = wait_for(&mut cache, identity.clone(), bake.clone()) else {
        panic!("Expected orbit");
    };
    assert_eq!(
        original.frames().len(),
        little_clip_scene::ORBIT_FRAME_COUNT
    );
    assert!(
        original
            .read_frame(0)
            .unwrap()
            .pixels()
            .any(|pixel| pixel[0] > 20)
    );
    for index in 0..100 {
        let Preview::Ready(current) = cache.request(identity.clone(), bake.clone()) else {
            panic!("Lost cached orbit");
        };
        assert!(Arc::ptr_eq(&original, &current));
        current.frame_at(Duration::from_millis(index * 50)).unwrap();
    }
    assert_eq!(starts.load(Ordering::SeqCst), 1);
    assert!(cache.retained_bytes() <= MAX_BYTES);

    // Exercise the production text-path card, cache admission and raster blit together.
    let renderer = crate::ScreenRenderer::fallback(little_clip_core::AppConfig::default());
    let manager = little_clip_core::SlotManager {
        left_hand: Some(little_clip_core::ClipItem::from_text(
            card_path.to_string_lossy(),
        )),
        ..Default::default()
    };
    assert!(manager.has_active_animation());
    let started = Instant::now();
    let initial = renderer.render_pixmap(&manager).unwrap();
    let rendered = loop {
        let frame = renderer.render_pixmap(&manager).unwrap();
        let ready = renderer
            .scene_previews
            .lock()
            .unwrap()
            .entries
            .iter()
            .any(|(key, entry)| key.path == card_path && matches!(entry.value, Value::Ready(_)));
        if ready {
            break frame;
        }
        assert!(started.elapsed() < Duration::from_secs(15));
        std::thread::sleep(Duration::from_millis(10));
    };
    assert_ne!(initial.data(), rendered.data());
    // The actual display-sized orbit must transfer to a fresh mirror renderer.
    let shared = renderer.scene_previews();
    let mut mirror = crate::ScreenRenderer::fallback(little_clip_core::AppConfig::default());
    mirror.set_scene_preview_source(move || shared.clone());
    let warm_start = Instant::now();
    let first_start = Instant::now();
    mirror.render_pixmap(&manager).unwrap();
    let first_frame = first_start.elapsed();
    loop {
        mirror.render_pixmap(&manager).unwrap();
        if !mirror.scene_previews().is_empty() {
            break;
        }
        assert!(warm_start.elapsed() < Duration::from_secs(2));
        std::thread::sleep(Duration::from_millis(1));
    }
    eprintln!(
        "fresh mirror first frame={first_frame:?}; shared orbit ready={:?}",
        warm_start.elapsed()
    );
    assert!(
        rendered
            .pixels()
            .iter()
            .filter(|pixel| { pixel.red() > 80 && pixel.green() < 60 && pixel.blue() < 60 })
            .count()
            > 100,
        "Baked splat pixels must reach the display card"
    );
    let frame_directory = {
        let cache = renderer.scene_previews.lock().unwrap();
        let entry = cache
            .entries
            .iter()
            .find(|(key, _)| key.path == card_path)
            .unwrap()
            .1;
        let Value::Ready(orbit) = &entry.value else {
            panic!("Expected baked preview")
        };
        orbit.frames()[0].parent().unwrap().to_owned()
    };
    renderer
        .render_pixmap(&little_clip_core::SlotManager::default())
        .unwrap();
    assert!(
        !frame_directory.exists(),
        "Removing the visible card must remove its baked frames"
    );
}

#[test]
fn ordinary_glb_uses_a_finite_cached_mesh_orbit() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("mesh.glb");
    std::fs::write(
        &path,
        include_bytes!("../../../little-clip-scene/tests/fixtures/mesh.glb"),
    )
    .unwrap();
    let renderer = crate::ScreenRenderer::fallback(little_clip_core::AppConfig::default());
    let started = Instant::now();
    let orbit = loop {
        match request(&path, [97, 64], &renderer) {
            Preview::Ready(orbit) => break orbit,
            Preview::Error(error) => panic!("{error}"),
            Preview::Loading => {}
        }
        assert!(started.elapsed() < Duration::from_secs(10));
        std::thread::sleep(Duration::from_millis(5));
    };
    assert_eq!(orbit.frames().len(), little_clip_scene::ORBIT_FRAME_COUNT);
    assert_ne!(
        orbit.read_frame(0).unwrap(),
        orbit
            .read_frame(little_clip_scene::ORBIT_FRAME_COUNT / 2)
            .unwrap()
    );
    for _ in 0..100 {
        let Preview::Ready(next) = request(&path, [97, 64], &renderer) else {
            panic!("Lost mesh loop");
        };
        assert!(Arc::ptr_eq(&orbit, &next));
    }
    assert!(
        bake_mesh(
            &renderer,
            &little_clip_core::model3d::Model3d::load_file(&path).unwrap(),
            [97, 64],
            || true
        )
        .is_err()
    );
}
