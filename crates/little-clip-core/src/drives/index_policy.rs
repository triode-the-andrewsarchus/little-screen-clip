//! Local no-index policy with descriptor-free filesystem coordinates.
use crate::{
    kernel_interface::{self, MountIdentity, MountLifetime},
    mount_inventory::{Mount, Snapshot, Watcher},
};
use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    io,
    os::fd::AsRawFd,
    path::{Component, Path, PathBuf},
    sync::{Arc, LazyLock, Mutex},
};

/// A daemon-published exclusion list; these rules do not affect backups or sync.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexPolicy {
    #[serde(with = "crate::path_serde::list")]
    pub excluded: Vec<PathBuf>,
    /// Observed alias context; absent in older literal-only policies.
    #[serde(default)]
    pub context: Option<IndexContext>,
}
/// Local mount observations used only to exclude background work.
///
/// No file descriptors are retained or serialized. Unknown context cannot
/// authorize indexing through an alias.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexContext {
    /// Mount observation used to verify a caller's metadata routing snapshot.
    pub signature: String,
    scopes: Vec<Scope>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Scope {
    mount: Mount,
    lifetime: MountLifetime,
    excluded: Vec<Exclusion>,
}
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
struct Exclusion {
    owner: String,
    #[serde(with = "crate::path_serde")]
    literal: PathBuf,
    #[serde(with = "crate::path_serde")]
    physical: PathBuf,
}
fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
fn anchor(file: &File) -> PathBuf {
    format!("/proc/self/fd/{}", file.as_raw_fd()).into()
}

impl IndexPolicy {
    fn empty(&self) -> bool {
        self.excluded.is_empty()
            && self
                .context
                .as_ref()
                .is_none_or(|context| context.scopes.is_empty())
    }

