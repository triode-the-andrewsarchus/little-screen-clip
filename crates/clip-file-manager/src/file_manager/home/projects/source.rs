//! Portable project metadata; every content operation is relative to held directories.
use crate::file_manager::local_state;
use little_clip_core::{
    file_source::{self, FileSource},
    kernel_interface,
};
use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    os::fd::AsRawFd,
    path::{Component, Path, PathBuf},
};
use uuid::Uuid;

pub(super) const MANIFEST: &str = ".fmproject";
const LIMIT: usize = 32 * 1024;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::file_manager) struct Manifest {
    pub version: u32,
    pub id: Uuid,
    pub title: String,
    #[serde(default)]
    pub description: String,
    pub readme: String,
    pub todo: String,
}
impl Manifest {
    pub fn validate(&self) -> Result<(), String> {
        if self.version != 1 || self.id.is_nil() {
            return Err(
                "Unsupported project version or invalid identity; manifest left unchanged".into(),
            );
        }
        if self.title.trim().is_empty() || self.title.len() > 256 || self.description.len() > 4096 {
            return Err("Project title/description exceeds its budget".into());
        }
        for path in [&self.readme, &self.todo] {
            relative_document(path)?;
        }
        if Path::new(&self.readme) == Path::new(&self.todo) {
            return Err("README and TODO must refer to different documents".into());
        }
        Ok(())
    }
}

fn relative_document(value: &str) -> Result<(), String> {
    let path = Path::new(value);
    if value.is_empty()
        || value.len() > 1024
        || value.contains('\0')
        || path.components().count() > 8
        || path
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
        || path
            .file_name()
            .is_none_or(|name| name == MANIFEST || name == ".fmproject.lock")
    {
        return Err("Project documents must be ordinary relative paths inside the project".into());
    }
    Ok(())
}

pub(super) fn held(directory: &File) -> PathBuf {
    PathBuf::from(format!("/proc/self/fd/{}", directory.as_raw_fd()))
}

/// Traverses normal components without following an alias outside the project.
pub(super) fn document_parent(root: &File, relative: &str) -> Result<File, String> {
    relative_document(relative)?;
    let mut parent = root.try_clone().map_err(|e| e.to_string())?;
    let components: Vec<_> = Path::new(relative).components().collect();
    for component in &components[..components.len() - 1] {
        let child = kernel_interface::pin(&held(&parent).join(component.as_os_str()), false)
            .map_err(|e| e.to_string())?;
        if !child.metadata().map_err(|e| e.to_string())?.is_dir()
            || kernel_interface::classify_handle(&child)
                .map_err(|e| e.to_string())?
                .protected()
        {
            return Err(
                "Project document parent is not an ordinary directory; symlinks are not followed"
                    .into(),
            );
        }
        parent = child;
    }
    Ok(parent)
}

pub(super) fn read(root: &File) -> Result<(Manifest, FileSource), String> {
    let (manifest, source) = local_state::read::<Manifest>(&held(root).join(MANIFEST), LIMIT)?;
    let source = source.ok_or("This folder has no .fmproject manifest")?;
    manifest.validate()?;
    document_parent(root, &manifest.readme)?;
    document_parent(root, &manifest.todo)?;
    Ok((manifest, source))
}

pub(super) fn create(folder: &Path, title: &str) -> Result<Manifest, String> {
    let root = kernel_interface::pin_content_directory(folder).map_err(|e| e.to_string())?;
    let _lock = local_state::lock(&root, ".fmproject.lock")?;
    let target = held(&root).join(MANIFEST);
    match file_source::open_regular_file(&target, false) {
        Ok(_) => return Err("A .fmproject already exists; register it instead".into()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.to_string()),
    }
    let manifest = Manifest {
        version: 1,
        id: Uuid::new_v4(),
        title: title.trim().to_owned(),
        description: String::new(),
        readme: existing_name(&root, "README.md", "README")?,
        todo: existing_name(&root, "TODO.md", "TODO")?,
    };
    manifest.validate()?;
    let readme = format!("# {}\n", manifest.title);
    let result = (|| {
        ensure_document(&root, &manifest.readme, readme.as_bytes())?;
        ensure_document(&root, &manifest.todo, b"# TODO\n\n- [ ] First task\n")?;
        let bytes = serde_json::to_vec_pretty(&manifest).map_err(|e| e.to_string())?;
        file_source::create_file_in(&target, &bytes, &root)?;
        Ok::<_, String>(manifest)
    })();
    result.map_err(|e| format!("Project creation did not finish: {e}. Existing files were preserved; any newly created README/TODO files remain in the folder."))
}

