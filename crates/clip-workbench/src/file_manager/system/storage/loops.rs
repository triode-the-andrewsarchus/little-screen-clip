//! Verified forward and reverse loop relationships from documented sysfs metadata, without device I/O.
use super::super::{
    Document, Relation,
    collector::Reads,
    identity,
    model::{Fact, Outcome},
};
use little_clip_core::kernel_interface::{self, Kind};
use serde::{Deserialize, Serialize};
use std::{
    io,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(in crate::file_manager) struct Association {
    #[serde(with = "super::super::path_serde")]
    pub object: PathBuf,
    #[serde(with = "super::super::path_serde")]
    pub backing: PathBuf,
    pub identity: Vec<u64>,
    pub sequence: u64,
}
impl Association {
    pub(in crate::file_manager::system) fn verify(&self) -> Result<(), String> {
        // Re-observe through the bounded child helper, never execute sysfs callbacks
        // on the inspector's worker thread while following a relationship.
        let current =
            super::super::cache::collect(&self.object, &std::sync::atomic::AtomicBool::new(false))?;
        if !self.matches_snapshot(&current) {
            return Err("Loop attachment changed; refresh the original object".into());
        }
        Ok(())
    }
    fn matches_snapshot(&self, current: &Document) -> bool {
        current.last_error.is_none()
            && current.path == self.object
            && current.relations.iter().any(|relation| {
                relation.loop_association.as_ref().is_some_and(|observed| {
                    observed.object == self.object
                        && observed.identity == self.identity
                        && observed.sequence == self.sequence
                })
            })
    }
    pub(super) fn capture(root: &Path, backing: &Path, reads: &mut Reads) -> io::Result<Self> {
        let identity = backing_identity(backing)?;
        let sequence = observe(root, backing, &identity, reads)?.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::Interrupted,
                "Backing identity changed during observation",
            )
        })?;
        Ok(Self {
            object: root.canonicalize()?,
            backing: backing.into(),
            identity,
            sequence,
        })
    }
    #[cfg(test)]
    fn verify_with(&self, root: &Path, reads: &mut Reads) -> Result<(), String> {
        let observed = observe(root, &self.backing, &self.identity, reads)
            .map_err(|error| error.to_string())?;
        if observed != Some(self.sequence) {
            return Err("Loop attachment changed; refresh the originating file".into());
        }
        Ok(())
    }
}
pub(in crate::file_manager::system) fn is_loop(root: &Path) -> bool {
    root.file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.strip_prefix("loop"))
        .is_some_and(|number| !number.is_empty() && number.bytes().all(|b| b.is_ascii_digit()))
}
fn backing_identity(path: &Path) -> io::Result<Vec<u64>> {
    let file = kernel_interface::pin(path, true)?;
    let kind = kernel_interface::classify_handle(&file)?;
    if !(matches!(kind, Kind::Block { .. })
        || (kind == Kind::Content && file.metadata()?.is_file()))
    {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Not a regular file or block backing resource",
        ));
    }
    Ok(identity::fingerprint(&file.metadata()?))
}
fn sequence(root: &Path, reads: &mut Reads) -> io::Result<u64> {
    reads
        .read_fresh(&root.join("diskseq"))?
        .parse()
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "Invalid loop disk sequence"))
}
fn observe(
    root: &Path,
    image: &Path,
    expected: &[u64],
    reads: &mut Reads,
) -> io::Result<Option<u64>> {
    let backing = reads.loop_backing(root)?;
    if backing_identity(&backing)? != expected || backing_identity(image)? != expected {
        return Ok(None);
    }
    let before = sequence(root, reads)?;
    let current = reads.loop_backing(root)?;
    let stable = current == backing
        && backing_identity(&current)? == expected
        && backing_identity(image)? == expected
        && sequence(root, reads)? == before;
    if !stable {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "Loop association changed during observation",
        ));
    }
    Ok(Some(before))
}
pub(in crate::file_manager::system) fn collect(
    document: &mut Document,
    checkpoint: &mut dyn FnMut(&Document),
) {
    collect_at(
        document,
        Path::new("/sys/class/block"),
        &mut Reads::new(),
        checkpoint,
    );
}
fn collect_at(
    document: &mut Document,
    class: &Path,
    reads: &mut Reads,
    checkpoint: &mut dyn FnMut(&Document),
) {
    if document.kind != Kind::Content || document.interface_directory {
        return;
    }
    let mut coverage = Outcome::Observed;
    let mut matches = 0;
    let entries = match std::fs::read_dir(class) {
        Ok(entries) => entries,
        Err(error) => {
            document.facts.push(Fact::failure(
                "Loop inventory coverage",
                class.into(),
                error,
            ));
            checkpoint(document);
            return;
        }
    };
    for (index, entry) in entries.take(129).enumerate() {
        if index == 128 || matches == 16 {
            coverage = Outcome::Truncated;
            break;
        }
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => {
                coverage = Outcome::Truncated;
                continue;
            }
        };
        if !is_loop(&entry.path()) {
            continue;
        }
        let root = entry.path();
        if !root.join("loop").is_dir() {
            continue;
        }
        match observe(&root, &document.path, &document.identity, reads) {
            Ok(Some(sequence)) => {
                let mut relation = Relation::new(
                    format!(
                        "Associated {} (observed)",
                        entry.file_name().to_string_lossy()
                    ),
                    root,
                );
                relation.loop_association = Some(Association {
                    object: relation
                        .target
                        .canonicalize()
                        .unwrap_or_else(|_| relation.target.clone()),
                    backing: document.path.clone(),
                    identity: document.identity.clone(),
                    sequence,
                });
                document.relations.push(relation);
                matches += 1;
                checkpoint(document);
            }
            Ok(None) => {}
            Err(error) => {
                coverage = Outcome::Truncated;
                document
                    .facts
                    .push(Fact::failure("Loop association observation", root, error));
            }
        }
    }
    let mut fact = Fact::text(
        "Loop inventory coverage",
        format!(
            "{matches} matching attachments observed; at most 128 entries scanned and 16 associations retained. No matching observation is not proof of absence; paths and attachments may change."
        ),
        class.into(),
    );
    fact.outcome = coverage;
    document.facts.push(fact);
    checkpoint(document);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        collections::BTreeMap,
        os::unix::{ffi::OsStringExt, fs::symlink},
    };
    fn fixture(
        image_name: std::ffi::OsString,
    ) -> (tempfile::TempDir, PathBuf, PathBuf, Document, Reads) {
        let temp = tempfile::tempdir().unwrap();
        let image = temp.path().join(image_name);
        std::fs::write(&image, b"image").unwrap();
        let class = temp.path().join("class/block");
        std::fs::create_dir_all(&class).unwrap();
        let root = class.join("loop7");
        std::fs::create_dir_all(root.join("loop")).unwrap();
        symlink(&class, root.join("subsystem")).unwrap();
        let document = super::super::super::worker::interface(&image).unwrap();
        use std::os::unix::ffi::OsStrExt;
        let mut path = image.as_os_str().as_bytes().to_vec();
        path.push(b'\n');
        let reads = Reads::fixture(BTreeMap::from([
            (root.join("loop/backing_file"), path),
            (root.join("diskseq"), b"12\n".to_vec()),
        ]));
        (temp, class, root, document, reads)
    }
    #[test]
    fn ordinary_file_inspection_preserves_filename_bytes_and_uses_shared_relations() {
        for name in [
            std::ffi::OsString::from("image with spaces  "),
            std::ffi::OsString::from_vec(b"image-\xff".to_vec()),
            std::ffi::OsString::from("image\n"),
        ] {
            let (_temp, class, root, mut document, mut reads) = fixture(name);
            assert!(document.relations.is_empty());
            assert_eq!(reads.loop_backing(&root).unwrap(), document.path);
            collect_at(&mut document, &class, &mut reads, &mut |_| {});
            assert_eq!(document.relations.len(), 1);
            assert_eq!(
                document.relations[0]
                    .loop_association
                    .as_ref()
                    .unwrap()
                    .sequence,
                12
            );
        }
    }
    #[test]
    fn hardlink_matches_but_replaced_file_and_deleted_path_do_not() {
        let (temp, class, root, mut document, mut reads) = fixture("image".into());
        let alias = temp.path().join("alias");
        std::fs::hard_link(&document.path, &alias).unwrap();
        document = super::super::super::worker::interface(&alias).unwrap();
        collect_at(&mut document, &class, &mut reads, &mut |_| {});
        assert_eq!(document.relations.len(), 1);
        std::fs::remove_file(&alias).unwrap();
        std::fs::write(&alias, b"replacement").unwrap();
        assert_eq!(
            observe(&root, &alias, &document.identity, &mut reads).unwrap(),
            None
        );
        let mut values = BTreeMap::new();
        values.insert(
            root.join("loop/backing_file"),
            format!("{} (deleted)\n", document.path.display()).into_bytes(),
        );
        assert_eq!(
            Reads::fixture(values)
                .loop_backing(&root)
                .unwrap_err()
                .kind(),
            io::ErrorKind::NotFound
        );
    }
    #[test]
    fn missing_sequence_and_lookalike_registration_never_publish_an_attachment() {
        let (_temp, class, root, mut document, _reads) = fixture("image".into());
        let mut reads = Reads::fixture(BTreeMap::from([(
            root.join("loop/backing_file"),
            format!("{}\n", document.path.display()).into_bytes(),
        )]));
        collect_at(&mut document, &class, &mut reads, &mut |_| {});
        assert!(document.relations.is_empty());
        // An unregistered directory with convincing loop naming stays unsupported.
        std::fs::remove_file(root.join("subsystem")).unwrap();
        assert_eq!(
            reads.loop_backing(&root).unwrap_err().kind(),
            io::ErrorKind::Unsupported
        );
        assert!(!is_loop(Path::new("loop7p1")));
    }
    #[test]
    fn relation_activation_rechecks_sequence_and_backing_identity() {
        let (_temp, class, root, mut document, mut reads) = fixture("image".into());
        collect_at(&mut document, &class, &mut reads, &mut |_| {});
        let association = document.relations[0].loop_association.as_ref().unwrap();
        association.verify_with(&root, &mut reads).unwrap();
        let mut changed = Reads::fixture(BTreeMap::from([
            (
                root.join("loop/backing_file"),
                format!("{}\n", document.path.display()).into_bytes(),
            ),
            (root.join("diskseq"), b"13\n".to_vec()),
        ]));
        assert!(association.verify_with(&root, &mut changed).is_err());
        std::fs::remove_file(&document.path).unwrap();
        std::fs::write(&document.path, b"new image").unwrap();
        assert!(association.verify_with(&root, &mut reads).is_err());
    }
    #[test]
    fn inventory_caps_and_kind_guard_are_reported_without_assuming_absence() {
        let (_temp, class, _root, mut document, mut reads) = fixture("image".into());
        for index in 0..140 {
            std::fs::create_dir(class.join(format!("other{index}"))).unwrap();
        }
        collect_at(&mut document, &class, &mut reads, &mut |_| {});
        assert_eq!(document.facts.last().unwrap().outcome, Outcome::Truncated);
        document.kind = Kind::Kernel {
            filesystem: kernel_interface::Filesystem::Proc,
        };
        document.facts.clear();
        document.relations.clear();
        collect_at(&mut document, &class, &mut reads, &mut |_| {});
        assert!(document.facts.is_empty() && document.relations.is_empty());
    }
    fn forward_link(document: &mut Document, root: &Path, reads: &mut Reads) -> Relation {
        let image = document.path.clone();
        document.path = root.into();
        let mut storage = super::super::tests::storage(root);
        super::super::backing(document, &mut storage, root, reads);
        assert_eq!(storage.backing.as_ref(), Some(&image));
        document
            .relations
            .iter()
            .find(|r| r.label == "Backing file")
            .unwrap()
            .clone()
    }
    #[test]
    fn forward_link_carries_attachment_evidence_and_both_directions_verify_same_snapshot() {
        let (temp, _class, root, mut document, mut reads) = fixture("image  ".into());
        let image = document.path.clone();
        let alias = temp.path().join("hardlink-alias");
        std::fs::hard_link(&image, &alias).unwrap();
        let relation = forward_link(&mut document, &root, &mut reads);
        assert!(relation.identity.is_some());
        let association = relation.loop_association.as_ref().unwrap().clone();
        assert_eq!(association.object, root);
        assert!(association.matches_snapshot(&document));
        let reverse = Association::capture(&root, &alias, &mut reads).unwrap();
        assert!(reverse.matches_snapshot(&document));
        document.last_error = Some("collector timed out".into());
        assert!(!association.matches_snapshot(&document));
        document.last_error = None;
        document.relations[0]
            .loop_association
            .as_mut()
            .unwrap()
            .sequence += 1;
        assert!(!association.matches_snapshot(&document));
    }
    #[test]
    fn unverified_forward_attachment_is_visible_but_navigation_is_disabled() {
        let (_temp, _class, root, mut document, _reads) = fixture("image".into());
        let image = document.path.clone();
        let mut reads = Reads::fixture(BTreeMap::from([(
            root.join("loop/backing_file"),
            format!("{}\n", image.display()).into_bytes(),
        )]));
        let relation = forward_link(&mut document, &root, &mut reads);
        assert!(relation.identity.is_none() && relation.loop_association.is_none());
        assert!(
            document
                .facts
                .iter()
                .any(|fact| fact.label == "Backing link verification"
                    && fact.outcome == Outcome::Missing
                    && fact.value.is_some())
        );
    }
}
