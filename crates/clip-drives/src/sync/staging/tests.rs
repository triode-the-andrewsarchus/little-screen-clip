//! Interruption receipts are verified before either ordinary or named unions.
use super::*;
use std::os::unix::fs::symlink;

struct Fixture {
    _temp: tempfile::TempDir,
    db: Store,
    pair: SyncPair,
    portable: Root,
    local: Root,
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let portable = Root::create(&temp.path().join("portable")).unwrap();
        let local = Root::create(&temp.path().join("local")).unwrap();
        fs::write(local.path.join("edit"), vec![0x6b; 1024 * 1024]).unwrap();
        let pair = SyncPair {
            id: "pair".into(),
            drive: "drive".into(),
            evidence: "test".into(),
            local_identity: None,
            relative: PathBuf::new(),
            local: local.path.clone(),
            paused: false,
            deletions: false,
            ram: false,
            last_success: None,
            status: String::new(),
        };
        let db = Store::open(&temp.path().join("registry.sqlite")).unwrap();
        Self {
            _temp: temp,
            db,
            pair,
            portable,
            local,
        }
    }
    fn owner(&self) -> Reconcile<'_> {
        Reconcile {
            db: &self.db,
            pair: &self.pair,
            portable: &self.portable,
            local: &self.local,
            cancel: &NEVER_CANCEL,
            report: Default::default(),
            operation: None,
        }
    }
    fn interrupted(&self, portable: bool, bytes: usize) -> (String, PathBuf) {
        let to = if portable {
            &self.portable
        } else {
            &self.local
        };
        let entry = scan::observe(
            &self.local,
            Path::new("edit"),
            true,
            u64::MAX,
            &NEVER_CANCEL,
        )
        .unwrap()
        .unwrap();
        let mut stage = Copy::new(&self.owner(), to, Path::new("nested/edit"), &entry).unwrap();
        let (parent, _) = to.parent(Path::new("nested/edit"), true).unwrap();
        let temporary = stage.create(&parent).unwrap();
        if bytes > 0 {
            temporary
                .as_file()
                .write_all_at(&vec![0x6b; bytes], 0)
                .unwrap();
            stage.progress(bytes as u64).unwrap();
        }
        temporary.as_file().sync_all().unwrap();
        let (file, path) = temporary.keep().unwrap();
        // Resolve the proc descriptor while the parent is still retained.
        let path = fs::read_link(crate::root::anchor(&file)).unwrap_or(path);
        drop(file);
        (stage.id.clone(), path)
    }
    fn run(&self, named: bool) -> Result<SyncReport> {
        if named {
            changed(
                &self.db,
                &self.pair,
                &self.portable,
                &self.local,
                &std::collections::HashSet::from([self.local.path.join("edit")]),
                &NEVER_CANCEL,
                Options {
                    reuse: true,
                    ..Default::default()
                },
            )
        } else {
            synchronize_with(
                &self.db,
                &self.pair,
                (&self.portable, &self.local),
                &NEVER_CANCEL,
                |_, _| {},
                Options::default(),
            )
        }
    }
    fn receipt(&self, id: &str) -> Option<Pending> {
        self.db.get(KIND, id).unwrap()
    }
}
static NEVER_CANCEL: AtomicBool = AtomicBool::new(false);

#[test]
fn empty_partial_and_complete_stages_recover_after_source_edits_and_parent_moves() {
    for portable in [true, false] {
        for bytes in [0, 256 * 1024, 1024 * 1024] {
            let fixture = Fixture::new();
            let (id, path) = fixture.interrupted(portable, bytes);
            let root = if portable {
                &fixture.portable
            } else {
                &fixture.local
            };
            fs::rename(root.path.join("nested"), root.path.join("moved")).unwrap();
            let moved = root.path.join("moved").join(path.file_name().unwrap());
            // Filesystem-normalized case and further edits must not invalidate
            // an already persisted prefix proof or leak its filename into union.
            let uppercase =
                moved.with_file_name(moved.file_name().unwrap().to_str().unwrap().to_uppercase());
            fs::rename(&moved, &uppercase).unwrap();
            fs::write(fixture.local.path.join("edit"), b"newer offline bytes").unwrap();
            fixture.run(true).unwrap();
            assert!(!uppercase.exists());
            assert!(fixture.receipt(&id).is_none());
            for root in [&fixture.portable, &fixture.local] {
                assert_eq!(
                    fs::read(root.path.join("edit")).unwrap(),
                    b"newer offline bytes"
                );
                assert!(
                    !root
                        .path
                        .join("moved")
                        .join(uppercase.file_name().unwrap())
                        .exists()
                );
            }
        }
    }
}

