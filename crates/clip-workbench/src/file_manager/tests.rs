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
    let temp = tempfile::tempdir().unwrap();
    let child = temp.path().join("child");
    std::fs::create_dir(&child).unwrap();
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
    let temp = tempfile::tempdir().unwrap();
    let child = temp.path().join("child");
    std::fs::create_dir(&child).unwrap();
    let (mut manager, jobs, ctx) = rendered(temp.path());
    ctx.memory_mut(|memory| memory.request_focus(egui::Id::new("file_manager_list_focus")));
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
    assert_eq!(manager.path(), child);
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
    let temp = tempfile::tempdir().unwrap();
    let child = temp.path().join("child");
    std::fs::create_dir(&child).unwrap();
    for index in 0..10_000 {
        std::fs::write(temp.path().join(format!("file-{index:05}.txt")), "data").unwrap();
    }
    let started = Instant::now();
    let (mut manager, jobs) = loaded(temp.path());
    let loaded_ms = started.elapsed().as_millis();
    assert_eq!(manager.visible.len(), 10_001);
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
    assert!(!output.shapes.iter().any(|shape| matches!(&shape.shape, egui::Shape::Text(text) if text.galley.job.text == "file-09999.txt")));
    println!(
        "10,000 files: load {loaded_ms} ms, first UI frame {} ms, {text_shapes} text shapes",
        started.elapsed().as_millis()
    );
    manager.select(temp.path().join("file-00150.txt"));
    manager.location_mut().unwrap().scroll = 5000.0;
    manager.restore_scroll = true;
    frame(&mut manager, &ctx, vec![]);
    let previous = manager.location().unwrap().clone();
    assert!(previous.scroll > 4000.0);
    manager.navigate(child, Navigation::New);
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

#[test]
fn stalled_directory_worker_leaves_address_navigation_responsive() {
    let temp = tempfile::tempdir().unwrap();
    let destination = temp.path().join("destination");
    std::fs::create_dir(&destination).unwrap();
    let (mut manager, jobs) = loaded(temp.path());
    let (job, release_tx) = paused_directory(temp.path(), &jobs);
    manager.pending = Some(Request {
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
    let temp = tempfile::tempdir().unwrap();
    let child = temp.path().join("child");
    std::fs::create_dir(&child).unwrap();
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
