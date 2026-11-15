//! Local project references merge under the shared guarded state transaction.
use super::source;
use crate::file_manager::local_state;
use little_clip_core::kernel_interface;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    os::unix::ffi::OsStrExt,
    path::{Component, Path, PathBuf},
};
use uuid::Uuid;

pub(super) const LIMIT: usize = 2 * 1024 * 1024;
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::file_manager) struct Record {
    pub id: Uuid,
    pub title: String,
    #[serde(with = "little_clip_core::path_serde")]
    pub folder: PathBuf,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct State {
    version: u32,
    pub projects: Vec<Record>,
}
impl Default for State {
    fn default() -> Self {
        Self {
            version: 1,
            projects: Vec::new(),
        }
    }
}
impl State {
    pub fn validate(&self) -> Result<(), String> {
        if self.version != 1 || self.projects.len() > 512 {
            return Err("Unsupported or oversized project registry; file left unchanged".into());
        }
        let mut ids = BTreeSet::new();
        let mut paths = BTreeSet::new();
        for project in &self.projects {
            let path = &project.folder;
            if project.id.is_nil()
                || !ids.insert(project.id)
                || !paths.insert(path)
                || project.title.trim().is_empty()
                || project.title.len() > 256
                || !path.is_absolute()
                || path.as_os_str().as_bytes().len() > 4096
                || path.as_os_str().as_bytes().contains(&0)
                || path.components().any(|c| matches!(c, Component::ParentDir))
            {
                return Err("Invalid or duplicate project reference".into());
            }
        }
        Ok(())
    }
}
#[derive(Clone)]
pub(in crate::file_manager) struct Conflict {
    pub project: Record,
    pub previous: PathBuf,
}
pub(super) enum Outcome {
    State(State),
    Conflict(Conflict),
}

pub(super) fn load(path: &Path) -> Result<State, String> {
    let (state, _) = local_state::read::<State>(path, LIMIT)?;
    state.validate()?;
    Ok(state)
}

pub(super) fn register(
    path: &Path,
    folder: &Path,
    title: Option<&str>,
    replace: Option<&Path>,
) -> Result<Outcome, String> {
    let created = title
        .map(|title| source::create(folder, title))
        .transpose()?;
    let root = kernel_interface::pin_content_directory(folder).map_err(|e| e.to_string())?;
    let (manifest, identity) = source::read(&root)?;
    if created.is_some_and(|created| created.id != manifest.id) {
        return Err(
            "Project folder changed after creation; register the intended folder explicitly".into(),
        );
    }
    identity.check()?;
    let folder = std::fs::read_link(source::held(&root)).map_err(|e| e.to_string())?;
    let project = Record {
        id: manifest.id,
        title: manifest.title,
        folder,
    };
    merge(path, project, replace)
}

fn merge(path: &Path, project: Record, replace: Option<&Path>) -> Result<Outcome, String> {
    let mut conflict = None;
    let result = local_state::update(
        path,
        ".projects.lock",
        LIMIT,
        State::validate,
        |state: &mut State| {
            if let Some(previous) = state.projects.iter_mut().find(|old| old.id == project.id) {
                if previous.folder != project.folder && replace != Some(previous.folder.as_path()) {
                    conflict = Some(Conflict {
                        project: project.clone(),
                        previous: previous.folder.clone(),
                    });
                    return Err("Project is registered at a different location".into());
                }
                *previous = project;
            } else {
                state.projects.push(project);
            }
            Ok(())
        },
    );
    if let Some(conflict) = conflict {
        return Ok(Outcome::Conflict(conflict));
    }
    result.map(Outcome::State)
}

pub(super) fn remove(path: &Path, id: Uuid) -> Result<State, String> {
    local_state::update(
        path,
        ".projects.lock",
        LIMIT,
        State::validate,
        |state: &mut State| {
            state.projects.retain(|project| project.id != id);
            Ok(())
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn registration_merges_and_relocation_requires_the_observed_previous_location() {
        let local = tempfile::tempdir().unwrap();
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let path = local.path().join("projects.json");
        let Outcome::State(initial) = register(&path, first.path(), Some("Work"), None).unwrap()
        else {
            panic!()
        };
        let id = initial.projects[0].id;
        let bytes = std::fs::read(first.path().join(source::MANIFEST)).unwrap();
        std::fs::write(second.path().join(source::MANIFEST), bytes).unwrap();
        let before = std::fs::read(&path).unwrap();
        assert!(matches!(
            register(&path, second.path(), None, None).unwrap(),
            Outcome::Conflict(_)
        ));
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert!(matches!(
            register(&path, second.path(), None, Some(Path::new("/wrong"))).unwrap(),
            Outcome::Conflict(_)
        ));
        let Outcome::State(moved) =
            register(&path, second.path(), None, Some(first.path())).unwrap()
        else {
            panic!()
        };
        assert_eq!(moved.projects.len(), 1);
        assert_eq!(moved.projects[0].id, id);
        assert_eq!(moved.projects[0].folder, second.path());
        let another = tempfile::tempdir().unwrap();
        register(&path, another.path(), Some("Other"), None).unwrap();
        assert_eq!(load(&path).unwrap().projects.len(), 2);
        remove(&path, id).unwrap();
        assert_eq!(load(&path).unwrap().projects.len(), 1);
        assert!(first.path().join(source::MANIFEST).exists());
        assert!(second.path().join(source::MANIFEST).exists());
    }
    #[test]
    fn malformed_and_future_registries_remain_byte_identical() {
        let local = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        source::create(project.path(), "Work").unwrap();
        let path = local.path().join("projects.json");
        for bad in [b"broken".as_slice(), br#"{"version":99,"projects":[]}"#] {
            std::fs::write(&path, bad).unwrap();
            assert!(register(&path, project.path(), None, None).is_err());
            assert_eq!(std::fs::read(&path).unwrap(), bad);
        }
    }
}
