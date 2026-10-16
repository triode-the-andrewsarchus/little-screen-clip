use super::*;
use std::ffi::OsString;
use std::os::unix::{
    ffi::OsStringExt,
    fs::{PermissionsExt, symlink},
};
use std::time::Instant;

fn read(path: &Path) -> Directory {
    read_directory(path, &AtomicBool::new(false)).unwrap()
}

fn finish(manager: &mut FileManager, jobs: &BackgroundJobs) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        manager.tick(jobs);
        if manager.pending.is_none() {
            return;
        }
        assert!(Instant::now() < deadline, "directory read did not finish");
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn loaded(path: &Path) -> (FileManager, BackgroundJobs) {
    let jobs = BackgroundJobs::new(2, 16);
    let mut manager = FileManager::new(path.into());
    finish(&mut manager, &jobs);
    assert!(manager.error.is_none(), "initial folder must load");
    (manager, jobs)
}

fn system_reveal_fixture(
    same_folder: bool,
) -> (
    tempfile::TempDir,
    FileManager,
    BackgroundJobs,
    PathBuf,
    PathBuf,
) {
    let temp = tempfile::tempdir().unwrap();
    let first = temp.path().join("first");
    let second = if same_folder {
        first.clone()
    } else {
        temp.path().join("second")
    };
    std::fs::create_dir_all(&first).unwrap();
    std::fs::create_dir_all(&second).unwrap();
    let original = first.join("original");
    let target = second.join(".target");
    std::fs::write(&original, b"original").unwrap();
    std::fs::write(&target, b"target").unwrap();
    let (mut manager, jobs) = loaded(&first);
    manager.select(original.clone());
    (temp, manager, jobs, original, target)
}

#[test]
fn system_reveal_preserves_browser_history_and_selects_target() {
    let (_temp, mut manager, jobs, original, target) = system_reveal_fixture(false);
    let first = original.parent().unwrap();
    let second = target.parent().unwrap();
    manager.location_mut().unwrap().scroll = 125.0;
    let alias = _temp.path().join("directory-alias");
    symlink(second, &alias).unwrap();
    manager.browse_system_target(
        system::BrowserTarget::new(system::Relation::new(
            "aliased target",
            alias.join(".target"),
        ))
        .unwrap(),
    );
    finish(&mut manager, &jobs);
    assert_eq!(manager.path(), second);
    assert_eq!(manager.location().unwrap().selected.as_ref(), Some(&target));
    assert!(manager.show_hidden);
    assert!(manager.reveal_selection);
    manager.history_step(false);
    finish(&mut manager, &jobs);
    assert_eq!(manager.path(), first);
    assert_eq!(
        manager.location().unwrap().selected.as_ref(),
        Some(&original)
    );
    assert_eq!(manager.location().unwrap().scroll, 125.0);
    manager.browse_system_target(
        system::BrowserTarget::new(system::Relation::new("target", original.clone())).unwrap(),
    );
    finish(&mut manager, &jobs);
    assert_eq!(
        manager.location().unwrap().selected.as_ref(),
        Some(&original)
    );
}

#[test]
fn replaced_system_reveal_does_not_commit_navigation_or_selection() {
    for same_folder in [false, true] {
        let (_temp, mut manager, jobs, original, target) = system_reveal_fixture(same_folder);
        let first = original.parent().unwrap();
        let second = target.parent().unwrap();
        let captured =
            system::BrowserTarget::new(system::Relation::new("target", target.clone())).unwrap();
        manager.browse_system_target(captured);
        std::fs::rename(&target, second.join("previous")).unwrap();
        std::fs::write(&target, b"replacement").unwrap();
        finish(&mut manager, &jobs);
        assert!(manager.error.is_some());
        assert!(
            manager.failed.is_none(),
            "retry must not drop the relationship guard"
        );
        assert!(manager.reveal_target.is_none());
        for retry in [false, true] {
            if retry {
                manager.retry();
                finish(&mut manager, &jobs);
            }
            let location = manager.location().unwrap();
            assert_eq!(manager.path(), first);
            assert_eq!(manager.history.len(), 1);
            assert_eq!(location.selected.as_ref(), Some(&original));
            assert_eq!(location.anchor.as_ref(), Some(&original));
            assert_eq!(location.selection, BTreeSet::from([original.clone()]));
        }
    }
}

