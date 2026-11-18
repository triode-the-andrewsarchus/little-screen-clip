//! Local ownership receipts keep interrupted unpublished files out of user unions.
use super::*;
use crate::{StorageIdentity, root::CopyStage};
use serde::{Deserialize, Serialize};
use std::{fs::File, os::unix::fs::FileExt, time::Duration};

const KIND: &str = "sync-stage";
#[derive(Serialize, Deserialize)]
struct Pending {
    drive: String,
    evidence: String,
    pair: String,
    portable: bool,
    #[serde(with = "little_clip_core::path_serde")]
    path: PathBuf,
    local_identity: Option<StorageIdentity>,
    size: u64,
    hash: String,
    created: bool,
}
fn names(id: &str) -> Result<(String, String)> {
    let id = uuid::Uuid::parse_str(id).map_err(|_| "Invalid staging receipt identifier")?;
    Ok((
        format!(".little-clip-stage-{id}"),
        format!(".little-clip-delete-{id}"),
    ))
}

pub(super) struct Copy<'a> {
    db: &'a Store,
    id: String,
    pending: Pending,
    file: Option<File>,
    digest: Sha256,
    bytes: u64,
    buffer: Vec<u8>,
}
impl<'a> Copy<'a> {
    pub fn new(owner: &Reconcile<'a>, to: &Root, target: &Path, entry: &Entry) -> Result<Self> {
        let (portable, root) = if to.path.starts_with(&owner.portable.path) {
            (true, owner.portable)
        } else if to.path.starts_with(&owner.local.path) {
            (false, owner.local)
        } else {
            return Err("Staging destination is outside the enrolled sync roots".into());
        };
        let relative = to
            .path
            .strip_prefix(&root.path)
            .map_err(|_| "Invalid staging root")?
            .join(target);
        if !crate::valid_relative(&relative) {
            return Err("Invalid staging destination".into());
        }
        Ok(Self {
            db: owner.db,
            id: uuid::Uuid::new_v4().to_string(),
            pending: Pending {
                drive: owner.pair.drive.clone(),
                evidence: owner.pair.evidence.clone(),
                pair: owner.pair.id.clone(),
                portable,
                path: if portable {
                    owner.pair.relative.join(relative)
                } else {
                    relative
                },
                local_identity: if portable {
                    None
                } else {
                    Some(root.storage_identity()?)
                },
                size: entry.size,
                hash: entry
                    .hash
                    .clone()
                    .ok_or("Staging source has no content fingerprint")?,
                created: false,
            },
            file: None,
            digest: Sha256::new(),
            bytes: 0,
            buffer: vec![0; 256 * 1024],
        })
    }
    pub fn finish(&self) -> Result<()> {
        retire(self.db, &self.id)
    }
    fn proof(&self) -> Result<()> {
        self.db.0.execute(
            "INSERT INTO sync_stage_proofs VALUES(?1,?2,?3)",
            params![
                self.id,
                self.bytes,
                format!("{:x}", self.digest.clone().finalize())
            ],
        )?;
        Ok(())
    }
}
impl CopyStage for Copy<'_> {
    fn create(&mut self, parent: &File) -> Result<tempfile::NamedTempFile> {
        // Commit a name before creating it. A crash in the creation/receipt gap
        // leaves an uncertain candidate, never permission to delete a user file.
        let transaction = self.db.0.unchecked_transaction()?;
        self.db.0.execute("CREATE TABLE IF NOT EXISTS sync_stage_proofs (id TEXT NOT NULL,bytes INTEGER NOT NULL,hash TEXT NOT NULL,PRIMARY KEY(id,bytes))", [])?;
        self.db
            .0
            .execute("UPDATE version SET value=max(value,4) WHERE id=1", [])?;
        self.db.put(KIND, &self.id, &self.pending)?;
        transaction.commit()?;
        let (name, _) = names(&self.id)?;
        let file = tempfile::Builder::new()
            .prefix(&name)
            .rand_bytes(0)
            .tempfile_in(anchor(parent))?;
        self.file = Some(file.as_file().try_clone()?);
        self.pending.created = true;
        let transaction = self.db.0.unchecked_transaction()?;
        self.db.put(KIND, &self.id, &self.pending)?;
        self.proof()?;
        transaction.commit()?;
        Ok(file)
    }
    fn progress(&mut self, bytes: u64) -> Result<()> {
        let end = self
            .bytes
            .checked_add(bytes)
            .ok_or("Staging proof length overflow")?;
        if end > self.pending.size {
            return Err("Staging contents exceed the admitted source length".into());
        }
        let file = self.file.as_ref().ok_or("Staging descriptor is missing")?;
        while self.bytes < end {
            let amount = (end - self.bytes).min(self.buffer.len() as u64) as usize;
            file.read_exact_at(&mut self.buffer[..amount], self.bytes)?;
            self.digest.update(&self.buffer[..amount]);
            self.bytes += amount as u64;
        }
        // Persist every acknowledged prefix before the next payload write. Only
        // hashes are stored locally; RAM-first payload bytes remain in RAM.
        self.proof()
    }
}
fn retire(db: &Store, id: &str) -> Result<()> {
    let transaction = db.0.unchecked_transaction()?;
    db.0.execute("DELETE FROM sync_stage_proofs WHERE id=?1", [id])?;
    db.remove(KIND, id)?;
    transaction.commit()?;
    Ok(())
}
pub(super) fn pending(db: &Store, pair: &SyncPair) -> Result<bool> {
    Ok(db.0.query_row(
        "SELECT EXISTS(SELECT 1 FROM entities WHERE kind=?1 AND (NOT json_valid(json) OR json_extract(json,'$.drive')=?2))",
        params![KIND, pair.drive], |row| row.get(0),
    )?)
}

