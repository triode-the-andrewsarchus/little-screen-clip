//! Baseline-based union reconciliation; incomplete scans never authorize deletions.
use crate::{
    DriveConflict, Result, SyncPair, cancelled,
    root::{Root, anchor},
    scan::{self, Entry, Limits},
    store::Store,
};
use rusqlite::{OptionalExtension, params};
use sha2::{Digest, Sha256};
use std::{
    fs,
    os::unix::{ffi::OsStrExt, fs::symlink},
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
};

fn signature(entry: &Entry) -> Result<String> {
    match entry.kind.as_str() {
        "file" => entry
            .hash
            .clone()
            .ok_or_else(|| "Missing content fingerprint".into()),
        "directory" => Ok("directory".into()),
        "symlink" => Ok(format!(
            "link:{:x}",
            Sha256::digest(
                entry
                    .link
                    .as_ref()
                    .ok_or("Missing symlink target")?
                    .as_os_str()
                    .as_bytes()
            )
        )),
        _ => Err("Unverified tree entry cannot authorize synchronization".into()),
    }
}
pub(crate) fn install(
    from: &Root,
    to: &Root,
    entry: &Entry,
    target: &Path,
    existing: Option<&Entry>,
    cancel: &AtomicBool,
    delta: bool,
) -> Result<u64> {
    match entry.kind.as_str() {
        "directory" => {
            to.ensure_directory(target)?;
            Ok(0)
        }
        "file" => to.copy(
            from,
            &entry.path,
            target,
            existing.and_then(|e| e.hash.as_deref()),
            entry.hash.as_deref(),
            cancel,
            delta,
        ),
        "symlink" => {
            cancelled(cancel)?;
            let (source_parent, source) = from.parent(&entry.path, false)?;
            let link = fs::read_link(&source)?;
            if entry.link.as_ref() != Some(&link)
                || !from.same_file(&entry.path, entry.device, entry.inode)?
            {
                return Err("Symlink changed".into());
            }
            let (parent, destination) = to.parent(target, true)?;
            let held = tempfile::tempdir_in(anchor(&parent))?;
            let staged = held.path().join("link");
            symlink(&link, &staged)?;
            if let Some(old) = existing {
                if old.kind != "symlink"
                    || fs::read_link(&destination)?
                        != *old.link.as_ref().ok_or("Missing destination link")?
                {
                    return Err("Symlink destination changed".into());
                }
                to.validate()?;
                from.validate()?;
                fs::rename(&staged, &destination)?;
            } else {
                little_clip_core::file_source::rename_noreplace(&staged, &destination)?;
            }
            drop(source_parent);
            std::fs::File::open(anchor(&parent))?.sync_all()?;
            Ok(0)
        }
        _ => Err("Special or excluded files cannot be synchronized".into()),
    }
}
fn baseline(db: &Store, pair: &str, path: &Path) -> Result<Option<String>> {
    Ok(db
        .0
        .query_row(
            "SELECT hash FROM baseline WHERE pair=?1 AND path=?2",
            params![pair, path.as_os_str().as_bytes()],
            |r| r.get(0),
        )
        .optional()?)
}
fn save_baseline(db: &Store, pair: &str, path: &Path, hash: Option<&str>) -> Result<()> {
    if let Some(hash) = hash {
        db.0.execute("INSERT INTO baseline VALUES(?1,?2,?3) ON CONFLICT(pair,path) DO UPDATE SET hash=excluded.hash",params![pair,path.as_os_str().as_bytes(),hash])?;
    } else {
        db.0.execute(
            "DELETE FROM baseline WHERE pair=?1 AND path=?2",
            params![pair, path.as_os_str().as_bytes()],
        )?;
    }
    Ok(())
}
fn variant(path: &Path, side: &str, hash: &str) -> PathBuf {
    let stem = path.file_stem().unwrap_or_default();
    let mut name = stem.to_os_string();
    name.push(format!(".{side}-{}", &hash[..12.min(hash.len())]));
    if let Some(extension) = path.extension() {
        name.push(".");
        name.push(extension);
    }
    path.with_file_name(name)
}
fn preserve_variant(
    db: &Store,
    scan: &str,
    source: &Root,
    other: &Root,
    entry: &Entry,
    side: &str,
    cancel: &AtomicBool,
) -> Result<PathBuf> {
    let hash = signature(entry)?;
    let name = variant(&entry.path, side, &hash);
    for (root, original) in [(source, true), (other, false)] {
        if let Some(existing) = root.metadata(&name)? {
            let same =
                entry.kind == "file" && existing.is_file() && root.digest(&name, cancel)? == hash;
            if !same {
                return Err(
                    "Conflict variant name is already occupied; existing file retained".into(),
                );
            }
        } else {
            let _ = original;
            install(source, root, entry, &name, None, cancel, false)?;
        }
    }
    let _ = (db, scan);
    Ok(name)
}
fn conflict(
    db: &Store,
    pair: &SyncPair,
    portable: &Root,
    local: &Root,
    a: &Entry,
    b: &Entry,
    cancel: &AtomicBool,
) -> Result<()> {
    let mut digest = Sha256::new();
    digest.update(pair.id.as_bytes());
    digest.update(a.path.as_os_str().as_bytes());
    digest.update(signature(a)?.as_bytes());
    digest.update(signature(b)?.as_bytes());
    let id = format!("{:x}", digest.finalize());
    if db.get::<DriveConflict>("conflict", &id)?.is_some() {
        return Ok(());
    }
    if a.kind == "directory" || b.kind == "directory" {
        return Err(format!(
            "File/directory conflict at {}; both trees retained",
            a.path.display()
        )
        .into());
    }
    let portable_variant = preserve_variant(db, "", portable, local, a, "portable", cancel)?;
    let local_variant = preserve_variant(db, "", local, portable, b, "local", cancel)?;
    db.put("conflict",&id,&DriveConflict{id:id.clone(),pair:pair.id.clone(),path:a.path.clone(),portable:portable_variant,local:local_variant,message:"Both sides changed. Both variants were preserved; edit the original to resolve, then sync.".into(),resolved:false})?;
    Ok(())
}
fn delete(root: &Root, entry: &Entry, cancel: &AtomicBool) -> Result<()> {
    match entry.kind.as_str() {
        "file" => root.delete_file(
            &entry.path,
            entry
                .hash
                .as_deref()
                .ok_or("Missing deletion fingerprint")?,
            cancel,
        ),
        "symlink" => {
            cancelled(cancel)?;
            let (parent, path) = root.parent(&entry.path, false)?;
            if fs::read_link(&path)? != *entry.link.as_ref().ok_or("Missing link")? {
                return Err("Symlink changed".into());
            }
            fs::remove_file(path)?;
            std::fs::File::open(anchor(&parent))?.sync_all()?;
            Ok(())
        }
        _ => Ok(()),
    }
}
#[derive(Default)]
pub(crate) struct SyncReport {
    pub files: u64,
    pub bytes: u64,
    pub conflicts: u64,
}
struct Reconcile<'a> {
    db: &'a Store,
    pair: &'a SyncPair,
    portable: &'a Root,
    local: &'a Root,
    cancel: &'a AtomicBool,
    report: SyncReport,
}
impl Reconcile<'_> {
    fn path(&mut self, path: &Path, a: Option<Entry>, b: Option<Entry>) -> Result<()> {
        cancelled(self.cancel)?;
        self.portable.validate()?;
        self.local.validate()?;
        let old = baseline(self.db, &self.pair.id, path)?;
        let ah = a.as_ref().map(signature).transpose()?;
        let bh = b.as_ref().map(signature).transpose()?;
        if ah == bh {
            if old != ah {
                save_baseline(self.db, &self.pair.id, path, ah.as_deref())?;
            }
            return Ok(());
        }
        let direction = match (&a, &b) {
            (Some(_), None) => Some(true),
            (None, Some(_)) => Some(false),
            (Some(_), Some(_)) if old == ah => Some(false),
            (Some(_), Some(_)) if old == bh => Some(true),
            (Some(a), Some(b)) => {
                conflict(
                    self.db,
                    self.pair,
                    self.portable,
                    self.local,
                    a,
                    b,
                    self.cancel,
                )?;
                self.report.conflicts += 1;
                None
            }
            _ => None,
        };
        if let Some(portable_to_local) = direction {
            let (from, to, entry, existing, hash) = if portable_to_local {
                (
                    self.portable,
                    self.local,
                    a.as_ref().unwrap(),
                    b.as_ref(),
                    ah.as_deref(),
                )
            } else {
                (
                    self.local,
                    self.portable,
                    b.as_ref().unwrap(),
                    a.as_ref(),
                    bh.as_deref(),
                )
            };
            if existing.is_none()
                && self.pair.deletions
                && old.as_deref() == hash
                && entry.kind != "directory"
            {
                delete(from, entry, self.cancel)?;
                save_baseline(self.db, &self.pair.id, path, None)?;
            } else {
                self.report.bytes += install(from, to, entry, path, existing, self.cancel, false)?;
                self.report.files += 1;
                save_baseline(self.db, &self.pair.id, path, hash)?;
            }
        }
        Ok(())
    }
}
pub(crate) fn synchronize(
    db: &Store,
    pair: &SyncPair,
    portable: &Root,
    local: &Root,
    cancel: &AtomicBool,
    mut progress: impl FnMut(u64, u64),
) -> Result<SyncReport> {
    if portable.overlap(local) {
        return Err("Sync roots overlap".into());
    }
    let a = format!("{}:portable", pair.id);
    let b = format!("{}:local", pair.id);
    for (root, id) in [(portable, &a), (local, &b)] {
        let coverage = scan::scan(db, root, id, &Limits::default(), cancel, &mut progress)?;
        if !coverage.complete {
            return Err(format!(
                "Synchronization requires complete coverage: {}",
                coverage.message
            )
            .into());
        }
    }
    let mut after = Vec::new();
    let mut reconcile = Reconcile {
        db,
        pair,
        portable,
        local,
        cancel,
        report: SyncReport::default(),
    };
    loop {
        let paths = {
            let mut statement=db.0.prepare("SELECT path FROM entries WHERE scan IN (?1,?2) AND path>?4 UNION SELECT path FROM baseline WHERE pair=?3 AND path>?4 ORDER BY path LIMIT 128")?;
            statement
                .query_map(params![a, b, pair.id, &after], |r| r.get::<_, Vec<u8>>(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?
        };
        if paths.is_empty() {
            break;
        }
        for bytes in paths {
            use std::os::unix::ffi::OsStringExt;
            let path = PathBuf::from(std::ffi::OsString::from_vec(bytes.clone()));
            reconcile.path(
                &path,
                scan::entry(db, &a, &path)?,
                scan::entry(db, &b, &path)?,
            )?;
            after = bytes;
            progress(reconcile.report.files, reconcile.report.bytes);
        }
    }
    portable.validate()?;
    local.validate()?;
    db.clear_scan(&a)?;
    db.clear_scan(&b)?;
    Ok(reconcile.report)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn pair() -> SyncPair {
        SyncPair {
            local_identity: None,
            id: "pair".into(),
            drive: "drive".into(),
            evidence: "test".into(),
            relative: PathBuf::new(),
            local: PathBuf::new(),
            paused: false,
            deletions: false,
            ram: false,
            last_success: None,
            status: String::new(),
        }
    }
    #[test]
    fn union_offline_conflicts_and_confirmed_deletions_preserve_data() {
        let temp = tempfile::tempdir().unwrap();
        let a = temp.path().join("portable");
        let b = temp.path().join("local");
        fs::create_dir(&a).unwrap();
        fs::create_dir(&b).unwrap();
        fs::write(a.join("one"), b"one").unwrap();
        fs::write(b.join("two"), b"two").unwrap();
        let db = Store::open(&temp.path().join("db")).unwrap();
        let cancel = AtomicBool::new(false);
        let mut pair = pair();
        let run = |pair: &SyncPair| {
            synchronize(
                &db,
                pair,
                &Root::open(&a).unwrap(),
                &Root::open(&b).unwrap(),
                &cancel,
                |_, _| {},
            )
            .unwrap()
        };
        run(&pair);
        assert_eq!(fs::read(b.join("one")).unwrap(), b"one");
        assert_eq!(fs::read(a.join("two")).unwrap(), b"two");
        fs::write(a.join("one"), b"portable edit").unwrap();
        fs::write(b.join("one"), b"local edit").unwrap();
        assert_eq!(run(&pair).conflicts, 1);
        let conflicts: Vec<DriveConflict> = db.list("conflict", 0, 100).unwrap();
        assert_eq!(conflicts.len(), 1);
        for root in [&a, &b] {
            assert_eq!(
                fs::read(root.join(&conflicts[0].portable)).unwrap(),
                b"portable edit"
            );
            assert_eq!(
                fs::read(root.join(&conflicts[0].local)).unwrap(),
                b"local edit"
            );
        }
        run(&pair);
        assert_eq!(
            db.list::<DriveConflict>("conflict", 0, 100).unwrap().len(),
            1
        );
        fs::remove_file(a.join("two")).unwrap();
        run(&pair);
        assert!(a.join("two").exists());
        pair.deletions = true;
        fs::remove_file(a.join("two")).unwrap();
        run(&pair);
        assert!(!b.join("two").exists());
    }
    #[test]
    fn same_size_same_timestamp_edits_still_conflict() {
        let temp = tempfile::tempdir().unwrap();
        let a = temp.path().join("a");
        let b = temp.path().join("b");
        fs::create_dir(&a).unwrap();
        fs::create_dir(&b).unwrap();
        fs::write(a.join("x"), b"0000").unwrap();
        let db = Store::open(&temp.path().join("db")).unwrap();
        let cancel = AtomicBool::new(false);
        let pair = pair();
        let run = || {
            synchronize(
                &db,
                &pair,
                &Root::open(&a).unwrap(),
                &Root::open(&b).unwrap(),
                &cancel,
                |_, _| {},
            )
            .unwrap()
        };
        run();
        let time = fs::metadata(a.join("x")).unwrap().modified().unwrap();
        fs::write(a.join("x"), b"1111").unwrap();
        fs::write(b.join("x"), b"2222").unwrap();
        for root in [&a, &b] {
            fs::File::options()
                .write(true)
                .open(root.join("x"))
                .unwrap()
                .set_times(fs::FileTimes::new().set_modified(time))
                .unwrap();
        }
        assert_eq!(run().conflicts, 1);
    }
}
