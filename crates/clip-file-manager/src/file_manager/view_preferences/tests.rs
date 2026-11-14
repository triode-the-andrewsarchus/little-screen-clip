use super::*;
use std::{os::unix::ffi::OsStringExt, sync::mpsc, time::Instant};

fn choice(view: BrowserView, descendants: bool) -> Preference {
    Preference { view, descendants }
}
fn change(path: &str, view: BrowserView, descendants: bool) -> Change {
    Change {
        path: path.into(),
        preference: Some(choice(view, descendants)),
        apply_epoch: Some(0),
    }
}
fn settle(preferences: &mut Preferences, jobs: &BackgroundJobs) {
    let start = Instant::now();
    while preferences.active() {
        preferences.tick(jobs);
        assert!(
            start.elapsed() < Duration::from_secs(3),
            "preference task did not settle"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(preferences.error.is_none(), "{:?}", preferences.error);
}
fn browser_in(folder: &Path) -> super::super::FileManager {
    let mut browser = super::super::FileManager::default();
    let directory = super::super::filesystem::read_directory(
        folder,
        &std::sync::atomic::AtomicBool::new(false),
    )
    .unwrap();
    browser.commit(directory, super::super::Navigation::New);
    browser
}
fn settle_browser(browser: &mut super::super::FileManager, jobs: &BackgroundJobs) {
    let start = Instant::now();
    while browser.view_preferences.active() {
        browser.tick_view_preferences(jobs);
        assert!(start.elapsed() < Duration::from_secs(3));
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(browser.view_preferences.error.is_none());
}
#[test]
fn exact_overrides_and_nearest_explicit_ancestor_inherit_by_path_components() {
    let mut state = State::default();
    state.update(
        Path::new("/photos"),
        Some(choice(BrowserView::Photos, true)),
    );
    state.update(
        Path::new("/photos/work"),
        Some(choice(BrowserView::List, false)),
    );
    assert_eq!(
        state.effective(Path::new("/photos/work")).unwrap().1.view,
        BrowserView::List
    );
    assert_eq!(
        state
            .effective(Path::new("/photos/work/raw"))
            .unwrap()
            .1
            .view,
        BrowserView::Photos
    );
    assert!(state.effective(Path::new("/photos-backup")).is_none());
    state.update(
        Path::new("/photos/work"),
        Some(choice(BrowserView::Columns, true)),
    );
    assert_eq!(
        state
            .effective(Path::new("/photos/work/raw"))
            .unwrap()
            .1
            .view,
        BrowserView::Columns
    );
    state.update(Path::new("/photos/work"), None);
    assert_eq!(
        state.effective(Path::new("/photos/work")).unwrap().1.view,
        BrowserView::Photos
    );
}
#[test]
fn loading_missing_state_creates_nothing_and_saves_merge_unrelated_folders() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("state/folder-views.json");
    assert!(store::read(&path).unwrap().0.folders.is_empty());
    assert!(!path.parent().unwrap().exists());
    store::save(&path, &change("/photos", BrowserView::Photos, true)).unwrap();
    store::save(&path, &change("/music", BrowserView::Columns, false)).unwrap();
    let (state, _) = store::read(&path).unwrap();
    assert_eq!(state.folders.len(), 2);
    assert_eq!(
        state.exact(Path::new("/photos")),
        Some(choice(BrowserView::Photos, true))
    );
    assert_eq!(
        state.exact(Path::new("/music")),
        Some(choice(BrowserView::Columns, false))
    );
    store::save(
        &path,
        &Change {
            path: "/photos".into(),
            preference: None,
            apply_epoch: Some(0),
        },
    )
    .unwrap();
    assert_eq!(store::read(&path).unwrap().0.folders.len(), 1);
}
#[test]
fn state_round_trip_preserves_non_utf8_folder_names() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("views.json");
    let folder = PathBuf::from(std::ffi::OsString::from_vec(b"/photos/\xff".to_vec()));
    store::save(
        &path,
        &Change {
            path: folder.clone(),
            preference: Some(choice(BrowserView::Photos, true)),
            apply_epoch: Some(0),
        },
    )
    .unwrap();
    assert_eq!(store::read(&path).unwrap().0.folders[0].path, folder);
}
#[test]
fn future_malformed_and_symlinked_preferences_are_never_overwritten() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("views.json");
    for bytes in [br#"{"version":99,"folders":[]}"#.as_slice(), b"not json"] {
        std::fs::write(&path, bytes).unwrap();
        assert!(store::save(&path, &change("/photos", BrowserView::Photos, true)).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }
    std::fs::remove_file(&path).unwrap();
    let other = root.path().join("other.json");
    std::fs::write(&other, b"do not replace").unwrap();
    std::os::unix::fs::symlink(&other, &path).unwrap();
    assert!(store::save(&path, &change("/photos", BrowserView::Photos, true)).is_err());
    assert_eq!(std::fs::read(&other).unwrap(), b"do not replace");
    assert!(store::read(Path::new("/proc/version")).is_err());
}
#[test]
fn cooperating_writer_lock_is_nonblocking_and_does_not_lose_existing_settings() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("views.json");
    store::save(&path, &change("/photos", BrowserView::Photos, true)).unwrap();
    let original = std::fs::read(&path).unwrap();
    let directory = little_clip_core::kernel_interface::pin_content_directory(root.path()).unwrap();
    let held = store::lock(&directory).unwrap();
    assert!(store::save(&path, &change("/music", BrowserView::Columns, false)).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), original);
    drop(held);
    store::save(&path, &change("/music", BrowserView::Columns, false)).unwrap();
    assert_eq!(store::read(&path).unwrap().0.folders.len(), 2);
}
#[test]
fn accepted_preference_write_survives_queue_saturation_until_publication() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("views.json");
    let jobs = BackgroundJobs::new(1, 1);
    let mut preferences = Preferences::new(path.clone());
    settle(&mut preferences, &jobs);
    let (started, began) = mpsc::channel();
    let (release, wait) = mpsc::channel();
    let blocker = Task::start_unbounded(&jobs, move |_| {
        started.send(()).unwrap();
        wait.recv().unwrap();
    });
    began.recv_timeout(Duration::from_secs(3)).unwrap();
    let queued = Task::start_unbounded(&jobs, |_| ());
    preferences.change(
        "/photos".into(),
        Some(choice(BrowserView::Photos, true)),
        Some(1),
    );
    preferences.tick(&jobs);
    preferences.tick(&jobs);
    assert!(preferences.writing());
    assert!(!path.exists());
    release.send(()).unwrap();
    settle(&mut preferences, &jobs);
    assert!(path.exists());
    assert_eq!(
        preferences.effective(Path::new("/photos/child")),
        Some(BrowserView::Photos)
    );
    drop((blocker, queued));
}
#[test]
fn late_preference_acknowledgement_keeps_a_newer_manual_view_choice() {
    let root = tempfile::tempdir().unwrap();
    let folder = root.path().join("folder");
    std::fs::create_dir(&folder).unwrap();
    let jobs = BackgroundJobs::new(1, 4);
    let mut browser = browser_in(&folder);
    browser.view_preferences = Preferences::new(root.path().join("views.json"));
    settle(&mut browser.view_preferences, &jobs);
    let epoch = browser.view_epoch;
    browser.view_preferences.change(
        folder.clone(),
        Some(choice(BrowserView::Photos, false)),
        Some(epoch),
    );
    browser.set_view(BrowserView::Columns);
    settle_browser(&mut browser, &jobs);
    assert_eq!(browser.view, BrowserView::Columns);
    assert_eq!(
        browser.location().unwrap().manual_view,
        Some(BrowserView::Columns)
    );
    assert_eq!(
        browser.view_preferences.effective(&folder),
        Some(BrowserView::Photos)
    );
}

