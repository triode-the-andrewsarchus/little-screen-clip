//! Per-execution scratch space and publication of final path-backed outputs.

use super::types::{PipelineExecutionResult, PortValue};
use little_clip_core::ArtifactStore;
use little_clip_core::file_source::{open_regular_file, rename_at_noreplace};
use little_clip_core::item::{ClipItem, ClipPayload, image_extension_for_mime};
use little_clip_core::kernel_interface::pin_content_directory;
use little_clip_core::staging::StagingError;
use std::collections::HashMap;
use std::fs::File;
use std::io;
use std::io::Write;
use std::os::fd::AsRawFd;
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};

pub(super) struct Workspace {
    // Cleanup runs through the held parent, even if its pathname is replaced.
    directory: tempfile::TempDir,
    parent: File,
    root: File,
    path: PathBuf,
}

impl Workspace {
    pub(super) fn new() -> io::Result<Self> {
        ArtifactStore::new()
            .map_err(io::Error::other)
            .and_then(|store| Self::in_parent(store.base_dir()))
            .or_else(|_| Self::in_parent(&std::env::temp_dir()))
    }

    pub(super) fn in_parent(parent: &Path) -> io::Result<Self> {
        let parent = pin_content_directory(parent)?;
        Self::create_in(&parent, "pipeline-run-")
    }

    fn create_in(parent: &File, prefix: &str) -> io::Result<Self> {
        let parent = parent.try_clone()?;
        let directory = tempfile::Builder::new()
            .prefix(prefix)
            .tempdir_in(descriptor_path(&parent))?;
        let root = pin_content_directory(directory.path())?;
        let path = std::fs::read_link(descriptor_path(&root))?;
        Ok(Self {
            directory,
            parent,
            root,
            path,
        })
    }

    pub(super) fn path(&self) -> &Path {
        &self.path
    }

    pub(super) fn identity(&self) -> io::Result<(u64, u64)> {
        let metadata = self.root.metadata()?;
        Ok((metadata.dev(), metadata.ino()))
    }

    pub(super) fn stage_output(&self) -> io::Result<tempfile::NamedTempFile> {
        tempfile::NamedTempFile::new_in(descriptor_path(&self.root))
    }

    pub(super) fn install_output(
        &self,
        staged: tempfile::NamedTempFile,
        path: &Path,
    ) -> io::Result<()> {
        let name = self.output_name(path)?;
        let named = pin_content_directory(&self.path)?.metadata()?;
        let held = self.root.metadata()?;
        if (named.dev(), named.ino()) != (held.dev(), held.ino()) {
            return Err(io::Error::other(
                "Pipeline workspace changed before output installation",
            ));
        }
        staged.as_file().sync_all()?;
        staged.persist_noclobber(descriptor_path(&self.root).join(name))?;
        Ok(())
    }

    pub(super) fn write_output(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        let mut staged = self.stage_output()?;
        staged.write_all(bytes)?;
        self.install_output(staged, path)
    }

    pub(super) fn source_value(&self, item: &ClipItem) -> Result<PortValue, StagingError> {
        if let ClipPayload::Image {
            bytes,
            mime,
            path: None,
            ..
        } = &item.payload
        {
            let extension = image_extension_for_mime(mime).unwrap_or("bin");
            let name = format!("input_{}.{}", uuid::Uuid::new_v4(), extension);
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(descriptor_path(&self.root).join(&name))?;
            file.write_all(bytes)?;
            file.sync_all()?;
            Ok(PortValue::ImagePath(self.path.join(name)))
        } else {
            PortValue::from_clip_item(item)
        }
    }

    pub(super) fn retain_outputs(&self, result: &mut PipelineExecutionResult) -> io::Result<()> {
        let mut paths = Vec::new();
        for item in result
            .left_hand_item
            .iter_mut()
            .chain(result.right_hand_item.iter_mut())
        {
            match &mut item.payload {
                ClipPayload::Image {
                    path: Some(path), ..
                }
                | ClipPayload::AudioRef { path, .. }
                | ClipPayload::Animation { path, .. }
                | ClipPayload::Directory(path) => paths.push(path),
                ClipPayload::FileBatch(batch) | ClipPayload::MultiImage { paths: batch, .. } => {
                    paths.extend(batch)
                }
                _ => {}
            }
        }
        paths.retain(|path| path.starts_with(&self.path));
        if paths.is_empty() {
            return Ok(());
        }

        let retained = Self::create_in(&self.parent, "pipeline-output-")?;
        let mut published = HashMap::new();
        for path in &paths {
            if published.contains_key(path.as_path()) {
                continue;
            }
            let installed_path = self.move_output(path, &retained)?;
            published.insert(path.to_path_buf(), installed_path);
        }
        for path in paths {
            *path = published[path.as_path()].clone();
        }
        let mut retained = retained;
        retained.directory.disable_cleanup(true);
        Ok(())
    }