fn paused_directory(path: &Path, jobs: &BackgroundJobs) -> (DirectoryJob, mpsc::Sender<()>) {
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let path = path.to_path_buf();
    let (ticket, receiver) = jobs.submit(Duration::MAX, move || {
        started_tx.send(()).unwrap();
        release_rx.recv().unwrap();
        Ok(read(&path))
    });
    started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    (
        DirectoryJob {
            ticket,
            receiver,
            cancelled: Arc::new(AtomicBool::new(false)),
        },
        release_tx,
    )
}

#[test]
fn listing_orders_folders_first_and_preserves_unreadable_metadata() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::create_dir(temp.path().join("z-folder")).unwrap();
    std::fs::write(temp.path().join("a.txt"), b"abc").unwrap();
    std::fs::write(temp.path().join(".hidden"), b"").unwrap();
    symlink(temp.path().join("missing"), temp.path().join("broken")).unwrap();
    let listing = read(temp.path());
    assert_eq!(listing.entries[0].name, "z-folder");
    let file = listing
        .entries
        .iter()
        .find(|entry| entry.name == "a.txt")
        .unwrap();
    assert_eq!(file.size, "3 B");
    assert_ne!(file.modified, "—");
    let broken = listing
        .entries
        .iter()
        .find(|entry| entry.name == "broken")
        .unwrap();
    assert!(broken.symlink);
    assert!(broken.warning.is_some());
    assert_eq!(broken.size, "—");
    assert!(
        listing
            .entries
            .iter()
            .find(|entry| entry.name == ".hidden")
            .unwrap()
            .hidden
    );
}

#[test]
fn photo_collection_excludes_hidden_and_nonimages_without_filtering_browser_entries() {
    let temp = tempfile::tempdir().unwrap();
    for name in ["a.png", "b.JPG", ".hidden.png", "notes.txt", "music.wav"] {
        std::fs::write(temp.path().join(name), b"fixture").unwrap();
    }
    std::fs::create_dir(temp.path().join("folder.png")).unwrap();
    let (mut manager, _jobs) = loaded(temp.path());
    let image = temp.path().join("a.png");
    assert_eq!(manager.photo_collection(&image).len(), 2);
    manager.set_view(BrowserView::Photos);
    assert_eq!(
        manager.photo_collection(&image),
        vec![image.clone(), temp.path().join("b.JPG")]
    );
    assert_eq!(manager.visible.len(), 5);
    assert!(
        manager
            .photo_collection(&temp.path().join("notes.txt"))
            .is_empty()
    );
    manager.toggle_hidden();
    assert_eq!(manager.photo_collection(&image).len(), 3);
    manager.toggle_hidden();
    manager.gallery_columns = 2;
    manager.select(image);
    manager.move_selection(true, false);
    assert_eq!(
        manager.location().unwrap().selected,
        Some(temp.path().join("music.wav"))
    );
    manager.move_selection_by(-1, true);
    assert_eq!(
        manager.location().unwrap().selected,
        Some(temp.path().join("b.JPG"))
    );
    assert_eq!(manager.selection_count(), 2);
}

#[test]
fn music_threshold_uses_visible_regular_files_and_manual_history_choices_survive_refresh() {
    let temp = tempfile::tempdir().unwrap();
    let child = temp.path().join("subfolder");
    std::fs::create_dir(&child).unwrap();
    for name in ["a.wav", "b.mp3", "c.flac", "notes.txt", ".extra.wav"] {
        std::fs::write(temp.path().join(name), "fixture").unwrap();
    }
    let (mut manager, jobs) = loaded(temp.path());
    assert_eq!(manager.view, BrowserView::Music);
    assert_eq!(manager.visible.len(), 5);
    manager.music.play(&temp.path().join("b.mp3"));
    manager.set_view(BrowserView::List);
    manager.refresh();
    finish(&mut manager, &jobs);
    assert_eq!(manager.view, BrowserView::List);
    manager.navigate(child, Navigation::New);
    finish(&mut manager, &jobs);
    assert_eq!(manager.view, BrowserView::List);
    assert_eq!(manager.music.current(), None);
    manager.history_step(false);
    finish(&mut manager, &jobs);
    assert_eq!(manager.view, BrowserView::List);
    manager.location_mut().unwrap().manual_view = None;
    manager.music_threshold = 76;
    manager.activate_media_view();
    assert_eq!(manager.view, BrowserView::List);
    manager.toggle_hidden();
    assert_eq!(manager.view, BrowserView::Music);
}