#[test]
fn changing_saved_inheritance_does_not_replace_the_current_temporary_view() {
    let root = tempfile::tempdir().unwrap();
    let folder = root.path().join("folder");
    std::fs::create_dir(&folder).unwrap();
    let state = root.path().join("views.json");
    store::save(
        &state,
        &Change {
            path: folder.clone(),
            preference: Some(choice(BrowserView::Photos, false)),
            apply_epoch: None,
        },
    )
    .unwrap();
    let jobs = BackgroundJobs::new(1, 4);
    let mut browser = browser_in(&folder);
    browser.view_preferences = Preferences::new(state);
    settle(&mut browser.view_preferences, &jobs);
    browser.set_view(BrowserView::Columns);
    browser.view_preferences.change(
        folder.clone(),
        Some(choice(BrowserView::Photos, true)),
        None,
    );
    settle_browser(&mut browser, &jobs);
    assert_eq!(browser.view, BrowserView::Columns);
    assert_eq!(
        browser.location().unwrap().manual_view,
        Some(BrowserView::Columns)
    );
    assert_eq!(
        browser.view_preferences.effective(&folder.join("child")),
        Some(BrowserView::Photos)
    );
}

#[test]
fn retired_media_view_preferences_load_as_list_without_losing_other_folders() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("views.json");
    let mut state = State::default();
    state.update(Path::new("/audio"), Some(choice(BrowserView::List, true)));
    state.update(Path::new("/movies"), Some(choice(BrowserView::List, false)));
    state.update(
        Path::new("/photos"),
        Some(choice(BrowserView::Photos, true)),
    );
    let mut json = serde_json::to_value(&state).unwrap();
    json["folders"][0]["preference"]["view"] = "music".into();
    json["folders"][1]["preference"]["view"] = "videos".into();
    std::fs::write(&path, serde_json::to_vec(&json).unwrap()).unwrap();
    let (loaded, _) = store::read(&path).unwrap();
    assert_eq!(
        loaded.effective(Path::new("/audio/child")).unwrap().1.view,
        BrowserView::List
    );
    assert_eq!(
        loaded.exact(Path::new("/movies")).unwrap().view,
        BrowserView::List
    );
    assert_eq!(
        loaded.exact(Path::new("/photos")).unwrap().view,
        BrowserView::Photos
    );
    let normalized = serde_json::to_value(&loaded).unwrap();
    assert_eq!(normalized["folders"][0]["preference"]["view"], "list");
    assert_eq!(normalized["folders"][1]["preference"]["view"], "list");
}
