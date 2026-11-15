//! Bounded source-backed task snapshots; checkbox writes change one verified byte.
use super::{Record, source};
use little_clip_core::{
    file_source::{self, FileSource},
    kernel_interface,
};
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
use std::{fs::File, io::Read, path::Path, sync::Arc};

const LIMIT: usize = 256 * 1024;
const TASKS: usize = 2048;
#[derive(Clone, Debug)]
pub(in crate::file_manager) struct Item {
    pub label: String,
    pub checked: bool,
    marker: usize,
}
pub(in crate::file_manager) struct Snapshot {
    pub project: Record,
    pub title: String,
    pub tasks: Vec<Item>,
    pub truncated: bool,
    pub path: std::path::PathBuf,
    bytes: Arc<[u8]>,
    source: FileSource,
    manifest: FileSource,
    directory: File,
    root: File,
}
impl Snapshot {
    pub fn load(project: Record) -> Result<Self, String> {
        let root =
            kernel_interface::pin_content_directory(&project.folder).map_err(|e| e.to_string())?;
        let (manifest, identity) = source::read(&root)?;
        if manifest.id != project.id {
            return Err(
                "This folder now belongs to a different project; register it explicitly".into(),
            );
        }
        let directory = source::document_parent(&root, &manifest.todo)?;
        let target = source::held(&directory).join(
            Path::new(&manifest.todo)
                .file_name()
                .ok_or("Missing TODO filename")?,
        );
        let mut file = file_source::open_regular_file(&target, false).map_err(|e| e.to_string())?;
        let source = FileSource::capture_file(&file)?;
        let mut bytes = Vec::new();
        file.by_ref()
            .take(LIMIT as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        if bytes.len() > LIMIT {
            return Err("TODO preview exceeds 256 KiB; open the document to edit it".into());
        }
        source.check_file(&file)?;
        identity.check()?;
        let (tasks, truncated) = parse(&bytes)?;
        Ok(Self {
            project,
            title: manifest.title,
            tasks,
            truncated,
            path: source.path().to_path_buf(),
            bytes: bytes.into(),
            source,
            manifest: identity,
            directory,
            root,
        })
    }
    pub fn toggle(&self, index: usize, checked: bool) -> Result<Self, String> {
        let task = self.tasks.get(index).ok_or("Task is no longer available")?;
        let _lock = crate::file_manager::local_state::lock(&self.root, ".fmproject.lock")?;
        self.manifest.check()?;
        let (manifest, _) = source::read(&self.root)?;
        if manifest.id != self.project.id {
            return Err("Project identity changed; reload before editing tasks".into());
        }
        let mut bytes = self.bytes.to_vec();
        let marker = bytes
            .get_mut(task.marker)
            .ok_or("Task marker changed; reload the document")?;
        if !matches!(*marker, b' ' | b'x' | b'X') {
            return Err("Task marker changed; reload the document".into());
        }
        *marker = if checked { b'x' } else { b' ' };
        let (tasks, truncated) = parse(&bytes)?;
        let directory = self.directory.try_clone().map_err(|e| e.to_string())?;
        let root = self.root.try_clone().map_err(|e| e.to_string())?;
        let installed = self
            .source
            .write_in(self.source.path(), &bytes, &self.directory)?;
        Ok(Self {
            project: self.project.clone(),
            title: self.title.clone(),
            tasks,
            truncated,
            path: installed.path().to_path_buf(),
            bytes: bytes.into(),
            source: installed,
            manifest: self.manifest.clone(),
            directory,
            root,
        })
    }
}
fn parse(bytes: &[u8]) -> Result<(Vec<Item>, bool), String> {
    let text =
        std::str::from_utf8(bytes).map_err(|_| "TODO is not UTF-8 text; open it in an editor")?;
    let mut tasks = Vec::new();
    let mut current: Option<Item> = None;
    let mut truncated = false;
    for (event, range) in Parser::new_ext(text, Options::ENABLE_TASKLISTS).into_offset_iter() {
        match event {
            Event::TaskListMarker(checked) => {
                if tasks.len() >= TASKS {
                    truncated = true;
                    break;
                }
                let raw = &bytes[range.clone()];
                let bracket = raw
                    .iter()
                    .position(|byte| *byte == b'[')
                    .ok_or("Unrecognized task marker")?;
                if raw.get(bracket + 2) != Some(&b']') {
                    return Err("Unrecognized task marker".into());
                }
                current = Some(Item {
                    label: String::new(),
                    checked,
                    marker: range.start + bracket + 1,
                });
            }
            Event::Text(label) | Event::Code(label) => {
                if let Some(task) = &mut current {
                    task.label.extend(
                        label
                            .chars()
                            .take(512usize.saturating_sub(task.label.chars().count())),
                    );
                }
            }
            Event::SoftBreak | Event::HardBreak => {
                if let Some(task) = &mut current
                    && task.label.chars().count() < 512
                {
                    task.label.push(' ');
                }
            }
            Event::End(TagEnd::Item) | Event::Start(Tag::List(_)) => {
                if let Some(mut task) = current.take() {
                    task.label = task.label.trim().to_owned();
                    tasks.push(task);
                }
            }
            _ => {}
        }
    }
    if let Some(task) = current {
        tasks.push(task);
    }
    Ok((tasks, truncated))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture(root: &Path) -> Record {
        let manifest = source::create(root, "Tasks").unwrap();
        Record {
            id: manifest.id,
            title: manifest.title,
            folder: root.into(),
        }
    }
    #[test]
    fn extensionless_todos_use_the_manifest_and_parent_replacement_cannot_redirect_writes() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("notes")).unwrap();
        let record = fixture(root.path());
        let path = root.path().join(source::MANIFEST);
        let mut manifest: source::Manifest =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        manifest.todo = "notes/TODO".into();
        std::fs::write(&path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        let bytes = b"- [ ] Read the field notes\r\n";
        std::fs::write(root.path().join("notes/TODO"), bytes).unwrap();
        let snapshot = Snapshot::load(record).unwrap();
        assert_eq!(snapshot.tasks[0].label, "Read the field notes");
        std::fs::rename(root.path().join("notes"), root.path().join("moved-notes")).unwrap();
        std::fs::create_dir(root.path().join("notes")).unwrap();
        let replacement = b"- [ ] Different source\n";
        std::fs::write(root.path().join("notes/TODO"), replacement).unwrap();
        assert!(snapshot.toggle(0, true).is_err());
        assert_eq!(
            std::fs::read(root.path().join("notes/TODO")).unwrap(),
            replacement
        );
        assert_eq!(
            std::fs::read(root.path().join("moved-notes/TODO")).unwrap(),
            bytes
        );
    }
    #[test]
    fn markdown_tasks_ignore_code_and_change_only_the_requested_marker_byte() {
        let root = tempfile::tempdir().unwrap();
        let record = fixture(root.path());
        let bytes = "# Café\r\n\r\n- [ ] **Résumé** [link](https://example.test)\r\n  - [X] Nested `code`\r\n\r\n```md\r\n- [ ] Not a task\r\n```\r\n".as_bytes();
        std::fs::write(root.path().join("TODO.md"), bytes).unwrap();
        let before = Snapshot::load(record).unwrap();
        assert_eq!(before.tasks.len(), 2);
        assert_eq!(before.tasks[0].label, "Résumé link");
        assert_eq!(before.tasks[1].label, "Nested code");
        let after = before.toggle(0, true).unwrap();
        let output = std::fs::read(root.path().join("TODO.md")).unwrap();
        let changed: Vec<_> = bytes
            .iter()
            .zip(&output)
            .enumerate()
            .filter(|(_, (a, b))| a != b)
            .map(|(index, _)| index)
            .collect();
        assert_eq!(changed, [before.tasks[0].marker]);
        assert!(after.tasks[0].checked);
        assert_eq!(after.toggle(0, false).unwrap().bytes.as_ref(), bytes);
    }
    #[test]
    fn stale_tasks_or_manifests_never_replace_external_edits_or_other_projects() {
        let root = tempfile::tempdir().unwrap();
        let record = fixture(root.path());
        let old = Snapshot::load(record.clone()).unwrap();
        let changed = b"- [ ] Changed by another editor\n";
        std::fs::write(root.path().join("TODO.md"), changed).unwrap();
        assert!(old.toggle(0, true).is_err());
        assert_eq!(std::fs::read(root.path().join("TODO.md")).unwrap(), changed);
        let old = Snapshot::load(record.clone()).unwrap();
        let path = root.path().join(source::MANIFEST);
        let mut manifest: source::Manifest =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        manifest.id = uuid::Uuid::new_v4();
        std::fs::write(path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        assert!(old.toggle(0, true).is_err());
        assert!(Snapshot::load(record).is_err());
        assert_eq!(std::fs::read(root.path().join("TODO.md")).unwrap(), changed);
    }
    #[test]
    fn aliases_are_never_task_payloads_and_oversized_or_missing_inputs_fail_explicitly() {
        let root = tempfile::tempdir().unwrap();
        let record = fixture(root.path());
        let todo = root.path().join("TODO.md");
        std::fs::remove_file(&todo).unwrap();
        assert!(Snapshot::load(record.clone()).is_err());
        let outside = tempfile::NamedTempFile::new().unwrap();
        std::os::unix::fs::symlink(outside.path(), &todo).unwrap();
        assert!(Snapshot::load(record.clone()).is_err());
        std::fs::remove_file(&todo).unwrap();
        std::fs::write(&todo, vec![b'a'; LIMIT + 1]).unwrap();
        assert!(Snapshot::load(record).is_err());
    }
}
