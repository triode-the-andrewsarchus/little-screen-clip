//! Explicit maintenance of generated records and consistent user-owned backups.
use super::{Store, record_storage_fault};
use rusqlite::{
    Connection, OpenFlags,
    backup::{Backup, StepResult},
};
use std::{
    os::fd::AsRawFd,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

impl Store {
    /// Export one self-contained snapshot, including committed WAL content.
    /// The destination must be unused; installation retains the ordinary parent.
    pub fn export(&self, destination: &Path, cancel: &AtomicBool) -> Result<(), String> {
        let directory = little_clip_core::kernel_interface::pin_content_directory(
            destination.parent().ok_or("Missing backup parent")?,
        )
        .map_err(|error| error.to_string())?;
        let held = std::path::PathBuf::from(format!("/proc/self/fd/{}", directory.as_raw_fd()));
        let target = held.join(destination.file_name().ok_or("Missing backup filename")?);
        let temporary =
            tempfile::NamedTempFile::new_in(&held).map_err(|error| error.to_string())?;
        let staging = temporary.path().to_path_buf();
        let result = (|| {
            let mut output =
                Connection::open_with_flags(&staging, OpenFlags::SQLITE_OPEN_READ_WRITE)
                    .map_err(|error| error.to_string())?;
            output
                .busy_timeout(Duration::from_secs(2))
                .map_err(|error| error.to_string())?;
            copy_database(&self.connection, &mut output, cancel, None)?;
            output
                .execute_batch("PRAGMA journal_mode=DELETE; PRAGMA synchronous=FULL;")
                .map_err(|error| error.to_string())?;
            output.close().map_err(|(_, error)| error.to_string())?;
            if cancel.load(Ordering::Relaxed) {
                return Err("Index backup cancelled before installation".into());
            }
            temporary
                .as_file()
                .sync_all()
                .map_err(|error| error.to_string())?;
            temporary
                .persist_noclobber(target)
                .map_err(|error| format!("Choose an unused backup filename: {error}"))?;
            // Installation is the commit point: report success after it happens.
            Ok(())
        })();
        for suffix in ["-wal", "-shm", "-journal"] {
            let mut sidecar = staging.as_os_str().to_os_string();
            sidecar.push(suffix);
            let _ = std::fs::remove_file(std::path::PathBuf::from(sidecar));
        }
        result
    }

    /// Clear generated inventory/vectors, retaining enrolled root identities.
    /// A post-commit compaction failure is a warning, never a false failed clear.
    pub fn clear_generated(&mut self) -> Result<Option<String>, String> {
        self.invalidate_vectors()?;
        let tx = self
            .connection
            .transaction()
            .map_err(|error| record_storage_fault(&self.storage_fault, error))?;
        tx.execute_batch("DELETE FROM paths; DELETE FROM directories; DELETE FROM files; UPDATE roots SET available=0,state=CASE WHEN state='disabled' THEN 'disabled' ELSE 'checking' END,error=NULL;").map_err(|error| record_storage_fault(&self.storage_fault, error))?;
        tx.commit()
            .map_err(|error| record_storage_fault(&self.storage_fault, error))?;
        self.vector_count = 0;
        let warning = match compact(&self.connection) {
            Ok(false) => None,
            Ok(true) => Some("Index cleared; another reader is delaying space reclamation".into()),
            Err(error) => Some(format!(
                "Index cleared; space compaction could not finish: {}",
                record_storage_fault(&self.storage_fault, error)
            )),
        };
        Ok(warning)
    }
}

fn compact(connection: &Connection) -> rusqlite::Result<bool> {
    let busy: i64 =
        connection.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| row.get(0))?;
    if busy != 0 {
        return Ok(true);
    }
    connection.execute_batch("VACUUM;")?;
    let busy: i64 =
        connection.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| row.get(0))?;
    Ok(busy != 0)
}

