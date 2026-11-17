use super::*;
use std::{ffi::OsString, os::unix::ffi::OsStringExt};

fn policy_fixture() -> (TestDrives, Drive, Store) {
    let fixture = TestDrives::new(0);
    let state = fixture.temp.path().join("state");
    fs::create_dir(&state).unwrap();
    let db = Store::open(&state.join("drives.sqlite")).unwrap();
    let drive = fixture.drives.lock().unwrap()[0].clone();
    (fixture, drive, db)
}

#[test]
fn a_saved_rule_is_published_before_its_completion_is_reported() {
    let (fixture, drive, _) = policy_fixture();
    let source = drive.mount.clone().unwrap();
    let selected = drive.id.clone();
    let service = fixture.start_authority(None, fixture.image_authority());
    finish(
        &service,
        DriveRequest::Rule {
            drive: selected,
            relative: "private".into(),
            catalog: true,
            index: false,
        },
    );
    let policy = published_policy(&service);
    assert!(policy.excluded.contains(&source.join("private")));
    service.shutdown().unwrap();
}

#[test]
fn published_policy_includes_rules_beyond_one_database_page() {
    let (fixture, drive, db) = policy_fixture();
    let state = fixture.temp.path().join("state");
    let source = drive.mount.as_ref().unwrap();
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
    let service = fixture.start_authority(None, fixture.image_authority());
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
fn exact_rule_reads_preserve_raw_addresses_and_do_not_create_operations() {
    let (fixture, drive, db) = policy_fixture();
    let selected = drive.id.clone();
    let original = [
        DriveRule {
            id: rule_id(&selected, Path::new("")),
            drive: selected.clone(),
            relative: PathBuf::new(),
            catalog: false,
            index: true,
        },
        DriveRule {
            id: rule_id(
                &selected,
                &PathBuf::from(OsString::from_vec(b"private_\xff".to_vec())),
            ),
            drive: selected.clone(),
            relative: OsString::from_vec(b"private_\xff".to_vec()).into(),
            catalog: true,
            index: false,
        },
    ];
    let transaction = db.0.unchecked_transaction().unwrap();
    for rule in &original {
        db.put("rule", &rule.id, rule).unwrap();
    }
    for index in 0..1025 {
        let rule = DriveRule {
            id: format!("other-{index}"),
            drive: "other-drive".into(),
            relative: PathBuf::new(),
            catalog: true,
            index: true,
        };
        db.put("rule", &rule.id, &rule).unwrap();
    }
    transaction.commit().unwrap();
    let service = fixture.start_authority(None, fixture.image_authority());
    assert!(
        status(&service)
            .rules
            .iter()
            .all(|rule| rule.drive != selected)
    );
    for expected in &original {
        let reply = service
            .request(DriveRequest::GetRule {
                drive: selected.clone(),
                relative: expected.relative.clone(),
            })
            .unwrap();
        let DriveReply::Rule(Some(actual)) = reply else {
            panic!("missing exact rule");
        };
        assert_eq!(actual.relative, expected.relative);
        assert_eq!(
            (actual.catalog, actual.index),
            (expected.catalog, expected.index)
        );
    }
    assert!(matches!(
        service
            .request(DriveRequest::GetRule {
                drive: selected.clone(),
                relative: "no-override".into()
            })
            .unwrap(),
        DriveReply::Rule(None)
    ));
    assert!(
        service
            .request(DriveRequest::GetRule {
                drive: selected.clone(),
                relative: "../escape".into()
            })
            .is_err()
    );
    let mut invalid = original[0].clone();
    invalid.relative = "wrong-address".into();
    db.put("rule", &invalid.id, &invalid).unwrap();
    assert!(
        service
            .request(DriveRequest::GetRule {
                drive: selected,
                relative: PathBuf::new()
            })
            .is_err()
    );
    assert!(status(&service).operations.is_empty());
    service.shutdown().unwrap();
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
