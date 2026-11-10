//! Recursive archives consume pinned ordinary sources, never tool-resolved pathnames.
use super::{EXTERNAL_COMMAND_TIMEOUT, artifacts::Workspace, content::Input};
use little_clip_core::{
    file_inventory::{Identity, identity},
    kernel_interface,
};
use rawzip::{
    Header, ZipArchiveWriter, extra_fields::ExtraFieldId, path::EntryPath, time::UtcDateTime,
};
use std::{
    collections::HashMap,
    fs::{File, Metadata},
    io::{self, Read, Write},
    os::{
        fd::AsRawFd,
        unix::{ffi::OsStrExt, fs::MetadataExt},
    },
    path::{Component, Path, PathBuf},
    time::Instant,
};

// Central-directory and revision records are bounded, independently of payload size.
const MAX_ENTRIES: usize = 100_000;
const MAX_NAMES: usize = 32 * 1024 * 1024;
const MAX_DEPTH: usize = 64;

struct Observation {
    path: PathBuf,
    revision: Identity,
}
impl Observation {
    fn check(&self) -> io::Result<()> {
        let file = ordinary_pin(&self.path)?;
        check_identity(&file, self.revision)
    }
}

struct Tree {
    observations: Vec<Observation>,
    members: HashMap<Vec<u8>, Identity>,
    ancestors: Vec<(u64, u64)>,
    workspace: (u64, u64),
    name_bytes: usize,
    entry_limit: usize,
    deadline: Instant,
}
impl Tree {
    fn new(workspace: &Workspace) -> io::Result<Self> {
        Ok(Self {
            observations: Vec::new(),
            members: HashMap::new(),
            ancestors: Vec::new(),
            workspace: workspace.identity()?,
            name_bytes: 0,
            entry_limit: MAX_ENTRIES,
            deadline: Instant::now() + EXTERNAL_COMMAND_TIMEOUT,
        })
    }

