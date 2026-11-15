//! User organization is local metadata, independent of content and visibility rules.
use crate::file_manager::filesystem::{Entry, Identity};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    os::unix::ffi::OsStrExt,
    path::{Component, Path, PathBuf},
};

pub(super) const LIMIT: usize = 8 * 1024 * 1024;
const MARKS: usize = 16_384;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(in crate::file_manager) enum Color {
    Red,
    Yellow,
    Green,
}
impl Color {
    pub const ALL: [Self; 3] = [Self::Red, Self::Yellow, Self::Green];
    pub fn index(self) -> usize {
        match self {
            Self::Red => 0,
            Self::Yellow => 1,
            Self::Green => 2,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::Red => "Red",
            Self::Yellow => "Yellow",
            Self::Green => "Green",
        }
    }
    pub fn letter(self) -> &'static str {
        match self {
            Self::Red => "R",
            Self::Yellow => "Y",
            Self::Green => "G",
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::file_manager) struct Observation {
    #[serde(with = "little_clip_core::path_serde")]
    pub path: PathBuf,
    pub identity: Identity,
}
impl Observation {
    pub fn for_entry(entry: &Entry) -> Option<Self> {
        Some(Self {
            path: entry.path.clone(),
            identity: entry.identity?,
        })
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::file_manager) struct Definitions {
    pub name: String,
    pub meanings: [String; 3],
}
impl Default for Definitions {
    fn default() -> Self {
        Self {
            name: "Sorting session".into(),
            meanings: Color::ALL.map(|color| color.name().into()),
        }
    }
}
impl Definitions {
    pub fn validate(&self) -> Result<(), String> {
        if std::iter::once(&self.name)
            .chain(self.meanings.iter())
            .any(|value| value.trim().is_empty() || value.len() > 128)
        {
            return Err("Session name and meanings must be nonempty and at most 128 bytes".into());
        }
        Ok(())
    }
    pub fn label(&self, color: Color) -> String {
        let meaning = &self.meanings[color.index()];
        if meaning == color.name() {
            color.name().into()
        } else {
            format!("{} · {meaning}", color.name())
        }
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Mark {
    target: Observation,
    color: Color,
}
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Session {
    enabled: bool,
    definitions: Definitions,
    marks: Vec<Mark>,
    previous: Vec<Mark>,
    #[serde(default)]
    recovery_revision: u64,
}
impl Session {
    fn recovery(&mut self, expected: u64) -> Result<(), String> {
        if self.recovery_revision != expected {
            return Err("Cleared marks changed in another window; reload before restoring or forgetting them".into());
        }
        self.recovery_revision = self
            .recovery_revision
            .checked_add(1)
            .ok_or("Session recovery revision exhausted")?;
        Ok(())
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct State {
    version: u32,
    session: Session,
}
impl Default for State {
    fn default() -> Self {
        Self {
            version: 1,
            session: Session::default(),
        }
    }
}

#[derive(Clone)]
pub(in crate::file_manager) enum Action {
    Enable(bool),
    Assign(Vec<Observation>, Option<Color>),
    Clear,
    Restore(u64),
    ForgetClear(u64),
    Definitions,
}
#[derive(Clone)]
pub(super) struct Change {
    pub definitions: Option<DefinitionEdit>,
    pub action: Action,
}
#[derive(Clone)]
pub(super) struct DefinitionEdit {
    pub base: Definitions,
    pub next: Definitions,
}
impl DefinitionEdit {
    fn apply(self, current: &mut Definitions) -> Result<(), String> {
        self.next.validate()?;
        merge_definition(&mut current.name, &self.base.name, &self.next.name)?;
        for index in 0..3 {
            merge_definition(
                &mut current.meanings[index],
                &self.base.meanings[index],
                &self.next.meanings[index],
            )?;
        }
        Ok(())
    }
}
fn merge_definition(current: &mut String, base: &str, next: &str) -> Result<(), String> {
    if base == next {
        return Ok(());
    }
    if current.as_str() != base && current.as_str() != next {
        return Err(
            "Session definitions changed in another window; reload before replacing that label"
                .into(),
        );
    }
    *current = next.to_owned();
    Ok(())
}
impl State {
    pub fn validate(&self) -> Result<(), String> {
        if self.version != 1 {
            return Err("Unsupported organization version; file left unchanged".into());
        }
        self.session.definitions.validate()?;
        for marks in [&self.session.marks, &self.session.previous] {
            if marks.len() > MARKS {
                return Err("Session exceeds 16,384 marks".into());
            }
            let mut paths = std::collections::BTreeSet::new();
            for mark in marks {
                validate_path(&mark.target.path)?;
                if !paths.insert(&mark.target.path) {
                    return Err("Duplicate session mark".into());
                }
            }
        }
        Ok(())
    }
    pub fn apply(&mut self, change: Change) -> Result<(), String> {
        if let Some(definitions) = change.definitions {
            definitions.apply(&mut self.session.definitions)?;
        }
        match change.action {
            Action::Enable(enabled) => self.session.enabled = enabled,
            Action::Assign(targets, color) => {
                if !self.session.enabled {
                    return Err("Enable the session before assigning marks".into());
                }
                if targets.len() > MARKS {
                    return Err("Choose at most 16,384 items to mark".into());
                }
                let mut marks: BTreeMap<_, _> = std::mem::take(&mut self.session.marks)
                    .into_iter()
                    .map(|mark| (mark.target.path.clone(), mark))
                    .collect();
                for target in targets {
                    validate_path(&target.path)?;
                    if let Some(color) = color {
                        marks.insert(target.path.clone(), Mark { target, color });
                    } else {
                        marks.remove(&target.path);
                    }
                }
                self.session.marks = marks.into_values().collect();
            }
            Action::Clear if !self.session.marks.is_empty() => {
                self.session.recovery(self.session.recovery_revision)?;
                self.session.previous = std::mem::take(&mut self.session.marks)
            }
            Action::Restore(expected) => {
                if self.session.previous.is_empty() {
                    return Err("There are no cleared marks to restore".into());
                }
                self.session.recovery(expected)?;
                std::mem::swap(&mut self.session.marks, &mut self.session.previous);
            }
            Action::ForgetClear(expected) => {
                self.session.recovery(expected)?;
                self.session.previous.clear();
            }
            _ => {}
        }
        self.validate()
    }
}
fn validate_path(path: &Path) -> Result<(), String> {
    if !path.is_absolute()
        || path.as_os_str().as_bytes().len() > 4096
        || path.as_os_str().as_bytes().contains(&0)
        || path
            .components()
            .any(|component| matches!(component, Component::ParentDir))
    {
        return Err("Invalid organization reference path".into());
    }
    Ok(())
}

pub(in crate::file_manager) struct Snapshot {
    state: State,
    marks: BTreeMap<PathBuf, (Identity, Color)>,
}
impl Default for Snapshot {
    fn default() -> Self {
        Self::new(State::default())
    }
}
impl Snapshot {
    pub(super) fn new(state: State) -> Self {
        let marks = state
            .session
            .marks
            .iter()
            .map(|mark| (mark.target.path.clone(), (mark.target.identity, mark.color)))
            .collect();
        Self { state, marks }
    }
    pub fn enabled(&self) -> bool {
        self.state.session.enabled
    }
    pub fn definitions(&self) -> &Definitions {
        &self.state.session.definitions
    }
    pub fn count(&self) -> usize {
        self.marks.len()
    }
    pub fn can_restore(&self) -> bool {
        !self.state.session.previous.is_empty()
    }
    pub fn recovery_revision(&self) -> u64 {
        self.state.session.recovery_revision
    }
    pub fn color(&self, entry: &Entry) -> Option<Color> {
        if !self.enabled() {
            return None;
        }
        let (identity, color) = self.marks.get(&entry.path)?;
        (entry.identity == Some(*identity)).then_some(*color)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn non_utf8_reference_names_round_trip_losslessly() {
        use std::os::unix::ffi::OsStringExt;
        let root = tempfile::tempdir().unwrap();
        let path = root
            .path()
            .join(std::ffi::OsString::from_vec(b"odd-\xff.txt".to_vec()));
        std::fs::write(path, b"unchanged").unwrap();
        let entry = entry(root.path());
        let mut state = State::default();
        state
            .apply(Change {
                definitions: None,
                action: Action::Enable(true),
            })
            .unwrap();
        state
            .apply(Change {
                definitions: None,
                action: Action::Assign(
                    vec![Observation::for_entry(&entry).unwrap()],
                    Some(Color::Red),
                ),
            })
            .unwrap();
        let saved: State = serde_json::from_slice(&serde_json::to_vec(&state).unwrap()).unwrap();
        saved.validate().unwrap();
        assert_eq!(Snapshot::new(saved).color(&entry), Some(Color::Red));
    }
    fn entry(root: &Path) -> Entry {
        little_clip_core::file_inventory::read_directory(
            root,
            &std::sync::atomic::AtomicBool::new(false),
        )
        .unwrap()
        .entries
        .remove(0)
    }
    #[test]
    fn assignments_restore_and_definitions_remain_distinct_from_file_contents() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("sample.txt");
        std::fs::write(&file, b"original").unwrap();
        let entry = entry(root.path());
        let observation = Observation::for_entry(&entry).unwrap();
        let mut state = State::default();
        let mut apply = |action| {
            state
                .apply(Change {
                    definitions: None,
                    action,
                })
                .unwrap()
        };
        apply(Action::Enable(true));
        apply(Action::Assign(vec![observation.clone()], Some(Color::Red)));
        apply(Action::Assign(vec![observation], Some(Color::Green)));
        assert_eq!(
            Snapshot::new(state.clone()).color(&entry),
            Some(Color::Green)
        );
        let definitions = Definitions {
            name: "Review".into(),
            meanings: ["Todo".into(), "Reviewing".into(), "Approved".into()],
        };
        state
            .apply(Change {
                definitions: Some(DefinitionEdit {
                    base: state.session.definitions.clone(),
                    next: definitions.clone(),
                }),
                action: Action::Definitions,
            })
            .unwrap();
        state
            .apply(Change {
                definitions: None,
                action: Action::Clear,
            })
            .unwrap();
        assert_eq!(Snapshot::new(state.clone()).count(), 0);
        assert!(Snapshot::new(state.clone()).can_restore());
        let cleared_revision = state.session.recovery_revision;
        state
            .apply(Change {
                definitions: None,
                action: Action::Restore(state.session.recovery_revision),
            })
            .unwrap();
        let saved = serde_json::to_vec(&state).unwrap();
        assert!(
            state
                .apply(Change {
                    definitions: None,
                    action: Action::Restore(cleared_revision)
                })
                .is_err()
        );
        assert_eq!(
            serde_json::to_vec(&state).unwrap(),
            saved,
            "a stale recovery request cannot clear newly restored marks"
        );
        let restored: State = serde_json::from_slice(&saved).unwrap();
        restored.validate().unwrap();
        let snapshot = Snapshot::new(restored);
        assert_eq!(snapshot.color(&entry), Some(Color::Green));
        assert_eq!(snapshot.definitions(), &definitions);
        assert_eq!(
            snapshot.definitions().label(Color::Green),
            "Green · Approved"
        );
        assert_eq!(std::fs::read(file).unwrap(), b"original");
        state
            .apply(Change {
                definitions: None,
                action: Action::Clear,
            })
            .unwrap();
        let before_forget = serde_json::to_vec(&state).unwrap();
        assert!(
            state
                .apply(Change {
                    definitions: None,
                    action: Action::ForgetClear(cleared_revision)
                })
                .is_err()
        );
        assert_eq!(serde_json::to_vec(&state).unwrap(), before_forget);
        state
            .apply(Change {
                definitions: None,
                action: Action::ForgetClear(state.session.recovery_revision),
            })
            .unwrap();
        let empty = Snapshot::new(state);
        assert_eq!(empty.count(), 0);
        assert!(!empty.can_restore());
    }
    #[test]
    fn changed_revisions_and_disabled_sessions_never_inherit_current_marks() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("file.txt");
        std::fs::write(&path, b"old").unwrap();
        let first = entry(root.path());
        let mut state = State::default();
        state
            .apply(Change {
                definitions: None,
                action: Action::Enable(true),
            })
            .unwrap();
        state
            .apply(Change {
                definitions: None,
                action: Action::Assign(
                    vec![Observation::for_entry(&first).unwrap()],
                    Some(Color::Yellow),
                ),
            })
            .unwrap();
        std::fs::write(path, b"changed source").unwrap();
        let later = entry(root.path());
        let snapshot = Snapshot::new(state.clone());
        assert_eq!(snapshot.color(&later), None);
        assert_eq!(
            snapshot.count(),
            1,
            "unmatched references are retained for clearing/recovery"
        );
        state
            .apply(Change {
                definitions: None,
                action: Action::Enable(false),
            })
            .unwrap();
        assert!(Snapshot::new(state).color(&first).is_none());
    }
    #[test]
    fn invalid_paths_newer_schemas_and_empty_definitions_fail_validation() {
        let mut state = State {
            version: 99,
            ..State::default()
        };
        assert!(state.validate().is_err());
        state.version = 1;
        state.session.definitions.meanings[0].clear();
        assert!(state.validate().is_err());
        for path in ["relative", "/folder/../other"] {
            assert!(validate_path(Path::new(path)).is_err());
        }
    }
}
