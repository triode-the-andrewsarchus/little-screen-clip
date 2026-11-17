use super::*;
use std::{ffi::OsString, os::unix::ffi::OsStringExt};

#[test]
fn published_policy_includes_rules_beyond_one_database_page() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("portable");
    let state = temp.path().join("state");
    fs::create_dir(&source).unwrap();
    fs::create_dir(&state).unwrap();
    let drive = fixture_drive(&source, "policy-pages");
    let db = Store::open(&state.join("drives.sqlite")).unwrap();
    let mut expected = IndexPolicy::default();
    let transaction = db.0.unchecked_transaction().unwrap();
    for index in 0..1026 {
        let relative = if index == 0 {
            PathBuf::from(OsString::from_vec(b"private_\xff".to_vec()))
        } else {
            PathBuf::from(format!("private-{index}"))
        };
        expected.excluded.push(source.join(&relative));
        let rule = DriveRule {
            id: format!("rule-{index}"),
            drive: drive.id.clone(),
            relative,
            catalog: true,
            index: false,
        };
        db.put("rule", &rule.id, &rule).unwrap();
    }
    transaction.commit().unwrap();
    fs::write(
        state.join("expected-policy.json"),
        serde_json::to_vec(&expected).unwrap(),
    )
    .unwrap();
    let service = Service::start_with_inventory(
        DriveConfig::default(),
        state.clone(),
        Arc::new(move || Ok(vec![drive.clone()])),
    )
    .unwrap();
    service.shutdown().unwrap();
    let mut location = OsString::from("LITTLE_CLIP_DRIVE_STATE_DIR=");
    location.push(&state);
    let result = CommandRequest::new("env")
        .arg(location)
        .arg("LITTLE_CLIP_DRIVE_POLICY_PROBE=1")
        .arg(std::env::current_exe().unwrap())
        .args([
            "service::tests::policy_coverage::index_policy_client_probe",
            "--exact",
            "--ignored",
            "--nocapture",
        ])
        .timeout(Duration::from_secs(15))
        .run()
        .unwrap();
    assert!(
        result.status.success(),
        "{}{}",
        result.stdout_lossy(),
        result.stderr_lossy()
    );
    assert_eq!(
        result
            .stdout_lossy()
            .matches("POLICY_CLIENT_VERIFIED")
            .count(),
        1
    );
}

#[test]
#[ignore = "owned subprocess probe; requires its explicitly selected policy fixture"]
fn index_policy_client_probe() {
    assert_eq!(
        std::env::var("LITTLE_CLIP_DRIVE_POLICY_PROBE").unwrap(),
        "1"
    );
    let state = little_clip_core::drives::state_directory();
    let expected: IndexPolicy =
        serde_json::from_slice(&fs::read(state.join("expected-policy.json")).unwrap()).unwrap();
    let allowed = state.parent().unwrap().join("ordinary-data/file.txt");
    let policy = little_clip_core::drives::index_policy().unwrap();
    for path in &expected.excluded {
        assert!(
            policy.excluded.contains(path),
            "Missing exclusion: {path:?}"
        );
        assert!(little_clip_core::drives::index_excluded(
            &path.join("child.txt")
        ));
    }
    assert!(!little_clip_core::drives::index_excluded(&allowed));
    let registry = state.join("index-policy.json");
    let original = fs::read(&registry).unwrap();
    for invalid in [b"malformed".to_vec(), vec![b' '; 1024 * 1024 + 1]] {
        fs::write(&registry, invalid).unwrap();
        assert!(little_clip_core::drives::index_policy().is_none());
        assert!(little_clip_core::drives::index_excluded(&allowed));
    }
    fs::write(&registry, original).unwrap();
    assert!(!little_clip_core::drives::index_excluded(&allowed));
    println!("POLICY_CLIENT_VERIFIED");
}