    fn check_time(&self) -> io::Result<()> {
        if Instant::now() >= self.deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "Archive creation timed out",
            ));
        }
        Ok(())
    }

    fn record(&mut self, path: &Path, name: &[u8], revision: Identity) -> io::Result<bool> {
        self.check_time()?;
        if let Some(previous) = self.members.get(name) {
            if *previous == revision {
                return Ok(false);
            }
            return Err(io::Error::other(
                "Distinct sources have the same archive member name",
            ));
        }
        self.name_bytes += name.len() + path.as_os_str().len();
        if self.observations.len() >= self.entry_limit || self.name_bytes > MAX_NAMES {
            return Err(io::Error::other(
                "Archive exceeds the entry/name memory limit",
            ));
        }
        self.members.insert(name.to_vec(), revision);
        self.observations.push(Observation {
            path: path.to_owned(),
            revision,
        });
        Ok(true)
    }

    fn visit<W: Write>(
        &mut self,
        archive: &mut ZipArchiveWriter<W>,
        lookup: &Path,
        logical: &Path,
        mut name: Vec<u8>,
    ) -> io::Result<()> {
        self.check_time()?;
        let pin = ordinary_pin(lookup)?;
        let metadata = pin.metadata()?;
        let revision = identity(&metadata);
        // The output may be inside a selected ancestor. Never recurse into our own stage.
        if revision.object() == self.workspace {
            return Ok(());
        }
        if metadata.is_dir() {
            if name.is_empty() {
                name.push(b'.');
            }
            name.push(b'/');
        }
        if !self.record(logical, &name, revision)? {
            return Ok(());
        }
        if metadata.is_dir() {
            self.write_directory(archive, &pin, &metadata, logical, &name)?;
        } else {
            let input = Input::open(&descriptor(&pin))?;
            check_identity(&input.file, revision)?;
            self.write_file(archive, &name, &metadata, &input.file)?;
            input.check()?;
        }
        check_identity(&pin, revision)
    }

    fn write_directory<W: Write>(
        &mut self,
        archive: &mut ZipArchiveWriter<W>,
        pin: &File,
        metadata: &Metadata,
        logical: &Path,
        name: &[u8],
    ) -> io::Result<()> {
        let object = identity(metadata).object();
        if self.ancestors.len() >= MAX_DEPTH || self.ancestors.contains(&object) {
            return Err(io::Error::other("Archive directory cycle or depth limit"));
        }
        let mut directory = archive
            .new_dir(entry_path(name))
            .unix_permissions(metadata.mode())
            .last_modified(UtcDateTime::from_unix(metadata.mtime()));
        for (id, bytes) in member_extras(name, metadata) {
            directory = directory
                .extra_field(id, &bytes, Header::default())
                .map_err(io::Error::other)?;
        }
        directory.create().map_err(io::Error::other)?;
        self.ancestors.push(object);
        let directory = descriptor(pin);
        // Enumeration uses the retained directory, not a re-resolved parent name.
        for child in std::fs::read_dir(&directory)? {
            let child = child?;
            let mut child_name = if name == b"./" {
                Vec::new()
            } else {
                name.to_vec()
            };
            child_name.extend_from_slice(child.file_name().as_bytes());
            self.visit(
                archive,
                &directory.join(child.file_name()),
                &logical.join(child.file_name()),
                child_name,
            )?;
        }
        self.ancestors.pop();
        Ok(())
    }

    fn write_file<W: Write>(
        &self,
        archive: &mut ZipArchiveWriter<W>,
        name: &[u8],
        metadata: &Metadata,
        file: &File,
    ) -> io::Result<()> {
        let mut file_entry = archive
            .new_file(entry_path(name))
            .compression_method(rawzip::CompressionMethod::DEFLATE)
            .unix_permissions(metadata.mode())
            .last_modified(UtcDateTime::from_unix(metadata.mtime()));
        for (id, bytes) in member_extras(name, metadata) {
            file_entry = file_entry
                .extra_field(id, &bytes, Header::default())
                .map_err(io::Error::other)?;
        }
        let (mut entry, config) = file_entry.start().map_err(io::Error::other)?;
        let encoder =
            flate2::write::DeflateEncoder::new(&mut entry, flate2::Compression::default());
        let mut writer = config.wrap(encoder);
        let mut remaining = metadata.len();
        let mut input = file;
        let mut buffer = [0; 64 * 1024];
        while remaining != 0 {
            self.check_time()?;
            let amount = remaining.min(buffer.len() as u64) as usize;
            input.read_exact(&mut buffer[..amount])?;
            writer.write_all(&buffer[..amount])?;
            remaining -= amount as u64;
        }
        let (encoder, summary) = writer.finish().map_err(io::Error::other)?;
        encoder.finish()?;
        entry.finish(summary).map_err(io::Error::other)?;
        self.check_time()
    }

    fn check_sources(&self) -> io::Result<()> {
        for source in &self.observations {
            self.check_time()?;
            source.check()?;
        }
        self.check_time()
    }
}

pub(super) fn write(
    workspace: &Workspace,
    paths: &[PathBuf],
    destination: &Path,
) -> io::Result<()> {
    let mut tree = Tree::new(workspace)?;
    let mut staged = workspace.stage_output()?;
    let mut archive = ZipArchiveWriter::new(staged.as_file_mut());
    for path in paths {
        tree.visit(&mut archive, path, path, member_name(path)?)?;
    }
    if tree.observations.is_empty() {
        return Err(io::Error::other("No files to archive"));
    }
    archive.finish().map_err(io::Error::other)?;
    tree.check_sources()?;
    workspace.install_output(staged, destination)
}

fn ordinary_pin(path: &Path) -> io::Result<File> {
    let file = kernel_interface::pin(path, true)?;
    if kernel_interface::classify_handle(&file)?.protected() {
        return Err(io::Error::other("Archive input is not ordinary content"));
    }
    Ok(file)
}

fn check_identity(file: &File, expected: Identity) -> io::Result<()> {
    if identity(&file.metadata()?) != expected {
        return Err(io::Error::other("Archive source changed while processing"));
    }
    Ok(())
}

fn descriptor(file: &File) -> PathBuf {
    PathBuf::from(format!("/proc/self/fd/{}", file.as_raw_fd()))
}

fn member_name(path: &Path) -> io::Result<Vec<u8>> {
    let mut components = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(name) => components.push(name.as_bytes()),
            Component::ParentDir => {
                components.pop();
            }
            Component::RootDir | Component::CurDir => {}
            Component::Prefix(_) => {
                return Err(io::Error::other("Unsupported archive path prefix"));
            }
        }
    }
    Ok(components.join(&b'/'))
}