#[test]
fn changed_uncertain_and_non_file_stages_block_union_without_deleting_user_data() {
    for mode in [
        "changed",
        "unproven",
        "symlink",
        "duplicate",
        "unknown-length",
    ] {
        let fixture = Fixture::new();
        let (id, path) = fixture.interrupted(true, 256 * 1024);
        let original = fs::read(&path).unwrap();
        let external = fixture._temp.path().join("outside");
        fs::write(&external, b"must survive").unwrap();
        match mode {
            "changed" => fs::write(&path, vec![0x7c; original.len()]).unwrap(),
            "unproven" => {
                let mut pending = fixture.receipt(&id).unwrap();
                pending.created = false;
                fixture.db.put(KIND, &id, &pending).unwrap();
            }
            "symlink" => {
                fs::remove_file(&path).unwrap();
                symlink(&external, &path).unwrap();
            }
            "duplicate" => {
                fs::copy(&path, fixture.portable.path.join(path.file_name().unwrap())).unwrap();
            }
            "unknown-length" => File::options()
                .write(true)
                .open(&path)
                .unwrap()
                .set_len(12345)
                .unwrap(),
            _ => unreachable!(),
        }
        let retained = fs::read(&path).unwrap();
        let error = fixture.run(false).err().unwrap().to_string();
        assert!(error.contains("retained"), "{mode}: {error}");
        assert_eq!(fs::read(&path).unwrap(), retained);
        assert_eq!(fs::read(external).unwrap(), b"must survive");
        assert!(fixture.receipt(&id).is_some());
        assert!(
            !fixture
                .local
                .path
                .join("nested")
                .join(path.file_name().unwrap())
                .exists()
        );
        assert!(!fixture.portable.path.join("edit").exists());
    }
}

#[test]
fn cleanup_quarantine_and_absent_stage_reconcile_with_reopened_registry() {
    for absent in [true, false] {
        let fixture = Fixture::new();
        let (id, path) = fixture.interrupted(true, 256 * 1024);
        let (_, quarantine) = names(&id).unwrap();
        let quarantined = path.with_file_name(quarantine);
        if absent {
            fs::remove_file(&path).unwrap();
        } else {
            fs::rename(&path, &quarantined).unwrap();
        }
        let db = Store::existing(&fixture._temp.path().join("registry.sqlite"), false).unwrap();
        assert_eq!(
            db.0.query_row("SELECT value FROM version", [], |row| row.get::<_, u32>(0))
                .unwrap(),
            4
        );
        recover(
            &db,
            &fixture.pair,
            (&fixture.portable, &fixture.local),
            &NEVER_CANCEL,
        )
        .unwrap();
        assert!(!quarantined.exists());
        assert!(fixture.receipt(&id).is_none());
        assert_eq!(
            db.0.query_row("SELECT count(*) FROM sync_stage_proofs", [], |row| row
                .get::<_, u64>(0))
                .unwrap(),
            0
        );
    }
}

#[test]
fn proof_commit_failure_retains_original_destination_and_can_retry() {
    let fixture = Fixture::new();
    fixture.run(false).unwrap();
    let original = fs::read(fixture.portable.path.join("edit")).unwrap();
    fs::write(fixture.local.path.join("edit"), vec![0x7c; original.len()]).unwrap();
    fixture.db.0.execute_batch("CREATE TRIGGER refuse_prefix BEFORE INSERT ON sync_stage_proofs WHEN NEW.bytes>0 BEGIN SELECT RAISE(FAIL,'prefix receipt fault'); END;").unwrap();
    assert!(
        fixture
            .run(false)
            .err()
            .unwrap()
            .to_string()
            .contains("prefix receipt fault")
    );
    assert_eq!(
        fs::read(fixture.portable.path.join("edit")).unwrap(),
        original
    );
    fixture
        .db
        .0
        .execute_batch("DROP TRIGGER refuse_prefix")
        .unwrap();
    fixture.run(false).unwrap();
    assert_eq!(
        fs::read(fixture.portable.path.join("edit")).unwrap(),
        vec![0x7c; original.len()]
    );
    assert!(!pending(&fixture.db, &fixture.pair).unwrap());
}