    /// Observes exclusions and their backing filesystem coordinates without reading payloads.
    ///
    /// Missing descendants retain their nearest existing ancestor's mount context.
    /// Temporary metadata pins are released before this returns. A prior policy
    /// retains unavailable-origin observations for unchanged exclusions while that
    /// filesystem still has mounts in this boot and namespace. Restoring the
    /// original path refreshes its lifetime; removing the exclusion releases it.
    ///
    /// # Errors
    /// Returns invalid paths, unknown mount context or changed observations.
    ///
    /// # Examples
    /// ```no_run
    /// let private = std::path::PathBuf::from("/media/card/private");
    /// let policy = little_clip_core::drives::IndexPolicy::observed(vec![private.clone()], None)?;
    /// assert!(policy.excluded.contains(&private));
    /// assert!(policy.context.is_some());
    /// # Ok::<(), std::io::Error>(())
    /// ```
    pub fn observed(excluded: Vec<PathBuf>, previous: Option<&Self>) -> io::Result<Self> {
        Self::observed_rules(
            excluded
                .into_iter()
                .map(|path| {
                    (
                        format!("path:{}", crate::path_address::display_path(&path)),
                        Some(path),
                    )
                })
                .collect(),
            previous,
        )
    }
    /// Observes named exclusions, including policies whose source is currently offline.
    ///
    /// A `None` path keeps that owner's prior same-boot filesystem observation
    /// while an alias remains mounted. Removing an owner releases its observations.
    ///
    /// # Errors
    /// Returns invalid paths, unknown mount context or changed observations.
    pub fn observed_rules(
        requests: Vec<(String, Option<PathBuf>)>,
        previous: Option<&Self>,
    ) -> io::Result<Self> {
        let owners = requests
            .iter()
            .map(|(owner, _)| owner.clone())
            .collect::<std::collections::HashSet<_>>();
        let mut excluded = Vec::new();
        let snapshot = Snapshot::capture()?;
        let mut context = IndexContext {
            signature: snapshot.signature.clone(),
            scopes: Vec::new(),
        };
        let mut present = std::collections::HashSet::new();
        for (owner, path) in requests {
            if let Some(path) = path {
                let (exists, device) = context.include(&owner, &path, &snapshot)?;
                if exists {
                    present.insert((owner.clone(), device));
                }
                excluded.push(path);
            }
        }
        excluded.sort();
        excluded.dedup();
        if let Some(previous) = previous.and_then(|policy| policy.context.as_ref()) {
            context.retain_unavailable(previous, &owners, &present, &snapshot);
        }
        if Snapshot::capture()?.signature != snapshot.signature {
            return Err(invalid("Mounts changed while observing exclusions"));
        }
        for scope in &mut context.scopes {
            scope.excluded.sort();
            scope.excluded.dedup();
        }
        context.scopes.sort_by_key(|scope| scope.mount.id);
        Ok(Self {
            excluded,
            context: Some(context),
        })
    }
    fn resolve(&self, path: &Path, snapshot: &Snapshot) -> io::Result<bool> {
        let context = self
            .context
            .as_ref()
            .ok_or_else(|| invalid("Policy has no mount context"))?;
        let pin = kernel_interface::pin(path, true)?;
        let canonical = std::fs::read_link(anchor(&pin))?;
        if self.excluded.iter().any(|root| canonical.starts_with(root)) {
            return Ok(true);
        }
        let identity = MountIdentity::pinned(&pin)?;
        let mount = snapshot
            .mounts
            .binary_search_by_key(&identity.mount, |mount| mount.id)
            .ok()
            .map(|index| &snapshot.mounts[index])
            .ok_or_else(|| invalid("Candidate mount was not observed"))?;
        let physical = mount
            .project(&canonical)
            .ok_or_else(|| invalid("Mount location changed"))?;
        let current = MountLifetime {
            identity,
            unique_mount: kernel_interface::unique_mount_id(&pin)?,
            namespace: snapshot.namespace.clone(),
            boot: snapshot.boot,
        };
        for scope in &context.scopes {
            if scope.contains(&current, mount, &physical, &canonical, snapshot)? {
                return Ok(true);
            }
        }
        Ok(false)
    }
}
impl IndexContext {
    fn include(
        &mut self,
        owner: &str,
        path: &Path,
        snapshot: &Snapshot,
    ) -> io::Result<(bool, u64)> {
        let (pin, canonical, present) = locate(path)?;
        let lifetime = MountLifetime::capture(&pin)?;
        let mount = snapshot
            .mounts
            .iter()
            .find(|mount| mount.id == lifetime.identity.mount)
            .ok_or_else(|| invalid("Exclusion mount was not observed"))?;
        let physical = mount
            .project(&canonical)
            .ok_or_else(|| invalid("Exclusion is outside its observed mount"))?;
        if let Some(scope) = self
            .scopes
            .iter_mut()
            .find(|scope| scope.lifetime.same_mount(&lifetime))
        {
            scope.excluded.push(Exclusion {
                owner: owner.into(),
                literal: path.into(),
                physical,
            });
        } else {
            let root = kernel_interface::pin(&mount.target, true)?;
            let origin = MountLifetime::capture(&root)?;
            if !origin.same_mount(&lifetime) {
                return Err(invalid("Exclusion mount changed during observation"));
            }
            self.scopes.push(Scope {
                mount: mount.clone(),
                lifetime: origin,
                excluded: vec![Exclusion {
                    owner: owner.into(),
                    literal: path.into(),
                    physical,
                }],
            });
        }
        Ok((present, mount.device))
    }
    fn retain_unavailable(
        &mut self,
        previous: &Self,
        owners: &std::collections::HashSet<String>,
        present: &std::collections::HashSet<(String, u64)>,
        snapshot: &Snapshot,
    ) {
        for old in &previous.scopes {
            if old.lifetime.boot != snapshot.boot
                || old.lifetime.namespace != snapshot.namespace
                || !snapshot
                    .mounts
                    .iter()
                    .any(|mount| mount.device == old.mount.device)
            {
                continue;
            }
            let retained = old
                .excluded
                .iter()
                .filter(|entry| {
                    owners.contains(&entry.owner)
                        && !present.contains(&(entry.owner.clone(), old.mount.device))
                })
                .cloned()
                .collect::<Vec<_>>();
            if retained.is_empty() {
                continue;
            }
            if let Some(current) = self
                .scopes
                .iter_mut()
                .find(|scope| scope.lifetime.same_mount(&old.lifetime))
            {
                current.excluded.extend(retained);
            } else {
                let mut scope = old.clone();
                scope.excluded = retained;
                self.scopes.push(scope);
            }
        }
    }
    fn valid(&self, snapshot: &Snapshot) -> bool {
        self.signature == snapshot.signature
            && !self.scopes.is_empty()
            && self.scopes.iter().all(|scope| {
                scope.lifetime.namespace.known()
                    && scope.lifetime.namespace == snapshot.namespace
                    && scope.lifetime.boot == snapshot.boot
                    && scope.lifetime.unique_mount != 0
                    && scope.mount.id == scope.lifetime.identity.mount
                    && scope.mount.target.is_absolute()
                    && scope.mount.root.is_absolute()
                    && !scope.excluded.is_empty()
                    && scope
                        .excluded
                        .iter()
                        .all(|entry| entry.literal.is_absolute() && entry.physical.is_absolute())
            })
    }
}
impl Scope {
    // Resolve the current rule object, not a persisted inode: ordinary delete
    // and recreation must not accidentally release a path-based exclusion.
    fn exclusion_identity(&self, entry: &Exclusion) -> io::Result<Option<MountIdentity>> {
        let location = self
            .mount
            .exclusion(&entry.physical)
            .ok_or_else(|| invalid("Exclusion is outside its original view"))?;
        let rule = match kernel_interface::pin(&location, true) {
            Ok(pin) => pin,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        let expected = MountIdentity::pinned(&rule)?;
        if expected.mount != self.lifetime.identity.mount
            || kernel_interface::unique_mount_id(&rule)? != self.lifetime.unique_mount
        {
            return Err(invalid("Exclusion now resolves through another mount"));
        }
        Ok(Some(expected))
    }
    fn contains(
        &self,
        current: &MountLifetime,
        mount: &Mount,
        physical: &Path,
        canonical: &Path,
        snapshot: &Snapshot,
    ) -> io::Result<bool> {
        if self.mount.device != mount.device {
            return Ok(false);
        }
        if !self.lifetime.same_mount(current) {
            let origin = snapshot
                .mounts
                .iter()
                .find(|mount| mount.id == self.mount.id && mount.device == self.mount.device)
                .ok_or_else(|| invalid("Exclusion origin is no longer mounted"))?;
            self.lifetime
                .validate(&kernel_interface::pin(&origin.target, true)?)?;
        }
        if self
            .excluded
            .iter()
            .any(|entry| physical.starts_with(&entry.physical))
        {
            return Ok(true);
        }
        for entry in &self.excluded {
            if entry.equivalent(self, current, mount, physical, canonical)? {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

impl Exclusion {
    // Ask the mounted filesystem to resolve the rule spelling. In particular,
    // an exclusion may have been saved before its final components existed.
    // No Unicode/upcase table is guessed and no directory contents are listed.
    fn equivalent(
        &self,
        origin: &Scope,
        current: &MountLifetime,
        mount: &Mount,
        physical: &Path,
        canonical: &Path,
    ) -> io::Result<bool> {
        let Some(levels) = physical
            .components()
            .count()
            .checked_sub(self.physical.components().count())
        else {
            return Ok(false);
        };
        let Some(expected) = origin.exclusion_identity(self)? else {
            return Ok(false);
        };
        let prefix = canonical
            .ancestors()
            .nth(levels)
            .filter(|prefix| prefix.starts_with(&mount.target))
            .ok_or_else(|| invalid("Exclusion ancestry is outside the candidate view"))?;
        let pin = kernel_interface::pin(prefix, true)?;
        let candidate = MountIdentity::pinned(&pin)?;
        if candidate.mount != current.identity.mount
            || kernel_interface::unique_mount_id(&pin)? != current.unique_mount
        {
            return Err(invalid("Candidate ancestry changed mounts"));
        }
        Ok(candidate.device == expected.device && candidate.inode == expected.inode)
    }
}

fn locate(path: &Path) -> io::Result<(File, PathBuf, bool)> {
    if !path.is_absolute()
        || path
            .components()
            .any(|part| matches!(part, Component::ParentDir))
    {
        return Err(invalid(
            "Exclusions must be absolute without parent components",
        ));
    }
    let mut current = path.to_path_buf();
    let mut missing = Vec::new();
    loop {
        match kernel_interface::pin(&current, true) {
            Ok(pin) => {
                let mut canonical = std::fs::read_link(anchor(&pin))?;
                if MountIdentity::pinned(&kernel_interface::pin(&canonical, true)?)?
                    != MountIdentity::pinned(&pin)?
                {
                    return Err(invalid("Exclusion directory changed during observation"));
                }
                for name in missing.iter().rev() {
                    canonical.push(name);
                }
                return Ok((pin, canonical, missing.is_empty()));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                if missing.len() >= 256 {
                    return Err(invalid("Exclusion ancestor depth exceeds its budget"));
                }
                missing.push(
                    current
                        .file_name()
                        .ok_or_else(|| invalid("No existing exclusion ancestor"))?
                        .to_os_string(),
                );
                current.pop();
            }
            Err(error) => return Err(error),
        }
    }
}
struct Cache {
    tag: Option<(PathBuf, crate::file_inventory::Identity)>,
    value: Option<Arc<IndexPolicy>>,
    watcher: Option<Watcher>,
}
impl Cache {
    fn load(&mut self, tag: (PathBuf, crate::file_inventory::Identity)) {
        let value = kernel_interface::read_content_bounded(&tag.0, 1024 * 1024)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<IndexPolicy>(&bytes).ok())
            .map(|mut policy| {
                policy.excluded.sort();
                policy.excluded.dedup();
                if policy.empty() {
                    return EMPTY.clone();
                }
                match self.value.as_ref() {
                    Some(old) if **old == policy => old.clone(),
                    _ => Arc::new(policy),
                }
            });
        self.tag = Some(tag);
        self.value = value;
        // A new publication also refreshes renamed mount targets, which do not
        // necessarily generate a mount-namespace poll event.
        self.watcher = Watcher::new().ok();
    }
}

static CACHE: LazyLock<Mutex<Cache>> = LazyLock::new(|| {
    Mutex::new(Cache {
        tag: None,
        value: None,
        watcher: None,
    })
});
static EMPTY: LazyLock<Arc<IndexPolicy>> = LazyLock::new(|| Arc::new(IndexPolicy::default()));
fn current() -> Option<(Arc<IndexPolicy>, Option<Arc<Snapshot>>)> {
    let registry = super::state_directory().join("index-policy.json");
    let metadata = match std::fs::symlink_metadata(&registry) {
        Ok(metadata) if metadata.is_file() => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Some((EMPTY.clone(), None));
        }
        _ => return None,
    };
    let tag = (registry.clone(), crate::file_inventory::identity(&metadata));
    let mut cache = CACHE.lock().ok()?;
    if cache.tag.as_ref() != Some(&tag) {
        cache.load(tag);
    }
    let policy = cache.value.clone()?;
    if policy.empty() {
        return Some((policy, None));
    }
    let context = policy.context.as_ref()?;
    let snapshot = cache.watcher.as_mut()?.observe().ok()?;
    if !context.valid(&snapshot) {
        return None;
    }
    Some((policy, Some(snapshot)))
}
/// Returns the cached local policy after validating its current mount observation.
///
/// Call on background workers. Missing registries allow ordinary admission;
/// malformed, legacy nonempty or stale observations fail closed. No content
/// mount descriptors are retained between calls.
pub fn index_policy() -> Option<Arc<IndexPolicy>> {
    current().map(|(policy, _)| policy)
}

/// Checks literal, symlink and observed bind aliases on background workers.
///
/// Unknown metadata or mount continuity excludes work. This never authorizes
/// payload access, modifies files or changes backup/sync enrollment.
pub fn index_excluded(path: &Path) -> bool {
    let Some((policy, snapshot)) = current() else {
        return true;
    };
    if policy.empty() {
        return false;
    }
    if policy.excluded.iter().any(|root| path.starts_with(root)) {
        return true;
    }
    let Some(snapshot) = snapshot else {
        return true;
    };
    if policy.resolve(path, &snapshot).unwrap_or(true) {
        return true;
    }
    // A mount or policy change during metadata lookup cannot publish an allowed
    // result from the earlier observation.
    current().is_none_or(|(active, _)| !Arc::ptr_eq(&active, &policy))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::ffi::OsStringExt;
    #[test]
    fn observed_policy_resolves_symlink_aliases_and_preserves_raw_missing_subtrees() {
        let temp = tempfile::tempdir().unwrap();
        let private = temp
            .path()
            .join(std::ffi::OsString::from_vec(b"private_\xff".to_vec()));
        std::fs::create_dir(&private).unwrap();
        std::fs::write(private.join("file"), b"owned payload").unwrap();
        let alias = temp.path().join("alias");
        std::os::unix::fs::symlink(&private, &alias).unwrap();
        let future = private.join("not-yet-created/nested");
        let policy = IndexPolicy::observed(vec![private.clone(), future.clone()], None).unwrap();
        let snapshot = Snapshot::capture().unwrap();
        assert!(policy.resolve(&alias.join("file"), &snapshot).unwrap());
        assert!(policy.context.as_ref().unwrap().scopes.iter().any(|scope| {
            scope
                .excluded
                .iter()
                .any(|path| path.physical.ends_with("not-yet-created/nested"))
        }));
        let decoded: IndexPolicy =
            serde_json::from_slice(&serde_json::to_vec(&policy).unwrap()).unwrap();
        assert_eq!(decoded, policy);
        assert!(decoded.excluded.contains(&private) && decoded.excluded.contains(&future));
        let ordinary = temp.path().join("allowed");
        std::fs::write(&ordinary, b"ordinary").unwrap();
        assert!(!decoded.resolve(&ordinary, &snapshot).unwrap());
        // Observation releases every source metadata pin before returning.
        for entry in std::fs::read_dir("/proc/self/fd").unwrap() {
            if let Ok(target) = std::fs::read_link(entry.unwrap().path()) {
                assert!(
                    !target.starts_with(temp.path()),
                    "Retained content descriptor: {target:?}"
                );
            }
        }
    }
    #[test]
    fn alias_matching_rejects_changed_lifetime_namespace_and_unobserved_mounts() {
        let temp = tempfile::tempdir().unwrap();
        let private = temp.path().join("private");
        std::fs::create_dir(&private).unwrap();
        let ordinary = temp.path().join("allowed");
        std::fs::write(&ordinary, b"ordinary").unwrap();
        let policy = IndexPolicy::observed(vec![private], None).unwrap();
        let snapshot = Snapshot::capture().unwrap();
        for field in ["unique_mount", "boot", "namespace"] {
            let mut changed = policy.clone();
            let scope = &mut changed.context.as_mut().unwrap().scopes[0];
            match field {
                "unique_mount" => scope.lifetime.unique_mount = 0,
                "boot" => scope.lifetime.boot = uuid::Uuid::nil(),
                _ => scope.lifetime.namespace.mount = None,
            }
            assert!(changed.resolve(&ordinary, &snapshot).is_err(), "{field}");
        }
        let empty = Snapshot {
            mounts: Vec::new(),
            signature: snapshot.signature.clone(),
            namespace: snapshot.namespace.clone(),
            boot: snapshot.boot,
        };
        assert!(policy.resolve(&ordinary, &empty).is_err());
    }
    #[test]
    fn offline_rule_owners_keep_scope_checks_until_the_policy_is_explicitly_removed() {
        let temp = tempfile::tempdir().unwrap();
        let private = temp.path().join("private");
        std::fs::create_dir(&private).unwrap();
        std::fs::write(private.join("secret"), b"owned private bytes").unwrap();
        let allowed = temp.path().join("public");
        std::fs::write(&allowed, b"owned ordinary bytes").unwrap();
        let policy =
            IndexPolicy::observed_rules(vec![("rule:owned".into(), Some(private.clone()))], None)
                .unwrap();
        let offline =
            IndexPolicy::observed_rules(vec![("rule:owned".into(), None)], Some(&policy)).unwrap();
        let snapshot = Snapshot::capture().unwrap();
        assert!(
            offline.excluded.is_empty(),
            "An offline observation must not become a literal-path exclusion for unrelated replacement media"
        );
        assert!(!offline.empty());
        assert!(offline.resolve(&private.join("secret"), &snapshot).unwrap());
        assert!(!offline.resolve(&allowed, &snapshot).unwrap());
        let decoded: IndexPolicy =
            serde_json::from_slice(&serde_json::to_vec(&offline).unwrap()).unwrap();
        assert!(decoded.resolve(&private.join("secret"), &snapshot).unwrap());
        let removed = IndexPolicy::observed_rules(Vec::new(), Some(&decoded)).unwrap();
        assert!(removed.empty());
        assert!(!removed.resolve(&private.join("secret"), &snapshot).unwrap());
    }
    #[test]
    fn policy_observation_rejects_parent_addresses_and_symlink_loops() {
        let temp = tempfile::tempdir().unwrap();
        for path in [
            PathBuf::from("relative"),
            temp.path().join("../escape"),
            temp.path().join("nul\0"),
        ] {
            assert!(IndexPolicy::observed(vec![path], None).is_err());
        }
        let link = temp.path().join("loop");
        std::os::unix::fs::symlink("loop", &link).unwrap();
        assert!(IndexPolicy::observed(vec![link], None).is_err());
    }
    #[test]
    fn current_kernel_identity_matches_file_aliases_without_persisting_reused_inodes() {
        let temp = tempfile::tempdir().unwrap();
        let private = temp.path().join("private");
        let alias = temp.path().join("alias");
        let allowed = temp.path().join("ordinary");
        std::fs::write(&private, b"private bytes").unwrap();
        std::fs::hard_link(&private, &alias).unwrap();
        std::fs::write(&allowed, b"public bytes").unwrap();
        let policy = IndexPolicy::observed(vec![private.clone()], None).unwrap();
        let snapshot = Snapshot::capture().unwrap();
        assert!(policy.resolve(&alias, &snapshot).unwrap());
        assert!(!policy.resolve(&allowed, &snapshot).unwrap());
        std::fs::remove_file(&private).unwrap();
        assert!(!policy.resolve(&alias, &snapshot).unwrap());
        std::fs::hard_link(&allowed, &private).unwrap();
        assert!(!policy.resolve(&alias, &snapshot).unwrap());
        assert!(policy.resolve(&allowed, &snapshot).unwrap());
    }
}

#[cfg(test)]
mod btrfs_tests {
    use super::*;
    use crate::process::CommandRequest;
    use std::os::unix::fs::MetadataExt;
    struct Subvolume(PathBuf);
    impl Drop for Subvolume {
        fn drop(&mut self) {
            // This owned writable fixture needs no snapshot ioctl. VFS removal
            // also works where the unprivileged delete ioctl is refused.
            let result = std::fs::remove_dir_all(&self.0);
            if !std::thread::panicking() {
                result.unwrap();
            }
        }
    }
    #[test]
    #[ignore = "requires an explicitly selected owned Btrfs fixture directory"]
    fn btrfs_subvolume_device_numbers_do_not_break_mount_coordinates() {
        let base = std::env::var_os("LITTLE_CLIP_DRIVE_TEST_BACKUP_DIR")
            .expect("set LITTLE_CLIP_DRIVE_TEST_BACKUP_DIR");
        let temp = tempfile::Builder::new()
            .prefix("little-clip-policy-btrfs-")
            .tempdir_in(base)
            .unwrap();
        assert_eq!(
            rustix::fs::statfs(temp.path()).unwrap().f_type as u64,
            0x9123683e
        );
        let path = temp.path().join("subvolume");
        let result = CommandRequest::new("btrfs")
            .args(["subvolume", "create"])
            .arg(&path)
            .run()
            .unwrap();
        assert!(result.status.success(), "{}", result.stderr_lossy());
        let volume = Subvolume(path);
        std::fs::create_dir(volume.0.join("private")).unwrap();
        std::fs::write(volume.0.join("private/secret"), b"owned private bytes").unwrap();
        std::fs::write(volume.0.join("public"), b"owned ordinary bytes").unwrap();
        let parent = kernel_interface::pin(temp.path(), true).unwrap();
        let child = kernel_interface::pin(&volume.0, true).unwrap();
        assert_ne!(
            parent.metadata().unwrap().dev(),
            child.metadata().unwrap().dev()
        );
        let a = MountLifetime::capture(&parent).unwrap();
        let b = MountLifetime::capture(&child).unwrap();
        assert_eq!(a.unique_mount, b.unique_mount);
        assert!(
            a.same_mount(&b),
            "Per-subvolume object device numbers are not mount boundaries"
        );
        let policy = IndexPolicy::observed(vec![volume.0.join("private")], None).unwrap();
        let snapshot = Snapshot::capture().unwrap();
        assert!(
            policy
                .resolve(&volume.0.join("private/secret"), &snapshot)
                .unwrap()
        );
        assert!(!policy.resolve(&volume.0.join("public"), &snapshot).unwrap());
    }
}
