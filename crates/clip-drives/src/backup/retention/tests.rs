use super::*;
use crate::backup::tests::FileFixture;
use std::{
    os::unix::{ffi::OsStringExt, fs::PermissionsExt},
    sync::atomic::Ordering,
};

fn versions() -> (FileFixture, Backup, Backup) {
    let fixture = FileFixture::new(b"old");
    fs::create_dir(fixture.source.path.join("nested")).unwrap();
    fs::write(fixture.source.path.join("nested/leaf"), b"leaf").unwrap();
    let old = fixture.backup(|_, _| {}).unwrap();
    fs::write(fixture.source.path.join("file"), b"current").unwrap();
    let current = fixture.backup(|_, _| {}).unwrap();
    (fixture, old, current)
}

#[test]
fn a_current_pointer_cannot_be_hidden_by_removing_its_completed_record() {
    let fixture = FileFixture::new(b"current");
    let current = fixture.backup(|_, _| {}).unwrap();
    let parent = Root::open(current.path.parent().unwrap()).unwrap();
    assert!(withdraw(&fixture.db, &current, &parent, &fixture.cancel).is_err());
    let intent = Intent {
        version: 1,
        backup: current.clone(),
        parent: parent.storage_identity().unwrap(),
        quarantined: false,
        paused: false,
    };
    fixture.db.put(KIND, &current.id, &intent).unwrap();
    fixture.db.remove("backup", &current.id).unwrap();
    recover(&fixture.db).unwrap();
    assert!(current.path.exists());
    assert!(
        fixture
            .db
            .get::<Intent>(KIND, &current.id)
            .unwrap()
            .is_some()
    );
    assert!(
        fixture
            .db
            .get::<crate::DriveRecovery>("recovery", &current.id)
            .unwrap()
            .is_some()
    );
}

#[test]
fn failed_withdrawal_keeps_both_the_completed_record_and_its_data() {
    for kind in ["journal", "withdraw"] {
        let (fixture, old, _) = versions();
        let trigger = if kind == "journal" {
            "CREATE TRIGGER fail_retire BEFORE INSERT ON entities WHEN NEW.kind='backup-prune' BEGIN SELECT RAISE(FAIL,'journal fault'); END"
        } else {
            "CREATE TRIGGER fail_retire BEFORE DELETE ON entities WHEN OLD.kind='backup' BEGIN SELECT RAISE(FAIL,'withdraw fault'); END"
        };
        fixture.db.0.execute_batch(trigger).unwrap();
        assert!(prune(&fixture.db, &fixture.drive.id, 1, &fixture.cancel).is_err());
        assert!(
            fixture
                .db
                .get::<Backup>("backup", &old.id)
                .unwrap()
                .is_some(),
            "{kind}"
        );
        assert!(
            fixture.db.get::<Intent>(KIND, &old.id).unwrap().is_none(),
            "{kind}"
        );
        assert_eq!(fs::read(old.path.join("file")).unwrap(), b"old");
        fixture
            .db
            .0
            .execute_batch("DROP TRIGGER fail_retire")
            .unwrap();
        prune(&fixture.db, &fixture.drive.id, 1, &fixture.cancel).unwrap();
        assert!(!old.path.exists());
    }
}