/// Called only after enrollment/clone/root validation, before any user inventory.
pub(super) fn recover(
    db: &Store,
    pair: &SyncPair,
    roots: (&Root, &Root),
    cancel: &AtomicBool,
) -> Result<()> {
    if !pending(db, pair)? {
        return Ok(());
    }
    let mut recovery = Recovery {
        db,
        pair,
        roots,
        cancel,
        scans: [None, None],
    };
    let result = recovery.run();
    for scan in recovery.scans.into_iter().flatten() {
        db.clear_scan(&scan)?;
    }
    result
}

/// Own the two metadata snapshots for the entire bounded receipt walk, including
/// failure cleanup. Individual receipts cannot retire another side's snapshot.
struct Recovery<'a> {
    db: &'a Store,
    pair: &'a SyncPair,
    roots: (&'a Root, &'a Root),
    cancel: &'a AtomicBool,
    scans: [Option<String>; 2],
}
impl Recovery<'_> {
    fn run(&mut self) -> Result<()> {
        let mut after = String::new();
        loop {
            let ids = self.db.ids(KIND, &after, 128)?;
            if ids.is_empty() {
                return Ok(());
            }
            for id in ids {
                self.record(&id)?;
                after = id;
            }
        }
    }
    fn record(&mut self, id: &str) -> Result<()> {
        let record: Pending = self
            .db
            .get_bounded(KIND, id, 64 * 1024)?
            .ok_or("Staging receipt disappeared")?;
        let Some(index) = record.side(self.pair, self.roots)? else {
            return Ok(());
        };
        let root = [self.roots.0, self.roots.1][index];
        if self.scans[index].is_none() {
            self.scans[index] = Some(inventory(self.db, self.pair, root, index, self.cancel)?);
        }
        let scan = self.scans[index]
            .as_ref()
            .ok_or("Missing staging inventory")?;
        recover_one(self.db, root, scan, id, &record, self.cancel)
    }
}

