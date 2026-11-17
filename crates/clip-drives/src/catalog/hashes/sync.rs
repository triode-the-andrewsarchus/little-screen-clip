//! One catalog pass imports only identity-bound portable scan fingerprints.
use super::*;
use crate::{SyncPair, sync::fingerprints};
use std::collections::BTreeMap;
struct Source {
    scan: String,
    generation: uuid::Uuid,
}

fn sources(
    state: &Store,
    drive: &crate::Drive,
    reuse: &Reuse<'_>,
) -> Result<BTreeMap<PathBuf, Vec<Source>>> {
    // Ordinary enrollment is capped at 256 pairs plus bounded RAM sessions.
    // Bound decoding as well as candidate count for legacy/corrupt registries.
    let mut query = state.0.prepare("SELECT CASE WHEN length(CAST(json AS BLOB))<=16384 THEN json END FROM entities WHERE kind='pair' ORDER BY rowid DESC LIMIT 513")?;
    let rows = query.query_map([], |row| row.get::<_, Option<String>>(0))?;
    let mut sources: BTreeMap<PathBuf, Vec<Source>> = BTreeMap::new();
    for (index, row) in rows.enumerate() {
        cancelled(reuse.cancel)?;
        if reuse.remaining().is_zero() {
            break;
        }
        if index == 512 {
            return Err("Sync fingerprint enrollment budget exceeded".into());
        }
        let pair: SyncPair = serde_json::from_str(&row?.ok_or("Oversized sync enrollment")?)?;
        if pair.drive != drive.id
            || pair.evidence != drive.evidence
            || !crate::valid_relative(&pair.relative)
        {
            continue;
        }
        let id = format!("{}:portable", pair.id);
        if let Ok(Some(provenance)) = fingerprints::Provenance::load(state, &id)
            && provenance.validate(&pair, reuse.source).is_ok()
        {
            sources.entry(pair.relative).or_default().push(Source {
                scan: id,
                generation: provenance.generation(),
            });
        }
    }
    Ok(sources)
}

impl Reuse<'_> {
    fn import_sync_entry(
        &self,
        state: &Store,
        sources: &BTreeMap<PathBuf, Vec<Source>>,
        entry: Entry,
    ) -> Result<()> {
        if entry.kind != "file" || entry.hash.is_some() {
            return Ok(());
        }
        for prefix in entry.path.ancestors().skip(1) {
            if let Some(ids) = sources.get(prefix) {
                for id in ids {
                    cancelled(self.cancel)?;
                    if self.remaining().is_zero() {
                        return Ok(());
                    }
                    if self.import(state, &id.scan, prefix, Some(id.generation), entry.clone())? {
                        return Ok(());
                    }
                }
            }
        }
        Ok(())
    }
    pub(super) fn sync(&self, state: &Store, drive: &crate::Drive) -> Result<()> {
        if self.remaining().is_zero() {
            return Ok(());
        }
        // Scan metadata and its provenance share one registry snapshot; a
        // concurrent scan publication cannot splice two generations together.
        let snapshot = state.0.unchecked_transaction()?;
        let transaction = self.catalog.0.unchecked_transaction()?;
        let result = (|| {
            let sources = sources(state, drive, self)?;
            if sources.is_empty() {
                return Ok(());
            }
            for entry in scan::Cursor::new(self.catalog, "catalog") {
                cancelled(self.cancel)?;
                if self.remaining().is_zero() {
                    break;
                }
                self.import_sync_entry(state, &sources, entry?)?;
            }
            Ok(())
        })();
        snapshot.commit()?;
        self.finish(transaction, result)
    }
}