// Info-ZIP metadata uses rawzip's extra-field framing, CRC from the existing
// compression library, and the public ZIP specification's third-party mappings.
fn member_extras(name: &[u8], metadata: &Metadata) -> Vec<(ExtraFieldId, Vec<u8>)> {
    let mut owner = vec![1, 4]; // Version 1, four-byte UID and GID.
    owner.extend_from_slice(&metadata.uid().to_le_bytes());
    owner.push(4);
    owner.extend_from_slice(&metadata.gid().to_le_bytes());
    let mut fields = vec![(ExtraFieldId::INFO_ZIP_UNIX_UID_GID, owner)];
    if std::str::from_utf8(name).is_ok() && name.iter().any(|byte| matches!(byte, b'\\' | b':')) {
        let mut crc = flate2::Crc::new();
        crc.update(name);
        let mut unicode = vec![1];
        unicode.extend_from_slice(&crc.sum().to_le_bytes());
        unicode.extend_from_slice(name);
        fields.push((ExtraFieldId::INFO_ZIP_UNICODE_PATH, unicode));
    }
    fields
}

fn entry_path(name: &[u8]) -> EntryPath<'_> {
    // Conformant paths carry the UTF-8 flag. Preserve Unix backslashes/colons and
    // non-UTF-8 bytes verbatim rather than letting cross-platform normalization alter them.
    match std::str::from_utf8(name) {
        Ok(text) if !text.contains(['\\', ':']) && name != b"./" => EntryPath::conformant(text),
        _ => EntryPath::verbatim(name),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        ffi::OsStr,
        fs,
        io::Cursor,
        os::unix::{
            ffi::OsStrExt,
            fs::{PermissionsExt, symlink},
        },
        time::{Duration, UNIX_EPOCH},
    };

    fn create(workspace: &Workspace, paths: &[PathBuf]) -> io::Result<zip::ZipArchive<File>> {
        let output = workspace.path().join("test.zip");
        write(workspace, paths, &output)?;
        zip::ZipArchive::new(File::open(output)?).map_err(io::Error::other)
    }

    #[test]
    fn recursive_archive_preserves_raw_names_contents_empty_directories_and_metadata() {
        let fixture = tempfile::tempdir().unwrap();
        let folder = fixture.path().join("instrument");
        fs::create_dir_all(folder.join("empty")).unwrap();
        let payload = b"mapped sample data\n".repeat(10_000);
        let names: &[&[u8]] = &[
            b"sample-\xff.wav",
            "café.wav".as_bytes(),
            b"C:\\sample.wav",
            "C:\\café.wav".as_bytes(),
        ];
        for name in names {
            let path = folder.join(OsStr::from_bytes(name));
            fs::write(&path, &payload).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
            File::options()
                .write(true)
                .open(&path)
                .unwrap()
                .set_times(
                    fs::FileTimes::new()
                        .set_modified(UNIX_EPOCH + Duration::from_secs(1_700_000_000)),
                )
                .unwrap();
        }
        let workspace = Workspace::new().unwrap();
        let mut archive = create(&workspace, &[folder.clone(), folder.join("empty")]).unwrap();
        assert_eq!(archive.len(), 6); // Overlapping roots are deduplicated.
        let mut found = Vec::new();
        let prefix = member_name(&folder).unwrap();
        for i in 0..archive.len() {
            let mut entry = archive.by_index(i).unwrap();
            assert!(!entry.name_raw().starts_with(b"/"));
            if entry.is_dir() {
                continue;
            }
            if std::str::from_utf8(entry.name_raw()).is_ok() {
                assert_eq!(entry.name().as_bytes(), entry.name_raw());
            }
            assert_eq!(entry.compression(), zip::CompressionMethod::Deflated);
            assert_eq!(entry.unix_mode().unwrap() & 0o777, 0o640);
            let extras = entry.extra_data().unwrap();
            let fields = rawzip::extra_fields::ExtraFields::new(extras).collect::<Vec<_>>();
            let owner = fields
                .iter()
                .find(|(id, _)| *id == ExtraFieldId::INFO_ZIP_UNIX_UID_GID)
                .unwrap()
                .1;
            let expected = fs::metadata(&folder).unwrap();
            assert_eq!(
                owner,
                [
                    vec![1, 4],
                    expected.uid().to_le_bytes().to_vec(),
                    vec![4],
                    expected.gid().to_le_bytes().to_vec()
                ]
                .concat()
            );

            // Extended Timestamp (0x5455) with UTC mtime, independent of DOS timezone.
            assert!(
                extras
                    .windows(9)
                    .any(|field| field == [0x55, 0x54, 5, 0, 1, 0, 0xf1, 0x53, 0x65])
            );
            found.push(entry.name_raw()[prefix.len() + 1..].to_vec());
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes).unwrap(); // Independent reader verifies CRC too.
            assert_eq!(bytes, payload);
        }
        for name in names {
            assert!(found.iter().any(|actual| actual == name));
        }
    }

    #[test]
    fn ordinary_links_are_followed_but_protected_links_and_cycles_never_publish() {
        let fixture = tempfile::tempdir().unwrap();
        let target = fixture.path().join("target");
        fs::create_dir(&target).unwrap();
        fs::write(target.join("note"), b"ordinary").unwrap();
        let link = fixture.path().join("link");
        symlink(&target, &link).unwrap();
        let file_link = target.join("alias");
        symlink(target.join("note"), &file_link).unwrap();
        let workspace = Workspace::new().unwrap();
        let mut archive = create(&workspace, std::slice::from_ref(&link)).unwrap();
        assert_eq!(archive.len(), 3);
        for i in 0..archive.len() {
            let mut entry = archive.by_index(i).unwrap();
            if entry.is_dir() {
                continue;
            }
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes).unwrap();
            assert_eq!(bytes, b"ordinary");
        }
        for protected in [Path::new("/proc"), Path::new("/dev/null")] {
            let workspace = Workspace::new().unwrap();
            let blocked = target.join("blocked");
            symlink(protected, &blocked).unwrap();
            let output = workspace.path().join("blocked.zip");
            assert!(
                write(&workspace, std::slice::from_ref(&link), &output)
                    .unwrap_err()
                    .to_string()
                    .contains("ordinary content")
            );
            assert_eq!(fs::read_dir(workspace.path()).unwrap().count(), 0);
            fs::remove_file(blocked).unwrap();
        }
        symlink(&target, target.join("cycle")).unwrap();
        let workspace = Workspace::new().unwrap();
        assert!(
            create(&workspace, &[link])
                .unwrap_err()
                .to_string()
                .contains("cycle")
        );
        assert_eq!(fs::read_dir(workspace.path()).unwrap().count(), 0);
    }

    fn source_tree() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let fixture = tempfile::tempdir().unwrap();
        let folder = fixture.path().join("source");
        fs::create_dir(&folder).unwrap();
        let file = folder.join("note");
        fs::write(&file, b"old").unwrap();
        (fixture, folder, file)
    }

    #[test]
    fn final_validation_rejects_replaced_directory_and_changed_file_or_alias() {
        for replacement in ["directory", "file", "alias"] {
            let (fixture, folder, file) = source_tree();
            let alias = fixture.path().join("alias");
            symlink(&file, &alias).unwrap();
            let workspace = Workspace::new().unwrap();
            let mut tree = Tree::new(&workspace).unwrap();
            let mut archive = ZipArchiveWriter::new(Cursor::new(Vec::new()));
            tree.visit(&mut archive, &folder, &folder, b"source".to_vec())
                .unwrap();
            tree.visit(&mut archive, &alias, &alias, b"alias".to_vec())
                .unwrap();
            archive.finish().unwrap();
            match replacement {
                "directory" => {
                    fs::rename(&folder, fixture.path().join("retired")).unwrap();
                    fs::create_dir(&folder).unwrap();
                    fs::write(&file, b"old").unwrap();
                }
                "file" => fs::write(&file, b"new").unwrap(),
                _ => {
                    fs::remove_file(&alias).unwrap();
                    symlink("/proc/self/status", &alias).unwrap();
                }
            }
            assert!(tree.check_sources().is_err(), "{replacement}");
        }
    }

    struct ChangeOnWrite<F> {
        bytes: Cursor<Vec<u8>>,
        change: Option<F>,
    }
    impl<F: FnOnce()> Write for ChangeOnWrite<F> {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if let Some(change) = self.change.take() {
                change();
            }
            self.bytes.write(bytes)
        }
        fn flush(&mut self) -> io::Result<()> {
            self.bytes.flush()
        }
    }

    #[test]
    fn changes_during_payload_or_directory_writes_reject_the_archive() {
        for kind in ["in-place", "replaced-file", "replaced-directory"] {
            let (fixture, folder, file) = source_tree();
            let input = if kind == "replaced-directory" {
                &folder
            } else {
                &file
            };
            let writer = ChangeOnWrite {
                bytes: Cursor::new(Vec::new()),
                change: Some(|| match kind {
                    "in-place" => fs::write(&file, b"new payload").unwrap(),
                    "replaced-file" => {
                        fs::remove_file(&file).unwrap();
                        symlink("/proc/self/status", &file).unwrap();
                    }
                    _ => {
                        fs::rename(&folder, fixture.path().join("retired")).unwrap();
                        symlink("/proc", &folder).unwrap();
                    }
                }),
            };
            let workspace = Workspace::new().unwrap();
            let mut tree = Tree::new(&workspace).unwrap();
            let mut archive = ZipArchiveWriter::new(writer);
            assert!(
                tree.visit(&mut archive, input, input, b"input".to_vec())
                    .is_err(),
                "{kind}"
            );
        }
    }

    #[test]
    fn bounded_traversal_checks_deadlines_entry_counts_and_member_collisions() {
        let fixture = tempfile::tempdir().unwrap();
        fs::write(fixture.path().join("first"), b"a").unwrap();
        fs::write(fixture.path().join("second"), b"b").unwrap();
        for policy in ["deadline", "count", "name", "collision"] {
            let workspace = Workspace::new().unwrap();
            let mut tree = Tree::new(&workspace).unwrap();
            let mut archive = ZipArchiveWriter::new(Cursor::new(Vec::new()));
            match policy {
                "deadline" => tree.deadline = Instant::now(),
                "count" => tree.entry_limit = 0,
                "name" => tree.name_bytes = MAX_NAMES,
                _ => tree
                    .visit(
                        &mut archive,
                        &fixture.path().join("first"),
                        &fixture.path().join("first"),
                        b"same".to_vec(),
                    )
                    .unwrap(),
            }
            assert!(
                tree.visit(
                    &mut archive,
                    &fixture.path().join("second"),
                    &fixture.path().join("second"),
                    b"same".to_vec()
                )
                .is_err(),
                "{policy}"
            );
        }
    }

    #[test]
    fn deeply_nested_directories_fail_at_the_owned_depth_budget() {
        let fixture = tempfile::tempdir().unwrap();
        let workspace = Workspace::new().unwrap();
        let mut tree = Tree::new(&workspace).unwrap();
        tree.ancestors = vec![(0, 0); MAX_DEPTH];
        let mut archive = ZipArchiveWriter::new(Cursor::new(Vec::new()));
        assert!(
            tree.visit(
                &mut archive,
                fixture.path(),
                fixture.path(),
                b"root".to_vec()
            )
            .unwrap_err()
            .to_string()
            .contains("depth limit")
        );
    }

    #[test]
    fn output_collision_preserves_previous_bytes_and_workspace_is_excluded() {
        let fixture = tempfile::tempdir().unwrap();
        let workspace = Workspace::in_parent(fixture.path()).unwrap();
        fs::write(fixture.path().join("source"), b"useful").unwrap();
        let archive = create(&workspace, &[fixture.path().to_owned()]).unwrap();
        assert_eq!(archive.len(), 2); // Parent and source, with no stage/workspace entries.
        let output = workspace.path().join("existing.zip");
        fs::write(&output, b"previous bytes").unwrap();
        assert!(write(&workspace, &[fixture.path().join("source")], &output).is_err());
        assert_eq!(fs::read(output).unwrap(), b"previous bytes");
        assert_eq!(fs::read_dir(workspace.path()).unwrap().count(), 2);
    }
}
