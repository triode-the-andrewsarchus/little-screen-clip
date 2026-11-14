//! Bounded, observed directory membership for an expanded `here` search.
use crate::{
    Results, policy,
    query::Scope,
    reader::{self, Reply, Request},
    source::SourceRoot,
};
use std::{
    collections::BTreeSet,
    path::{Component, Path},
    sync::atomic::{AtomicBool, Ordering},
};

/// Maximum directory observations, including the current browser folder.
pub const MAX_DIRECTORIES: usize = 33;
/// Maximum expanded-container depth below the current browser folder.
pub const MAX_DEPTH: usize = 4;

/// Session-only scope observations, independent from query text and index enrollment.
///
/// The first directory is the browser location; others are expanded descendants.
/// Observations never authorize content reads without worker revalidation.
#[derive(Clone, Default, Debug)]
pub struct HereScope {
    pub directories: Vec<SourceRoot>,
    /// Expanded directory entries whose first mounted-source probe is pending.
    /// These are hints, checked against fresh helper observations before listing.
    pub pending: Vec<little_clip_core::file_inventory::Entry>,
    /// Expanded directories without usable observations, or outside the budget.
    pub incomplete: usize,
}
impl HereScope {
    /// Records a descendant hint within the scope budget and parent chain.
    ///
    /// Returns whether it was admitted; omissions increment coverage. This
    /// performs no filesystem access or content authorization.
    pub fn observe(
        &mut self,
        entry: &little_clip_core::file_inventory::Entry,
        source: Option<&SourceRoot>,
    ) -> bool {
        let parent_present = entry.path.parent().is_some_and(|parent| {
            self.directories.iter().any(|root| root.path == parent)
                || self.pending.iter().any(|entry| entry.path == parent)
        });
        if self.directories.len() + self.pending.len() >= MAX_DIRECTORIES
            || !pending_directory(entry)
            || !parent_present
        {
            self.incomplete = self.incomplete.saturating_add(1);
            return false;
        }
        if let Some(source) = source {
            self.directories.push(source.clone());
        } else {
            self.pending.push(entry.clone());
        }
        true
    }
    /// Retains worker-validated context for previously requested addresses only.
    ///
    /// Successful first probes become observed roots, so refreshes cannot
    /// silently adopt a different mount or session at those addresses.
    pub fn accept(&mut self, sources: &[SourceRoot]) {
        for source in sources {
            if let Some(old) = self
                .directories
                .iter_mut()
                .find(|old| old.path == source.path)
            {
                if same(old, source, false) {
                    *old = source.clone();
                }
            } else if self.pending.iter().any(|entry| {
                entry.path == source.path
                    && entry
                        .identity
                        .is_some_and(|identity| identity.object() == (source.device, source.inode))
            }) && source.mount.is_some()
            {
                self.pending.retain(|entry| entry.path != source.path);
                self.directories.push(source.clone());
            }
        }
    }
    /// Checks the bounded parent chain without performing filesystem I/O.
    ///
    /// # Errors
    /// Rejects unrelated, duplicate, over-budget or unobserved directory scopes.
    pub fn check_shape(&self, path: &Path, scope: Scope) -> Result<(), String> {
        if !path.is_absolute()
            || path
                .components()
                .any(|part| matches!(part, Component::ParentDir))
            || scope != Scope::Here
            || self.directories.is_empty()
            || self.directories.len() + self.pending.len() > MAX_DIRECTORIES
            || self.directories[0].path != path
        {
            return Err("Invalid expanded-folder search scope".into());
        }
        let paths: BTreeSet<_> = self
            .directories
            .iter()
            .map(|d| d.path.as_path())
            .chain(self.pending.iter().map(|entry| entry.path.as_path()))
            .collect();
        if paths.len() != self.directories.len() + self.pending.len() {
            return Err("Duplicate expanded-folder scope".into());
        }
        if self.pending.iter().any(|entry| !pending_directory(entry)) {
            return Err("Expanded scope needs observed ordinary directory identities".into());
        }
        paths
            .iter()
            .try_for_each(|current| check_address(current, path, &paths))
    }
    /// Revalidates every observed directory through disposable readers.
    /// A replaced base location fails; unavailable descendants remain disclosed.
    pub(crate) fn prepare(
        &self,
        path: &Path,
        hidden: bool,
        exclusions: &[std::path::PathBuf],
        cancel: &AtomicBool,
    ) -> Result<Self, String> {
        self.prepare_with(path, hidden, exclusions, cancel, reader::call)
    }
    fn prepare_with(
        &self,
        path: &Path,
        hidden: bool,
        exclusions: &[std::path::PathBuf],
        cancel: &AtomicBool,
        mut read: impl FnMut(&Request, &AtomicBool) -> Result<Reply, String>,
    ) -> Result<Self, String> {
        self.check_shape(path, Scope::Here)?;
        let mut candidates: Vec<_> = self
            .directories
            .iter()
            .cloned()
            .map(|root| (root, false))
            .collect();
        candidates.extend(self.pending.iter().map(|entry| {
            let (device, inode) = entry
                .identity
                .expect("validated pending directory identity")
                .object();
            (
                SourceRoot {
                    path: entry.path.clone(),
                    device,
                    inode,
                    mount: None,
                    session: None,
                },
                true,
            )
        }));
        candidates.sort_by_key(|(root, _)| root.path.components().count());
        let mut ready = Self {
            directories: Vec::new(),
            pending: Vec::new(),
            incomplete: self.incomplete,
        };
        for (expected, pending) in candidates {
            if cancel.load(Ordering::Relaxed) {
                return Err("Search cancelled".into());
            }
            let excluded = policy::excluded_by_user(&expected.path, exclusions)
                || (!hidden
                    && expected
                        .path
                        .strip_prefix(path)
                        .unwrap()
                        .components()
                        .any(|c| c.as_os_str().as_encoded_bytes().starts_with(b".")));
            let parent_live = expected.path == path
                || ready
                    .directories
                    .iter()
                    .any(|root| Some(root.path.as_path()) == expected.path.parent());
            let observed = (!excluded && parent_live)
                .then(|| read(&Request::Probe(expected.path.clone()), cancel));
            if cancel.load(Ordering::Relaxed) {
                return Err("Search cancelled".into());
            }
            match observed {
                Some(Ok(Reply::Root(root))) if same(&expected, &root, pending) => {
                    ready.directories.push(root)
                }
                _ if expected.path == path => {
                    return Err(
                        "Current folder changed or is unavailable; refresh before searching".into(),
                    );
                }
                _ => ready.incomplete = ready.incomplete.saturating_add(1),
            }
        }
        Ok(ready)
    }
    /// Exact directory membership; this never implies recursive traversal.
    pub fn contains_parent(&self, path: &Path) -> bool {
        path.parent()
            .is_some_and(|parent| self.directories.iter().any(|root| root.path == parent))
    }
    pub(crate) fn filter(
        &self,
        path: &Path,
        result: &mut Results,
        hidden: bool,
        cancel: &AtomicBool,
    ) -> Result<(), String> {
        let live = self.prepare(path, hidden, &[], cancel)?;
        result
            .entries
            .retain(|entry| live.contains_parent(&entry.path));
        let paths: BTreeSet<_> = result.entries.iter().map(|entry| &entry.path).collect();
        result.explanations.retain(|path, _| paths.contains(path));
        let label = format!(
            "Folder + expanded ({} folders)",
            live.directories.len().saturating_sub(1)
        );
        result.sources = live.directories;
        result.message = if result.message.starts_with("Explicit search scope") {
            result.message.replacen("Explicit search scope", &label, 1)
        } else {
            format!("{label} · {}", result.message)
        };
        if live.incomplete > 0 {
            result.message.push_str(&format!(
                " · {} expanded folders skipped/unavailable",
                live.incomplete
            ));
        }
        Ok(())
    }
}
fn pending_directory(entry: &little_clip_core::file_inventory::Entry) -> bool {
    entry.directory
        && !entry.symlink
        && entry.system.is_none()
        && entry.warning.is_none()
        && entry.identity.is_some()
}