    fn output_name<'p>(&self, path: &'p Path) -> io::Result<&'p std::ffi::OsStr> {
        let relative = path.strip_prefix(&self.path).map_err(io::Error::other)?;
        let mut components = relative.components();
        match (components.next(), components.next()) {
            (Some(Component::Normal(name)), None) => Ok(name),
            _ => Err(io::Error::other(
                "Pipeline output must be a workspace filename",
            )),
        }
    }

    // One file publication: no overwrite, no byte copy, and identity checked
    // against the retained source handle. The destination owner rolls back on error.
    fn move_output(&self, path: &Path, destination: &Self) -> io::Result<PathBuf> {
        let name = self.output_name(path)?;
        let original = open_regular_file(&descriptor_path(&self.root).join(name), false)?;
        rename_at_noreplace(
            self.root.as_raw_fd(),
            name,
            destination.root.as_raw_fd(),
            name,
        )?;
        let installed = open_regular_file(&descriptor_path(&destination.root).join(name), false)?;
        let before = original.metadata()?;
        let after = installed.metadata()?;
        if (before.dev(), before.ino()) != (after.dev(), after.ino()) {
            return Err(io::Error::other(
                "Pipeline output changed during publication",
            ));
        }
        std::fs::read_link(descriptor_path(&installed))
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        let owned = self.root.metadata().and_then(|original| {
            let named = std::fs::symlink_metadata(self.directory.path())?;
            Ok((original.dev(), original.ino()) == (named.dev(), named.ino()))
        });
        if !owned.unwrap_or(false) {
            self.directory.disable_cleanup(true);
        }
    }
}