#[test]
fn address_round_trips_unicode_control_backslash_and_invalid_bytes() {
    for bytes in [
        b"/tmp/a\\b\n\xff".to_vec(),
        "/tmp/照片 and spaces".as_bytes().to_vec(),
        b"/tmp/\xfe\xffend".to_vec(),
    ] {
        let path = PathBuf::from(OsString::from_vec(bytes));
        let text = display_path(&path);
        assert!(!text.contains('\n'));
        assert_eq!(
            parse_address(&text, Path::new("/"), Path::new("/home/user")).unwrap(),
            path
        );
    }
    assert_eq!(display_path(Path::new("/tmp/a\\b")), "/tmp/a\\\\b");
}

#[test]
fn address_expands_home_and_relative_paths_without_shell_evaluation() {
    let current = Path::new("/tmp/current");
    let home = Path::new("/home/test");
    for (input, expected) in [
        ("~", "/home/test"),
        ("~/Pictures", "/home/test/Pictures"),
        ("../a b", "/tmp/current/../a b"),
        ("$(touch sentinel)", "/tmp/current/$(touch sentinel)"),
        ("~other", "/tmp/current/~other"),
    ] {
        assert_eq!(
            parse_address(input, current, home).unwrap(),
            Path::new(expected)
        );
    }
    for invalid in ["", "\\x00", "\\xQQ", "trailing\\"] {
        assert!(parse_address(invalid, current, home).is_err());
    }
}

#[test]
fn history_restores_selection_scroll_and_truncates_forward_branch() {
    let temp = tempfile::tempdir().unwrap();
    let a = temp.path().join("a");
    let b = temp.path().join("b");
    let c = temp.path().join("c");
    for path in [&a, &b, &c] {
        std::fs::create_dir(path).unwrap();
    }
    let selected = a.join("file");
    std::fs::write(&selected, "file").unwrap();
    let (mut manager, jobs) = loaded(temp.path());
    manager.navigate(a.clone(), Navigation::New);
    finish(&mut manager, &jobs);
    manager.select(selected.clone());
    manager.location_mut().unwrap().scroll = 400.0;
    manager.navigate(b.clone(), Navigation::New);
    finish(&mut manager, &jobs);
    manager.history_step(false);
    finish(&mut manager, &jobs);
    assert_eq!(manager.path(), a);
    assert_eq!(
        manager.location().unwrap().selected.as_ref(),
        Some(&selected)
    );
    assert_eq!(manager.location().unwrap().scroll, 400.0);
    manager.history_step(true);
    finish(&mut manager, &jobs);
    assert_eq!(manager.path(), b);
    manager.history_step(false);
    finish(&mut manager, &jobs);
    manager.navigate(c.clone(), Navigation::New);
    finish(&mut manager, &jobs);
    assert_eq!(manager.path(), c);
    assert_eq!(manager.history.len(), 3);
    assert_eq!(manager.index, Some(2));
}

#[test]
fn failed_and_repeated_navigation_do_not_change_history_or_listing() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(temp.path().join("file"), "data").unwrap();
    let (mut manager, jobs) = loaded(temp.path());
    for path in [temp.path().join("missing"), temp.path().join("file")] {
        manager.navigate(path, Navigation::New);
        finish(&mut manager, &jobs);
        assert!(manager.error.is_some());
        assert_eq!(manager.path(), temp.path());
        assert_eq!(manager.history.len(), 1);
        assert_eq!(manager.directory.as_ref().unwrap().entries.len(), 1);
    }
    manager.navigate(temp.path().into(), Navigation::New);
    assert!(manager.pending.is_none());
    manager.refresh();
    finish(&mut manager, &jobs);
    assert_eq!(manager.history.len(), 1);
}

#[test]
fn unavailable_history_destination_remains_retryable() {
    let (temp, child) = folder_fixture();
    let (mut manager, jobs) = loaded(temp.path());
    manager.navigate(child.clone(), Navigation::New);
    finish(&mut manager, &jobs);
    manager.parent();
    finish(&mut manager, &jobs);
    std::fs::remove_dir(&child).unwrap();
    manager.history_step(false);
    finish(&mut manager, &jobs);
    assert_eq!(manager.index, Some(2));
    assert_eq!(manager.history[1].path, child);
    assert!(manager.error.as_ref().unwrap().contains("child"));
    std::fs::create_dir(&child).unwrap();
    manager.retry();
    finish(&mut manager, &jobs);
    assert_eq!(manager.path(), child);
}

