//! RAM exports share retained worker ownership without changing the media baseline.
use super::*;

#[derive(Clone)]
pub(super) struct Save {
    pub session: RamSession,
    pub source: Arc<Root>,
    pub target: Arc<Root>,
}

impl Save {
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
        self.source.validate()?;
        self.target.validate()?;
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
                false,
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
impl Owner {
    pub(super) fn prepare_ram_save(&mut self, id: &str, destination: &Path) -> Result<Arc<Save>> {
        self.mount_ram(id)?;
        let session: RamSession = self.db.get("ram", id)?.ok_or("Unknown RAM session")?;
        self.ram_available(&session)?;
        let source = Arc::new(Root::open(&session.path)?);
        let base = Root::create(destination)?;
        self.validate_ram_backing_target(&base)?;
        if source.overlap(&base) {
            return Err("Choose a save destination outside this RAM session".into());
        }
        let name = format!("ram-copy-{}", uuid::Uuid::new_v4());
        base.ensure_directory(Path::new(&name))?;
        let target = Arc::new(Root::open(&base.path.join(name))?);
        let pin = target.directory(Path::new(""), false)?;
        File::open(crate::root::anchor(&pin))?
            .set_permissions(fs::Permissions::from_mode(0o700))?;
        Ok(Arc::new(Save {
            session,
            source,
            target,
        }))
    }
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
    fn cancelled_export_retains_completed_files_and_a_retry_preserves_directory_metadata() {
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