pub(super) fn copy_database(
    source: &Connection,
    destination: &mut Connection,
    cancel: &AtomicBool,
    fault: Option<&std::cell::Cell<bool>>,
) -> Result<(), String> {
    let record = |error| match fault {
        Some(fault) => record_storage_fault(fault, error),
        None => error.to_string(),
    };
    let backup = Backup::new(source, destination).map_err(&record)?;
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        if cancel.load(Ordering::Relaxed) || Instant::now() >= deadline {
            return Err("Index backup cancelled or exceeded its preparation deadline".into());
        }
        match backup.step(128).map_err(&record)? {
            StepResult::Done => return Ok(()),
            StepResult::More => {}
            StepResult::Busy | StepResult::Locked => std::thread::sleep(Duration::from_millis(10)),
            _ => return Err("Unexpected SQLite backup status".into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_manager::search::store::{DIMENSION, Segment};
    use std::os::unix::fs::PermissionsExt;
    fn populated(path: &Path) -> Store {
        let mut store = Store::open(path).unwrap();
        store
            .connection
            .pragma_update(None, "wal_autocheckpoint", 0)
            .unwrap();
        store.connection.execute_batch("INSERT INTO roots(id,path,device,inode,available,state) VALUES(1,x'2f736f75726365',2,3,1,'ready'); INSERT INTO files(id,device,inode,revision,pipeline,metadata) VALUES(1,2,4,1,'model','{}');").unwrap();
        store
            .publish(
                1,
                1,
                "model",
                &[Segment {
                    modality: "text".into(),
                    location: "passage".into(),
                    text: "a cat beside the sea".into(),
                    input_hash: vec![1; 32],
                    vector: Some(vec![1.0; DIMENSION]),
                }],
                "complete",
            )
            .unwrap();
        store
    }
    #[test]
    fn online_backup_includes_wal_and_never_overwrites_or_leaks_staging() {
        let cache = tempfile::tempdir().unwrap();
        let output = tempfile::tempdir().unwrap();
        let store = populated(cache.path());
        let target = output.path().join("backup.sqlite");
        store.export(&target, &AtomicBool::new(false)).unwrap();
        let copy = Connection::open_with_flags(&target, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        let count: u64 = copy
            .query_row(
                "SELECT COUNT(*) FROM segment_fts WHERE segment_fts MATCH 'cat'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
        assert_eq!(
            copy.query_row("PRAGMA integrity_check", [], |row| row.get::<_, String>(0))
                .unwrap(),
            "ok"
        );
        assert_eq!(
            std::fs::metadata(&target).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let before = std::fs::read(&target).unwrap();
        assert!(store.export(&target, &AtomicBool::new(false)).is_err());
        assert_eq!(std::fs::read(&target).unwrap(), before);
        assert!(
            store
                .export(
                    &output.path().join("cancelled.sqlite"),
                    &AtomicBool::new(true)
                )
                .is_err()
        );
        assert_eq!(std::fs::read_dir(output.path()).unwrap().count(), 1);
    }
    #[test]
    fn committed_clear_with_a_busy_reader_reports_delayed_reclamation() {
        let cache = tempfile::tempdir().unwrap();
        let mut store = populated(cache.path());
        store
            .connection
            .busy_timeout(Duration::from_millis(10))
            .unwrap();
        let reader = Connection::open(cache.path().join("index.sqlite")).unwrap();
        reader
            .execute_batch("BEGIN; SELECT COUNT(*) FROM files;")
            .unwrap();
        assert!(
            store
                .clear_generated()
                .unwrap()
                .unwrap()
                .contains("another reader")
        );
        assert_eq!(store.vector_count, 0);
        assert_eq!(
            store
                .connection
                .query_row("SELECT COUNT(*) FROM files", [], |row| row.get::<_, u64>(0))
                .unwrap(),
            0
        );
        drop(reader);
        assert!(store.clear_generated().unwrap().is_none());
    }
    #[test]
    fn clearing_preserves_root_identity_models_and_existing_backup() {
        let cache = tempfile::tempdir().unwrap();
        let output = tempfile::tempdir().unwrap();
        let mut store = populated(cache.path());
        let target = output.path().join("backup.sqlite");
        store.export(&target, &AtomicBool::new(false)).unwrap();
        std::fs::write(cache.path().join("model-marker"), b"keep").unwrap();
        assert!(store.clear_generated().unwrap().is_none());
        assert_eq!(store.vector_count, 0);
        assert_eq!(
            store
                .connection
                .query_row("SELECT COUNT(*) FROM files", [], |row| row.get::<_, u64>(0))
                .unwrap(),
            0
        );
        assert_eq!(
            store
                .connection
                .query_row("SELECT device,inode FROM roots", [], |row| Ok((
                    row.get::<_, u64>(0)?,
                    row.get::<_, u64>(1)?
                )))
                .unwrap(),
            (2, 3)
        );
        assert_eq!(
            std::fs::read(cache.path().join("model-marker")).unwrap(),
            b"keep"
        );
        let copy = Connection::open(&target).unwrap();
        assert_eq!(
            copy.query_row("SELECT COUNT(*) FROM segments", [], |row| row
                .get::<_, u64>(0))
                .unwrap(),
            1
        );
    }
}