fn event(key: egui::Key, modifiers: egui::Modifiers) -> egui::Event {
    egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers,
    }
}

fn frame(
    manager: &mut FileManager,
    ctx: &egui::Context,
    events: Vec<egui::Event>,
) -> egui::FullOutput {
    let palette =
        crate::ui_appearance::palette(little_clip_core::config::WorkbenchAppearance::Dark);
    ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(960.0, 600.0),
            )),
            events,
            ..Default::default()
        },
        |ctx| {
            manager.handle_address_escape(ctx);
            egui::CentralPanel::default().show(ctx, |ui| manager.ui(ui, &palette));
        },
    )
}

fn rendered(path: &Path) -> (FileManager, BackgroundJobs, egui::Context) {
    let (mut manager, jobs) = loaded(path);
    let ctx = egui::Context::default();
    egui_extras::install_image_loaders(&ctx);
    frame(&mut manager, &ctx, vec![]);
    (manager, jobs, ctx)
}

fn press(
    manager: &mut FileManager,
    ctx: &egui::Context,
    key: egui::Key,
    modifiers: egui::Modifiers,
) {
    frame(manager, ctx, vec![event(key, modifiers)]);
    let mut release = event(key, modifiers);
    if let egui::Event::Key { pressed, .. } = &mut release {
        *pressed = false;
    }
    frame(manager, ctx, vec![release]);
}

#[test]
fn address_focus_selection_submission_and_escape_work_in_rendered_ui() {
    let temp = tempfile::tempdir().unwrap();
    let child = temp.path().join("child with spaces");
    std::fs::create_dir(&child).unwrap();
    let (mut manager, jobs, ctx) = rendered(temp.path());
    press(&mut manager, &ctx, egui::Key::L, egui::Modifiers::CTRL);
    assert!(ctx.memory(|memory| memory.has_focus(egui::Id::new("file_manager_address"))));
    frame(
        &mut manager,
        &ctx,
        vec![egui::Event::Text("child with spaces".into())],
    );
    assert_eq!(
        manager.address, "child with spaces",
        "Ctrl+L must replace the entire address"
    );
    press(&mut manager, &ctx, egui::Key::Enter, egui::Modifiers::NONE);
    finish(&mut manager, &jobs);
    assert_eq!(manager.path(), child);
    press(&mut manager, &ctx, egui::Key::L, egui::Modifiers::CTRL);
    frame(
        &mut manager,
        &ctx,
        vec![egui::Event::Text("discarded".into())],
    );
    press(&mut manager, &ctx, egui::Key::Escape, egui::Modifiers::NONE);
    assert_eq!(manager.address, display_path(&child));
    assert!(ctx.memory(|memory| memory.has_focus(egui::Id::new("file_manager_list_focus"))));
    assert!(manager.pending.is_none());
}

#[test]
fn keyboard_arrows_select_without_navigating_and_enter_opens_a_folder() {
    let (temp, child) = folder_fixture();
    let (mut manager, jobs, ctx) = rendered(temp.path());
    ctx.memory_mut(|memory| memory.request_focus(egui::Id::new("file_manager_list_focus")));
    // Publish the requested focus and its lock filter before sending a key.
    frame(&mut manager, &ctx, vec![]);
    press(
        &mut manager,
        &ctx,
        egui::Key::ArrowDown,
        egui::Modifiers::NONE,
    );
    assert_eq!(manager.location().unwrap().selected.as_ref(), Some(&child));
    assert_eq!(manager.path(), temp.path());
    press(&mut manager, &ctx, egui::Key::Enter, egui::Modifiers::NONE);
    finish(&mut manager, &jobs);
    assert_eq!(
        manager.path(),
        child,
        "open_file={:?}, focus={:?}, pending={}",
        manager.open_file,
        ctx.memory(|memory| memory.focused()),
        manager.pending.is_some()
    );
    press(
        &mut manager,
        &ctx,
        egui::Key::ArrowLeft,
        egui::Modifiers::ALT,
    );
    finish(&mut manager, &jobs);
    assert_eq!(manager.path(), temp.path());
}

