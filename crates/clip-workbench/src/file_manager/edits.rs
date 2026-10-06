//! Atomic naming operations. Never fall back to an overwriting rename.

use std::ffi::{CString, OsString};
use std::io::Write;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use super::filesystem::decode_path;
use super::{BackgroundJobs, FileManager, JobError, Navigation, display_path};

#[derive(Clone, Debug)]
pub(super) enum NamingAction {
    Folder,
    File,
    Rename(PathBuf),
    Content(super::content::Content),
}

impl NamingAction {
    pub(super) fn title(&self) -> &'static str {
        match self {
            Self::Folder => "New folder",
            Self::File => "New empty file",
            Self::Rename(_) => "Rename",
            Self::Content(_) => "Create from clipboard",
        }
    }
}

pub(super) struct NameDialog {
    pub action: NamingAction,
    pub parent: PathBuf,
    pub name: String,
    pub error: Option<String>,
    pub focus: bool,
}

#[derive(Clone)]
struct NamingOperation {
    action: NamingAction,
    parent: PathBuf,
    name: OsString,
    selection_revision: u64,
}

struct PendingEdit {
    operation: NamingOperation,
    receiver: Option<mpsc::Receiver<Result<Result<PathBuf, String>, JobError>>>,
}

pub(super) struct EditReport {
    pub text: String,
    pub failed: bool,
}

#[derive(Default)]
pub(super) struct EditState {
    pub dialog: Option<NameDialog>,
    pending: Option<PendingEdit>,
    refresh_parent: Option<PathBuf>,
    pub report: Option<EditReport>,
}

impl FileManager {
    pub(super) fn edit_busy(&self) -> bool {
        self.edits.pending.is_some()
    }

    pub(super) fn can_edit(&self) -> bool {
        self.directory.is_some()
            && self.pending.is_none()
            && !self.filesystem_busy()
            && self.edits.dialog.is_none()
    }

    pub(super) fn begin_naming(&mut self, action: NamingAction) {
        if !self.can_edit() {
            return;
        }
        let name = match &action {
            NamingAction::Folder => "New folder".into(),
            NamingAction::File => "New file.txt".into(),
            NamingAction::Rename(source) => {
                display_path(Path::new(source.file_name().unwrap_or_default()))
            }
            NamingAction::Content(content) => content.default_name(),
        };
        self.edits.dialog = Some(NameDialog {
            action,
            parent: self.path().into(),
            name,
            error: None,
            focus: true,
        });
    }

    pub(super) fn begin_rename(&mut self) {
        if self.selection_count() == 1
            && let Some(source) = self
                .location()
                .and_then(|location| location.selection.first())
                .cloned()
        {
            self.begin_naming(NamingAction::Rename(source));
        }
    }

    pub(super) fn name_content(&mut self, content: super::content::Content, parent: PathBuf) {
        if self.edits.dialog.is_some() || self.edit_busy() {
            self.edits.report = Some(EditReport {
                text: "Finish the current naming operation before pasting content.".into(),
                failed: true,
            });
            return;
        }
        self.edits.dialog = Some(NameDialog {
            name: content.default_name(),
            action: NamingAction::Content(content),
            parent,
            error: None,
            focus: true,
        });
    }

    pub(super) fn submit_naming(&mut self) {
        let Some(dialog) = &mut self.edits.dialog else {
            return;
        };
        let name = match parse_name(&dialog.name) {
            Ok(name) => name,
            Err(error) => {
                dialog.error = Some(error);
                return;
            }
        };
        // Renaming to the identical byte sequence is a genuine no-op.
        if let NamingAction::Rename(source) = &dialog.action
            && source.file_name() == Some(name.as_os_str())
        {
            self.edits.dialog = None;
            return;
        }
        self.edits.pending = Some(PendingEdit {
            operation: NamingOperation {
                action: dialog.action.clone(),
                parent: dialog.parent.clone(),
                name,
                selection_revision: self.selection_revision,
            },
            receiver: None,
        });
        self.edits.dialog = None;
        self.edits.report = None;
    }