fn recover_one(
    db: &Store,
    root: &Root,
    scan: &str,
    id: &str,
    record: &Pending,
    cancel: &AtomicBool,
) -> Result<()> {
    let (stage, quarantine) = names(id)?;
    let mut statement = db.0.prepare(
        "SELECT json FROM entries WHERE scan=?1 AND lower(CAST(name AS TEXT)) IN (?2,?3) LIMIT 2",
    )?;
    let candidates = statement
        .query_map(params![scan, stage, quarantine], |row| {
            row.get::<_, String>(0)
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if candidates.len() > 1 {
        return Err("Ambiguous interrupted staging names; files retained".into());
    }
    let Some(json) = candidates.first() else {
        return retire(db, id);
    };
    let candidate: Entry = serde_json::from_str(json)?;
    let expected = record
        .verify(db, root, id, &candidate, cancel)
        .map_err(|error| {
            crate::Error::Message(format!(
                "{error}; review {}",
                little_clip_core::path_address::display_path(&root.path.join(&candidate.path))
            ))
        })?;
    // Persist the alternate name before the atomic quarantine. A second removal
    // during cleanup can therefore recover either name on the next mount.
    let alternate = if candidate.path.file_name().is_some_and(|name| {
        name.as_encoded_bytes()
            .eq_ignore_ascii_case(stage.as_bytes())
    }) {
        &quarantine
    } else {
        &stage
    };
    root.delete_file_as(
        &candidate.path,
        &expected,
        cancel,
        Some(std::ffi::OsStr::new(alternate)),
    )?;
    retire(db, id)
}

fn inventory(
    db: &Store,
    pair: &SyncPair,
    root: &Root,
    index: usize,
    cancel: &AtomicBool,
) -> Result<String> {
    let id = format!("staging-recovery:{}:{index}", pair.id);
    let limits = Limits {
        hashes: false,
        entries: 1_000_000,
        bytes: 128 * 1024 * 1024,
        timeout: Duration::from_secs(30),
        ..Default::default()
    };
    let coverage = scan::scan(db, root, &id, &limits, cancel, &mut |_, _| {})?;
    if !coverage.complete {
        db.clear_scan(&id)?;
        return Err(format!(
            "Interrupted staging requires complete metadata coverage: {}; files retained",
            coverage.message
        )
        .into());
    }
    Ok(id)
}
impl Pending {
    fn side(&self, pair: &SyncPair, roots: (&Root, &Root)) -> Result<Option<usize>> {
        if self.drive != pair.drive || (!self.portable && self.pair != pair.id) {
            return Ok(None);
        }
        if self.evidence != pair.evidence || !crate::valid_relative(&self.path) {
            return Err(
                "Interrupted staging belongs to different drive evidence; files retained".into(),
            );
        }
        if self.portable {
            return Ok(self.path.starts_with(&pair.relative).then_some(0));
        }
        if self.local_identity != Some(roots.1.storage_identity()?) {
            return Err("Interrupted staging local root was replaced; files retained".into());
        }
        Ok(Some(1))
    }
    fn verify(
        &self,
        db: &Store,
        root: &Root,
        id: &str,
        candidate: &Entry,
        cancel: &AtomicBool,
    ) -> Result<String> {
        if !self.created || candidate.kind != "file" || candidate.size > self.size {
            return Err("Unverified interrupted staging candidate; files retained".into());
        }
        let proof: Option<String> =
            db.0.query_row(
                "SELECT hash FROM sync_stage_proofs WHERE id=?1 AND bytes=?2",
                params![id, candidate.size],
                |row| row.get(0),
            )
            .optional()?;
        let expected = proof
            .or_else(|| (candidate.size == self.size).then(|| self.hash.clone()))
            .ok_or("Interrupted staging has no proof for its persisted length; files retained")?;
        if root.digest(&candidate.path, self.size, cancel)? != expected {
            return Err("Interrupted staging was changed; files retained".into());
        }
        Ok(expected)
    }
}

#[cfg(test)]
mod tests;
