//! Explicit archive browsing materializes a bounded private snapshot, never links.
use super::*;
use little_clip_core::staging::{ArtifactStore, TemporaryWorkspace};
use std::{os::fd::AsRawFd, path::PathBuf};

const EXPANDED_BYTES: u64 = 64 * 1024 * 1024;
const WORKSPACE_COST: u64 = 16 * 1024;

pub(in crate::file_manager) struct Snapshot {
    pub source: FileSource,
    pub _workspace: TemporaryWorkspace,
    pub bytes: u64,
    pub root: PathBuf,
    pub members: std::collections::BTreeMap<usize, (String, bool)>,
}
impl Snapshot {
    pub(in crate::file_manager) fn load(
        path: &Path,
        cancel: &AtomicBool,
        budget: u64,
    ) -> Result<Self, String> {
        checkpoint(cancel)?;
        let mut file = kernel_interface::open_content(path, false).map_err(|e| e.to_string())?;
        let source = FileSource::capture_file(&file)?;
        let workspace = ArtifactStore::new()
            .map_err(|e| e.to_string())?
            .create_temporary_workspace("archive-browser", "contents")
            .map_err(|e| e.to_string())?;
        let root = workspace.path().join("contents");
        let result = scan(
            &mut file,
            path,
            cancel,
            Members {
                display_limit: Some(ARCHIVE_ENTRIES),
                output: Some(Output::new(&root, budget)?),
                ..Default::default()
            },
        )?;
        if !result.verified {
            return Err(result
                .notice
                .unwrap_or_else(|| "Archive exceeds the bounded browsing limit".into()));
        }
        checkpoint(cancel)?;
        // A named-source check also verifies the held input identity; reject
        // changed or replaced archives before publishing the private snapshot.
        source.check()?;
        let mut bytes = WORKSPACE_COST;
        let mut members = std::collections::BTreeMap::new();
        for child in result.children {
            if let Child::Member {
                ordinal,
                name,
                directory,
                size,
            } = child
            {
                bytes += (Path::new(&name).components().count() as u64) * 4096;
                if !directory {
                    bytes += size;
                }
                members.insert(ordinal, (name, directory));
            }
        }
        Ok(Self {
            source,
            _workspace: workspace,
            bytes,
            root,
            members,
        })
    }
}

