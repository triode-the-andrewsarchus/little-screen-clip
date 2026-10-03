use super::*;
use std::sync::atomic::AtomicUsize;

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
