use super::*;
use std::os::unix::ffi::{OsStrExt, OsStringExt};

impl FileFixture {
    /// Edits after fingerprint capture, while the private version is still unpublished.
    pub(super) fn edit_before_copy(&self, mut edit: impl FnMut(u64)) -> (Result<Backup>, u64) {
        let mut attempts = 0;
        let result = self.backup(|count, _| {
            if count == 0 {
                attempts += 1;
                edit(attempts);
            }
        });
        (result, attempts)
    }

    pub(super) fn assert_only_current(&self, expected: &Backup) {
        let versions = self.db.list::<Backup>("backup", 0, 10).unwrap();
        assert_eq!(versions.len(), 1);
        assert_eq!(versions[0].id, expected.id);
        assert_eq!(
            publication::current(&self.db, &self.drive.id, BackupKind::Files)
                .unwrap()
                .unwrap()
                .id,
            expected.id
        );
        let reference: Backup = serde_json::from_slice(
            &fs::read(expected.path.parent().unwrap().join("current-files.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(reference.id, expected.id);
        publication::check_manifest(expected, &AtomicBool::new(false)).unwrap();
    }
}

#[test]
fn changed_file_is_recollected_and_manifest_describes_the_installed_revision() {
    let fixture = FileFixture::new(b"first");
    let relative = PathBuf::from(std::ffi::OsString::from_vec(b"raw-\xff".to_vec()));
    let source = fixture.source.path.join(&relative);
    fs::rename(fixture.source.path.join("file"), &source).unwrap();
    let first = fixture.backup(|_, _| {}).unwrap();
    let modified = fs::metadata(&source).unwrap().modified().unwrap();
    let (second, attempts) = fixture.edit_before_copy(|attempt| {
        if attempt == 1 {
            fs::write(&source, b"after").unwrap();
            File::open(&source)
                .unwrap()
                .set_times(fs::FileTimes::new().set_modified(modified))
                .unwrap();
        }
    });
    let second = second.unwrap();
    assert_eq!(attempts, 2);
    assert_eq!(fs::read(first.path.join(&relative)).unwrap(), b"first");
    assert_eq!(fs::read(second.path.join(&relative)).unwrap(), b"after");
    let manifest = Store::existing(second.manifest.as_ref().unwrap(), true).unwrap();
    let recorded = scan::entry(&manifest, &format!("backup:{}", second.id), &relative)
        .unwrap()
        .unwrap();
    let actual = scan::observe(&fixture.source, &relative, true, u64::MAX, &fixture.cancel)
        .unwrap()
        .unwrap();
    assert!(recorded.same_revision(&actual));
    assert_eq!(recorded.hash, actual.hash);
    let retry: serde_json::Value = manifest.get("collection-retry", "0").unwrap().unwrap();
    assert_eq!(retry["attempts"], 2);
    assert_eq!(
        retry["path"],
        serde_json::json!(relative.as_os_str().as_bytes())
    );
    assert_eq!(
        manifest
            .get::<u64>("collection", "retried_entries")
            .unwrap(),
        Some(1)
    );
    assert_eq!(
        manifest
            .get::<u8>("collection", "per_file_attempt_limit")
            .unwrap(),
        Some(3)
    );
    let started: u64 = manifest.get("collection", "started_at").unwrap().unwrap();
    let completed: u64 = manifest.get("collection", "completed_at").unwrap().unwrap();
    assert!(started > 0 && completed > 0);
    publication::check_manifest(&second, &AtomicBool::new(false)).unwrap();
}

#[test]
fn repeated_source_changes_exhaust_the_budget_without_advancing_current() {
    let fixture = FileFixture::new(b"first");
    let first = fixture.backup(|_, _| {}).unwrap();
    let (result, attempts) = fixture.edit_before_copy(|attempt| {
        fs::write(
            fixture.source.path.join("file"),
            format!("change-{attempt}"),
        )
        .unwrap();
    });
    let error = result.unwrap_err();
    assert_eq!(attempts, 3);
    assert!(error.to_string().contains("after 3 attempt(s)"), "{error}");
    assert_eq!(fs::read(first.path.join("file")).unwrap(), b"first");
    fixture.assert_only_current(&first);
}

#[test]
fn cancellation_between_capture_and_copy_stops_without_retrying_or_publishing() {
    let fixture = FileFixture::new(b"first");
    let first = fixture.backup(|_, _| {}).unwrap();
    let (result, attempts) = fixture.edit_before_copy(|_| {
        fixture
            .cancel
            .store(true, std::sync::atomic::Ordering::Relaxed);
    });
    let error = result.unwrap_err();
    assert_eq!(attempts, 1);
    assert!(error.to_string().contains("Cancelled"), "{error}");
    fixture.assert_only_current(&first);
    assert_eq!(fs::read(first.path.join("file")).unwrap(), b"first");
}

#[test]
fn file_replacement_with_a_symlink_refuses_collection_and_preserves_the_old_backup() {
    let fixture = FileFixture::new(b"first");
    let first = fixture.backup(|_, _| {}).unwrap();
    let outside = fixture.temp.path().join("outside");
    fs::write(&outside, b"outside the admitted source").unwrap();
    let source = fixture.source.path.join("file");
    let (result, attempts) = fixture.edit_before_copy(|_| {
        fs::remove_file(&source).unwrap();
        std::os::unix::fs::symlink(&outside, &source).unwrap();
    });
    let error = result.unwrap_err();
    assert_eq!(attempts, 1);
    assert!(error.to_string().contains("changed type"), "{error}");
    fixture.assert_only_current(&first);
    assert_eq!(fs::read(first.path.join("file")).unwrap(), b"first");
}