#[test]
fn unwritable_retirement_parent_is_refused_before_withdrawing_the_completed_version() {
    if rustix::process::geteuid().is_root() {
        return;
    }
    let (fixture, old, _) = versions();
    let parent = old.path.parent().unwrap();
    fs::set_permissions(parent, fs::Permissions::from_mode(0o555)).unwrap();
    let result = prune(&fixture.db, &fixture.drive.id, 1, &fixture.cancel);
    fs::set_permissions(parent, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(result.is_err());
    assert!(
        fixture
            .db
            .get::<Backup>("backup", &old.id)
            .unwrap()
            .is_some()
    );
    assert!(fixture.db.get::<Intent>(KIND, &old.id).unwrap().is_none());
    assert_eq!(fs::read(old.path.join("file")).unwrap(), b"old");
}

#[test]
fn retirement_reopens_after_each_durable_filesystem_boundary() {
    for boundary in [
        "withdrawn",
        "renamed",
        "quarantined",
        "partial",
        "artifact-gone",
        "manifest-gone",
    ] {
        let (fixture, old, current) = versions();
        run_crash(&fixture, &old, boundary);
        let intent: Intent = fixture.db.get(KIND, &old.id).unwrap().unwrap();
        let target = old.path.parent().unwrap().join(intent.name());
        assert!(
            fixture
                .db
                .get::<Backup>("backup", &old.id)
                .unwrap()
                .is_none()
        );
        let reopened = Store::existing(&fixture.temp.path().join("db"), false).unwrap();
        recover(&reopened).unwrap();
        recover(&reopened).unwrap();
        assert!(!old.path.exists() && !target.exists(), "{boundary}");
        assert!(!old.manifest.as_ref().unwrap().exists(), "{boundary}");
        assert!(
            reopened.get::<Intent>(KIND, &old.id).unwrap().is_none(),
            "{boundary}"
        );
        assert_eq!(
            fs::read(current.path.join("file")).unwrap(),
            b"current",
            "{boundary}"
        );
    }
}

#[test]
fn cancelled_retirement_stays_paused_across_restart_until_explicit_retry() {
    let (fixture, old, current) = versions();
    let parent = Root::open(old.path.parent().unwrap()).unwrap();
    let mut intent = withdraw(&fixture.db, &old, &parent, &fixture.cancel).unwrap();
    fixture.cancel.store(true, Ordering::Relaxed);
    assert!(attempt(&fixture.db, &mut intent, &fixture.cancel, None).is_err());
    assert!(intent.paused);
    let reopened = Store::existing(&fixture.temp.path().join("db"), false).unwrap();
    recover(&reopened).unwrap();
    assert!(old.path.exists());
    assert!(
        reopened
            .get::<Intent>(KIND, &old.id)
            .unwrap()
            .unwrap()
            .paused
    );
    fixture.cancel.store(false, Ordering::Relaxed);
    assert!(
        prune(&reopened, &fixture.drive.id, 1, &fixture.cancel)
            .unwrap()
            .contains("Pruned 1")
    );
    assert!(!old.path.exists());
    assert!(current.path.exists());
}

#[test]
fn replaced_or_corrupt_retirement_inputs_are_retained_without_deleting_foreign_data() {
    for fault in [
        "parent",
        "artifact",
        "quarantine",
        "manifest",
        "version",
        "layout",
    ] {
        let (fixture, old, current) = versions();
        let parent = Root::open(old.path.parent().unwrap()).unwrap();
        let mut intent = withdraw(&fixture.db, &old, &parent, &fixture.cancel).unwrap();
        let target = parent.path.join(intent.name());
        match fault {
            "parent" => {
                fs::rename(&parent.path, parent.path.with_extension("retained")).unwrap();
                fs::create_dir(&parent.path).unwrap();
                fs::write(parent.path.join("foreign"), b"foreign data").unwrap();
            }
            "artifact" => {
                fs::rename(&old.path, old.path.with_extension("retained")).unwrap();
                fs::create_dir(&old.path).unwrap();
                fs::write(old.path.join("foreign"), b"foreign data").unwrap();
            }
            "quarantine" => {
                fs::create_dir(&target).unwrap();
                fs::write(target.join("foreign"), b"foreign data").unwrap();
            }
            "manifest" => {
                Store::existing(old.manifest.as_ref().unwrap(), false)
                    .unwrap()
                    .put("corrupt", "header", &true)
                    .unwrap();
            }
            "version" => intent.version = 100,
            "layout" => intent.backup.manifest = Some(parent.path.join("foreign.sqlite")),
            _ => unreachable!(),
        }
        fixture.db.put(KIND, &old.id, &intent).unwrap();
        recover(&fixture.db).unwrap();
        assert!(
            fixture.db.get::<Intent>(KIND, &old.id).unwrap().is_some(),
            "{fault}"
        );
        assert!(
            fixture
                .db
                .get::<crate::DriveRecovery>("recovery", &old.id)
                .unwrap()
                .is_some(),
            "{fault}"
        );
        if fault == "parent" {
            assert_eq!(
                fs::read(parent.path.join("foreign")).unwrap(),
                b"foreign data"
            );
            assert!(
                parent
                    .path
                    .with_extension("retained")
                    .join(current.path.file_name().unwrap())
                    .exists()
            );
        } else {
            assert!(current.path.exists(), "{fault}");
            if ["artifact", "quarantine"].contains(&fault) {
                let foreign = if fault == "artifact" {
                    &old.path
                } else {
                    &target
                };
                assert_eq!(fs::read(foreign.join("foreign")).unwrap(), b"foreign data");
            }
        }
    }
}

#[test]
#[ignore = "requires an explicitly selected empty owned Btrfs fixture directory"]
fn real_btrfs_retention_falls_back_to_owned_vfs_removal_without_elevation() {
    use std::os::unix::fs::MetadataExt;
    assert!(!rustix::process::geteuid().is_root());
    let destination =
        PathBuf::from(std::env::var_os("LITTLE_CLIP_DRIVE_RETENTION_DESTINATION").unwrap());
    assert!(fs::read_dir(&destination).unwrap().next().is_none());
    let mut fixture = FileFixture::new(b"old backup bytes");
    fixture.destination = Root::create(&destination.join("backups")).unwrap();
    assert!(btrfs(&fixture.destination).unwrap());
    let outside = fixture.temp.path().join("outside");
    fs::write(&outside, b"outside bytes").unwrap();
    std::os::unix::fs::symlink(&outside, fixture.source.path.join("outside-link")).unwrap();
    let raw = std::ffi::OsString::from_vec(b"raw-\xff".to_vec());
    fs::create_dir(fixture.source.path.join(&raw)).unwrap();
    fs::write(fixture.source.path.join(&raw).join("leaf"), b"nested bytes").unwrap();
    fs::set_permissions(
        fixture.source.path.join(&raw),
        fs::Permissions::from_mode(0o555),
    )
    .unwrap();
    let old = fixture.backup(|_, _| {}).unwrap();
    let middle = fixture.backup(|_, _| {}).unwrap();
    fs::write(fixture.source.path.join("file"), b"current backup bytes").unwrap();
    let current = fixture.backup(|_, _| {}).unwrap();
    assert!(old.incremental && middle.incremental && current.incremental);
    fs::set_permissions(
        fixture.source.path.join(&raw),
        fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    assert_eq!(fs::metadata(&current.path).unwrap().ino(), 256);
    assert_eq!(
        fs::write(current.path.join("must-not-write"), b"readonly control")
            .unwrap_err()
            .raw_os_error(),
        Some(libc::EROFS)
    );
    let message = prune(&fixture.db, &fixture.drive.id, 1, &fixture.cancel).unwrap();
    assert!(message.contains("Pruned 2"), "{message}");
    for old in [old, middle] {
        assert!(!old.path.exists());
        assert!(!old.manifest.unwrap().exists());
    }
    assert_eq!(
        fs::read(current.path.join("file")).unwrap(),
        b"current backup bytes"
    );
    assert_eq!(fs::read(&outside).unwrap(), b"outside bytes");
    assert!(fixture.db.list::<Intent>(KIND, 0, 10).unwrap().is_empty());
    println!("BTRFS_RETAINED_CURRENT={}", current.path.display());
}

fn run_crash(fixture: &FileFixture, backup: &Backup, boundary: &str) {
    let token = uuid::Uuid::new_v4().to_string();
    fs::write(fixture.temp.path().join(".retention-fixture-owned"), &token).unwrap();
    let mut root_env = std::ffi::OsString::from("LITTLE_CLIP_RETIRE_CRASH_ROOT=");
    root_env.push(fixture.temp.path());
    let output = CommandRequest::new("/usr/bin/env")
        .arg(root_env)
        .arg(format!("LITTLE_CLIP_RETIRE_CRASH_TOKEN={token}"))
        .arg(format!("LITTLE_CLIP_RETIRE_CRASH_POINT={boundary}"))
        .arg(format!("LITTLE_CLIP_RETIRE_CRASH_ID={}", backup.id))
        .arg(std::env::current_exe().unwrap())
        .args([
            "backup::retention::tests::retirement_crash_probe",
            "--exact",
            "--ignored",
            "--nocapture",
        ])
        .timeout(Duration::from_secs(15))
        .own_process_group()
        .die_with_parent()
        .run()
        .unwrap();
    assert_eq!(output.status.code(), Some(74), "{}", output.stderr_lossy());
    assert!(
        output
            .stdout_lossy()
            .contains(&format!("RETIREMENT_EXIT_AFTER_{boundary}"))
    );
}

#[test]
#[ignore = "owned abrupt-exit probe; invoked only by the parent recovery test"]
fn retirement_crash_probe() {
    use std::io::Write;
    let base = PathBuf::from(std::env::var_os("LITTLE_CLIP_RETIRE_CRASH_ROOT").unwrap());
    let token = std::env::var("LITTLE_CLIP_RETIRE_CRASH_TOKEN").unwrap();
    uuid::Uuid::parse_str(&token).unwrap();
    assert_eq!(
        fs::read_to_string(base.join(".retention-fixture-owned")).unwrap(),
        token
    );
    let boundary = std::env::var("LITTLE_CLIP_RETIRE_CRASH_POINT").unwrap();
    assert!(
        [
            "withdrawn",
            "renamed",
            "quarantined",
            "partial",
            "artifact-gone",
            "manifest-gone"
        ]
        .contains(&boundary.as_str())
    );
    let id = std::env::var("LITTLE_CLIP_RETIRE_CRASH_ID").unwrap();
    let db = Store::existing(&base.join("db"), false).unwrap();
    let backup: Backup = db.get("backup", &id).unwrap().unwrap();
    assert_eq!(backup.drive, "part:test");
    assert_eq!(backup.kind, BackupKind::Files);
    assert_eq!(backup.path.parent().unwrap(), base.join("backups/test"));
    let parent = Root::open(backup.path.parent().unwrap()).unwrap();
    let mut intent = withdraw(&db, &backup, &parent, &AtomicBool::new(false)).unwrap();
    let target = parent.path.join(intent.name());
    let _held = if boundary != "withdrawn" {
        let artifact = Artifact::open(&parent, backup.path.file_name().unwrap(), &backup)
            .unwrap()
            .unwrap();
        Some(quarantine(&db, &mut intent, &parent, artifact).unwrap())
    } else {
        None
    };
    if boundary == "renamed" {
        intent.quarantined = false;
        db.put(KIND, &backup.id, &intent).unwrap();
    }
    if ["partial", "artifact-gone", "manifest-gone"].contains(&boundary.as_str()) {
        fs::remove_file(target.join("nested/leaf")).unwrap();
        File::open(target.join("nested"))
            .unwrap()
            .sync_all()
            .unwrap();
    }
    if ["artifact-gone", "manifest-gone"].contains(&boundary.as_str()) {
        fs::remove_dir_all(&target).unwrap();
    }
    if boundary == "manifest-gone" {
        fs::remove_file(backup.manifest.as_ref().unwrap()).unwrap();
    }
    File::open(&parent.path).unwrap().sync_all().unwrap();
    println!("RETIREMENT_EXIT_AFTER_{boundary}");
    std::io::stdout().flush().unwrap();
    // Exit with the WAL connection, directory lease and pins live: no Rust cleanup.
    std::process::exit(74);
}