pub(super) struct Output {
    root: File,
    pending: Option<std::fs::File>,
    charged: u64,
    budget: u64,
}
impl Output {
    fn new(root: &Path, budget: u64) -> Result<Self, String> {
        if budget < WORKSPACE_COST {
            return Err("Archive browsing storage budget exhausted".into());
        }
        let root = kernel_interface::create_content_directory(root).map_err(|e| e.to_string())?;
        Ok(Self {
            root,
            pending: None,
            charged: WORKSPACE_COST,
            budget: budget.min(EXPANDED_BYTES),
        })
    }
    pub(super) fn admit(&mut self, name: &str, directory: bool, size: u64) -> Result<(), String> {
        if Path::new(name).components().count() > 32 {
            return Err("Archive path exceeds 32 levels".into());
        }
        self.charged = self
            .charged
            .checked_add((Path::new(name).components().count() as u64).saturating_mul(4096))
            .and_then(|charge| charge.checked_add(if directory { 0 } else { size }))
            .filter(|charge| *charge <= self.budget)
            .ok_or("Archive exceeds its browsing storage budget, including directory overhead")?;
        let path = PathBuf::from(format!("/proc/self/fd/{}", self.root.as_raw_fd())).join(name);
        // Names have passed Members' canonical component validation. This tree
        // is private and only this worker writes it; links are never installed.
        if directory {
            kernel_interface::create_content_directory(&path).map_err(|e| e.to_string())?;
        } else {
            let parent = kernel_interface::create_content_directory(
                path.parent().ok_or("Missing member parent")?,
            )
            .map_err(|e| e.to_string())?;
            let destination = PathBuf::from(format!("/proc/self/fd/{}", parent.as_raw_fd()))
                .join(path.file_name().ok_or("Missing member filename")?);
            self.pending = Some(
                std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(destination)
                    .map_err(|e| e.to_string())?,
            );
        }
        Ok(())
    }
    pub(super) fn copy(
        &mut self,
        input: &mut impl Read,
        size: u64,
        cancel: &AtomicBool,
    ) -> Result<(), String> {
        let mut output = self
            .pending
            .take()
            .ok_or("Missing admitted archive output")?;
        copy_payload(input, &mut output, size, cancel)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn zip(path: &Path, members: &[(&str, &[u8])]) {
        let mut zip = zip::ZipWriter::new(File::create(path).unwrap());
        for (name, bytes) in members {
            zip.start_file(
                *name,
                zip::write::SimpleFileOptions::default()
                    .compression_method(zip::CompressionMethod::Stored),
            )
            .unwrap();
            zip.write_all(bytes).unwrap();
        }
        zip.finish().unwrap();
    }
    #[test]
    fn browsing_snapshot_uses_real_contents_and_retains_workspace_for_readers() {
        let root = tempfile::tempdir().unwrap();
        let archive = root.path().join("example.zip");
        zip(
            &archive,
            &[
                ("nested/readme.txt", b"real payload"),
                ("other.txt", b"second"),
            ],
        );
        let original = std::fs::read(&archive).unwrap();
        let snapshot = Snapshot::load(&archive, &AtomicBool::new(false), EXPANDED_BYTES).unwrap();
        let child = snapshot.root.join("nested/readme.txt");
        assert_eq!(std::fs::read(&child).unwrap(), b"real payload");
        let source = FileSource::capture(&child).unwrap();
        let workspace = snapshot.root.clone();
        assert_eq!(snapshot.bytes, WORKSPACE_COST + 18 + 3 * 4096);
        drop(snapshot);
        assert!(child.exists());
        drop(source);
        assert!(!workspace.exists());
        assert_eq!(std::fs::read(archive).unwrap(), original);
        assert!(!root.path().join("nested").exists());
    }
    #[test]
    fn browsing_rejects_traversal_duplicates_prefix_collisions_and_expansion_budget() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("bad.zip");
        for members in [
            vec![("../escape", b"bad".as_slice())],
            vec![("same", b"one".as_slice()), ("./same", b"two".as_slice())],
            vec![
                ("parent", b"one".as_slice()),
                ("parent/child", b"two".as_slice()),
            ],
        ] {
            zip(&path, &members);
            assert!(Snapshot::load(&path, &AtomicBool::new(false), EXPANDED_BYTES).is_err());
        }
        zip(&path, &[("valid", b"too many bytes")]);
        assert!(Snapshot::load(&path, &AtomicBool::new(false), 2).is_err());
        assert!(Snapshot::load(&path, &AtomicBool::new(true), EXPANDED_BYTES).is_err());
        assert!(!root.path().join("escape").exists());
    }
    #[test]
    fn browsing_rejects_corrupt_payloads_and_links() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("bad.zip");
        zip(&path, &[("note.txt", b"checksum payload")]);
        let mut encoded = std::fs::read(&path).unwrap();
        let offset = encoded
            .windows(b"checksum payload".len())
            .position(|window| window == b"checksum payload")
            .unwrap();
        encoded[offset] ^= 1;
        std::fs::write(&path, encoded).unwrap();
        assert!(Snapshot::load(&path, &AtomicBool::new(false), EXPANDED_BYTES).is_err());
        let path = root.path().join("link.tar");
        let mut builder = tar::Builder::new(File::create(&path).unwrap());
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Symlink);
        header.set_size(0);
        header.set_mode(0o777);
        header.set_link_name("/etc/passwd").unwrap();
        header.set_cksum();
        builder
            .append_data(&mut header, "link", std::io::empty())
            .unwrap();
        builder.finish().unwrap();
        drop(builder);
        assert!(Snapshot::load(&path, &AtomicBool::new(false), EXPANDED_BYTES).is_err());
    }
    #[test]
    fn tar_and_compressed_tar_materialize_through_the_same_validated_sink() {
        let root = tempfile::tempdir().unwrap();
        let mut builder = tar::Builder::new(Vec::new());
        let mut header = tar::Header::new_gnu();
        header.set_size(7);
        header.set_mode(0o644);
        header.set_cksum();
        builder
            .append_data(&mut header, "folder/note.txt", b"payload".as_slice())
            .unwrap();
        let bytes = builder.into_inner().unwrap();
        let plain = root.path().join("files.tar");
        std::fs::write(&plain, &bytes).unwrap();
        let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        gzip.write_all(&bytes).unwrap();
        let compressed = root.path().join("files.tgz");
        std::fs::write(&compressed, gzip.finish().unwrap()).unwrap();
        for path in [&plain, &compressed] {
            let snapshot = Snapshot::load(path, &AtomicBool::new(false), EXPANDED_BYTES).unwrap();
            assert_eq!(
                std::fs::read(snapshot.root.join("folder/note.txt")).unwrap(),
                b"payload"
            );
        }
    }
}