fn existing_name(root: &File, preferred: &str, alternate: &str) -> Result<String, String> {
    for name in [preferred, alternate] {
        match file_source::open_regular_file(&held(root).join(name), false) {
            Ok(_) => return Ok(name.into()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("{name}: {e}")),
        }
    }
    Ok(preferred.into())
}
fn ensure_document(root: &File, name: &str, bytes: &[u8]) -> Result<(), String> {
    let target = held(root).join(name);
    match file_source::open_regular_file(&target, false) {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            file_source::create_file_in(&target, bytes, root).map(|_| ())
        }
        Err(e) => Err(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn aliases_and_future_manifests_are_preserved_without_creating_defaults() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(outside.path(), "untouched").unwrap();
        std::os::unix::fs::symlink(outside.path(), root.path().join("TODO.md")).unwrap();
        assert!(create(root.path(), "Work").is_err());
        assert!(!root.path().join("README.md").exists());
        assert!(!root.path().join(MANIFEST).exists());
        assert_eq!(
            std::fs::read_to_string(outside.path()).unwrap(),
            "untouched"
        );
        let bytes = br#"{"version":99,"id":"00000000-0000-0000-0000-000000000000","title":"Future","readme":"README.md","todo":"TODO.md"}"#;
        std::fs::write(root.path().join(MANIFEST), bytes).unwrap();
        let directory = kernel_interface::pin_content_directory(root.path()).unwrap();
        assert!(read(&directory).is_err());
        assert!(create(root.path(), "Replace").is_err());
        assert_eq!(std::fs::read(root.path().join(MANIFEST)).unwrap(), bytes);
    }
    #[test]
    fn creation_preserves_existing_documents_and_refuses_repeated_conversion() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("README.md"), "My existing readme\r\n").unwrap();
        std::fs::write(root.path().join("TODO"), "- [ ] My task\r\n").unwrap();
        let manifest = create(root.path(), " Work ").unwrap();
        assert_eq!(manifest.todo, "TODO");
        assert_eq!(manifest.title, "Work");
        assert_eq!(
            std::fs::read(root.path().join("TODO")).unwrap(),
            b"- [ ] My task\r\n"
        );
        assert_eq!(
            std::fs::read(root.path().join("README.md")).unwrap(),
            b"My existing readme\r\n"
        );
        assert!(!root.path().join("TODO.md").exists());
        assert!(create(root.path(), "Replacement").is_err());
        let directory = kernel_interface::pin_content_directory(root.path()).unwrap();
        assert_eq!(read(&directory).unwrap().0.id, manifest.id);
    }
    #[test]
    fn new_project_has_defaults_but_never_accepts_aliases_or_escaping_document_paths() {
        let root = tempfile::tempdir().unwrap();
        create(root.path(), "New project").unwrap();
        assert!(root.path().join("README.md").is_file());
        assert!(
            std::fs::read_to_string(root.path().join("TODO.md"))
                .unwrap()
                .contains("- [ ]")
        );
        let directory = kernel_interface::pin_content_directory(root.path()).unwrap();
        for invalid in [
            "",
            "../TODO.md",
            "/tmp/TODO.md",
            "x/../../TODO",
            ".fmproject",
            "TODO\0.md",
        ] {
            assert!(document_parent(&directory, invalid).is_err(), "{invalid:?}");
        }
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), root.path().join("alias")).unwrap();
        assert!(document_parent(&directory, "alias/TODO.md").is_err());
        std::fs::create_dir(root.path().join("docs")).unwrap();
        assert!(document_parent(&directory, "docs/TODO.md").is_ok());
    }
}
