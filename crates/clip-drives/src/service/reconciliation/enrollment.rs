//! Accepted enrollment preparation shares reconciliation's pins, claims and budget.
use super::*;

pub(super) struct Input {
    source: scheduling::Source,
    targets: Vec<SyncPair>,
    pub claims: Vec<crate::root::Scope>,
}
impl Input {
    pub fn current(&self, db: &Store) -> Result<bool> {
        policy::same_pair_roots(db, &self.targets)
    }
    pub fn prepare(
        &self,
        pair: &SyncPair,
        storage: &policy::Storage,
        drive: &Drive,
        cancel: &AtomicBool,
    ) -> Result<Plan> {
        if *drive != self.source.drive {
            return Err("Drive changed while preparing folder enrollment".into());
        }
        if drive.read_only {
            return Err("Drive is read-only; edits remain local".into());
        }
        self.source.root.validate()?;
        let portable = Arc::new(self.source.root.subtree(&pair.relative)?);
        let target = self
            .claims
            .last()
            .ok_or("Missing enrollment destination claim")?;
        if Root::planned_scope(&pair.local)? != *target {
            return Err("Enrollment destination changed since scheduling admission; retry".into());
        }
        cancelled(cancel)?;
        let local = Arc::new(Root::create(&pair.local)?);
        storage.validate(&portable, &local, None)?;
        policy::validate_pair_targets(&self.targets, &local, None)?;
        let mut pair = pair.clone();
        pair.local = local.path.clone();
        pair.local_identity = Some(local.storage_identity()?);
        portable.validate()?;
        local.validate()?;
        Ok(Plan {
            pair,
            portable,
            local,
            cached: false,
            observed: false,
            storage: Some(storage.clone()),
        })
    }
}

impl Owner {
    pub(in crate::service) fn start_enrollment(
        &mut self,
        envelope: Envelope,
        source: Option<scheduling::Source>,
        claims: Vec<crate::root::Scope>,
    ) {
        if self.sync_jobs.len() + self.sync_preparing.len() >= 4 || self.sync_waiting.len() >= 16 {
            self.defer_operation(envelope);
            return;
        }
        let prepared = (|| {
            let DriveRequest::Pair {
                relative, local, ..
            } = &envelope.request
            else {
                return Err("Invalid enrollment request".into());
            };
            if !valid_relative(relative) {
                return Err("Invalid sync subtree".into());
            }
            let source = source.ok_or("Mount the drive before pairing a folder")?;
            let drive = &source.drive;
            if drive.ambiguous || (drive.part_uuid.is_none() && drive.filesystem_uuid.is_none()) {
                return Err("Enrollment requires an unambiguous persistent media identity".into());
            }
            let pair = SyncPair {
                id: uuid::Uuid::new_v4().to_string(),
                drive: drive.id.clone(),
                evidence: drive.evidence.clone(),
                relative: relative.clone(),
                local: local.clone(),
                local_identity: None,
                paused: false,
                deletions: false,
                ram: false,
                last_success: None,
                status: "Waiting for initial union sync".into(),
            };
            let input = Input {
                source,
                targets: policy::records::<SyncPair>(&self.db, "pair")?,
                claims,
            };
            Ok((pair, input))
        })();
        match prepared {
            Ok((pair, input)) => {
                if let Ok(Some(mut operation)) =
                    self.db.get::<DriveOperation>("operation", &envelope.id)
                {
                    operation.state = "preparing".into();
                    operation.message = "Preparing folder enrollment".into();
                    let _ = self.db.put("operation", &envelope.id, &operation);
                }
                self.prepare_sync_roots(
                    Intent {
                        queued: Instant::now(),
                        pair: pair.id.clone(),
                        operation: Some(envelope.id),
                        cancel: envelope.cancel,
                        paths: None,
                        initialize: false,
                        save: None,
                        writeback: None,
                    },
                    pair,
                    Some(input),
                );
            }
            Err(error) => self.finish_operation(&envelope.id, &envelope.cancel, Err(error)),
        }
    }
}
