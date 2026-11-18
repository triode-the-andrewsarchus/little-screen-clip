//! Durable request identities share the existing serialized operation admission.
use super::*;
use rusqlite::{Transaction, TransactionBehavior};
use sha2::{Digest, Sha256};

#[derive(serde::Serialize, serde::Deserialize)]
struct Submission {
    fingerprint: String,
}

fn replay(db: &Store, id: &str, fingerprint: &str, live_owner: bool) -> Result<bool> {
    match (db.get::<Submission>("submission", id)?, db.get::<DriveOperation>("operation", id)?) {
        (Some(previous), Some(mut operation)) if previous.fingerprint == fingerprint => {
            if !live_owner && operation.finished.is_none() {
                operation.state = "interrupted".into();
                operation.finished = Some(now());
                operation.message = "The retained operation has no live owner; completed writes are retained. Reconcile before submitting new work.".into();
                db.put("operation", id, &operation)?;
            }
            Ok(true)
        }
        (Some(_), Some(_)) => Err("Drive request identity was already used for different parameters; inspect its operation before submitting new work".into()),
        (Some(_), None) => Err("Drive request has no operation record; recovery is required before retrying".into()),
        (None, Some(_)) => Err("Drive operation identity already exists without its request binding; inspect it before submitting new work".into()),
        (None, None) => Ok(false),
    }
}

fn record(
    db: &Store,
    transaction: Transaction<'_>,
    id: &str,
    request: &DriveRequest,
    fingerprint: String,
) -> Result<DriveOperation> {
    if matches!(
        request,
        DriveRequest::Backup {
            kind: BackupKind::Files,
            restore_mounted: true,
            ..
        }
    ) {
        return Err("Mounted-state restoration requires a partition image backup".into());
    }
    let operation = DriveOperation {
        id: id.into(),
        title: title(request).into(),
        state: "queued".into(),
        message: String::new(),
        files: 0,
        bytes: 0,
        started: now(),
        finished: None,
        sync_completion: None,
        partition: None,
    };
    db.put("operation", id, &operation)?;
    db.put("submission", id, &Submission { fingerprint })?;
    // Both records become durable before the owner can observe an envelope.
    transaction.commit()?;
    Ok(operation)
}

fn rejected(db: &Store, mut operation: DriveOperation) -> Error {
    // One statement atomically releases both records on known queue rejection.
    if let Err(error) = db.0.execute(
        "DELETE FROM entities WHERE id=?1 AND kind IN ('operation','submission')",
        [&operation.id],
    ) {
        operation.state = "failed".into();
        operation.finished = Some(now());
        operation.message = format!(
            "Queue rejected this operation before execution; admission cleanup failed: {error}"
        );
        if let Err(record_error) = db.put("operation", &operation.id, &operation) {
            return format!("Queue rejected operation {}, but its completion could not be recorded: {record_error}; inspect status and recover before retrying", operation.id).into();
        }
        return format!("Queue rejected operation {}; its failed result is retained because admission cleanup failed: {error}", operation.id).into();
    }
    "Drive operation queue is full or unavailable; try again".into()
}

impl Service {
    /// Submits a retained operation once for a client-chosen request identity.
    ///
    /// Repeating the same request returns its original operation ID, including a
    /// completed or interrupted result after restart. A retry never restarts work.
    /// Status, review and cancellation requests use [`Self::request`] instead.
    ///
    /// # Errors
    /// Returns admission failures or a reused identity with different parameters.
    pub fn submit(&self, request_id: uuid::Uuid, request: DriveRequest) -> Result<DriveReply> {
        if !request.is_operation() {
            return Err("Only retained drive operations accept a request identity".into());
        }
        self.admit(request_id, request)
    }

    pub(super) fn admit(
        &self,
        request_id: uuid::Uuid,
        request: DriveRequest,
    ) -> Result<DriveReply> {
        let id = request_id.to_string();
        let fingerprint = format!("{:x}", Sha256::digest(serde_json::to_vec(&request)?));
        let mut prepared = None;
        loop {
            let mut tokens = self
                .runtime
                .tokens
                .lock()
                .map_err(|_| "Drive admission unavailable")?;
            let db = Store::existing(&self.state.join("drives.sqlite"), false)?;
            let transaction = Transaction::new_unchecked(&db.0, TransactionBehavior::Immediate)?;
            if replay(&db, &id, &fingerprint, tokens.contains_key(&id))? {
                transaction.commit()?;
                return Ok(DriveReply::Accepted(id));
            }
            if self.runtime.closing.load(Ordering::Acquire) {
                return Err("Drive services are shutting down; admission is closed".into());
            }
            if matches!(request, DriveRequest::Rule { .. }) && prepared.is_none() {
                // Resolve device selectors before acceptance, without holding either
                // the SQLite write transaction or cancellation/admission mutex while
                // lsblk runs. Recheck durable replay/closure after preparation.
                drop(transaction);
                drop(db);
                drop(tokens);
                prepared = Some(rules::bind(request.clone(), &self.inventory)?);
                continue;
            }
            let request = prepared.take().unwrap_or(request);
            let (request, mut review) = self.take_backup_review(request)?;
            let result = (|| {
                let operation = record(&db, transaction, &id, &request, fingerprint)?;
                let cancel = Arc::new(AtomicBool::new(false));
                tokens.insert(id.clone(), cancel.clone());
                if let Err(error) = self.sender.try_send(Envelope {
                    id: id.clone(),
                    request,
                    cancel,
                    review: review.take(),
                }) {
                    let (mpsc::TrySendError::Full(envelope)
                    | mpsc::TrySendError::Disconnected(envelope)) = error;
                    review = envelope.review;
                    tokens.remove(&id);
                    return Err(rejected(&db, operation));
                }
                Ok(DriveReply::Accepted(id))
            })();
            if let Some(review) = review {
                self.previews
                    .lock()
                    .map_err(|_| "Backup review storage unavailable")?
                    .insert(review.info.token.clone(), review);
            }
            return result;
        }
    }
}