fn check_address(current: &Path, base: &Path, paths: &BTreeSet<&Path>) -> Result<(), String> {
    let relative = current
        .strip_prefix(base)
        .map_err(|_| "Expanded scope escapes the current folder")?;
    if relative.components().count() > MAX_DEPTH
        || relative
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
        || (current != base
            && !current
                .parent()
                .is_some_and(|parent| paths.contains(parent)))
    {
        return Err("Expanded scope needs a bounded, complete parent chain".into());
    }
    Ok(())
}

fn same(expected: &SourceRoot, current: &SourceRoot, first_probe: bool) -> bool {
    expected.path == current.path
        && expected.device == current.device
        && expected.inode == current.inode
        && current.mount.is_some()
        && (first_probe || (expected.mount.is_some() && expected.mount == current.mount))
        && (expected.session.is_none() || expected.session == current.session)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (tempfile::TempDir, std::path::PathBuf, HereScope) {
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path().join("source");
        std::fs::create_dir_all(base.join("expanded/nested")).unwrap();
        std::fs::create_dir(base.join("closed")).unwrap();
        let scope = HereScope {
            directories: vec![
                SourceRoot::capture(&base).unwrap(),
                SourceRoot::capture(&base.join("expanded")).unwrap(),
            ],
            ..Default::default()
        };
        (temp, base, scope)
    }
    fn probe(request: &Request, _: &AtomicBool) -> Result<Reply, String> {
        let Request::Probe(path) = request else {
            panic!("unexpected reader request")
        };
        SourceRoot::capture(path).map(Reply::Root)
    }
    #[test]
    fn membership_is_exact_and_shape_rejects_escaping_missing_and_duplicate_parents() {
        let (_temp, base, scope) = fixture();
        scope.check_shape(&base, Scope::Here).unwrap();
        assert!(scope.contains_parent(&base.join("root.txt")));
        assert!(scope.contains_parent(&base.join("expanded/child.txt")));
        assert!(!scope.contains_parent(&base.join("expanded/nested/deep.txt")));
        assert!(!scope.contains_parent(&base.join("closed/child.txt")));
        assert!(scope.check_shape(&base, Scope::Recursive).is_err());
        let mut invalid = scope.clone();
        invalid.directories.push(scope.directories[1].clone());
        assert!(invalid.check_shape(&base, Scope::Here).is_err());
        invalid = scope.clone();
        invalid.directories[1] = SourceRoot::capture(&base.join("expanded/nested")).unwrap();
        assert!(invalid.check_shape(&base, Scope::Here).is_err());
        invalid.directories[1] = SourceRoot::capture(base.parent().unwrap()).unwrap();
        assert!(invalid.check_shape(&base, Scope::Here).is_err());
        invalid.directories = vec![scope.directories[0].clone(); MAX_DIRECTORIES + 1];
        assert!(invalid.check_shape(&base, Scope::Here).is_err());
    }
    #[test]
    fn pending_entries_need_fresh_matching_identity_and_cannot_adopt_a_replacement() {
        let (_temp, base, mut scope) = fixture();
        let child = scope.directories.pop().unwrap();
        let entry =
            little_clip_core::file_inventory::read_directory(&base, &AtomicBool::new(false))
                .unwrap()
                .entries
                .into_iter()
                .find(|entry| entry.path == child.path)
                .unwrap();
        scope.pending.push(entry);
        let ready = scope
            .prepare_with(&base, false, &[], &AtomicBool::new(false), probe)
            .unwrap();
        assert_eq!(ready.directories.len(), 2);
        assert!(ready.pending.is_empty());
        assert!(
            ready
                .directories
                .iter()
                .all(|root| root.mount.is_some() && root.session.is_some())
        );
        let mut upgraded = scope.clone();
        upgraded.accept(&ready.directories);
        assert!(upgraded.pending.is_empty());
        assert_eq!(upgraded.directories.len(), 2);
        let mut changed_mount = upgraded.directories[1].clone();
        changed_mount.mount = Some(changed_mount.mount.unwrap() + 1);
        upgraded.accept(&[changed_mount]);
        assert_eq!(upgraded.directories[1].mount, ready.directories[1].mount);

        std::fs::rename(&child.path, base.join("retired")).unwrap();
        std::fs::create_dir(&child.path).unwrap();
        let unavailable = scope
            .prepare_with(&base, false, &[], &AtomicBool::new(false), probe)
            .unwrap();
        assert_eq!(unavailable.directories.len(), 1);
        assert_eq!(unavailable.incomplete, 1);
        assert!(!unavailable.contains_parent(&child.path.join("new.txt")));
    }
    #[test]
    fn replaced_base_and_missing_observed_mount_fail_without_adopting_new_sources() {
        let (temp, base, scope) = fixture();
        let mut legacy = scope.clone();
        legacy.directories[0].mount = None;
        assert!(
            legacy
                .prepare_with(&base, false, &[], &AtomicBool::new(false), probe)
                .is_err()
        );
        std::fs::rename(&base, temp.path().join("retired")).unwrap();
        std::fs::create_dir(&base).unwrap();
        assert!(
            scope
                .prepare_with(&base, false, &[], &AtomicBool::new(false), probe)
                .is_err()
        );
    }
    #[test]
    fn exclusions_skip_descendants_and_cancellation_stops_scope_admission() {
        let (_temp, base, scope) = fixture();
        let excluded = base.join("expanded");
        let mut calls = Vec::new();
        let ready = scope
            .prepare_with(
                &base,
                false,
                std::slice::from_ref(&excluded),
                &AtomicBool::new(false),
                |request, cancel| {
                    if let Request::Probe(path) = request {
                        calls.push(path.clone());
                    }
                    probe(request, cancel)
                },
            )
            .unwrap();
        assert_eq!(calls, std::slice::from_ref(&base));
        assert_eq!(ready.incomplete, 1);
        assert_eq!(ready.directories.len(), 1);
        assert!(
            scope
                .prepare_with(&base, false, &[], &AtomicBool::new(true), |_, _| panic!(
                    "cancelled scope issued a read"
                ))
                .unwrap_err()
                .contains("cancelled")
        );
    }
    #[test]
    fn hidden_scopes_and_aliases_cannot_bypass_the_existing_reader_policy() {
        let (_temp, base, mut scope) = fixture();
        std::fs::create_dir(base.join(".hidden")).unwrap();
        scope
            .directories
            .push(SourceRoot::capture(&base.join(".hidden")).unwrap());
        let hidden = scope
            .prepare_with(&base, false, &[], &AtomicBool::new(false), probe)
            .unwrap();
        assert_eq!(hidden.directories.len(), 2);
        let shown = scope
            .prepare_with(&base, true, &[], &AtomicBool::new(false), probe)
            .unwrap();
        assert_eq!(shown.directories.len(), 3);
        std::fs::rename(base.join("expanded"), base.join("retired")).unwrap();
        std::os::unix::fs::symlink(base.join("retired"), base.join("expanded")).unwrap();
        let changed = scope
            .prepare_with(&base, true, &[], &AtomicBool::new(false), probe)
            .unwrap();
        assert!(!changed.contains_parent(&base.join("expanded/file.txt")));
    }
}