fn descriptor_path(file: &File) -> PathBuf {
    PathBuf::from(format!("/proc/self/fd/{}", file.as_raw_fd()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn staged_outputs_never_replace_existing_workspace_entries_or_escape() {
        let fixture = tempfile::tempdir().unwrap();
        let workspace = Workspace::in_parent(fixture.path()).unwrap();
        let path = workspace.path().join("output.png");
        workspace.write_output(&path, b"first output").unwrap();
        assert!(workspace.write_output(&path, b"replacement").is_err());
        assert_eq!(fs::read(&path).unwrap(), b"first output");
        assert!(
            workspace
                .write_output(&workspace.path().join("../outside"), b"escape")
                .is_err()
        );
        assert!(!fixture.path().join("outside").exists());
        assert_eq!(fs::read_dir(workspace.path()).unwrap().count(), 1);
    }

    #[test]
    fn replaced_workspace_cannot_redirect_tool_output_installation() {
        let fixture = tempfile::tempdir().unwrap();
        let workspace = Workspace::in_parent(fixture.path()).unwrap();
        let path = workspace.path().join("output.png");
        let mut staged = workspace.stage_output().unwrap();
        staged.write_all(b"new output").unwrap();
        let moved = fixture.path().join("moved");
        fs::rename(workspace.path(), &moved).unwrap();
        fs::create_dir(workspace.path()).unwrap();
        fs::write(&path, b"unrelated replacement").unwrap();
        assert!(workspace.install_output(staged, &path).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"unrelated replacement");
        assert_eq!(fs::read_dir(&moved).unwrap().count(), 0);
    }

    #[test]
    fn publication_collision_preserves_source_and_existing_destination() {
        let fixture = tempfile::tempdir().unwrap();
        let source = Workspace::in_parent(fixture.path()).unwrap();
        let destination = Workspace::in_parent(fixture.path()).unwrap();
        let input = source.path().join("output.bin");
        let existing = destination.path().join("output.bin");
        fs::write(&input, b"new output").unwrap();
        fs::write(&existing, b"existing output").unwrap();
        assert_eq!(
            source.move_output(&input, &destination).unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(fs::read(&input).unwrap(), b"new output");
        assert_eq!(fs::read(&existing).unwrap(), b"existing output");
    }

    #[test]
    fn publication_keeps_only_final_references_without_copying_bytes() {
        let fixture = tempfile::tempdir().unwrap();
        let workspace = Workspace::in_parent(fixture.path()).unwrap();
        let scratch = workspace.path().to_owned();
        let output = scratch.join("final.wav");
        fs::write(&output, b"output").unwrap();
        let inode = fs::metadata(&output).unwrap().ino();
        fs::write(scratch.join("intermediate.wav"), b"intermediate").unwrap();
        let external = fixture.path().join("external.wav");
        fs::write(&external, b"original").unwrap();
        let mut audio = ClipItem::from_text("");
        audio.payload = ClipPayload::AudioRef {
            path: output.clone(),
            duration_ms: 0,
            sample_rate: 1,
            channels: 1,
            format: "wav".into(),
            waveform: None,
        };
        let mut result = PipelineExecutionResult {
            left_hand_item: Some(audio),
            right_hand_item: Some(ClipItem::from_files(vec![
                output.clone(),
                external.clone(),
                output,
            ])),
            ..Default::default()
        };
        workspace.retain_outputs(&mut result).unwrap();
        let retained = result.left_hand_item.as_ref().unwrap().file_path().unwrap();
        assert_eq!(fs::metadata(&retained).unwrap().ino(), inode);
        assert_eq!(
            result.right_hand_item.unwrap().payload,
            ClipPayload::FileBatch(vec![retained.clone(), external.clone(), retained.clone()])
        );
        drop(workspace);
        assert!(!scratch.exists());
        assert_eq!(fs::read(&retained).unwrap(), b"output");
        assert_eq!(fs::read(&external).unwrap(), b"original");
        assert_eq!(fs::read_dir(retained.parent().unwrap()).unwrap().count(), 1);
    }

    #[test]
    fn failed_publication_removes_partial_outputs_and_scratch() {
        let fixture = tempfile::tempdir().unwrap();
        let workspace = Workspace::in_parent(fixture.path()).unwrap();
        let output = workspace.path().join("output.bin");
        fs::write(&output, b"partial").unwrap();
        let mut result = PipelineExecutionResult {
            left_hand_item: Some(ClipItem::from_files(vec![
                output,
                workspace.path().join("missing.bin"),
            ])),
            ..Default::default()
        };
        assert!(workspace.retain_outputs(&mut result).is_err());
        assert_eq!(fs::read_dir(fixture.path()).unwrap().count(), 1);
        drop(workspace);
        assert_eq!(fs::read_dir(fixture.path()).unwrap().count(), 0);
    }

    #[test]
    fn substituted_output_symlink_is_not_published() {
        let fixture = tempfile::tempdir().unwrap();
        let workspace = Workspace::in_parent(fixture.path()).unwrap();
        let external = fixture.path().join("external.bin");
        fs::write(&external, b"original").unwrap();
        let output = workspace.path().join("output.bin");
        std::os::unix::fs::symlink(&external, &output).unwrap();
        let mut result = PipelineExecutionResult {
            left_hand_item: Some(ClipItem::from_files(vec![output])),
            ..Default::default()
        };
        assert!(workspace.retain_outputs(&mut result).is_err());
        drop(workspace);
        assert_eq!(fs::read(&external).unwrap(), b"original");
        assert_eq!(fs::read_dir(fixture.path()).unwrap().count(), 1);
    }

    #[test]
    fn cleanup_uses_held_parent_and_leaves_replacement_workspaces_alone() {
        let fixture = tempfile::tempdir().unwrap();
        let parent = fixture.path().join("parent");
        fs::create_dir(&parent).unwrap();
        let workspace = Workspace::in_parent(&parent).unwrap();
        let old = workspace.path().to_owned();
        let moved_parent = fixture.path().join("moved-parent");
        fs::rename(&parent, &moved_parent).unwrap();
        fs::create_dir_all(&old).unwrap();
        fs::write(old.join("foreign.bin"), b"untouched").unwrap();
        drop(workspace);
        assert_eq!(fs::read_dir(&moved_parent).unwrap().count(), 0);
        assert_eq!(fs::read(old.join("foreign.bin")).unwrap(), b"untouched");

        let workspace = Workspace::in_parent(&parent).unwrap();
        let old = workspace.path().to_owned();
        fs::rename(&old, fixture.path().join("moved-workspace")).unwrap();
        fs::create_dir(&old).unwrap();
        fs::write(old.join("foreign.bin"), b"untouched").unwrap();
        drop(workspace);
        assert_eq!(fs::read(old.join("foreign.bin")).unwrap(), b"untouched");
    }

    #[test]
    fn materialized_image_inputs_are_owned_by_the_run() {
        let fixture = tempfile::tempdir().unwrap();
        let workspace = Workspace::in_parent(fixture.path()).unwrap();
        let mut item = ClipItem::from_text("");
        item.payload = ClipPayload::Image {
            bytes: vec![1, 2, 3],
            path: None,
            width: 1,
            height: 1,
            mime: "image/png".into(),
            thumbnail: None,
        };
        let input = workspace.source_value(&item).unwrap();
        assert_eq!(fs::read(input.as_path().unwrap()).unwrap(), [1, 2, 3]);
        workspace
            .retain_outputs(&mut PipelineExecutionResult::default())
            .unwrap();
        drop(workspace);
        assert_eq!(fs::read_dir(fixture.path()).unwrap().count(), 0);
        assert!(Workspace::in_parent(&fixture.path().join("missing")).is_err());
    }
}
