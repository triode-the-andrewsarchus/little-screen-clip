//! Persistent path classifications, separate from revision-specific session marks.
use super::model::validate_path;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};
use uuid::Uuid;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::file_manager) struct Group {
    pub id: Uuid,
    pub parent: Option<Uuid>,
    pub name: String,
    pub color: [u8; 3],
}
impl Group {
    pub fn new(name: &str, parent: Option<Uuid>) -> Self {
        Self {
            id: Uuid::new_v4(),
            parent,
            name: name.into(),
            color: [91, 151, 201],
        }
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Member {
    #[serde(with = "little_clip_core::path_serde")]
    path: PathBuf,
    group: Uuid,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Sets {
    groups: Vec<Group>,
    members: Vec<Member>,
}
impl Default for Sets {
    fn default() -> Self {
        Self {
            groups: [(1, "A", [91, 151, 201]), (2, "B", [190, 128, 194])]
                .into_iter()
                .map(|(id, name, color)| Group {
                    id: Uuid::from_u128(id),
                    parent: None,
                    name: name.into(),
                    color,
                })
                .collect(),
            members: Vec::new(),
        }
    }
}
#[derive(Clone)]
pub(in crate::file_manager) enum SetChange {
    Create(Group),
    Edit {
        before: Group,
        after: Group,
    },
    Remove(Uuid),
    Assign {
        paths: Vec<PathBuf>,
        group: Option<Uuid>,
    },
}
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(in crate::file_manager) enum Filter {
    #[default]
    All,
    Unassigned,
    Group(Uuid),
}
impl Sets {
    pub fn validate(&self) -> Result<(), String> {
        if self.groups.len() > 256 || self.members.len() > 16_384 {
            return Err("Sorting sets exceed 256 groups or 16,384 references".into());
        }
        let groups: BTreeMap<_, _> = self.groups.iter().map(|g| (g.id, g)).collect();
        if groups.len() != self.groups.len() {
            return Err("Duplicate sorting-set ID".into());
        }
        let mut labels = BTreeSet::new();
        for group in &self.groups {
            if group.id.is_nil() || group.name.trim().is_empty() || group.name.len() > 128 {
                return Err(
                    "Sorting-set names must be nonempty and at most 128 bytes; IDs must be non-nil"
                        .into(),
                );
            }
            if !labels.insert((group.parent, group.name.trim())) {
                return Err("Sibling sorting sets must have distinct names".into());
            }
            ancestry(group.id, &groups)?;
        }
        let mut paths = BTreeSet::new();
        for member in &self.members {
            validate_path(&member.path)?;
            if !groups.contains_key(&member.group) || !paths.insert(&member.path) {
                return Err("Sorting-set reference has a missing group or duplicate path".into());
            }
        }
        Ok(())
    }
    pub fn apply(&mut self, change: SetChange) -> Result<(), String> {
        match change {
            SetChange::Create(group) => self.groups.push(group),
            SetChange::Edit { before, after } => {
                let current = self
                    .groups
                    .iter_mut()
                    .find(|g| g.id == before.id)
                    .ok_or("Sorting set was removed; reload")?;
                if *current != before || after.id != before.id {
                    return Err(
                        "Sorting set changed in another window; reload before editing".into(),
                    );
                }
                *current = after;
            }
            SetChange::Remove(id) => {
                if self.groups.iter().any(|g| g.parent == Some(id))
                    || self.members.iter().any(|m| m.group == id)
                {
                    return Err("Remove assignments and child sets before deleting this set".into());
                }
                self.groups.retain(|g| g.id != id);
            }
            SetChange::Assign { paths, group } => self.assign(paths, group)?,
        }
        self.validate()
    }
    fn assign(&mut self, paths: Vec<PathBuf>, group: Option<Uuid>) -> Result<(), String> {
        if paths.len() > 16_384 || group.is_some_and(|id| !self.groups.iter().any(|g| g.id == id)) {
            return Err("Choose an existing sorting set and at most 16,384 references".into());
        }
        let mut members: BTreeMap<_, _> = std::mem::take(&mut self.members)
            .into_iter()
            .map(|m| (m.path, m.group))
            .collect();
        for path in paths {
            validate_path(&path)?;
            if let Some(group) = group {
                members.insert(path, group);
            } else {
                members.remove(&path);
            }
        }
        self.members = members
            .into_iter()
            .map(|(path, group)| Member { path, group })
            .collect();
        Ok(())
    }
    pub fn index(&self) -> Index {
        let groups = self.groups.iter().map(|g| (g.id, g)).collect();
        let branches = self
            .groups
            .iter()
            .map(|g| (g.id, ancestry(g.id, &groups).unwrap_or_default()))
            .collect();
        Index {
            groups: self.groups.clone(),
            members: self
                .members
                .iter()
                .map(|m| (m.path.clone(), m.group))
                .collect(),
            branches,
        }
    }
}
fn ancestry(id: Uuid, groups: &BTreeMap<Uuid, &Group>) -> Result<Vec<Uuid>, String> {
    let mut result = Vec::new();
    let mut next = Some(id);
    while let Some(id) = next {
        if result.len() >= 16 || result.contains(&id) {
            return Err("Sorting-set hierarchy is cyclic or deeper than 16 levels".into());
        }
        let group = groups.get(&id).ok_or("Sorting set has a missing parent")?;
        result.push(id);
        next = group.parent;
    }
    Ok(result)
}
pub(in crate::file_manager) struct Index {
    pub groups: Vec<Group>,
    members: BTreeMap<PathBuf, Uuid>,
    branches: BTreeMap<Uuid, Vec<Uuid>>,
}
impl Index {
    pub fn group(&self, id: Uuid) -> Option<&Group> {
        self.groups.iter().find(|g| g.id == id)
    }
    pub fn assigned(&self, path: &Path) -> Option<&Group> {
        self.group(*self.members.get(path)?)
    }
    pub fn label(&self, id: Uuid) -> String {
        self.branches
            .get(&id)
            .into_iter()
            .flatten()
            .rev()
            .filter_map(|id| self.group(*id))
            .map(|g| g.name.as_str())
            .collect::<Vec<_>>()
            .join(" / ")
    }
    pub fn matches(&self, path: &Path, filter: Filter) -> bool {
        match filter {
            Filter::All => true,
            Filter::Unassigned => !self.members.contains_key(path),
            Filter::Group(id) => self
                .members
                .get(path)
                .and_then(|id| self.branches.get(id))
                .is_some_and(|branch| branch.contains(&id)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn non_utf8_memberships_round_trip_and_missing_or_invalid_groups_are_rejected() {
        use std::os::unix::ffi::OsStringExt;
        let path = PathBuf::from(std::ffi::OsString::from_vec(b"/odd-\xff".to_vec()));
        let mut sets = Sets::default();
        let id = sets.groups[0].id;
        sets.apply(SetChange::Assign {
            paths: vec![path.clone()],
            group: Some(id),
        })
        .unwrap();
        let decoded: Sets = serde_json::from_slice(&serde_json::to_vec(&sets).unwrap()).unwrap();
        decoded.validate().unwrap();
        assert_eq!(decoded.index().assigned(&path).unwrap().id, id);
        assert!(
            sets.apply(SetChange::Assign {
                paths: vec!["/other".into()],
                group: Some(Uuid::new_v4())
            })
            .is_err()
        );
        for name in [String::new(), "x".repeat(129)] {
            let mut invalid = Sets::default();
            assert!(
                invalid
                    .apply(SetChange::Create(Group::new(&name, None)))
                    .is_err()
            );
        }
        let mut invalid = Sets::default();
        assert!(
            invalid
                .apply(SetChange::Create(Group::new(
                    "Orphan",
                    Some(Uuid::new_v4())
                )))
                .is_err()
        );
    }
    #[test]
    fn subdivisions_filter_ancestors_and_leave_other_classifications_alone() {
        let mut sets = Sets::default();
        let a = sets.groups[0].id;
        let b = sets.groups[1].id;
        let child = Group::new("Keep", Some(a));
        let child_id = child.id;
        sets.apply(SetChange::Create(child.clone())).unwrap();
        for (path, group) in [("/a", a), ("/b", b), ("/child", child_id)] {
            sets.apply(SetChange::Assign {
                paths: vec![path.into()],
                group: Some(group),
            })
            .unwrap();
        }
        let index = sets.index();
        assert!(index.matches(Path::new("/child"), Filter::Group(a)));
        assert!(!index.matches(Path::new("/b"), Filter::Group(a)));
        assert!(index.matches(Path::new("/none"), Filter::Unassigned));
        assert_eq!(index.label(child_id), "A / Keep");
        assert!(sets.apply(SetChange::Remove(a)).is_err());
        let mut renamed = child.clone();
        renamed.name = "Archive".into();
        sets.apply(SetChange::Edit {
            before: child.clone(),
            after: renamed,
        })
        .unwrap();
        assert!(
            sets.apply(SetChange::Edit {
                before: child.clone(),
                after: child
            })
            .is_err()
        );
        assert_eq!(sets.index().label(child_id), "A / Archive");
    }
    #[test]
    fn invalid_hierarchies_and_oversized_or_duplicate_names_fail() {
        let mut sets = Sets::default();
        let mut group = Group::new("Cycle", None);
        group.parent = Some(group.id);
        assert!(sets.apply(SetChange::Create(group)).is_err());
        let mut sets = Sets::default();
        assert!(
            sets.apply(SetChange::Create(Group::new("A", None)))
                .is_err()
        );
        let mut sets = Sets::default();
        let mut parent = None;
        for level in 0..16 {
            let group = Group::new(&format!("Level {level}"), parent);
            parent = Some(group.id);
            sets.apply(SetChange::Create(group)).unwrap();
        }
        assert!(
            sets.apply(SetChange::Create(Group::new("Too deep", parent)))
                .is_err()
        );
    }
}
