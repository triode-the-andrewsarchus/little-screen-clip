//! Crash recovery for workspaces whose paths are never retained as exported outputs.
use super::{ArtifactStore, StagingError, TEMPORARY_WORKSPACES, safe_component};
use std::{
    fs::{self, File, OpenOptions},
    io,
    os::{
        fd::AsRawFd,
        unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    },
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

const NEW_WORKSPACE_GRACE: Duration = Duration::from_secs(60);
const OWNER_FILE: &str = ".owner-lock";

/// A temporary workspace protected from automatic reclamation for its owner's lifetime.
///
/// Dropping the owner removes the workspace through its pinned parent. After a
/// process crash, another creator in the same scope can reclaim the unlocked
/// workspace. Filesystems without advisory locks retain normal-drop cleanup. This type does not offer retention for external path consumers.
#[derive(Debug)]
pub struct TemporaryWorkspace {
    // Field order keeps both the lock and parent alive during directory cleanup.
    _temporary: tempfile::TempDir,
    _owner_lock: Option<File>,
    directory: File,
    _parent: File,
    path: PathBuf,
}

impl TemporaryWorkspace {
    /// Returns the ordinary filesystem path used by this workspace's consumers.
    pub fn path(&self) -> &Path {
        &self.path
    }
    /// Extends workspace ownership through an internal helper's inherited handle.
    ///
    /// # Errors
    /// Returns descriptor-duplication failures before the helper can start.
    pub(crate) fn guard_command(
        &self,
        request: crate::CommandRequest,
    ) -> io::Result<crate::CommandRequest> {
        let request = request.die_with_parent().own_process_group();
        match &self._owner_lock {
            Some(lock) => Ok(request.inherit_handle(lock)?.0),
            None => Ok(request),
        }
    }
}

impl Drop for TemporaryWorkspace {
    fn drop(&mut self) {
        if let Some(owner) = self._owner_lock.take() {
            let probe = OpenOptions::new()
                .read(true)
                .write(true)
                .open(descriptor_path(&owner));
            drop(owner);
            match probe {
                Ok(lock) if lock.try_lock().is_ok() => self._owner_lock = Some(lock),
                // Another request or child still owns the workspace. A later
                // creator reclaims it once that final inherited handle closes.
                _ => {
                    self._temporary.disable_cleanup(true);
                    return;
                }
            }
        }
        // Remove the owner marker last, so a crash during cleanup leaves a
        // registered workspace that the next process can finish reclaiming.
        self._temporary.disable_cleanup(true);
        let name = self
            ._temporary
            .path()
            .file_name()
            .expect("temporary workspace has a filename");
        if let Err(error) = remove_workspace(&self.directory, &self._parent, name) {
            tracing::debug!(%error, "Could not remove temporary workspace");
        }
    }
}

impl ArtifactStore {
    /// Creates a private workspace with normal-drop cleanup and crash recovery.
    ///
    /// Old unlocked workspaces in the same scope are reclaimed before creation.
    /// Active owners retain their lock even if the workspace is old. Exported
    /// files should use [`Self::create_file`] or [`Self::create_workspace`].
    ///
    /// # Errors
    /// Returns an I/O error if the ordinary parent, temporary directory, or
    /// lifetime-lock file cannot be created. Unsupported advisory locks fall back
    /// to normal-drop cleanup without automatic reclamation.
    pub fn create_temporary_workspace(
        &self,
        scope: &str,
        prefix: &str,
    ) -> Result<TemporaryWorkspace, StagingError> {
        let root = self
            .base_dir
            .join(TEMPORARY_WORKSPACES)
            .join(safe_component(scope, "general"));
        let parent = crate::kernel_interface::create_content_directory(&root)?;
        if let Err(error) = reclaim(&parent, NEW_WORKSPACE_GRACE) {
            tracing::debug!(%error, "Could not reclaim abandoned workspaces");
        }
        let mut temporary = tempfile::Builder::new()
            .prefix(&format!("job-{}-", safe_component(prefix, "workspace")))
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir_in(descriptor_path(&parent))?;
        let directory = crate::kernel_interface::pin_content_directory(temporary.path())?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(descriptor_path(&directory).join(OWNER_FILE))?;
        let owner_lock = match lock.try_lock_shared() {
            Ok(()) => Some(lock),
            Err(error) => {
                // Preserve ordinary preview functionality on filesystems without
                // advisory locks. An untracked prefix excludes this fallback
                // from automatic reclamation, including while it is empty.
                tracing::debug!(%error, "Workspace locks unavailable; retaining normal-drop cleanup");
                drop(lock);
                temporary.close()?;
                temporary = tempfile::Builder::new()
                    .prefix("untracked-")
                    .permissions(fs::Permissions::from_mode(0o700))
                    .tempdir_in(descriptor_path(&parent))?;
                None
            }
        };
        let directory = crate::kernel_interface::pin_content_directory(temporary.path())?;
        let path = fs::read_link(descriptor_path(&directory))?;
        Ok(TemporaryWorkspace {
            _temporary: temporary,
            _owner_lock: owner_lock,
            directory,
            _parent: parent,
            path,
        })
    }
}

fn descriptor_path(directory: &File) -> PathBuf {
    PathBuf::from(format!("/proc/self/fd/{}", directory.as_raw_fd()))
}

fn reclaim(parent: &File, grace: Duration) -> io::Result<usize> {
    let mut removed = 0;
    for entry in fs::read_dir(descriptor_path(parent))?.flatten() {
        match reclaim_entry(parent, &entry, grace) {
            Ok(true) => removed += 1,
            Ok(false) => {}
            Err(error) => tracing::debug!(%error, "Could not reclaim temporary workspace"),
        }
    }
    Ok(removed)
}

// Keep one entry's identity checks, lifetime lock and deletion together. A
// disappearing or unreadable entry must not stop recovery of the other jobs.
fn reclaim_entry(parent: &File, entry: &fs::DirEntry, grace: Duration) -> io::Result<bool> {
    if !entry.file_name().to_string_lossy().starts_with("job-")
        || !entry.file_type().is_ok_and(|kind| kind.is_dir())
    {
        return Ok(false);
    }
    let Ok(directory) = crate::kernel_interface::pin_content_directory(&entry.path()) else {
        return Ok(false);
    };
    let Ok(pinned_lock) =
        crate::file_source::open_regular_file(&descriptor_path(&directory).join(OWNER_FILE), false)
    else {
        // A crash before creating the lock can leave an empty directory.
        // remove_dir cannot remove a workspace that contains payload files.
        if super::is_expired(&directory.metadata()?, SystemTime::now(), grace.as_secs())
            .unwrap_or(false)
        {
            let _ = fs::remove_dir(entry.path());
        }
        return Ok(false);
    };
    // Reopen the validated inode for NFS/CIFS write-lock compatibility.
    let Ok(lock) = OpenOptions::new()
        .read(true)
        .write(true)
        .open(descriptor_path(&pinned_lock))
    else {
        return Ok(false);
    };
    let old = lock
        .metadata()?
        .modified()?
        .elapsed()
        .is_ok_and(|age| age >= grace);
    if !old || lock.try_lock().is_err() {
        return Ok(false);
    }
    let held = directory.metadata()?;
    let Ok(named) = fs::symlink_metadata(entry.path()) else {
        return Ok(false);
    };
    if (held.dev(), held.ino()) != (named.dev(), named.ino()) {
        return Ok(false);
    }
    remove_workspace(&directory, parent, &entry.file_name())?;
    Ok(true)
}

fn remove_workspace(directory: &File, parent: &File, name: &std::ffi::OsStr) -> io::Result<()> {
    let anchor = descriptor_path(directory);
    for entry in fs::read_dir(&anchor)? {
        let entry = entry?;
        if entry.file_name() == OWNER_FILE {
            continue;
        }
        if entry.file_type()?.is_dir() {
            fs::remove_dir_all(entry.path())?;
        } else {
            fs::remove_file(entry.path())?;
        }
    }
    match fs::remove_file(anchor.join(OWNER_FILE)) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
        _ => {}
    }
    let named = descriptor_path(parent).join(name);
    let held = directory.metadata()?;
    if let Ok(current) = fs::symlink_metadata(&named)
        && (held.dev(), held.ino()) == (current.dev(), current.ino())
    {
        fs::remove_dir(named)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn abandoned_workspaces_are_reclaimed_while_live_and_recent_workspaces_survive() {
        let root = tempfile::tempdir().unwrap();
        let store = ArtifactStore {
            base_dir: root.path().to_owned(),
        };
        let active = store.create_temporary_workspace("pdf", "active").unwrap();
        active
            ._owner_lock
            .as_ref()
            .unwrap()
            .set_modified(SystemTime::UNIX_EPOCH)
            .unwrap();
        let retained = store
            .stage_bytes("pdf", "export", "bin", b"external reference")
            .unwrap();
        File::open(&retained)
            .unwrap()
            .set_modified(SystemTime::UNIX_EPOCH)
            .unwrap();
        let orphan = store.create_temporary_workspace("pdf", "orphan").unwrap();
        fs::write(orphan.path().join("partial.pdf"), b"abandoned bytes").unwrap();
        orphan
            ._owner_lock
            .as_ref()
            .unwrap()
            .set_modified(SystemTime::UNIX_EPOCH)
            .unwrap();
        let abandoned = orphan.path().to_owned();
        // Simulate process death: the directory remains and the OS lock closes.
        let inherited = orphan
            .guard_command(crate::CommandRequest::new("unused-test-command"))
            .unwrap();
        drop(orphan);
        drop(inherited);
        assert!(abandoned.exists());
        let recent = store.create_temporary_workspace("other", "recent").unwrap();
        let recent_path = recent.path().to_owned();
        let inherited = recent
            .guard_command(crate::CommandRequest::new("unused-test-command"))
            .unwrap();
        drop(recent);
        drop(inherited);
        let foreign = active.path().parent().unwrap().join("job-unregistered");
        fs::create_dir(&foreign).unwrap();
        fs::write(foreign.join("keep"), b"unknown owner").unwrap();
        File::open(&foreign)
            .unwrap()
            .set_modified(SystemTime::UNIX_EPOCH)
            .unwrap();
        let unfinished = active.path().parent().unwrap().join("job-before-lock");
        fs::create_dir(&unfinished).unwrap();
        File::open(&unfinished)
            .unwrap()
            .set_modified(SystemTime::UNIX_EPOCH)
            .unwrap();
        let untracked = active.path().parent().unwrap().join("untracked-job");
        fs::create_dir(&untracked).unwrap();
        File::open(&untracked)
            .unwrap()
            .set_modified(SystemTime::UNIX_EPOCH)
            .unwrap();
        let alias = active.path().parent().unwrap().join("job-alias");
        std::os::unix::fs::symlink(root.path(), &alias).unwrap();
        let parent =
            crate::kernel_interface::pin_content_directory(active.path().parent().unwrap())
                .unwrap();
        // The next creation may already have reclaimed the abandoned entry.
        let _next = store.create_temporary_workspace("pdf", "next").unwrap();
        assert_eq!(reclaim(&parent, NEW_WORKSPACE_GRACE).unwrap(), 0);
        assert_eq!(fs::read(&retained).unwrap(), b"external reference");
        assert!(!abandoned.exists());
        assert!(!unfinished.exists());
        assert!(active.path().is_dir());
        assert!(foreign.is_dir());
        assert!(untracked.is_dir());
        assert!(alias.is_symlink());
        let recent_parent =
            crate::kernel_interface::pin_content_directory(recent_path.parent().unwrap()).unwrap();
        assert_eq!(reclaim(&recent_parent, NEW_WORKSPACE_GRACE).unwrap(), 0);
        assert!(recent_path.is_dir());
        // Legacy explicit age cleanup must also respect the managed namespace.
        store.cleanup_old_items(0).unwrap();
        assert!(active.path().join(OWNER_FILE).is_file());
        let active_path = active.path().to_owned();
        drop(active);
        assert!(!active_path.exists());
    }

    #[test]
    fn inherited_helper_ownership_preserves_the_workspace_until_the_request_is_released() {
        let root = tempfile::tempdir().unwrap();
        let store = ArtifactStore {
            base_dir: root.path().to_owned(),
        };
        let workspace = store.create_temporary_workspace("pdf", "helper").unwrap();
        workspace
            ._owner_lock
            .as_ref()
            .unwrap()
            .set_modified(SystemTime::UNIX_EPOCH)
            .unwrap();
        let path = workspace.path().to_owned();
        let request = workspace
            .guard_command(crate::CommandRequest::new("unused-test-command"))
            .unwrap();
        drop(workspace);
        assert!(path.is_dir(), "the request retains the lifetime lock");
        let _while_held = store.create_temporary_workspace("pdf", "held").unwrap();
        assert!(path.is_dir());
        drop(request);
        let _after_release = store.create_temporary_workspace("pdf", "released").unwrap();
        assert!(!path.exists());
    }

    #[test]
    fn normal_cleanup_removes_nested_payloads_without_following_symlinks() {
        let root = tempfile::tempdir().unwrap();
        let store = ArtifactStore {
            base_dir: root.path().to_owned(),
        };
        let workspace = store.create_temporary_workspace("pdf", "payload").unwrap();
        let path = workspace.path().to_owned();
        fs::create_dir(path.join("nested")).unwrap();
        fs::write(path.join("nested/page.pdf"), b"temporary page").unwrap();
        let outside = root.path().join("outside");
        fs::write(&outside, b"keep").unwrap();
        std::os::unix::fs::symlink(&outside, path.join("reference")).unwrap();
        drop(workspace);
        assert!(!path.exists());
        assert_eq!(fs::read(outside).unwrap(), b"keep");
    }

    #[test]
    fn workspace_drop_cleans_through_the_original_parent_after_replacement() {
        let root = tempfile::tempdir().unwrap();
        let store = ArtifactStore {
            base_dir: root.path().to_owned(),
        };
        let workspace = store.create_temporary_workspace("pdf", "moving").unwrap();
        let parent = workspace.path().parent().unwrap().to_owned();
        let name = workspace.path().file_name().unwrap().to_owned();
        let moved = parent.with_extension("moved");
        fs::rename(&parent, &moved).unwrap();
        fs::create_dir_all(parent.join(&name)).unwrap();
        fs::write(parent.join(&name).join("keep"), b"other directory").unwrap();
        drop(workspace);
        assert!(!moved.join(&name).exists());
        assert_eq!(
            fs::read(parent.join(&name).join("keep")).unwrap(),
            b"other directory"
        );
    }
}