#[test]
fn hidden_toggle_and_refresh_preserve_only_existing_visible_selection() {
    let temp = tempfile::tempdir().unwrap();
    let hidden = temp.path().join(".secret");
    std::fs::write(&hidden, "data").unwrap();
    let (mut manager, jobs) = loaded(temp.path());
    assert!(manager.visible.is_empty());
    assert!(!manager.directory.as_ref().unwrap().entries.is_empty());
    manager.toggle_hidden();
    manager.select(hidden.clone());
    manager.refresh();
    finish(&mut manager, &jobs);
    assert_eq!(manager.location().unwrap().selected.as_ref(), Some(&hidden));
    manager.toggle_hidden();
    assert!(manager.location().unwrap().selected.is_none());
    manager.toggle_hidden();
    manager.select(hidden.clone());
    std::fs::remove_file(hidden).unwrap();
    manager.refresh();
    finish(&mut manager, &jobs);
    assert!(manager.directory.as_ref().unwrap().entries.is_empty());
    assert!(manager.location().unwrap().selected.is_none());
}

#[test]
fn symlinks_resolve_navigation_and_cycles_fail_without_recursing() {
    let temp = tempfile::tempdir().unwrap();
    let target = temp.path().join("target");
    std::fs::create_dir(&target).unwrap();
    symlink(&target, temp.path().join("alias")).unwrap();
    symlink(temp.path().join("loop-b"), temp.path().join("loop-a")).unwrap();
    symlink(temp.path().join("loop-a"), temp.path().join("loop-b")).unwrap();
    let (mut manager, jobs) = loaded(temp.path());
    manager.navigate(temp.path().join("alias"), Navigation::New);
    finish(&mut manager, &jobs);
    assert_eq!(manager.path(), target);
    manager.navigate(temp.path().join("loop-a"), Navigation::New);
    finish(&mut manager, &jobs);
    assert!(manager.error.is_some());
    assert_eq!(manager.path(), target);
}

#[test]
fn tree_reveals_hidden_ancestors_and_collapsing_releases_descendants() {
    let temp = tempfile::tempdir().unwrap();
    let nested = temp.path().join(".hidden").join("nested");
    std::fs::create_dir_all(&nested).unwrap();
    let mut tree = FolderTree::new(temp.path().into());
    tree.reveal(&nested);
    tree.branches.get_mut(temp.path()).unwrap().directory = Some(read(temp.path()));
    tree.branches
        .get_mut(&temp.path().join(".hidden"))
        .unwrap()
        .directory = Some(read(&temp.path().join(".hidden")));
    let rows = tree.rows(false);
    assert!(rows.iter().any(|row| row.path == nested));
    tree.toggle(temp.path().into());
    assert!(!tree.rows(false).iter().any(|row| row.path == nested));
    assert!(!tree.branches.contains_key(&temp.path().join(".hidden")));
}

