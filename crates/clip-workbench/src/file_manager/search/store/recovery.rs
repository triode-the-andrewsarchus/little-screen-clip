//! Explicit cache reset with resumable preservation of the closed SQLite file set.
use super::{
    File, OpenOptions, OpenOptionsExt, Path, PathBuf, PermissionsExt, Store, open_private,
    require_local_storage, restore,
};
use crate::file_manager::filesystem::rename_noreplace;
use serde::{Deserialize, Serialize};
use std::{
    io::{Read, Write},
    os::{fd::AsRawFd, unix::fs::MetadataExt},
};

/// Check before SQLite can recover/remove sidecars. New empty files have no header.
pub(super) fn check_header(input: &File) -> Result<(), String> {
    if input.metadata().map_err(|e| e.to_string())?.len() != 0 {
        restore::read_header(input).map_err(|e| format!("Unreadable search index header: {e}. Use Recover unreadable index in Search settings; existing files were preserved."))?;
    }
    Ok(())
}

const MARKER: &str = ".search-recovery-pending";
const FILES: [&str; 4] = [
    "index.sqlite",
    "index.sqlite-wal",
    "index.sqlite-shm",
    "index.sqlite-journal",
];

pub(super) struct IndexOwner {
    _lock: File,
    pub(super) directory: File,
}
impl Drop for IndexOwner {
    fn drop(&mut self) {
        // A concurrent fork can briefly inherit this open-file description even
        // with CLOEXEC. Retire ownership explicitly rather than waiting for all
        // inherited descriptors to close after their exec.
        // SAFETY: the lock descriptor remains owned and live during Drop.
        unsafe {
            libc::flock(self._lock.as_raw_fd(), libc::LOCK_UN);
        }
    }
}
impl IndexOwner {
    pub(super) fn acquire(folder: &Path) -> Result<Self, String> {
        let pin = little_clip_core::kernel_interface::create_content_directory(folder)
            .map_err(|e| e.to_string())?;
        let held = PathBuf::from(format!("/proc/self/fd/{}", pin.as_raw_fd()));
        // Reopen the held directory itself for fsync; the routing pin is O_PATH.
        let directory = File::open(&held).map_err(|e| e.to_string())?;
        std::fs::set_permissions(&held, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| e.to_string())?;
        require_local_storage(&directory)?;
        let lock = open_private(&held.join("index.lock"))?;
        // SAFETY: the owned file remains live for the entire ownership lifetime.
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err("Another file manager owns the search index".into());
        }
        let owner = Self {
            _lock: lock,
            directory,
        };
        owner.resume()?;
        restore::clean_staging(&held)?;
        Ok(owner)
    }
    pub(super) fn held(&self) -> PathBuf {
        PathBuf::from(format!("/proc/self/fd/{}", self.directory.as_raw_fd()))
    }
    fn preserve(&self) -> Result<PathBuf, String> {
        let held = self.held();
        let mut files = Vec::new();
        for name in FILES {
            files.push(identity(&held.join(name))?);
        }
        let archive = tempfile::Builder::new()
            .prefix(".search-recovery-")
            .tempdir_in(&held)
            .map_err(|e| e.to_string())?;
        let name = archive
            .path()
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned();
        let journal = Journal {
            version: 1,
            archive: name.clone(),
            files,
        };
        let mut marker = tempfile::NamedTempFile::new_in(&held).map_err(|e| e.to_string())?;
        serde_json::to_writer(marker.as_file_mut(), &journal).map_err(|e| e.to_string())?;
        marker.flush().map_err(|e| e.to_string())?;
        marker.as_file().sync_all().map_err(|e| e.to_string())?;
        // Keep the empty archive before publishing the durable recovery intent.
        // A crash before marker publication can leave an empty private directory.
        let _ = archive.keep();
        marker
            .persist_noclobber(held.join(MARKER))
            .map_err(|e| e.to_string())?;
        self.directory.sync_all().map_err(|e| e.to_string())?;
        self.resume()?;
        Ok(PathBuf::from(name))
    }
    fn resume(&self) -> Result<(), String> {
        let held = self.held();
        let marker = match OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(held.join(MARKER))
        {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.to_string()),
        };
        if !marker.metadata().map_err(|e| e.to_string())?.is_file() {
            return Err("Search recovery journal is not an ordinary file".into());
        }
        let mut bytes = Vec::new();
        marker
            .take(8193)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        if bytes.len() > 8192 {
            return Err("Invalid search recovery journal size".into());
        }
        let journal: Journal = serde_json::from_slice(&bytes)
            .map_err(|e| format!("Invalid search recovery journal: {e}"))?;
        journal.validate()?;
        // The cache parent is already pinned and the journal name is a single
        // validated component. Never follow a substituted archive symlink.
        let archive = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
            .open(held.join(&journal.archive))
            .map_err(|e| e.to_string())?;
        let target = PathBuf::from(format!("/proc/self/fd/{}", archive.as_raw_fd()));
        for (name, expected) in FILES.into_iter().zip(journal.files) {
            preserve_one(&held.join(name), &target.join(name), expected)?;
        }
        archive.sync_all().map_err(|e| e.to_string())?;
        self.directory.sync_all().map_err(|e| e.to_string())?;
        std::fs::remove_file(held.join(MARKER)).map_err(|e| e.to_string())?;
        self.directory.sync_all().map_err(|e| e.to_string())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Object {
    device: u64,
    inode: u64,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Journal {
    version: u8,
    archive: String,
    files: Vec<Option<Object>>,
}
impl Journal {
    fn validate(&self) -> Result<(), String> {
        let mut parts = Path::new(&self.archive).components();
        let single =
            matches!(parts.next(), Some(std::path::Component::Normal(_))) && parts.next().is_none();
        if self.version != 1
            || self.files.len() != FILES.len()
            || !self.archive.starts_with(".search-recovery-")
            || !single
        {
            return Err(
                "Unsupported or invalid search recovery journal; files were preserved".into(),
            );
        }
        Ok(())
    }
}
fn identity(path: &Path) -> Result<Option<Object>, String> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() => Ok(Some(Object {
            device: metadata.dev(),
            inode: metadata.ino(),
        })),
        Ok(_) => Err("Search recovery refuses non-ordinary database files".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}
fn preserve_one(source: &Path, target: &Path, expected: Option<Object>) -> Result<(), String> {
    let observed = (identity(source)?, identity(target)?);
    match (expected, observed) {
        (None, (None, None)) => Ok(()),
        (Some(expected), (None, Some(found))) if expected == found => Ok(()),
        (Some(expected), (Some(found), None)) if expected == found => {
            rename_noreplace(source, target).map_err(|e| e.to_string())?;
            if identity(target)? != Some(expected) {
                return Err("Search cache changed during recovery; files were preserved".into());
            }
            Ok(())
        }
        _ => Err("Search cache changed since recovery began; files were preserved".into()),
    }
}

impl Store {
    /// Explicitly preserve the closed cache before creating an empty replacement.
    /// The ownership lock survives closing a live database and all file moves.
    pub fn recover_empty(folder: &Path, previous: Option<Self>) -> Result<(Self, PathBuf), String> {
        let owner = match previous {
            Some(Self {
                connection, owner, ..
            }) => {
                drop(connection);
                owner
            }
            None => IndexOwner::acquire(folder)?,
        };
        let archive = owner.preserve()?;
        let store = Self::from_owner(owner)?;
        Ok((store, folder.join(archive)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn interrupted(folder: &Path) -> (IndexOwner, PathBuf, Journal) {
        let owner = IndexOwner::acquire(folder).unwrap();
        std::fs::write(folder.join(FILES[0]), b"damaged main database").unwrap();
        std::fs::write(folder.join(FILES[1]), b"uncheckpointed WAL bytes").unwrap();
        let archive = tempfile::Builder::new()
            .prefix(".search-recovery-")
            .tempdir_in(folder)
            .unwrap()
            .keep();
        let journal = Journal {
            version: 1,
            archive: archive.file_name().unwrap().to_str().unwrap().into(),
            files: FILES
                .iter()
                .map(|name| identity(&folder.join(name)).unwrap())
                .collect(),
        };
        std::fs::write(folder.join(MARKER), serde_json::to_vec(&journal).unwrap()).unwrap();
        (owner, archive, journal)
    }
    #[test]
    fn corrupt_cache_is_preserved_with_sidecars_before_an_empty_replacement() {
        let cache = tempfile::tempdir().unwrap();
        let main = cache.path().join(FILES[0]);
        std::fs::write(&main, b"not a sqlite database").unwrap();
        std::fs::write(cache.path().join(FILES[1]), b"original wal").unwrap();
        std::fs::write(cache.path().join(FILES[2]), b"original shared memory").unwrap();
        assert!(Store::open(cache.path()).is_err());
        assert_eq!(
            std::fs::read(cache.path().join(FILES[1])).unwrap(),
            b"original wal"
        );
        std::fs::write(cache.path().join("model-marker"), b"keep model").unwrap();
        let (store, archive) = Store::recover_empty(cache.path(), None).unwrap();
        assert_eq!(
            std::fs::read(archive.join(FILES[0])).unwrap(),
            b"not a sqlite database"
        );
        assert_eq!(
            std::fs::read(archive.join(FILES[1])).unwrap(),
            b"original wal"
        );
        assert_eq!(
            std::fs::read(archive.join(FILES[2])).unwrap(),
            b"original shared memory"
        );
        assert_eq!(
            std::fs::read(cache.path().join("model-marker")).unwrap(),
            b"keep model"
        );
        assert_eq!(store.vector_count, 0);
        assert!(store.roots().unwrap().is_empty());
        assert!(!cache.path().join(MARKER).exists());
        assert!(
            Store::open(cache.path()).is_err(),
            "ownership survives reset"
        );
        drop(store);
        assert!(Store::open(cache.path()).is_ok());
    }
    #[test]
    fn interrupted_file_set_move_finishes_before_sqlite_is_opened() {
        let cache = tempfile::tempdir().unwrap();
        let (owner, archive, journal) = interrupted(cache.path());
        preserve_one(
            &cache.path().join(FILES[0]),
            &archive.join(FILES[0]),
            journal.files[0],
        )
        .unwrap();
        let inherited = owner._lock.try_clone().unwrap();
        drop(owner); // Simulate the owner exiting after only the main-file move.
        let store = Store::open(cache.path()).unwrap();
        drop(inherited);
        assert_eq!(store.vector_count, 0);
        assert_eq!(
            std::fs::read(archive.join(FILES[0])).unwrap(),
            b"damaged main database"
        );
        assert_eq!(
            std::fs::read(archive.join(FILES[1])).unwrap(),
            b"uncheckpointed WAL bytes"
        );
        assert!(!cache.path().join(MARKER).exists());
    }
    #[test]
    fn interrupted_recovery_refuses_replacements_and_retains_its_journal() {
        let cache = tempfile::tempdir().unwrap();
        let (owner, archive, _) = interrupted(cache.path());
        std::fs::rename(
            cache.path().join(FILES[0]),
            cache.path().join("original-held"),
        )
        .unwrap();
        std::fs::write(cache.path().join(FILES[0]), b"outside replacement").unwrap();
        drop(owner);
        assert!(Store::open(cache.path()).is_err());
        assert_eq!(
            std::fs::read(cache.path().join(FILES[0])).unwrap(),
            b"outside replacement"
        );
        assert!(cache.path().join(MARKER).exists());
        assert!(!archive.join(FILES[0]).exists());
    }
    #[test]
    fn recovery_never_follows_database_or_archive_symlinks() {
        let cache = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let target = outside.path().join("untouched");
        std::fs::write(&target, b"keep").unwrap();
        symlink(&target, cache.path().join(FILES[0])).unwrap();
        assert!(Store::recover_empty(cache.path(), None).is_err());
        assert_eq!(std::fs::read(&target).unwrap(), b"keep");
        std::fs::remove_file(cache.path().join(FILES[0])).unwrap();
        let (owner, archive, _) = interrupted(cache.path());
        std::fs::remove_dir(&archive).unwrap();
        symlink(outside.path(), &archive).unwrap();
        drop(owner);
        assert!(Store::open(cache.path()).is_err());
        assert!(!outside.path().join(FILES[0]).exists());
        assert!(cache.path().join(MARKER).exists());
    }
    #[test]
    fn future_schema_requires_explicit_reset_and_live_ownership_is_retained() {
        let cache = tempfile::tempdir().unwrap();
        let store = Store::open(cache.path()).unwrap();
        store
            .connection
            .pragma_update(None, "user_version", 999)
            .unwrap();
        drop(store);
        assert!(Store::open(cache.path()).is_err());
        let old = std::fs::read(cache.path().join(FILES[0])).unwrap();
        let (store, archive) = Store::recover_empty(cache.path(), None).unwrap();
        assert_eq!(std::fs::read(archive.join(FILES[0])).unwrap(), old);
        let (store, second) = Store::recover_empty(cache.path(), Some(store)).unwrap();
        assert_ne!(archive, second);
        assert!(archive.join(FILES[0]).is_file());
        assert!(Store::open(cache.path()).is_err());
        assert_eq!(store.vector_count, 0);
    }
}