    pub(super) fn tick_edits(&mut self, jobs: &BackgroundJobs) {
        if self.transfers.mutation_running() {
            return;
        }
        if let Some(pending) = &mut self.edits.pending {
            if pending.receiver.is_none() {
                let operation = pending.operation.clone();
                // A post-operation timeout would discard the result of a mutation
                // that already happened. Preserve completion and never auto-retry it.
                let (_, receiver) = jobs.submit(Duration::MAX, move || operation.execute());
                pending.receiver = Some(receiver);
            }
            match pending
                .receiver
                .as_ref()
                .expect("submitted edit")
                .try_recv()
            {
                Ok(Err(JobError::QueueFull)) => pending.receiver = None,
                Ok(result) => {
                    let pending = self
                        .edits
                        .pending
                        .take()
                        .expect("edit completion has an operation");
                    let result = result.unwrap_or_else(|error| {
                        Err(format!(
                            "Completion unavailable ({error}); inspect the folder before retrying"
                        ))
                    });
                    self.finish_edit(pending.operation, result);
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    let pending = self
                        .edits
                        .pending
                        .take()
                        .expect("disconnected edit has an operation");
                    self.finish_edit(
                        pending.operation,
                        Err("Worker disconnected; inspect the folder before retrying".into()),
                    );
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        if self.pending.is_none()
            && let Some(parent) = self.edits.refresh_parent.take()
            && parent == self.path()
        {
            self.navigate(parent, Navigation::Refresh);
        }
    }

    fn finish_edit(&mut self, operation: NamingOperation, result: Result<PathBuf, String>) {
        let title = operation.action.title();
        self.tree.refresh(operation.parent.clone());
        self.edits.refresh_parent = Some(operation.parent.clone());
        match result {
            Ok(path) => {
                if let NamingAction::Rename(source) = &operation.action {
                    self.follow_rename(source, &path);
                } else if self.path() == operation.parent
                    && self.selection_revision == operation.selection_revision
                {
                    self.select(path.clone());
                }
                self.edits.report = Some(EditReport {
                    text: format!("{title}: {}", display_path(&path)),
                    failed: false,
                });
            }
            Err(error) => {
                self.edits.report = Some(EditReport {
                    text: format!("{title} failed: {error}"),
                    failed: true,
                });
                // Keep the captured destination and proposed name available for
                // correction. A failed filesystem operation is never retried automatically.
                self.edits.dialog = Some(NameDialog {
                    action: operation.action,
                    parent: operation.parent,
                    name: display_path(Path::new(&operation.name)),
                    error: Some(error),
                    focus: true,
                });
            }
        }
    }

    pub(super) fn follow_rename(&mut self, source: &Path, destination: &Path) {
        for location in &mut self.history {
            relocate(&mut location.path, source, destination);
            for selection in std::iter::once(&mut location.selection).chain(
                location
                    .selection_undo
                    .iter_mut()
                    .map(|snapshot| &mut snapshot.paths),
            ) {
                *selection = std::mem::take(selection)
                    .into_iter()
                    .map(|mut path| {
                        relocate(&mut path, source, destination);
                        path
                    })
                    .collect();
            }
            for path in [&mut location.selected, &mut location.anchor]
                .into_iter()
                .chain(
                    location
                        .selection_undo
                        .iter_mut()
                        .flat_map(|snapshot| [&mut snapshot.selected, &mut snapshot.anchor]),
                )
                .flatten()
            {
                relocate(path, source, destination);
            }
        }
        if let Some(directory) = &mut self.directory {
            relocate(&mut directory.path, source, destination);
            for entry in &mut directory.entries {
                if entry.path == source {
                    entry.name =
                        display_path(Path::new(destination.file_name().unwrap_or_default()));
                }
                relocate(&mut entry.path, source, destination);
            }
        }
        if let Some(request) = &mut self.pending
            && relocate(&mut request.path, source, destination)
        {
            request.job = None; // Reject a read of the old directory identity.
        }
        if let Some((path, _)) = &mut self.failed {
            relocate(path, source, destination);
        }
        let current = self.path().to_path_buf();
        if current.starts_with(destination) {
            self.edits.refresh_parent = Some(current.clone());
        }
        self.tree.reveal(&current);
        if !self.address_editing {
            self.address = display_path(&current);
        }
    }
}

pub(super) fn parse_name(text: &str) -> Result<OsString, String> {
    let path = decode_path(text)?;
    let bytes = path.as_os_str().as_bytes();
    if bytes.is_empty()
        || bytes.contains(&b'/')
        || path == Path::new(".")
        || path == Path::new("..")
    {
        return Err("Enter one filename, without /, . or ..".into());
    }
    Ok(path.into_os_string())
}

impl NamingOperation {
    fn execute(&self) -> Result<PathBuf, String> {
        if let NamingAction::Content(content) = &self.action {
            let staging = tempfile::Builder::new()
                .prefix(".little-clip-copy-")
                .tempdir_in(&self.parent)
                .map_err(|e| e.to_string())?;
            let staged = staging.path().join("content");
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&staged)
                .map_err(|e| e.to_string())?;
            file.write_all(&content.bytes)
                .and_then(|_| file.sync_all())
                .map_err(|e| e.to_string())?;
            let target = self.parent.join(&self.name);
            super::filesystem::rename_noreplace(&staged, &target).map_err(|e| e.to_string())?;
            return Ok(target);
        }
        let directory = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_CLOEXEC)
            .open(&self.parent)
            .map_err(|error| error.to_string())?;
        let name = CString::new(self.name.as_bytes()).map_err(|error| error.to_string())?;
        let fd = directory.as_raw_fd();
        let result = match &self.action {
            NamingAction::Folder => {
                // SAFETY: fd stays open, name is NUL-terminated, and mkdirat
                // creates one validated leaf relative to this directory.
                unsafe { libc::mkdirat(fd, name.as_ptr(), 0o777) }
            }
            NamingAction::File => {
                // SAFETY: same directory/name invariants. O_EXCL prevents both
                // overwriting a file and following a pre-existing destination link.
                let created = unsafe {
                    libc::openat(
                        fd,
                        name.as_ptr(),
                        libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_CLOEXEC,
                        0o666,
                    )
                };
                if created >= 0 {
                    // SAFETY: openat returned a newly owned descriptor.
                    drop(unsafe { OwnedFd::from_raw_fd(created) });
                    0
                } else {
                    -1
                }
            }
            NamingAction::Rename(source) => {
                if source.parent() != Some(self.parent.as_path()) {
                    return Err("Rename source is outside the captured folder".into());
                }
                let source_name = source.file_name().ok_or("Missing source name")?;
                super::filesystem::rename_at_noreplace(fd, source_name, fd, &self.name)
                    .map_err(|e| e.to_string())?;
                0
            }
            NamingAction::Content(_) => unreachable!("content is staged above"),
        };
        if result < 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        Ok(self.parent.join(&self.name))
    }
}

// Path::strip_prefix compares components, so renaming "a" never rewrites "ab".
fn relocate(path: &mut PathBuf, source: &Path, destination: &Path) -> bool {
    if let Ok(suffix) = path.strip_prefix(source) {
        *path = destination.join(suffix);
        true
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;
    #[test]
    fn names_round_trip_bytes_and_reject_paths() {
        for invalid in ["", ".", "..", "../escape", "a/b", "\\x00"] {
            assert!(parse_name(invalid).is_err(), "{invalid}");
        }
        let name = parse_name("space \\xFF\\\\end").unwrap();
        assert_eq!(name.as_bytes(), b"space \xff\\end");
    }
    #[test]
    fn create_and_rename_never_overwrite_existing_files_or_links() {
        let root = tempfile::tempdir().unwrap();
        let operation = |action, name: &str| NamingOperation {
            action,
            parent: root.path().into(),
            name: name.into(),
            selection_revision: 0,
        };
        let folder = operation(NamingAction::Folder, "folder").execute().unwrap();
        assert!(folder.is_dir());
        let source = operation(NamingAction::File, "file").execute().unwrap();
        assert_eq!(std::fs::metadata(&source).unwrap().len(), 0);
        std::fs::write(&source, b"keep").unwrap();
        assert!(operation(NamingAction::File, "file").execute().is_err());
        symlink("missing", root.path().join("link")).unwrap();
        assert!(
            operation(NamingAction::Rename(source.clone()), "link")
                .execute()
                .is_err()
        );
        assert_eq!(std::fs::read(&source).unwrap(), b"keep");
        let renamed = operation(NamingAction::Rename(source.clone()), "renamed")
            .execute()
            .unwrap();
        assert!(!source.exists());
        assert_eq!(std::fs::read(renamed).unwrap(), b"keep");
    }
}
