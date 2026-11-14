//! Archive snapshots reuse ordinary browser listings and preview pipelines.
use super::*;
use container_preview::inventory::extract::Snapshot;
use std::sync::LazyLock;

static JOBS: LazyLock<BackgroundJobs> = LazyLock::new(|| BackgroundJobs::new(1, 1));
const CACHE_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Default)]
pub(super) struct Archives {
    snapshots: Vec<Snapshot>,
    pending: Option<preview_job::Task<Result<Snapshot, String>>>,
    member: Option<usize>,
}
impl Archives {
    pub fn context(&self, path: &Path) -> Option<&Snapshot> {
        self.snapshots
            .iter()
            .rev()
            .find(|snapshot| path.starts_with(&snapshot.root))
    }
    pub fn label(&self, path: &Path) -> Option<PathBuf> {
        let mut label = path.to_path_buf();
        let mut changed = false;
        for _ in 0..32 {
            let Some(snapshot) = self.context(&label) else {
                break;
            };
            label = snapshot
                .source
                .path()
                .join(label.strip_prefix(&snapshot.root).ok()?);
            changed = true;
        }
        changed.then_some(label)
    }
    pub fn export_directory(&self, path: &Path) -> Option<PathBuf> {
        let mut source = self.context(path)?.source.path();
        while let Some(outer) = self.context(source) {
            source = outer.source.path();
        }
        source.parent().map(Path::to_path_buf)
    }
    pub fn status(&self, path: &Path) -> Option<String> {
        if self.busy() {
            return Some("Opening compressed archive…".into());
        }
        let snapshot = self.context(path)?;
        Some(format!(
            "Browsing compressed archive: {} · temporary contents",
            snapshot
                .source
                .path()
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
        ))
    }
    pub fn resolve(&self, path: &Path) -> PathBuf {
        if let Some(snapshot) = self.snapshots.iter().rev().find(|snapshot| {
            self.label(&snapshot.root)
                .as_deref()
                .is_some_and(|label| path.starts_with(label))
        }) {
            let label = self.label(&snapshot.root).expect("matched snapshot label");
            return snapshot
                .root
                .join(path.strip_prefix(label).expect("matched prefix"));
        }
        self.snapshots
            .iter()
            .rev()
            .find_map(|snapshot| {
                path.strip_prefix(snapshot.source.path())
                    .ok()
                    .map(|suffix| snapshot.root.join(suffix))
            })
            .unwrap_or_else(|| path.to_path_buf())
    }
    pub fn cancel(&mut self) {
        self.pending = None;
    }
    pub fn busy(&self) -> bool {
        self.pending.is_some()
    }
}
impl FileManager {
    pub(super) fn location_label(&self) -> String {
        display_path(
            &self
                .archives
                .label(self.path())
                .unwrap_or_else(|| self.path().to_path_buf()),
        )
    }
    pub(super) fn archive_context(&self) -> Option<&Snapshot> {
        self.archives.context(self.path())
    }
    pub(super) fn open_archive(&mut self, path: PathBuf) {
        let (member, revision) = match self.document_start.take() {
            Some((source, DocumentPosition::ArchiveMember(member, revision))) if source == path => {
                (Some(member), Some(revision))
            }
            _ => (None, None),
        };
        self.open_archive_member(path, member, revision);
    }
    pub(super) fn open_archive_member(
        &mut self,
        path: PathBuf,
        member: Option<usize>,
        revision: Option<u64>,
    ) {
        self.archives.cancel();
        if let Some(revision) = revision
            && let Err(error) = filesystem::check_revision(&path, revision)
        {
            self.set_error(error);
            return;
        }
        self.archives.member = member;
        if let Some(index) =
            self.archives.snapshots.iter().position(|snapshot| {
                snapshot.source.path() == path && snapshot.source.check().is_ok()
            })
        {
            self.browse_archive_snapshot(index, member);
            return;
        }
        // History retains its snapshot until that history branch is discarded.
        // Refuse extra decompression rather than silently filling the disk.
        let history = &self.history;
        self.archives.snapshots.retain(|snapshot| {
            history
                .iter()
                .any(|location| location.path.starts_with(&snapshot.root))
        });
        let used: u64 = self
            .archives
            .snapshots
            .iter()
            .map(|snapshot| snapshot.bytes)
            .sum();
        if used >= CACHE_BYTES {
            self.set_error("Archive browsing cache reached 256 MiB; leave old history before opening another archive");
            return;
        }
        self.stop_previews();
        self.archives.pending = Some(preview_job::Task::start(
            &JOBS,
            Duration::from_secs(30),
            move |cancel| {
                if let Some(revision) = revision {
                    filesystem::check_revision(&path, revision)?;
                }
                Snapshot::load(&path, cancel, CACHE_BYTES - used)
            },
        ));
    }
    fn browse_archive_snapshot(&mut self, index: usize, member: Option<usize>) {
        let snapshot = &self.archives.snapshots[index];
        let selected = member
            .and_then(|ordinal| snapshot.members.get(&ordinal))
            .map(|(name, directory)| (snapshot.root.join(name), *directory));
        let destination = selected.as_ref().map_or_else(
            || snapshot.root.clone(),
            |(path, directory)| {
                if *directory {
                    path.clone()
                } else {
                    path.parent().unwrap_or(&snapshot.root).to_path_buf()
                }
            },
        );
        self.navigate(destination, Navigation::New);
        self.reveal_target = selected
            .filter(|(_, directory)| !directory)
            .map(|(path, _)| path);
    }
    pub(super) fn tick_archives(&mut self) {
        let Some(result) = self
            .archives
            .pending
            .as_ref()
            .and_then(preview_job::Task::poll)
        else {
            return;
        };
        self.archives.pending = None;
        match result {
            Ok(Ok(snapshot)) => {
                self.archives.snapshots.push(snapshot);
                let member = self.archives.member.take();
                self.browse_archive_snapshot(self.archives.snapshots.len() - 1, member);
            }
            Ok(Err(error)) => self.set_error(error),
            Err(error) => self.set_error(error.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    fn snapshot(root: &Path) -> Snapshot {
        let path = root.join("example.zip");
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
        zip.start_file("folder/note.txt", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"archive payload").unwrap();
        zip.finish().unwrap();
        Snapshot::load(&path, &AtomicBool::new(false), CACHE_BYTES).unwrap()
    }
    #[test]
    fn archive_paths_round_trip_and_parent_returns_to_the_containing_folder() {
        let root = tempfile::tempdir().unwrap();
        let snapshot = snapshot(root.path());
        let physical = snapshot.root.join("folder");
        let logical = root.path().join("example.zip/folder");
        let mut browser = FileManager {
            pending: None,
            ..Default::default()
        };
        browser.archives.snapshots.push(snapshot);
        assert_eq!(browser.archives.label(&physical), Some(logical.clone()));
        assert_eq!(browser.archives.resolve(&logical), physical);
        let snapshot_root = browser.archives.snapshots[0].root.clone();
        browser.commit(
            filesystem::read_directory(&snapshot_root, &AtomicBool::new(false)).unwrap(),
            Navigation::New,
        );
        assert!(!browser.can_edit());
        assert!(!browser.selection_is_mutable());
        assert!(
            browser
                .archives
                .status(browser.path())
                .unwrap()
                .contains("Browsing compressed archive")
        );
        assert_eq!(browser.tree.selected.as_deref(), Some(root.path()));
        browser.parent();
        assert_eq!(browser.pending.as_ref().unwrap().path, root.path());
    }
    #[test]
    fn member_activation_uses_normal_folder_navigation_and_reveal() {
        let root = tempfile::tempdir().unwrap();
        let snapshot = snapshot(root.path());
        let parent = snapshot.root.join("folder");
        let selected = parent.join("note.txt");
        let mut browser = FileManager {
            pending: None,
            ..Default::default()
        };
        browser.archives.snapshots.push(snapshot);
        browser.browse_archive_snapshot(0, Some(0));
        assert_eq!(browser.pending.as_ref().unwrap().path, parent);
        assert_eq!(browser.reveal_target.as_ref(), Some(&selected));
        browser.commit(
            filesystem::read_directory(&parent, &AtomicBool::new(false)).unwrap(),
            Navigation::New,
        );
        assert_eq!(browser.selected_paths(), [selected]);
    }
}
