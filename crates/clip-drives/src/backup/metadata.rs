//! Captured source facts, verified restoration and durable per-path omissions.
use super::*;
use crate::BackupFidelity;
use scan::Entry;
use serde::{Deserialize, Serialize};
use std::os::unix::{ffi::OsStrExt, fs::MetadataExt};

mod attributes;
mod links;
mod object;
use object::Object;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Field {
    Ownership,
    Mode,
    AccessTime,
    ModificationTime,
    ExtendedAttribute,
    ExtendedAttributeList,
    HardLinks,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Omission {
    field: Field,
    attribute: Option<Vec<u8>>,
    errno: Option<i32>,
    reason: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Attribute {
    name: Vec<u8>,
    value: Option<Vec<u8>>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Facts {
    accessed: (i64, i64),
    created: Option<(i64, i64)>,
    links: u64,
    attributes_complete: bool,
    attribute_names_complete: bool,
    attribute_list_errno: Option<i32>,
    attributes: Vec<Attribute>,
    omissions: Vec<Omission>,
    #[serde(default, with = "little_clip_core::path_serde::optional")]
    hard_link_to: Option<PathBuf>,
}
impl Facts {
    fn new(observed: &fs::Metadata) -> Self {
        Self {
            accessed: (observed.atime(), observed.atime_nsec()),
            created: observed.created().ok().and_then(epoch),
            links: observed.nlink(),
            attributes_complete: true,
            attribute_names_complete: true,
            attribute_list_errno: None,
            attributes: vec![],
            omissions: vec![],
            hard_link_to: None,
        }
    }
    fn note(
        &mut self,
        field: Field,
        attribute: Option<&[u8]>,
        error: Option<&std::io::Error>,
        reason: &str,
    ) {
        self.omissions.push(Omission {
            field,
            attribute: attribute.map(<[u8]>::to_vec),
            errno: error.and_then(std::io::Error::raw_os_error),
            reason: match error {
                Some(error) => format!("{reason}: {error}"),
                None => reason.into(),
            }
            .chars()
            .take(512)
            .collect(),
        });
    }
    fn verify(
        &mut self,
        field: Field,
        attribute: Option<&[u8]>,
        result: std::io::Result<()>,
        matches: bool,
    ) {
        match result {
            Err(error) => self.note(
                field,
                attribute,
                Some(&error),
                "Metadata could not be restored",
            ),
            Ok(()) if !matches => self.note(
                field,
                attribute,
                None,
                "Destination did not retain the requested value",
            ),
            Ok(()) => (),
        }
    }
    fn restore_times(&mut self, output: &Object, entry: &Entry) -> Result<()> {
        let result = output.times(self.accessed, (entry.modified, entry.nanos));
        let observed = output.metadata()?;
        let modified = (observed.mtime(), observed.mtime_nsec()) == (entry.modified, entry.nanos);
        let accessed = (observed.atime(), observed.atime_nsec()) == self.accessed;
        match result {
            Err(error) => {
                for (field, matches) in [
                    (Field::ModificationTime, modified),
                    (Field::AccessTime, accessed),
                ] {
                    if !matches {
                        self.note(field, None, Some(&error), "Timestamp could not be restored");
                    }
                }
            }
            Ok(()) => {
                self.verify(Field::ModificationTime, None, Ok(()), modified);
                self.verify(Field::AccessTime, None, Ok(()), accessed);
            }
        }
        Ok(())
    }
}

pub(super) struct Snapshot {
    origin: Entry,
    object: Object,
    facts: Facts,
}
impl Snapshot {
    pub fn capture(source: &Root, entry: &Entry, cancel: &AtomicBool) -> Result<Self> {
        cancelled(cancel)?;
        let object = Object::open(source, entry)?;
        let observed = object.metadata()?;
        if !entry.same_revision(&Entry::from_metadata(&entry.path, &observed)) {
            return Err(crate::Error::SourceChanged(
                "Source changed before metadata collection".into(),
            ));
        }
        let mut facts = Facts::new(&observed);
        facts.accessed = entry.accessed.unwrap_or(facts.accessed);
        let mut snapshot = Self {
            origin: entry.clone(),
            object,
            facts,
        };
        attributes::capture(&snapshot.object, &mut snapshot.facts, cancel)?;
        snapshot.check(source, entry, cancel)?;
        Ok(snapshot)
    }
    pub fn check(&self, source: &Root, entry: &Entry, cancel: &AtomicBool) -> Result<()> {
        cancelled(cancel)?;
        source.validate()?;
        let metadata = self.object.metadata()?;
        let held = Entry::from_metadata(&entry.path, &metadata);
        if !self.origin.same_revision(entry)
            || !self.origin.same_revision(&held)
            || self.origin.link != entry.link
            || !source.same_file(&entry.path, held.device, held.inode)?
            || self.facts.links != metadata.nlink()
        {
            return Err(crate::Error::SourceChanged(
                "Source changed during metadata collection".into(),
            ));
        }
        self.check_attributes(source, &metadata, cancel)
    }
    fn check_attributes(
        &self,
        source: &Root,
        metadata: &fs::Metadata,
        cancel: &AtomicBool,
    ) -> Result<()> {
        if !source.reliable_revision_cache()? {
            let mut current = Facts::new(metadata);
            attributes::capture(&self.object, &mut current, cancel)?;
            if self.facts.attributes != current.attributes
                || self.facts.attributes_complete != current.attributes_complete
                || self.facts.attribute_names_complete != current.attribute_names_complete
                || self.facts.attribute_list_errno != current.attribute_list_errno
            {
                return Err(crate::Error::SourceChanged(
                    "Source attributes changed during backup collection".into(),
                ));
            }
        }
        Ok(())
    }
    pub fn restore(&mut self, target: &Root, entry: &Entry, cancel: &AtomicBool) -> Result<()> {
        cancelled(cancel)?;
        let output = Object::open(target, entry)?;
        verify_contents(&output, entry, cancel)?;
        self.restore_fields(&output, entry, cancel)?;
        verify_contents(&output, entry, cancel)?;
        self.facts.restore_times(&output, entry)?;
        output.validate_name(target, entry, cancel)?;
        output.sync()?;
        target.validate()
    }
    fn restore_fields(
        &mut self,
        output: &Object,
        entry: &Entry,
        cancel: &AtomicBool,
    ) -> Result<()> {
        let owner_matches = self.restore_owner(output, entry)?;
        attributes::remove_obsolete(output, &mut self.facts, cancel)?;
        self.restore_mode(output, entry, owner_matches)?;
        attributes::restore(output, &mut self.facts, cancel)?;
        let expected_mode = entry.mode & if owner_matches { 0o7777 } else { 0o1777 };
        if output.metadata()?.mode() & 0o7777 != expected_mode
            && !self
                .facts
                .omissions
                .iter()
                .any(|omission| omission.field == Field::Mode)
        {
            self.facts.note(
                Field::Mode,
                None,
                None,
                "Final mode differs after ACL/attribute restoration",
            );
        }
        Ok(())
    }
    fn restore_owner(&mut self, output: &Object, entry: &Entry) -> Result<bool> {
        let observed = output.metadata()?;
        let (Some(uid), Some(gid)) = (entry.uid, entry.gid) else {
            self.facts
                .note(Field::Ownership, None, None, "Source ownership is unknown");
            return Ok(false);
        };
        if (observed.uid(), observed.gid()) == (uid, gid) {
            return Ok(true);
        }
        let result = output.chown(uid, gid);
        let observed = output.metadata()?;
        let matches = (observed.uid(), observed.gid()) == (uid, gid);
        self.facts.verify(Field::Ownership, None, result, matches);
        Ok(matches)
    }
    fn restore_mode(&mut self, output: &Object, entry: &Entry, owner_matches: bool) -> Result<()> {
        let mut mode = entry.mode & 0o7777;
        if !owner_matches && mode & 0o6000 != 0 {
            mode &= !0o6000;
            self.facts.note(
                Field::Mode,
                None,
                None,
                "Set-ID bits were not installed with different ownership",
            );
        }
        if output.metadata()?.mode() & 0o7777 != mode {
            let result = output.chmod(mode);
            let matches = output.metadata()?.mode() & 0o7777 == mode;
            self.facts.verify(Field::Mode, None, result, matches);
        }
        Ok(())
    }
}

fn verify_contents(output: &Object, entry: &Entry, cancel: &AtomicBool) -> Result<()> {
    if let Object::File(file) = output
        && entry.kind == "file"
    {
        let hash = little_clip_core::file_source::FileSource::capture_file(file)?.sha256(
            file,
            cancel,
            Duration::MAX,
        )?;
        if entry.hash.as_ref() != Some(&hash) {
            return Err("Backup contents differ during metadata restoration".into());
        }
    }
    Ok(())
}

pub(super) struct Report<'a> {
    db: &'a Store,
    pub summary: BackupFidelity,
}
impl<'a> Report<'a> {
    pub fn new(db: &'a Store, source: &Root) -> Result<Self> {
        db.0.execute_batch(
            "CREATE TABLE backup_metadata(path BLOB PRIMARY KEY,json TEXT NOT NULL);
             CREATE TABLE backup_links(identity TEXT NOT NULL,path BLOB PRIMARY KEY);
             CREATE INDEX backup_link_identity ON backup_links(identity,path)",
        )?;
        db.put("collection", "metadata_policy", &"Restore and verify exposed ownership, modes, access/modification times and xattrs (including ACLs). Access times use initial inventory observations while the inode is unchanged. Preserve original change/creation times and inode/link facts in the manifest; they are not filesystem-recreated timestamps. Metadata failures are per-path omissions, not missing payloads.")?;
        let root = source.directory(Path::new(""), false)?;
        db.put(
            "collection",
            "source_mount_read_only_at_start",
            &rustix::fs::fstatvfs(&root)?
                .f_flag
                .contains(rustix::fs::StatVfsMountFlags::RDONLY),
        )?;
        Ok(Self {
            db,
            summary: BackupFidelity::default(),
        })
    }
    pub fn record(&mut self, entry: &Entry, snapshot: Snapshot) -> Result<()> {
        let facts = snapshot.facts;
        if entry.kind != "directory" && facts.links > 1 {
            self.db.0.execute(
                "INSERT INTO backup_links VALUES(?1,?2)",
                rusqlite::params![
                    format!("{}:{}", entry.device, entry.inode),
                    entry.path.as_os_str().as_bytes()
                ],
            )?;
        }
        self.save_facts(entry, &facts)
    }
    fn save_facts(&self, entry: &Entry, facts: &Facts) -> Result<()> {
        self.db.0.execute("INSERT INTO backup_metadata VALUES(?1,?2) ON CONFLICT(path) DO UPDATE SET json=excluded.json",
            rusqlite::params![entry.path.as_os_str().as_bytes(), serde_json::to_string(&facts)?])?;
        Ok(())
    }
    pub fn finish_links(&self, target: &Root, scan_id: &str, cancel: &AtomicBool) -> Result<()> {
        links::finish(self, target, scan_id, cancel)
    }
    pub fn summarize(&mut self, cancel: &AtomicBool) -> Result<()> {
        self.summary = BackupFidelity::default();
        let mut statement = self
            .db
            .0
            .prepare("SELECT json FROM backup_metadata ORDER BY path")?;
        let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
        for json in rows {
            cancelled(cancel)?;
            let facts: Facts = serde_json::from_str(&json?)?;
            self.summary.entries += 1;
            self.summary.entries_with_omissions += u64::from(!facts.omissions.is_empty());
            let fields: std::collections::BTreeSet<_> = facts
                .omissions
                .iter()
                .map(|omission| (omission.field, omission.attribute.as_deref()))
                .collect();
            self.summary.omitted_fields += fields.len() as u64;
            self.summary.source_attributes_unavailable += u64::from(!facts.attributes_complete);
        }
        self.db.put("collection", "fidelity", &self.summary)
    }
    pub fn finish_directories(
        &mut self,
        source: &Root,
        target: &Root,
        scan_id: &str,
        cancel: &AtomicBool,
    ) -> Result<()> {
        let mut statement = self.db.0.prepare("SELECT json FROM entries WHERE scan=?1 AND json_extract(json,'$.kind')='directory' ORDER BY path DESC")?;
        let rows = statement.query_map([scan_id], |r| r.get::<_, String>(0))?;
        for row in rows {
            let expected: Entry = serde_json::from_str(&row?)?;
            self.finish_directory(source, target, scan_id, &expected, cancel)?;
        }
        let root = self
            .db
            .get::<Entry>("source", "root")?
            .ok_or("Missing captured source root")?;
        self.finish_directory(source, target, scan_id, &root, cancel)?;
        Ok(())
    }
    fn finish_directory(
        &mut self,
        source: &Root,
        target: &Root,
        scan_id: &str,
        expected: &Entry,
        cancel: &AtomicBool,
    ) -> Result<()> {
        let mut entry = scan::observe(source, &expected.path, false, u64::MAX, cancel)?
            .ok_or("Source directory disappeared")?;
        if entry.kind != "directory" {
            return Err("Source directory changed type before metadata restoration".into());
        }
        entry.retain_accessed(expected);
        let mut snapshot = Snapshot::capture(source, &entry, cancel)?;
        snapshot.restore(target, &entry, cancel)?;
        snapshot.check(source, &entry, cancel)?;
        let transaction = self.db.0.unchecked_transaction()?;
        if entry.path.as_os_str().is_empty() {
            self.db.put("source", "root", &entry)?;
        } else {
            scan::put(self.db, scan_id, &entry)?;
        }
        self.record(&entry, snapshot)?;
        transaction.commit()?;
        Ok(())
    }
}

fn epoch(time: std::time::SystemTime) -> Option<(i64, i64)> {
    match time.duration_since(std::time::UNIX_EPOCH) {
        Ok(value) => Some((
            value.as_secs().try_into().ok()?,
            value.subsec_nanos().into(),
        )),
        Err(error) => {
            let value = error.duration();
            let seconds = i64::try_from(value.as_secs()).ok()?.checked_neg()?;
            if value.subsec_nanos() == 0 {
                Some((seconds, 0))
            } else {
                Some((
                    seconds.checked_sub(1)?,
                    1_000_000_000 - i64::from(value.subsec_nanos()),
                ))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn creation_times_keep_subsecond_pre_epoch_observations() {
        for (seconds, nanos) in [
            (-1, 0),
            (-1, 500_000_000),
            (0, 0),
            (1_600_000_000, 123_456_789),
        ] {
            let original = time(seconds, nanos).unwrap();
            assert_eq!(epoch(original), Some((seconds, nanos)));
        }
    }
}
