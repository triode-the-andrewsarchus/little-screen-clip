//! RAM exports share retained worker ownership without changing the media baseline.
use super::*;

#[derive(Clone)]
pub(super) struct Save {
    pub session: RamSession,
    pub source: Arc<Root>,
    pub target: Arc<Root>,
}

impl Save {
    fn validate_roots(db: &Store, source: &Root, target: &Root) -> Result<()> {
        source.validate()?;
        target.validate()?;
        policy::validate_ram_backing_storage(db, target)?;
        policy::validate_view_target(db, target)?;
        if source.overlap(target) {
            return Err("Choose a save destination outside this RAM session".into());
        }
        Ok(())
    }
    pub fn run(
        &self,
        db: &Store,
        cancel: &AtomicBool,
        progress: &mut impl FnMut(u64, u64),
    ) -> Result<String> {
        self.copy(db, cancel, progress).map_err(|error| {
            format!(
                "RAM save incomplete in {}: {error}. Completed files are retained",
                self.target.path.display()
            )
            .into()
        })
    }
    fn copy(
        &self,
        db: &Store,
        cancel: &AtomicBool,
        progress: &mut impl FnMut(u64, u64),
    ) -> Result<String> {
        cancelled(cancel)?;
        Self::validate_roots(db, &self.source, &self.target)?;
        let id = format!("save:{}", self.session.id);
        let root_metadata = scan::observe(&self.source, Path::new(""), false, u64::MAX, cancel)?
            .ok_or("RAM root disappeared")?;
        let coverage = scan::scan(
            db,
            &self.source,
            &id,
            &scan::Limits {
                hash_bytes: self
                    .session
                    .backing
                    .as_ref()
                    .map_or(u64::MAX, |b| b.capacity),
                ..Default::default()
            },
            cancel,
            &mut *progress,
        )?;
        if !coverage.complete {
            return Err(format!("RAM inventory incomplete: {}", coverage.message).into());
        }
        let mut count = 0;
        let mut bytes = 0;
        for entry in scan::Cursor::new(db, &id) {
            let entry = entry?;
            bytes += sync::install(
                &self.source,
                &self.target,
                &entry,
                &entry.path,
                None,
                cancel,
                Default::default(),
            )?;
            count += 1;
            progress(count, bytes);
        }
        backup::finish_directories(db, &id, &self.target, cancel)?;
        backup::apply_directory_metadata(&self.target, &root_metadata)?;
        self.target.validate()?;
        db.clear_scan(&id)?;
        Ok(format!(
            "{}RAM copy saved to {}",
            if self.session.ready { "" } else { "Partial " },
            self.target.path.display()
        ))
    }
}
impl Save {
    pub(super) fn prepare(
        db: &Store,
        session: RamSession,
        destination: &Path,
        claim: &crate::root::Scope,
        cancel: &AtomicBool,
    ) -> Result<Arc<Self>> {
        if Root::planned_scope(destination)? != *claim {
            return Err("RAM save destination changed since scheduling admission; retry".into());
        }
        cancelled(cancel)?;
        let source = Arc::new(Root::open(&session.path)?);
        let base = Root::create(destination)?;
        Self::validate_roots(db, &source, &base)?;
        let name = format!("ram-copy-{}", uuid::Uuid::new_v4());
        base.ensure_directory(Path::new(&name))?;
        let target = Arc::new(base.subtree(Path::new(&name))?);
        let pin = target.directory(Path::new(""), false)?;
        File::open(crate::root::anchor(&pin))?
            .set_permissions(fs::Permissions::from_mode(0o700))?;
        Ok(Arc::new(Save {
            session,
            source,
            target,
        }))
    }
}
impl Owner {
    pub(super) fn saved_ram_result(&self, save: &Save, result: &Result<String>) -> Result<()> {
        if let Some(mut session) = self.db.get::<RamSession>("ram", &save.session.id)? {
            session.save_status = Some(match result {
                Ok(message) => message.clone(),
                Err(error) => error.to_string(),
            });
            if result.is_ok() {
                session.last_saved = Some(RamSave {
                    path: save.target.path.clone(),
                    completed: now(),
                    populated: save.session.ready,
                });
            }
            self.db.put("ram", &session.id, &session)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::MetadataExt;

    #[test]
    fn guarded_and_cancelled_exports_retain_files_and_retry_preserves_directory_metadata() {
        let temp = tempfile::tempdir().unwrap();
        let source = Arc::new(Root::create(&temp.path().join("ram")).unwrap());
        let target = Arc::new(Root::create(&temp.path().join("saved")).unwrap());
        fs::create_dir(source.path.join("folder")).unwrap();
        fs::write(source.path.join("folder/a"), b"keep completed output").unwrap();
        fs::write(source.path.join("folder/b"), b"also keep this").unwrap();
        fs::set_permissions(
            source.path.join("folder"),
            fs::Permissions::from_mode(0o750),
        )
        .unwrap();
        let session = RamSession {
            id: "export-fixture".into(),
            drive: "offline".into(),
            identity: kernel_interface::MountIdentity::directory(&source.path).unwrap(),
            path: source.path.clone(),
            ready: false,
            connected: false,
            status: "Media is offline".into(),
            backing: None,
            last_saved: None,
            save_status: None,
            last_flushed: None,
            pending: None,
        };
        let db = Store::open(&temp.path().join("state.sqlite")).unwrap();
        let save = Save {
            session,
            source,
            target,
        };
        let cancel = AtomicBool::new(false);
        for kind in ["catalog", "backup"] {
            // The destination became managed after the Save pinned its roots.
            db.put(
                kind,
                "mounted-view",
                &serde_json::json!({"mount": save.target.path}),
            )
            .unwrap();
            assert!(
                save.run(&db, &cancel, &mut |_, _| panic!(
                    "Managed target reached copying"
                ))
                .unwrap_err()
                .to_string()
                .contains("managed catalog or image view")
            );
            assert_eq!(fs::read_dir(&save.target.path).unwrap().count(), 0);
            let scanned: u64 =
                db.0.query_row("SELECT count(*) FROM entries", [], |row| row.get(0))
                    .unwrap();
            assert_eq!(
                scanned, 0,
                "Managed target must be rejected before source enumeration"
            );
            db.remove(kind, "mounted-view").unwrap();
        }
        let result = save.run(&db, &cancel, &mut |_, _| {
            if save.target.path.join("folder/a").is_file() {
                cancel.store(true, Ordering::Release);
            }
        });
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Completed files are retained")
        );
        assert_eq!(
            fs::read(save.target.path.join("folder/a")).unwrap(),
            b"keep completed output"
        );
        // A fresh destination follows the same retained-root path as a new user request.
        let retry = Save {
            target: Arc::new(Root::create(&temp.path().join("retry")).unwrap()),
            ..save
        };
        cancel.store(false, Ordering::Release);
        assert!(
            retry
                .run(&db, &cancel, &mut |_, _| {})
                .unwrap()
                .starts_with("Partial RAM copy saved")
        );
        assert_eq!(
            fs::read(retry.target.path.join("folder/b")).unwrap(),
            b"also keep this"
        );
        let original = fs::metadata(retry.source.path.join("folder")).unwrap();
        let copied = fs::metadata(retry.target.path.join("folder")).unwrap();
        assert_eq!(copied.mode(), original.mode());
        assert_eq!(
            (copied.mtime(), copied.mtime_nsec()),
            (original.mtime(), original.mtime_nsec())
        );
    }
}
