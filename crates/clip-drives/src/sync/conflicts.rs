//! Complete, resumable preservation of conflicting files and directory trees.
use super::*;

fn variant(path: &Path, side: &str, hash: &str) -> PathBuf {
    let mut name = path.file_stem().unwrap_or_default().to_os_string();
    name.push(format!(".{side}-{}", &hash[..12]));
    if let Some(extension) = path.extension() {
        name.push(".");
        name.push(extension);
    }
    path.with_file_name(name)
}
fn tree_hash(db: &Store, id: &str) -> Result<String> {
    let mut digest = Sha256::new();
    digest.update(b"directory\0");
    for entry in scan::Cursor::new(db, id) {
        let entry = entry?;
        let path = entry.path.as_os_str().as_bytes();
        digest.update((path.len() as u64).to_le_bytes());
        digest.update(path);
        digest.update(signature(&entry)?.as_bytes());
    }
    Ok(format!("{:x}", digest.finalize()))
}
fn tree_scan(
    db: &Store,
    source: &Root,
    path: &Path,
    cancel: &AtomicBool,
) -> Result<(Root, String, String)> {
    let held = source.directory(path, false)?;
    let root = Root::open(&anchor(&held))?;
    let id = format!("conflict-tree:{}", uuid::Uuid::new_v4());
    let coverage = scan::scan(db, &root, &id, &Limits::default(), cancel, |_, _| {})?;
    if !coverage.complete {
        return Err("Cannot preserve a directory with incomplete coverage".into());
    }
    let hash = tree_hash(db, &id)?;
    Ok((root, id, hash))
}
struct PreservedTree {
    root: Root,
    scan: String,
    hash: String,
}
fn content_key(
    db: &Store,
    root: &Root,
    entry: &Entry,
    cancel: &AtomicBool,
) -> Result<(String, Option<PreservedTree>)> {
    if entry.kind == "directory" {
        let (root, scan, hash) = tree_scan(db, root, &entry.path, cancel)?;
        Ok((hash.clone(), Some(PreservedTree { root, scan, hash })))
    } else {
        let hash = signature(entry)?;
        Ok((format!("{:x}", Sha256::digest(hash.as_bytes())), None))
    }
}
fn matches_variant(
    db: &Store,
    root: &Root,
    name: &Path,
    entry: &Entry,
    tree: Option<&PreservedTree>,
    cancel: &AtomicBool,
) -> Result<bool> {
    let Some(metadata) = root.metadata(name)? else {
        return Ok(true);
    };
    match entry.kind.as_str() {
        "file" => Ok(metadata.is_file() && root.digest(name, cancel)? == signature(entry)?),
        "symlink" => {
            if !metadata.file_type().is_symlink() {
                return Ok(false);
            }
            let (_p, path) = root.parent(name, false)?;
            Ok(Some(fs::read_link(path)?) == entry.link)
        }
        "directory" => {
            if !metadata.is_dir() {
                return Ok(false);
            }
            let (_, scan, hash) = tree_scan(db, root, name, cancel)?;
            db.clear_scan(&scan)?;
            Ok(tree.is_some_and(|tree| tree.hash == hash))
        }
        _ => Ok(false),
    }
}
fn choose_variant(
    db: &Store,
    roots: [&Root; 2],
    entry: &Entry,
    side: &str,
    key: &str,
    tree: Option<&PreservedTree>,
    cancel: &AtomicBool,
) -> Result<PathBuf> {
    for attempt in 0..64 {
        let side = if attempt == 0 {
            side.into()
        } else {
            format!("{side}-{attempt}")
        };
        let name = variant(&entry.path, &side, key);
        if matches_variant(db, roots[0], &name, entry, tree, cancel)?
            && matches_variant(db, roots[1], &name, entry, tree, cancel)?
        {
            return Ok(name);
        }
    }
    Err("Too many occupied conflict names; existing files retained".into())
}
fn preserve(
    db: &Store,
    from: &Root,
    to: &Root,
    entry: &Entry,
    name: &Path,
    tree: Option<&PreservedTree>,
    owner: &str,
    cancel: &AtomicBool,
) -> Result<()> {
    if let Some(tree) = tree {
        let recorded = db.get::<crate::StorageIdentity>("conflict-owner", owner)?;
        if to.metadata(name)?.is_none() {
            to.ensure_directory(name)?;
        }
        let held = to.directory(name, false)?;
        let target = Root::open(&anchor(&held))?;
        let identity = target.storage_identity()?;
        if recorded.as_ref().is_some_and(|key| *key != identity) {
            return Err("Preserved conflict directory was replaced".into());
        }
        if recorded.is_none() {
            db.put("conflict-owner", owner, &identity)?;
        }
        for child in scan::Cursor::new(db, &tree.scan) {
            let child = child?;
            if let Some(existing) = scan::observe(&target, &child.path, true, cancel)? {
                if signature(&existing)? != signature(&child)? {
                    return Err("A preserved variant was edited; both originals retained".into());
                }
            } else {
                install(
                    &tree.root,
                    &target,
                    &child,
                    &child.path,
                    None,
                    cancel,
                    false,
                )?;
            }
        }
    } else if let Some(existing) = scan::observe(to, name, true, cancel)? {
        if signature(&existing)? != signature(entry)? {
            return Err("A preserved variant was edited; both originals retained".into());
        }
    } else {
        install(from, to, entry, name, None, cancel, false)?;
    }
    Ok(())
}
impl Reconcile<'_> {
    pub(super) fn preserve_conflict(&mut self, a: &Entry, b: &Entry) -> Result<()> {
        let (ah, at) = content_key(self.db, self.portable, a, self.cancel)?;
        let (bh, bt) = content_key(self.db, self.local, b, self.cancel)?;
        let mut digest = Sha256::new();
        digest.update(self.pair.id.as_bytes());
        digest.update(a.path.as_os_str().as_bytes());
        digest.update(ah.as_bytes());
        digest.update(bh.as_bytes());
        let id = format!("{:x}", digest.finalize());
        if a.kind == "directory" || b.kind == "directory" {
            let mut prefix = a.path.as_os_str().as_bytes().to_vec();
            prefix.push(b'/');
            self.db.0.execute(
                "INSERT OR IGNORE INTO sync_blocked VALUES(?1,?2)",
                params![self.pair.id, prefix],
            )?;
        }
        let mut conflict = if let Some(conflict) = self.db.get::<DriveConflict>("conflict", &id)? {
            conflict
        } else {
            let portable = choose_variant(
                self.db,
                [self.portable, self.local],
                a,
                "portable",
                &ah,
                at.as_ref(),
                self.cancel,
            )?;
            let local = choose_variant(
                self.db,
                [self.portable, self.local],
                b,
                "local",
                &bh,
                bt.as_ref(),
                self.cancel,
            )?;
            let record=DriveConflict{complete:false,id:id.clone(),pair:self.pair.id.clone(),path:a.path.clone(),portable,local,message:"Preserving both sides; originals remain unchanged until both variants are complete".into(),resolved:false};
            self.db.put("conflict", &id, &record)?;
            record
        };
        if !conflict.complete {
            for (root, side) in [(self.portable, "portable"), (self.local, "local")] {
                preserve(
                    self.db,
                    self.portable,
                    root,
                    a,
                    &conflict.portable,
                    at.as_ref(),
                    &format!("{id}:portable:{side}"),
                    self.cancel,
                )?;
                preserve(
                    self.db,
                    self.local,
                    root,
                    b,
                    &conflict.local,
                    bt.as_ref(),
                    &format!("{id}:local:{side}"),
                    self.cancel,
                )?;
            }
            conflict.complete = true;
            conflict.message="Both sides changed. Complete portable/local variants were preserved on both sides. Edit the original to resolve, then sync.".into();
            self.db.put("conflict", &id, &conflict)?;
        }
        self.db.0.execute(
            "INSERT OR IGNORE INTO conflict_paths VALUES(?1,?2,?3)",
            params![self.pair.id, a.path.as_os_str().as_bytes(), id],
        )?;
        if !conflict.resolved {
            self.report.conflicts += 1;
        }
        if let Some(tree) = at {
            self.db.clear_scan(&tree.scan)?;
        }
        if let Some(tree) = bt {
            self.db.clear_scan(&tree.scan)?;
        }
        Ok(())
    }
    pub(super) fn deleted_edit(
        &mut self,
        path: &Path,
        entry: &Entry,
        from_portable: bool,
    ) -> Result<()> {
        let mut digest = Sha256::new();
        digest.update(self.pair.id.as_bytes());
        digest.update(path.as_os_str().as_bytes());
        digest.update(signature(entry)?.as_bytes());
        digest.update(if from_portable {
            b"deleted-local".as_slice()
        } else {
            b"deleted-portable".as_slice()
        });
        let id = format!("{:x}", digest.finalize());
        let conflict = DriveConflict {
            complete: true,
            id: id.clone(),
            pair: self.pair.id.clone(),
            path: path.into(),
            portable: path.into(),
            local: path.into(),
            message: format!(
                "{} deleted this path while the other side edited it. The edit was restored to both folders.",
                if from_portable {
                    "Local folder"
                } else {
                    "Portable drive"
                }
            ),
            resolved: false,
        };
        self.db.put("conflict", &id, &conflict)?;
        self.db.0.execute(
            "INSERT OR IGNORE INTO conflict_paths VALUES(?1,?2,?3)",
            params![self.pair.id, path.as_os_str().as_bytes(), id],
        )?;
        self.report.conflicts += 1;
        Ok(())
    }
    pub(super) fn finish_directories(&mut self) -> Result<()> {
        let mut statement = self
            .db
            .0
            .prepare("SELECT path FROM sync_dirs WHERE pair=?1 ORDER BY path DESC")?;
        let rows = statement.query_map([&self.pair.id], |r| r.get::<_, Vec<u8>>(0))?;
        for row in rows {
            cancelled(self.cancel)?;
            use std::os::unix::ffi::OsStringExt;
            let path = PathBuf::from(std::ffi::OsString::from_vec(row?));
            let mut retained = false;
            for root in [self.portable, self.local] {
                if root.metadata(&path)?.is_some() {
                    let held = root.directory(&path, false)?;
                    if fs::read_dir(anchor(&held))?.next().is_none() {
                        let (parent, target) = root.parent(&path, false)?;
                        root.validate()?;
                        fs::remove_dir(target)?;
                        std::fs::File::open(anchor(&parent))?.sync_all()?;
                    } else {
                        retained = true;
                    }
                }
            }
            if retained {
                self.portable.ensure_directory(&path)?;
                self.local.ensure_directory(&path)?;
                save_baseline(self.db, &self.pair.id, &path, Some("directory"))?;
            } else {
                save_baseline(self.db, &self.pair.id, &path, None)?;
            }
        }
        self.db
            .0
            .execute("DELETE FROM sync_dirs WHERE pair=?1", [&self.pair.id])?;
        Ok(())
    }
}