#[test]
fn permission_failure_is_distinct_from_an_empty_directory() {
    if unsafe { libc::geteuid() } == 0 {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let blocked = temp.path().join("blocked");
    std::fs::create_dir(&blocked).unwrap();
    std::fs::set_permissions(&blocked, std::fs::Permissions::from_mode(0o000)).unwrap();
    let result = read_directory(&blocked, &AtomicBool::new(false));
    std::fs::set_permissions(&blocked, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert!(result.is_err());
    assert!(read(&blocked).entries.is_empty());
}

#[test]
fn stale_published_completion_cannot_replace_a_new_request() {
    let temp = tempfile::tempdir().unwrap();
    let old = temp.path().join("old");
    let new = temp.path().join("new");
    std::fs::create_dir(&old).unwrap();
    std::fs::create_dir(&new).unwrap();
    let jobs = BackgroundJobs::new(1, 16);
    let mut manager = FileManager::new(temp.path().into());
    finish(&mut manager, &jobs);
    // Publish an old result, then replace its request before the UI polls it.
    let (ticket, receiver) = jobs.submit(Duration::MAX, {
        let old = old.clone();
        move || read(&old)
    });
    let directory = receiver
        .recv_timeout(Duration::from_secs(5))
        .unwrap()
        .unwrap();
    let (sender, receiver) = mpsc::channel();
    sender.send(Ok(Ok(directory))).unwrap();
    manager.pending = Some(Request {
        browser_target: None,
        path: old,
        navigation: Navigation::New,
        job: Some(DirectoryJob {
            ticket,
            receiver,
            cancelled: Arc::new(AtomicBool::new(false)),
        }),
    });
    manager.navigate(new.clone(), Navigation::New);
    assert_eq!(
        manager.path(),
        temp.path(),
        "old listing stays visible during navigation"
    );
    finish(&mut manager, &jobs);
    assert_eq!(manager.path(), new);
    assert_eq!(manager.history.len(), 2);
}

#[test]
fn saturated_executor_retries_without_losing_pending_navigation() {
    let temp = tempfile::tempdir().unwrap();
    let jobs = BackgroundJobs::new(1, 1);
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let (_, _running) = jobs.submit(Duration::MAX, move || {
        started_tx.send(()).unwrap();
        release_rx.recv().unwrap();
    });
    started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let (_, _queued) = jobs.submit(Duration::MAX, || ());
    let mut manager = FileManager::new(temp.path().into());
    manager.tick(&jobs);
    assert!(manager.pending.is_some());
    assert!(manager.error.is_none());
    release_tx.send(()).unwrap();
    finish(&mut manager, &jobs);
    assert_eq!(manager.path(), temp.path());
    assert!(manager.directory.is_some());
}

#[test]
fn initial_refresh_creates_a_location_and_root_has_no_parent_navigation() {
    let temp = tempfile::tempdir().unwrap();
    let mut manager = FileManager::new(temp.path().into());
    let jobs = BackgroundJobs::new(2, 16);
    manager.refresh();
    finish(&mut manager, &jobs);
    assert_eq!(manager.index, Some(0));
    manager.commit(read(Path::new("/")), Navigation::New);
    manager.parent();
    assert!(manager.pending.is_none());
}

#[test]
fn large_directory_virtualizes_rows_and_restores_history_viewport() {
    exercise_large_directory("txt");
}

#[test]
fn large_audio_directory_virtualizes_music_and_restores_history_viewport() {
    exercise_large_directory("wav");
}

#[test]
fn waveform_keyboard_focus_survives_frames_and_routes_clipboard_keys() {
    let (directory, _) = folder_fixture();
    let (mut manager, _jobs, ctx) = rendered(directory.path());
    manager.set_view(BrowserView::Music);
    frame(&mut manager, &ctx, vec![]);
    let id = super::ui::music::waveform_id();
    ctx.memory_mut(|memory| memory.request_focus(id));
    for _ in 0..3 {
        frame(&mut manager, &ctx, vec![]);
        assert!(ctx.memory(|memory| memory.has_focus(id)));
        assert_eq!(
            ctx.data(|data| data
                .get_temp::<bool>(egui::Id::new(egui_winit::CLIPBOARD_SHORTCUTS_AS_KEYS))),
            Some(true)
        );
    }
}

fn exercise_large_directory(extension: &str) {
    let (temp, child) = folder_fixture();
    let (_audio_dir, audio) = crate::audio_preview::tests::generate_fixture_wav("large", 0.1);
    for index in 0..10_000 {
        let path = temp.path().join(format!("file-{index:05}.{extension}"));
        if extension == "wav" {
            std::fs::hard_link(&audio, path).unwrap();
        } else {
            std::fs::write(path, "data").unwrap();
        }
    }
    let started = Instant::now();
    let (mut manager, jobs) = loaded(temp.path());
    let loaded_ms = started.elapsed().as_millis();
    assert_eq!(manager.visible.len(), 10_001);
    for view in [BrowserView::List, BrowserView::Photos, BrowserView::Music] {
        manager.set_view(view);
        manager.location_mut().unwrap().scroll = 0.0;
        manager.restore_scroll = true;
        let ctx = egui::Context::default();
        egui_extras::install_image_loaders(&ctx);
        let started = Instant::now();
        let output = frame(&mut manager, &ctx, vec![]);
        let text_shapes = output
            .shapes
            .iter()
            .filter(|shape| matches!(shape.shape, egui::Shape::Text(_)))
            .count();
        assert!(
            text_shapes < 200,
            "visible-row rendering must not lay out all 10,000 entries"
        );
        assert!(!output.shapes.iter().any(|shape| matches!(&shape.shape, egui::Shape::Text(text) if text.galley.job.text == format!("file-09999.{extension}"))));
        println!(
            "10,000 files: load {loaded_ms} ms, first UI frame {} ms, {text_shapes} text shapes",
            started.elapsed().as_millis()
        );
        manager.select(temp.path().join(format!("file-00150.{extension}")));
        manager.location_mut().unwrap().scroll = 5000.0;
        manager.restore_scroll = true;
        frame(&mut manager, &ctx, vec![]);
        let previous = manager.location().unwrap().clone();
        assert!(previous.scroll > 4000.0);
        manager.navigate(child.clone(), Navigation::New);
        finish(&mut manager, &jobs);
        frame(&mut manager, &ctx, vec![]);
        manager.history_step(false);
        finish(&mut manager, &jobs);
        frame(&mut manager, &ctx, vec![]);
        let restored = manager.location().unwrap();
        assert_eq!(restored.selected, previous.selected);
        assert!(
            (restored.scroll - previous.scroll).abs() < 1.0,
            "rendered viewport should restore its prior offset"
        );
    }
}

#[test]
fn stalled_directory_worker_leaves_address_navigation_responsive() {
    let temp = tempfile::tempdir().unwrap();
    let destination = temp.path().join("destination");
    std::fs::create_dir(&destination).unwrap();
    let (mut manager, jobs) = loaded(temp.path());
    let (job, release_tx) = paused_directory(temp.path(), &jobs);
    manager.pending = Some(Request {
        browser_target: None,
        path: temp.path().join("stalled"),
        navigation: Navigation::New,
        job: Some(job),
    });
    let ctx = egui::Context::default();
    egui_extras::install_image_loaders(&ctx);
    frame(&mut manager, &ctx, vec![]);
    press(&mut manager, &ctx, egui::Key::L, egui::Modifiers::CTRL);
    frame(
        &mut manager,
        &ctx,
        vec![egui::Event::Text(display_path(&destination))],
    );
    press(&mut manager, &ctx, egui::Key::Enter, egui::Modifiers::NONE);
    release_tx.send(()).unwrap();
    finish(&mut manager, &jobs);
    assert_eq!(manager.path(), destination);
    assert_eq!(manager.history.len(), 2);
}

#[test]
fn tree_keyboard_expansion_selection_and_navigation_are_separate() {
    let (temp, child) = folder_fixture();
    let (mut manager, jobs, ctx) = rendered(temp.path());
    ctx.memory_mut(|memory| memory.request_focus(egui::Id::new("file_manager_tree_focus")));
    frame(&mut manager, &ctx, vec![]);
    press(
        &mut manager,
        &ctx,
        egui::Key::ArrowRight,
        egui::Modifiers::NONE,
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    while manager
        .tree
        .branches
        .get(temp.path())
        .unwrap()
        .directory
        .is_none()
    {
        assert!(Instant::now() < deadline);
        manager.tick(&jobs);
        std::thread::sleep(Duration::from_millis(2));
    }
    press(
        &mut manager,
        &ctx,
        egui::Key::ArrowDown,
        egui::Modifiers::NONE,
    );
    assert_eq!(manager.tree.selected.as_ref(), Some(&child));
    assert_eq!(manager.path(), temp.path(), "selection must not navigate");
    press(
        &mut manager,
        &ctx,
        egui::Key::ArrowUp,
        egui::Modifiers::NONE,
    );
    press(
        &mut manager,
        &ctx,
        egui::Key::ArrowLeft,
        egui::Modifiers::NONE,
    );
    assert!(!manager.tree.branches.get(temp.path()).unwrap().expanded);
    press(
        &mut manager,
        &ctx,
        egui::Key::ArrowRight,
        egui::Modifiers::NONE,
    );
    press(
        &mut manager,
        &ctx,
        egui::Key::ArrowDown,
        egui::Modifiers::NONE,
    );
    press(&mut manager, &ctx, egui::Key::Enter, egui::Modifiers::NONE);
    finish(&mut manager, &jobs);
    assert_eq!(manager.path(), child);
}

#[test]
fn background_completion_preserves_an_in_progress_address_edit() {
    let temp = tempfile::tempdir().unwrap();
    let (mut manager, jobs, ctx) = rendered(temp.path());
    let (job, release_tx) = paused_directory(temp.path(), &jobs);
    manager.pending = Some(Request {
        browser_target: None,
        path: temp.path().into(),
        navigation: Navigation::Refresh,
        job: Some(job),
    });
    press(&mut manager, &ctx, egui::Key::L, egui::Modifiers::CTRL);
    frame(
        &mut manager,
        &ctx,
        vec![egui::Event::Text("an unfinished path".into())],
    );
    release_tx.send(()).unwrap();
    finish(&mut manager, &jobs);
    assert_eq!(manager.address, "an unfinished path");
    press(&mut manager, &ctx, egui::Key::Escape, egui::Modifiers::NONE);
    assert_eq!(manager.address, display_path(temp.path()));
}

#[test]
fn known_ancestry_is_visible_before_loading_and_through_unreadable_parents() {
    let path = Path::new("/restricted/.hidden/child");
    let mut tree = FolderTree::new(PathBuf::from("/home/test"));
    tree.reveal(path);
    tree.branches
        .get_mut(Path::new("/restricted"))
        .unwrap()
        .error = Some("Permission denied".into());
    let rows = tree.rows(false);
    for ancestor in path.ancestors() {
        assert!(
            rows.iter().any(|row| row.path == ancestor),
            "missing {}",
            ancestor.display()
        );
    }
}

#[test]
fn multiselection_ranges_toggles_select_all_and_refresh_preserve_valid_members() {
    let root = populated_folder(&["a", "b", "c", "d"]);
    let (mut browser, jobs) = loaded(root.path());
    browser.select_with(root.path().join("a"), false, false);
    browser.select_with(root.path().join("c"), false, true);
    assert_eq!(browser.selection_count(), 3);
    browser.select_with(root.path().join("b"), true, false);
    assert_eq!(
        browser.selected_paths(),
        [root.path().join("a"), root.path().join("c")]
    );
    browser.select_all();
    assert_eq!(browser.selection_count(), 4);
    std::fs::remove_file(root.path().join("b")).unwrap();
    browser.refresh();
    finish(&mut browser, &jobs);
    assert_eq!(browser.selection_count(), 3);
    browser.clear_selection();
    assert_eq!(browser.selection_count(), 0);
}

#[test]
fn quiet_external_refresh_preserves_unrelated_selection_and_address_draft() {
    let root = populated_folder(&["selected"]);
    let (mut browser, jobs) = loaded(root.path());
    browser.select(root.path().join("selected"));
    browser.address_editing = true;
    browser.address = "unfinished address".into();
    std::fs::write(root.path().join("new"), "new").unwrap();
    browser.watch_started = Instant::now() - Duration::from_secs(4);
    let deadline = Instant::now() + Duration::from_secs(5);
    while browser.directory.as_ref().unwrap().entries.len() != 2 {
        browser.tick(&jobs);
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(browser.selected_paths(), [root.path().join("selected")]);
    assert_eq!(browser.address, "unfinished address");
}

fn populated_folder(names: &[&str]) -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    for name in names {
        std::fs::write(root.path().join(name), name).unwrap();
    }
    root
}

#[test]
fn native_clipboard_key_event_copies_selected_references() {
    let root = populated_folder(&["alpha", "beta"]);
    let (mut browser, _jobs, ctx) = rendered(root.path());
    browser.select_all();
    ctx.memory_mut(|m| m.request_focus(super::ui::list_id()));
    frame(
        &mut browser,
        &ctx,
        vec![egui::Event::Key {
            key: egui::Key::C,
            physical_key: Some(egui::Key::Escape),
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers {
                ctrl: true,
                command: true,
                ..Default::default()
            },
        }],
    );
    assert_eq!(browser.transfers.clipboard.as_ref().unwrap().paths.len(), 2);
}

fn folder_fixture() -> (tempfile::TempDir, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let child = temp.path().join("child");
    std::fs::create_dir(&child).unwrap();
    (temp, child)
}

#[test]
fn cancelling_a_confirmation_returns_keyboard_focus_to_file_list() {
    let root = populated_folder(&["file"]);
    let (mut browser, _jobs, ctx) = rendered(root.path());
    browser.select_all();
    browser.confirm_delete(false);
    frame(&mut browser, &ctx, vec![]);
    assert!(browser.transfers_modal());
    press(&mut browser, &ctx, egui::Key::Escape, egui::Modifiers::NONE);
    frame(&mut browser, &ctx, vec![]);
    assert!(!browser.transfers_modal());
    assert!(ctx.memory(|memory| memory.has_focus(super::ui::list_id())));
    press(
        &mut browser,
        &ctx,
        egui::Key::Delete,
        egui::Modifiers::SHIFT,
    );
    assert!(browser.transfers_modal());
    assert!(root.path().join("file").exists());
}

#[test]
fn escape_clears_rendered_selection_without_losing_list_focus() {
    let root = populated_folder(&["one", "two"]);
    let (mut browser, _jobs, ctx) = rendered(root.path());
    browser.select_all();
    ctx.memory_mut(|memory| memory.request_focus(super::ui::list_id()));
    frame(&mut browser, &ctx, vec![]);
    press(&mut browser, &ctx, egui::Key::Escape, egui::Modifiers::NONE);
    assert_eq!(browser.selection_count(), 0);
    assert!(ctx.memory(|memory| memory.has_focus(super::ui::list_id())));
}
