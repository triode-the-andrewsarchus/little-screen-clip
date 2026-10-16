//! Observation keys include namespace view and object lifetime, not just a pathname.
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{
        LazyLock, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub(in crate::file_manager) struct Namespace {
    pub mount: Option<u64>,
    pub pid: Option<u64>,
    pub network: Option<u64>,
    #[serde(default)]
    pub cgroup: Option<u64>,
}
impl Namespace {
    pub fn known(&self) -> bool {
        self.mount.is_some()
            && self.pid.is_some()
            && self.network.is_some()
            && self.cgroup.is_some()
    }
    pub fn current() -> Self {
        use std::os::unix::fs::MetadataExt;
        let inode = |name: &str| {
            std::fs::metadata(format!("/proc/self/ns/{name}"))
                .ok()
                .map(|m| m.ino())
        };
        Self {
            mount: inode("mnt"),
            pid: inode("pid"),
            network: inode("net"),
            cgroup: inode("cgroup"),
        }
    }
}
/// Object and mount identity from one pinned metadata handle, without payload I/O.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub(in crate::file_manager) struct MountIdentity {
    pub device: u64,
    pub inode: u64,
    pub mount: u64,
}
impl MountIdentity {
    pub fn directory(path: &std::path::Path) -> std::io::Result<Self> {
        let file = little_clip_core::kernel_interface::pin(path, true)?;
        let metadata = file.metadata()?;
        if !metadata.is_dir() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "Not a directory hierarchy object",
            ));
        }
        Self::pinned(&file)
    }
    pub fn pinned(file: &std::fs::File) -> std::io::Result<Self> {
        use std::os::{fd::AsRawFd, unix::fs::MetadataExt};
        let metadata = file.metadata()?;
        let mut stats = std::mem::MaybeUninit::<libc::statx>::uninit();
        // SAFETY: the live O_PATH descriptor is queried using an empty C string;
        // stats points to writable storage for the complete statx structure.
        if unsafe {
            libc::statx(
                file.as_raw_fd(),
                c"".as_ptr(),
                libc::AT_EMPTY_PATH,
                libc::STATX_MNT_ID,
                stats.as_mut_ptr(),
            )
        } != 0
        {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: successful statx initialized the structure.
        let stats = unsafe { stats.assume_init() };
        if stats.stx_mask & libc::STATX_MNT_ID == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "Mount identity not supplied by the kernel",
            ));
        }
        Ok(Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            mount: stats.stx_mnt_id,
        })
    }
    pub fn same_hierarchy_mount(self, other: Self) -> bool {
        self.device == other.device && self.mount == other.mount && self.inode != other.inode
    }
}
/// Mounted interface context; a bind remount can preserve the underlying inode.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub(in crate::file_manager) struct ObjectContext {
    pub object: MountIdentity,
    pub kind: little_clip_core::kernel_interface::Kind,
    pub file_type: u32,
}
impl ObjectContext {
    pub fn pinned(file: &std::fs::File) -> std::io::Result<Self> {
        use std::os::unix::fs::MetadataExt;
        Ok(Self {
            object: MountIdentity::pinned(file)?,
            kind: little_clip_core::kernel_interface::classify_handle(file)?,
            file_type: file.metadata()?.mode() & libc::S_IFMT,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct Key {
    pub path: PathBuf,
    pub namespace: Namespace,
    pub context: Option<ObjectContext>,
}
impl Key {
    pub fn for_document(document: &super::Document) -> Self {
        Self {
            path: document.shared_object().unwrap_or(&document.path).into(),
            namespace: document.namespace.clone(),
            context: document.observation_context().copied(),
        }
    }
}
pub(super) fn observation_identity(document: &super::Document) -> &[u64] {
    if document.shared_object().is_some() {
        document
            .kernel_identity
            .as_deref()
            .unwrap_or(&document.identity)
    } else {
        &document.identity
    }
}
/// Compares one access interface, independently from alias-shared observations.
pub(super) fn same_interface(left: &super::Document, right: &super::Document) -> bool {
    left.kind == right.kind
        && left.interface_directory == right.interface_directory
        && left.interface_context == right.interface_context
        && left.identity.get(..3) == right.identity.get(..3)
}

/// Compares the alias-shared object together with its typed mount/namespace view.
pub(super) fn same_observation(left: &super::Document, right: &super::Document) -> bool {
    Key::for_document(left) == Key::for_document(right)
        && observation_identity(left) == observation_identity(right)
        && left.kernel_identity == right.kernel_identity
        && left.kernel_context == right.kernel_context
}

/// Checks a fresh O_PATH pin after semantic collection without opening its payload.
pub(super) fn verify_source(document: &super::Document) -> Result<(), String> {
    use little_clip_core::kernel_interface::{self, Kind};
    if document.namespace != Namespace::current() {
        return Err("Namespace view changed during observation".into());
    }
    let pinned = kernel_interface::pin(&document.path, true)
        .map_err(|_| "Object disappeared during observation")?;
    if kernel_interface::classify_handle(&pinned).map_err(|e| e.to_string())? != document.kind
        || ObjectContext::pinned(&pinned).ok() != document.interface_context
    {
        return Err("Object mount or type changed during observation".into());
    }
    let current = fingerprint(&pinned.metadata().map_err(|e| e.to_string())?);
    let same = if document.kind == Kind::Content {
        document.identity == current
    } else {
        document.identity.get(..3) == current.get(..3)
    };
    if !same {
        return Err("Object changed during observation".into());
    }
    Ok(())
}

#[derive(Default)]
struct Lifetimes {
    entries: BTreeMap<Key, (Vec<u64>, u64, u64)>,
}
static LIFETIMES: LazyLock<Mutex<Lifetimes>> = LazyLock::new(|| Mutex::new(Lifetimes::default()));
static NEXT: AtomicU64 = AtomicU64::new(1);
pub(super) fn fingerprint(metadata: &std::fs::Metadata) -> Vec<u64> {
    use std::os::unix::fs::MetadataExt;
    vec![
        metadata.dev(),
        metadata.ino(),
        metadata.rdev(),
        metadata.ctime() as u64,
        metadata.ctime_nsec() as u64,
    ]
}
/// nsfs recreates inode wrappers; their timestamps are not namespace lifetime evidence.
pub(super) fn interface_fingerprint(
    kind: little_clip_core::kernel_interface::Kind,
    metadata: &std::fs::Metadata,
) -> Vec<u64> {
    use little_clip_core::kernel_interface::{Filesystem, Kind};
    let mut identity = fingerprint(metadata);
    if matches!(
        kind,
        Kind::Kernel {
            filesystem: Filesystem::Namespace
        }
    ) {
        identity.truncate(3);
    }
    identity
}
pub(super) fn stamp(document: &mut super::Document) {
    let key = Key::for_document(document);
    let mut lifetimes = LIFETIMES.lock().unwrap_or_else(|e| e.into_inner());
    let previous = lifetimes.entries.get(&key);
    let mut fingerprint = if document.shared_object().is_some() {
        document
            .kernel_identity
            .clone()
            .unwrap_or_else(|| document.identity.clone())
    } else {
        document.identity.clone()
    };
    let sequence = document
        .observed_raw("Disk sequence")
        .and_then(|value| value.parse::<u64>().ok());
    if let Some(sequence) = sequence {
        fingerprint.push(sequence);
    }
    let known_sequence = !document.facts.iter().any(|fact| {
        fact.label == "Disk sequence"
            && fact.outcome != super::model::Outcome::Missing
            && sequence.is_none()
    });
    let same = document.namespace.known()
        && document.observation_context().is_some()
        && known_sequence
        && previous.is_some_and(|(identity, _, event)| {
            *identity == fingerprint && !super::events::affected(*event, document.event_paths())
        });
    document.generation = if same {
        previous.unwrap().1
    } else {
        NEXT.fetch_add(1, Ordering::Relaxed)
    };
    document.stamp_fact_generations();
    lifetimes.entries.insert(
        key,
        (fingerprint, document.generation, document.event_revision),
    );
    while lifetimes.entries.len() > 2048 {
        if let Some(key) = lifetimes.entries.keys().next().cloned() {
            lifetimes.entries.remove(&key);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn block_sequence_changes_start_a_new_lifetime_for_reused_loop_objects() {
        let temp = tempfile::tempdir().unwrap();
        let mut document =
            super::super::worker::interface(std::path::Path::new("/proc/uptime")).unwrap();
        document.path = temp.path().join("loop-fixture");
        document.facts.clear();
        document.event_revision = super::super::events::revision();
        document.facts.push(super::super::model::Fact::text(
            "Disk sequence",
            "10",
            document.path.join("diskseq"),
        ));
        stamp(&mut document);
        let first = document.generation;
        stamp(&mut document);
        assert_eq!(document.generation, first);
        document.facts[0].raw = Some("11".into());
        stamp(&mut document);
        assert_ne!(document.generation, first);
        let second = document.generation;
        document.facts[0].outcome = super::super::model::Outcome::PermissionDenied;
        stamp(&mut document);
        assert_ne!(
            document.generation, second,
            "unverified sequence must not reuse a previous lifetime"
        );
    }
    #[test]
    fn cgroup_namespace_changes_separate_keys_and_object_generations() {
        let mut document =
            super::super::worker::interface(std::path::Path::new("/proc/uptime")).unwrap();
        stamp(&mut document);
        let key = Key::for_document(&document);
        let generation = document.generation;
        document.namespace.cgroup = Some(
            document
                .namespace
                .cgroup
                .unwrap_or_default()
                .saturating_add(1),
        );
        assert_ne!(Key::for_document(&document), key);
        stamp(&mut document);
        assert_ne!(document.generation, generation);
        document.namespace.cgroup = None;
        assert!(!document.namespace.known());
    }
    #[test]
    fn same_inode_on_a_different_mount_or_type_starts_a_new_object_lifetime() {
        let mut document =
            super::super::worker::interface(std::path::Path::new("/proc/uptime")).unwrap();
        stamp(&mut document);
        let original = document.clone();
        let key = Key::for_document(&original);
        document.interface_context.as_mut().unwrap().object.mount += 1;
        assert_eq!(document.identity, original.identity);
        assert_ne!(Key::for_document(&document), key);
        assert!(!same_interface(&document, &original));
        assert!(!same_observation(&document, &original));
        stamp(&mut document);
        assert_ne!(document.generation, original.generation);
        document = original.clone();
        document.interface_context.as_mut().unwrap().file_type = libc::S_IFDIR;
        assert_ne!(Key::for_document(&document), key);
        assert!(!same_observation(&document, &original));
    }

    #[test]
    fn relationship_targets_reject_unknown_or_changed_mount_context() {
        let temp = tempfile::tempdir().unwrap();
        let mut relation = super::super::Relation::new("Folder", temp.path().into());
        relation.verify().unwrap();
        relation.context.as_mut().unwrap().object.mount += 1;
        assert!(relation.verify().unwrap_err().contains("mount/type"));
        relation.context = None;
        assert!(relation.verify().is_err());
    }

    #[test]
    fn namespace_relationships_ignore_recreated_wrapper_timestamps() {
        let relation =
            super::super::Relation::new("Namespace", std::path::PathBuf::from("/proc/self/ns/mnt"));
        assert_eq!(relation.identity.as_ref().unwrap().len(), 3);
        std::thread::sleep(std::time::Duration::from_millis(10));
        relation.verify().unwrap();
    }

    #[test]
    fn older_mount_context_snapshots_remain_readable_but_cannot_reuse_lifetimes() {
        let original =
            super::super::worker::interface(std::path::Path::new("/proc/uptime")).unwrap();
        let mut json = serde_json::to_value(&original).unwrap();
        json.as_object_mut().unwrap().remove("interface_context");
        json.as_object_mut().unwrap().remove("kernel_context");
        let mut old: super::super::Document = serde_json::from_value(json).unwrap();
        assert!(old.observation_context().is_none());
        assert_ne!(Key::for_document(&old), Key::for_document(&original));
        stamp(&mut old);
        let first = old.generation;
        stamp(&mut old);
        assert_ne!(old.generation, first);
    }

    #[test]
    fn pinned_mount_context_preserves_symlink_aliases_and_revalidates_type() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("file");
        std::fs::write(&path, "ordinary backing data").unwrap();
        let alias = temp.path().join("alias");
        std::os::unix::fs::symlink(&path, &alias).unwrap();
        let original = little_clip_core::kernel_interface::pin(&path, true).unwrap();
        let aliased = little_clip_core::kernel_interface::pin(&alias, true).unwrap();
        assert_eq!(
            ObjectContext::pinned(&original).unwrap(),
            ObjectContext::pinned(&aliased).unwrap()
        );
        let document = super::super::worker::interface(&alias).unwrap();
        verify_source(&document).unwrap();
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        assert!(verify_source(&document).is_err());
    }

    #[test]
    fn older_namespace_snapshots_deserialize_but_cannot_authorize_reuse() {
        let namespace: Namespace =
            serde_json::from_str(r#"{"mount":1,"pid":2,"network":3}"#).unwrap();
        assert_eq!(namespace.cgroup, None);
        assert!(!namespace.known());
    }
    #[test]
    fn hierarchy_mount_identity_excludes_bind_boundaries_and_same_object_aliases() {
        let root = MountIdentity {
            device: 7,
            inode: 10,
            mount: 5,
        };
        assert!(root.same_hierarchy_mount(MountIdentity { inode: 11, ..root }));
        assert!(!root.same_hierarchy_mount(MountIdentity {
            inode: 11,
            mount: 6,
            ..root
        }));
        assert!(!root.same_hierarchy_mount(root));
        assert!(!root.same_hierarchy_mount(MountIdentity {
            inode: 11,
            device: 8,
            ..root
        }));
    }
    #[test]
    fn pinned_directory_mount_identity_tracks_same_mount_children_and_rejects_files() {
        let temp = tempfile::tempdir().unwrap();
        let child = temp.path().join("child");
        std::fs::create_dir(&child).unwrap();
        let root = MountIdentity::directory(temp.path()).unwrap();
        assert!(root.same_hierarchy_mount(MountIdentity::directory(&child).unwrap()));
        let file = temp.path().join("file");
        std::fs::write(&file, "not a directory").unwrap();
        assert_eq!(
            MountIdentity::directory(&file).unwrap_err().kind(),
            std::io::ErrorKind::Unsupported
        );
    }
}
