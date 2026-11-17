//! Bounded per-file collection into an unpublished backup version.
use super::*;
use scan::Entry;
use serde::{Deserialize, Serialize};
use std::os::unix::fs::MetadataExt;

const ATTEMPTS: u8 = 3;

#[derive(Debug, Serialize, Deserialize)]
struct Retry {
    #[serde(with = "little_clip_core::path_serde")]
    path: PathBuf,
    attempts: u8,
}

pub(super) struct Collection<'a> {
    pub db: &'a Store,
    pub id: &'a str,
    pub source: &'a Root,
    pub target: &'a Root,
    pub cancel: &'a AtomicBool,
    pub incremental: bool,
}

impl Collection<'_> {
    pub fn populate(
        &self,
        metadata: &mut metadata::Report<'_>,
        progress: &mut impl FnMut(u64, u64),
    ) -> Result<u64> {
        self.db
            .put("collection", "per_file_attempt_limit", &ATTEMPTS)?;
        let mut written = 0;
        let mut count = 0;
        let mut retried = 0_u64;
        for entry in scan::Cursor::new(self.db, self.id) {
            let entry = entry?;
            cancelled(self.cancel)?;
            if entry.kind == "directory" {
                let collected = self.observe(&entry, false)?;
                prepare_entry(self.db, self.target, &collected, self.cancel)?;
                self.target.ensure_directory(&collected.path)?;
                count += 1;
                progress(count, written);
                continue;
            }
            let (collected, bytes, attempts, snapshot) =
                self.collect(&entry, &mut || progress(count, written))?;
            let transaction = self.db.0.unchecked_transaction()?;
            scan::put(self.db, self.id, &collected)?;
            metadata.record(&collected, snapshot)?;
            if attempts > 1 {
                retried += 1;
                self.db.put(
                    "collection-retry",
                    &count.to_string(),
                    &Retry {
                        path: collected.path,
                        attempts,
                    },
                )?;
            }
            transaction.commit()?;
            written += bytes;
            count += 1;
            progress(count, written);
        }
        self.db.put("collection", "retried_entries", &retried)?;
        Ok(written)
    }

    fn observe(&self, expected: &Entry, hashes: bool) -> Result<Entry> {
        let mut observed =
            scan::observe(self.source, &expected.path, hashes, u64::MAX, self.cancel)?
                .ok_or("Source disappeared during backup collection")?;
        if observed.kind != expected.kind {
            return Err("Source entry changed type; restart backup collection".into());
        }
        observed.retain_accessed(expected);
        Ok(observed)
    }

    fn collect(
        &self,
        expected: &Entry,
        progress: &mut impl FnMut(),
    ) -> Result<(Entry, u64, u8, metadata::Snapshot)> {
        let mut attempt = 0;
        let mut written = 0;
        loop {
            attempt += 1;
            cancelled(self.cancel)?;
            let mut entry = self.observe(expected, false)?;
            let result = self.attempt(&mut entry, progress, &mut written);
            match result {
                Ok(snapshot) => return Ok((entry, written, attempt, snapshot)),
                Err(error) => {
                    cancelled(self.cancel)?;
                    self.source.validate()?;
                    self.target.validate()?;
                    if attempt == ATTEMPTS
                        || !(matches!(error, crate::Error::SourceChanged(_))
                            || self.source_changed(&entry)?)
                    {
                        return Err(format!(
                            "Could not collect {} after {attempt} attempt(s); previous backup retained: {error}",
                            expected.path.display()
                        ).into());
                    }
                }
            }
        }
    }

    fn attempt(
        &self,
        entry: &mut Entry,
        progress: &mut impl FnMut(),
        written: &mut u64,
    ) -> Result<metadata::Snapshot> {
        if entry.kind == "file" {
            self.target.check_file_size(entry.size)?;
        }
        let mut snapshot = metadata::Snapshot::capture(self.source, entry, self.cancel)?;
        *entry = self.observe(entry, true)?;
        snapshot.check(self.source, entry, self.cancel)?;
        progress();
        cancelled(self.cancel)?;
        prepare_entry(self.db, self.target, entry, self.cancel)?;
        *written += self.install(entry)?;
        snapshot.restore(self.target, entry, self.cancel)?;
        snapshot.check(self.source, entry, self.cancel)?;
        let after = self.observe(entry, false)?;
        if !entry.same_revision(&after) || entry.link != after.link {
            return Err("Source changed during backup collection".into());
        }
        Ok(snapshot)
    }

    fn source_changed(&self, entry: &Entry) -> Result<bool> {
        let observed = self.observe(entry, false)?;
        if !entry.same_revision(&observed) || entry.link != observed.link {
            return Ok(true);
        }
        if entry.kind != "file" || self.source.reliable_revision_cache()? {
            return Ok(false);
        }
        // Coarse/preserved timestamps cannot establish that a failed copy's
        // source stayed unchanged. Rehash before deciding whether to retry.
        match self.observe(entry, true) {
            Ok(observed) => Ok(entry.hash != observed.hash),
            Err(_) => {
                cancelled(self.cancel)?;
                self.source.validate()?;
                Ok(true)
            }
        }
    }

    fn install(&self, entry: &Entry) -> Result<u64> {
        let old = if self.target.metadata(&entry.path)?.is_some() {
            scan::observe(self.target, &entry.path, true, u64::MAX, self.cancel)?
        } else {
            None
        };
        if entry.kind == "file"
            && old.as_ref().and_then(|old| old.hash.as_ref()) == entry.hash.as_ref()
            && self.target.input(&entry.path)?.metadata()?.nlink() == 1
        {
            return Ok(0);
        }
        if entry.kind == "file" {
            return self.target.copy(
                self.source,
                crate::root::CopySpec {
                    source: &entry.path,
                    destination: &entry.path,
                    source_hash: entry.hash.as_deref(),
                    destination_hash: old.as_ref().and_then(|entry| entry.hash.as_deref()),
                    delta: self.incremental,
                    basic_metadata: false,
                },
                self.cancel,
            );
        }
        sync::install(
            self.source,
            self.target,
            entry,
            &entry.path,
            old.as_ref(),
            self.cancel,
            self.incremental,
        )
    }
}
