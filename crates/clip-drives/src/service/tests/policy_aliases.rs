//! Real mount aliases are exercised only inside the disposable filesystem guest.
use super::*;
use crate::sync_edge_tests::native_fs::GuestMount;
use little_clip_core::{
    drives::{IndexPolicy, index_excluded, index_policy},
    mount_inventory::Watcher,
};

fn source() -> (PathBuf, tempfile::TempDir) {
    crate::require_disposable_guest();
    let media = Path::new("/work/media");
    let source = media.join(format!("policy-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(source.join("private")).unwrap();
    fs::write(source.join("private/secret"), b"must not decode").unwrap();
    fs::write(source.join("public"), b"ordinary fixture").unwrap();
    let target = tempfile::tempdir_in("/work").unwrap();
    (source, target)
}

fn save_policy(excluded: Vec<PathBuf>) {
    crate::require_disposable_guest();
    let state = little_clip_core::drives::state_directory();
    assert!(state.starts_with("/work"));
    fs::create_dir_all(&state).unwrap();
    let staged = state.join("owned-policy.new");
    let previous = fs::read(state.join("index-policy.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<IndexPolicy>(&bytes).ok());
    fs::write(
        &staged,
        serde_json::to_vec(&IndexPolicy::observed(excluded, previous.as_ref()).unwrap()).unwrap(),
    )
    .unwrap();
    fs::rename(staged, state.join("index-policy.json")).unwrap();
}

#[test]
#[ignore = "requires the disposable native-filesystem guest and real bind mounts"]
fn native_bind_aliases_and_mount_changes_do_not_bypass_no_index_policy() {
    crate::require_disposable_guest();
    let media = Path::new("/work/media");
    let (source, target) = source();
    let private = source.join("private");
    let relative = source.strip_prefix(media).unwrap();
    save_policy(vec![private.clone()]);
    assert!(index_excluded(&source.join("private/secret")));
    assert!(!index_excluded(&source.join("public")));
    let mut watcher = Watcher::new().unwrap();
    let initial = watcher.observe().unwrap();
    assert!(Arc::ptr_eq(&initial, &watcher.observe().unwrap()));
    {
        let parent = target.path().join("parent");
        fs::create_dir(&parent).unwrap();
        let whole = parent.join("whole");
        fs::create_dir(&whole).unwrap();
        let bind = GuestMount::attach(media, &whole);
        assert_ne!(initial.signature, watcher.observe().unwrap().signature);
        assert!(
            index_policy().is_none(),
            "A new mount cannot use the older publication"
        );
        assert!(
            index_excluded(&source.join("public")),
            "Unknown mount observation must fail closed"
        );
        save_policy(vec![private.clone()]);
        assert!(index_excluded(&whole.join(relative).join("private/secret")));
        assert!(!index_excluded(&whole.join(relative).join("public")));
        let nested = target.path().join("subtree");
        fs::create_dir(&nested).unwrap();
        let nested_bind = GuestMount::attach(&private, &nested);
        assert!(index_policy().is_none());
        save_policy(vec![private.clone()]);
        assert!(index_excluded(&nested.join("secret")));
        let leaf = target.path().join("file-alias");
        fs::write(&leaf, b"ordinary mount target").unwrap();
        let leaf_bind = GuestMount::attach(&private.join("secret"), &leaf);
        save_policy(vec![private.clone()]);
        assert!(index_excluded(&leaf));
        let link = target.path().join("link");
        std::os::unix::fs::symlink(nested.join("secret"), &link).unwrap();
        assert!(index_excluded(&link));
        // A mounted directory itself is EBUSY for rename. Moving its ordinary
        // parent changes the observed target without requiring a mount event.
        let moved = target.path().join("moved");
        fs::rename(&parent, &moved).unwrap();
        let moved_whole = moved.join("whole");
        assert!(index_excluded(
            &moved_whole.join(relative).join("private/secret")
        ));
        assert!(index_excluded(&moved_whole.join(relative).join("public")));
        save_policy(vec![private.clone()]);
        assert!(index_excluded(
            &moved_whole.join(relative).join("private/secret")
        ));
        assert!(!index_excluded(&moved_whole.join(relative).join("public")));
        // Restore the observed target before the identity-checked fixture drops.
        fs::rename(&moved, &parent).unwrap();
        drop(leaf_bind);
        drop(nested_bind);
        drop(bind);
    }
    assert!(index_policy().is_none());
    save_policy(vec![private]);
    assert!(!index_excluded(&source.join("public")));
    fs::remove_dir_all(source).unwrap();
    fs::remove_file(little_clip_core::drives::state_directory().join("index-policy.json")).unwrap();
}

#[test]
#[ignore = "requires the disposable native-filesystem guest; origin-removal acceptance gate"]
fn native_policy_republication_after_origin_unmount_keeps_alias_excluded() {
    crate::require_disposable_guest();
    let media = Path::new("/work/media");
    let (source, target) = source();
    let whole = target.path().join("whole");
    fs::create_dir(&whole).unwrap();
    let bind = GuestMount::attach(&source.join("private"), &whole);
    let private = source.join("private");
    let alias = whole.join("secret");
    save_policy(vec![private.clone()]);
    assert!(index_excluded(&alias));
    let unmounted = CommandRequest::new("/usr/bin/umount")
        .arg(media)
        .run()
        .unwrap();
    assert!(unmounted.status.success(), "{}", unmounted.stderr_lossy());
    assert!(index_policy().is_none());
    let device = PathBuf::from(std::env::var_os("LITTLE_CLIP_DRIVE_TEST_DEVICE").unwrap());
    // Bare loop volumes intentionally stay out of removable-drive discovery.
    // Exercise its shared mount-routing function against this real owned device.
    let fixture = TestDrives::new(0);
    let mut logical = fixture.drives.lock().unwrap()[0].clone();
    logical.device = device.clone();
    logical.mount = Some(whole.clone());
    logical.filesystem = std::env::var("LITTLE_CLIP_DRIVE_TEST_FILESYSTEM").unwrap();
    use std::os::unix::fs::MetadataExt;
    let mounts = crate::mounts::inventory().unwrap();
    let observed_mount =
        crate::discovery::select_mount(&logical, &mounts, fs::metadata(&device).unwrap().rdev())
            .map(|mount| mount.target.clone());
    save_policy(vec![private]);
    let retained = index_excluded(&alias);
    // A newly configured rule also applies through this partial view, without
    // inheriting a previous observation or claiming a whole-volume mount.
    logical.mount = None;
    let mut rule = DriveRule {
        id: "new-partial-policy".into(),
        drive: logical.id.clone(),
        relative: source.join("private").strip_prefix(media).unwrap().into(),
        catalog: true,
        index: false,
    };
    let sources = rules::IndexSources::observed(&[logical], &mounts);
    let routed = IndexPolicy::observed_rules(sources.requests(&rule), None).unwrap();
    let state = little_clip_core::drives::state_directory();
    fs::write(
        state.join("fresh-policy.new"),
        serde_json::to_vec(&routed).unwrap(),
    )
    .unwrap();
    fs::rename(
        state.join("fresh-policy.new"),
        state.join("index-policy.json"),
    )
    .unwrap();
    let fresh = index_excluded(&alias);
    rule.relative = source.join("PRIVATE").strip_prefix(media).unwrap().into();
    let uncertain = IndexPolicy::observed_rules(sources.requests(&rule), None).unwrap();
    fs::write(
        state.join("uncertain-policy.new"),
        serde_json::to_vec(&uncertain).unwrap(),
    )
    .unwrap();
    fs::rename(
        state.join("uncertain-policy.new"),
        state.join("index-policy.json"),
    )
    .unwrap();
    let uncertain_denied = index_excluded(&alias);

    // Always restore the harness-owned media before asserting the outcome.
    let mounted = CommandRequest::new("/usr/bin/mount")
        .arg("--internal-only")
        .arg("-t")
        .arg(std::env::var("LITTLE_CLIP_DRIVE_TEST_FILESYSTEM").unwrap())
        .arg(std::env::var_os("LITTLE_CLIP_DRIVE_TEST_DEVICE").unwrap())
        .arg(media)
        .run()
        .unwrap();
    assert!(mounted.status.success(), "{}", mounted.stderr_lossy());
    drop(bind);
    fs::remove_dir_all(source).unwrap();
    fs::remove_file(little_clip_core::drives::state_directory().join("index-policy.json")).unwrap();
    assert!(
        observed_mount.is_none(),
        "A subtree-only bind is not a whole-partition mount: {observed_mount:?}"
    );
    assert!(
        fresh,
        "A first-time partition-relative rule must cover its visible subtree alias"
    );
    assert!(
        uncertain_denied,
        "A partial view cannot establish disjointness by lexical case alone"
    );
    assert!(
        retained,
        "Republishing a missing origin must not drop its still-mounted alias exclusion"
    );
}

#[test]
#[ignore = "requires the disposable native-filesystem guest; real name-equivalence gate"]
fn native_future_case_aliases_follow_kernel_name_equivalence() {
    let (source, _target) = source();
    let mut outcomes = Vec::new();
    for (rule, spelling) in [
        ("Future", "future"),
        ("Résumé", "résumé"),
        ("Ångström", "ångström"),
        ("Future/Inner", "future/inner"),
    ] {
        let excluded = source.join("private").join(rule);
        save_policy(vec![excluded.clone()]);
        let actual = source.join("private").join(spelling);
        fs::create_dir_all(&actual).unwrap();
        fs::write(actual.join("secret"), b"owned private future bytes").unwrap();
        let equivalent = excluded.exists();
        let denied = index_excluded(&actual.join("secret"));
        let public_allowed = !index_excluded(&source.join("public"));
        eprintln!("Native kernel resolves {rule:?} / {spelling:?}: {equivalent}");
        outcomes.push((rule, equivalent, denied, public_allowed));
        fs::remove_dir_all(
            source
                .join("private")
                .join(spelling.split('/').next().unwrap()),
        )
        .unwrap();
    }
    fs::remove_dir_all(source).unwrap();
    fs::remove_file(little_clip_core::drives::state_directory().join("index-policy.json")).unwrap();
    for (rule, equivalent, denied, public_allowed) in outcomes {
        assert_eq!(
            denied, equivalent,
            "Future-folder policy must use the filesystem's actual name equivalence: {rule}"
        );
        assert!(public_allowed, "Unrelated file excluded by {rule}");
    }
}
