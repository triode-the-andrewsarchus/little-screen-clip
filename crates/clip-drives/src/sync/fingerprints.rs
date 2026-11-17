//! Bind reusable portable scan rows to the observed enrollment and source lifetime.
use super::*;
use crate::{StorageIdentity, mounts::Lifetime};
use serde::{Deserialize, Serialize};

pub(crate) const KIND: &str = "sync-fingerprints";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Provenance {
    #[serde(flatten)]
    context: Context,
    generation: uuid::Uuid,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Context {
    version: u32,
    drive: String,
    evidence: String,
    #[serde(with = "little_clip_core::path_serde")]
    relative: PathBuf,
    root: StorageIdentity,
    lifetime: Lifetime,
}
impl Provenance {
    pub fn load(db: &Store, id: &str) -> Result<Option<Self>> {
        db.get_bounded(KIND, id, 16 * 1024)
    }

    pub fn capture(pair: &SyncPair, root: &Root) -> Result<Self> {
        root.validate()?;
        if !root.queried_change_tracking()? {
            return Err("Source has no queried-change tracking provenance".into());
        }
        let identity = root.storage_identity()?;
        if identity.birth.is_none() {
            return Err("Source root has no persistent birth identity".into());
        }
        let pin = root.directory(Path::new(""), false)?;
        Ok(Self {
            generation: uuid::Uuid::new_v4(),
            context: Context {
                version: 1,
                drive: pair.drive.clone(),
                evidence: pair.evidence.clone(),
                relative: pair.relative.clone(),
                root: identity,
                lifetime: Lifetime::capture(&pin)?,
            },
        })
    }
    pub fn generation(&self) -> uuid::Uuid {
        self.generation
    }
    fn same_source(&self, other: &Self) -> bool {
        self.context == other.context && !self.generation.is_nil()
    }
    pub fn validate(&self, pair: &SyncPair, source: &Root) -> Result<()> {
        if self.context.version != 1
            || self.generation.is_nil()
            || !crate::valid_relative(&pair.relative)
        {
            return Err("Unsupported sync fingerprint provenance".into());
        }
        let pin = source.directory(&pair.relative, false)?;
        if self.context.drive != pair.drive
            || self.context.evidence != pair.evidence
            || self.context.relative != pair.relative
            || self.context.root.birth.is_none()
            || StorageIdentity::pinned(&pin)? != self.context.root
        {
            return Err("Sync fingerprints belong to another source".into());
        }
        Ok(self.context.lifetime.validate(&pin)?)
    }
}

/// Named updates retain a proven row generation; they cannot upgrade a legacy scan.
pub(super) fn existing(db: &Store, pair: &SyncPair, root: &Root) -> Option<Provenance> {
    let current = Provenance::capture(pair, root).ok()?;
    let previous = Provenance::load(db, &format!("{}:portable", pair.id)).ok()??;
    previous.same_source(&current).then_some(previous)
}

/// Only rows carrying a proven generation can avoid fresh hashing on an eligible source.
pub(super) fn admit_reuse(
    db: &Store,
    pair: &SyncPair,
    current: Option<&Provenance>,
    limits: &mut Limits,
) {
    let Some(current) = current.filter(|_| limits.reuse_scan.is_some()) else {
        return;
    };
    let previous = Provenance::load(db, &format!("{}:portable", pair.id))
        .ok()
        .flatten();
    if let Some(previous) = previous.filter(|prior| prior.same_source(current)) {
        limits.reuse_generation = Some(previous.generation);
    } else {
        limits.reuse_scan = None;
    }
}

/// Called in the same transaction that replaces the portable scan rows.
pub(super) fn publish(
    db: &Store,
    id: &str,
    pair: &SyncPair,
    root: &Root,
    provenance: Option<&Provenance>,
) -> Result<()> {
    db.remove(KIND, id)?;
    if let Some(provenance) = provenance.filter(|saved| {
        Provenance::capture(pair, root).is_ok_and(|current| saved.same_source(&current))
    }) {
        db.put(KIND, id, provenance)?;
    }
    Ok(())
}

/// Refresh only freshly observed portable rows; incoming copies await observation.
pub(super) fn refresh(
    db: &Store,
    pair: &SyncPair,
    root: &Root,
    provenance: &Provenance,
    entries: Vec<(PathBuf, Option<Entry>)>,
) -> Result<()> {
    let id = format!("{}:portable", pair.id);
    let transaction = db.0.unchecked_transaction()?;
    if Provenance::load(db, &id).ok().flatten().as_ref() != Some(provenance)
        || !Provenance::capture(pair, root).is_ok_and(|current| provenance.same_source(&current))
    {
        return Ok(());
    }
    for (path, entry) in entries {
        if let Some(mut entry) = entry {
            entry.accessed = None;
            entry.hash_generation = Some(provenance.generation);
            scan::put(db, &id, &entry)?;
        } else {
            db.0.execute(
                "DELETE FROM entries WHERE scan=?1 AND path=?2",
                params![id, path.as_os_str().as_bytes()],
            )?;
        }
    }
    transaction.commit()?;
    Ok(())
}
