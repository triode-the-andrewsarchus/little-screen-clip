//! Image controls retain worker ownership and exclude unknown mount locations.
use super::*;
use serde::{Deserialize, Serialize};

pub(super) use crate::backup::IMAGE_CONTROL_KIND as KIND;
const LIMIT: u64 = 64 * 1024;

use crate::backup::{ImageAction as Action, ImagePhase as Phase};

#[derive(Clone, Serialize, Deserialize)]
struct Intent {
    operation: String,
    backup: String,
    original: serde_json::Value,
    mounting: bool,
    inherited: bool,
    phase: Phase,
}
impl Intent {
    fn begin(db: &Store, id: &str, operation: &str, mounting: bool) -> Result<Self> {
        let transaction = db.0.unchecked_transaction()?;
        let original = db
            .get_bounded::<serde_json::Value>("backup", id, LIMIT)?
            .ok_or("Unknown backup version")?;
        let previous = db.get_bounded::<Self>(KIND, id, LIMIT)?;
        let intent = Self::admit(id, operation, original, mounting, previous)?;
        db.put(KIND, id, &intent)?;
        db.0.execute(
            "UPDATE version SET value=3 WHERE id=1 AND value IN (1,2)",
            [],
        )?;
        transaction.commit()?;
        Ok(intent)
    }
    fn admit(
        id: &str,
        operation: &str,
        original: serde_json::Value,
        mounting: bool,
        previous: Option<Self>,
    ) -> Result<Self> {
        let version: DriveBackup = serde_json::from_value(original.clone())?;
        if version.id != id || version.kind != BackupKind::Image {
            return Err("This backup is not an addressed partition image".into());
        }
        if mounting
            && (previous.is_some() || version.mount.is_some() || version.loop_object.is_some())
        {
            return Err("Image already has a loop mapping or pending control; unmount/recover it before retrying".into());
        }
        if let Some(previous) = &previous {
            previous.check_retry(&version)?;
        }
        Ok(Self {
            operation: operation.into(),
            backup: id.into(),
            original,
            mounting,
            inherited: previous.is_some()
                || (!mounting && version.mount.is_none() && version.loop_object.is_some()),
            phase: Phase::Prepared,
        })
    }
    fn check_retry(&self, version: &DriveBackup) -> Result<()> {
        self.check_version(version)?;
        if matches!(
            self.phase,
            Phase::StartingLoop | Phase::Mounting | Phase::Retiring
        ) && version.loop_object.is_none()
        {
            return Err("Image control outcome is uncertain and has no recorded loop owner; retain exclusions and recover the mapping before retrying".into());
        }
        Ok(())
    }
    fn check_owner(&self, db: &Store) -> Result<()> {
        let current = db
            .get_bounded::<Self>(KIND, &self.backup, LIMIT)?
            .ok_or("Image control ownership disappeared")?;
        if current.operation != self.operation
            || current.backup != self.backup
            || current.original != self.original
            || current.mounting != self.mounting
            || current.inherited != self.inherited
        {
            return Err("Image control ownership changed; retaining exclusions".into());
        }
        Ok(())
    }
    fn phase(&mut self, db: &Store, phase: Phase) -> Result<()> {
        self.phase = phase;
        let transaction = db.0.unchecked_transaction()?;
        self.check_owner(db)?;
        db.put(KIND, &self.backup, self)?;
        transaction.commit()?;
        Ok(())
    }
    fn check_version(&self, version: &DriveBackup) -> Result<()> {
        let original: DriveBackup = serde_json::from_value(self.original.clone())?;
        if version.id != self.backup
            || original.id != self.backup
            || version.kind != BackupKind::Image
            || version.path != original.path
            || version.identity != original.identity
        {
            return Err("Image control version identity changed; retaining exclusions".into());
        }
        Ok(())
    }
    fn proven(&self, db: &Store, evidence: &Evidence) -> Result<bool> {
        self.check_version(&evidence.version)?;
        match self.phase {
            Phase::Prepared | Phase::NoEffect => {
                Ok(!self.inherited && evidence.header == self.original)
            }
            Phase::Retired => Ok(evidence.version.mount.is_none()
                && evidence.version.loop_object.is_none()
                && evidence.observation.is_none()
                && evidence.attachment.is_none()),
            Phase::Ready => backup::ImageMount::validate_view(db, &evidence.version).map(|()| true),
            _ => Ok(false),
        }
    }
    fn check_input(&self, db: &Store) -> Result<()> {
        self.check_owner(db)?;
        if Evidence::capture(db, &self.backup)?.header != self.original {
            return Err("Image control input changed before native admission; retry with the current version".into());
        }
        Ok(())
    }
    fn settle(&self, db: &Store) -> Result<bool> {
        let evidence = Evidence::capture(db, &self.backup)?;
        if !self.proven(db, &evidence)? {
            return Ok(false);
        }
        // Filesystem proof precedes this transaction: a FUSE wait never holds
        // SQLite's writer. Publication/removal compares the saved evidence again.
        let transaction = db.0.unchecked_transaction()?;
        self.check_owner(db)?;
        if !evidence.current(db, &self.backup)? {
            return Err("Image control evidence changed; retaining exclusions".into());
        }
        db.remove(KIND, &self.backup)?;
        db.remove("recovery", &format!("image-control:{}", self.backup))?;
        transaction.commit()?;
        Ok(true)
    }
    fn run(mut self, state: PathBuf, action: Action, cancel: Arc<AtomicBool>) -> Result<String> {
        let db = Store::existing(&state.join("drives.sqlite"), false)?;
        let result = cancelled(&cancel).and_then(|()| {
            self.check_input(&db)?;
            action(&db, &self.backup.clone(), &mut |phase| {
                if matches!(phase, Phase::StartingLoop | Phase::Mounting)
                    || (!self.mounting && phase == Phase::Retiring)
                {
                    cancelled(&cancel)?;
                }
                self.phase(&db, phase)
            })
        });
        let settlement = self.settle(&db);
        let warning = match settlement {
            Ok(true) => return result,
            Ok(false) => "Image control is unresolved; background indexing remains paused until its mapping is recovered".to_owned(),
            Err(error) => format!("Background indexing remains paused; image control recovery could not be recorded: {error}"),
        };
        let warning = match self.record_recovery(&db, &warning) {
            Ok(()) => warning,
            Err(error) => format!("{warning}; recovery notice could not be saved: {error}"),
        };
        match result {
            Ok(message) => Ok(format!("{message}; {warning}")),
            Err(error) => Err(format!("{error}; {warning}").into()),
        }
    }
    fn record_recovery(&self, db: &Store, message: &str) -> Result<()> {
        let transaction = db.0.unchecked_transaction()?;
        self.check_owner(db)?;
        let original: DriveBackup = serde_json::from_value(self.original.clone())?;
        let id = format!("image-control:{}", self.backup);
        db.put(
            "recovery",
            &id,
            &crate::DriveRecovery {
                id: id.clone(),
                drive: original.drive,
                artifact: original.path,
                cause: crate::RecoveryCause::Unavailable,
                message: message.into(),
            },
        )?;
        transaction.commit()?;
        Ok(())
    }
}

struct Evidence {
    version: DriveBackup,
    header: serde_json::Value,
    observation: Option<serde_json::Value>,
    attachment: Option<serde_json::Value>,
}
impl Evidence {
    fn capture(db: &Store, id: &str) -> Result<Self> {
        let header = db
            .get_bounded::<serde_json::Value>("backup", id, LIMIT)?
            .ok_or("Image version disappeared during control")?;
        Ok(Self {
            version: serde_json::from_value(header.clone())?,
            header,
            observation: db.get_bounded("image-mount", id, LIMIT)?,
            attachment: db.get_bounded("image-loop", id, LIMIT)?,
        })
    }
    fn current(&self, db: &Store, id: &str) -> Result<bool> {
        Ok(db
            .get_bounded::<serde_json::Value>("backup", id, LIMIT)?
            .as_ref()
            == Some(&self.header)
            && db.get_bounded::<serde_json::Value>("image-mount", id, LIMIT)? == self.observation
            && db.get_bounded::<serde_json::Value>("image-loop", id, LIMIT)? == self.attachment)
    }
}

pub(super) fn policy_ready(db: &Store) -> Result<()> {
    if let Some(id) = db.ids(KIND, "", 1)?.into_iter().next() {
        let intent = db
            .get_bounded::<Intent>(KIND, &id, LIMIT)?
            .ok_or("Image-control journal disappeared")?;
        return Err(format!(
            "Image control {} for {} has not established its final mount location",
            intent.operation, id
        )
        .into());
    }
    Ok(())
}

pub(super) fn recover(db: &Store, id: &str, cancel: &AtomicBool) -> Result<bool> {
    cancelled(cancel)?;
    let Some(intent) = db.get_bounded::<Intent>(KIND, id, LIMIT)? else {
        return Ok(false);
    };
    if intent.backup != id {
        return Err("Image control key differs from its owner; retaining exclusions".into());
    }
    match intent.settle(db) {
        Ok(true) => Ok(true),
        result => {
            let message = result.as_ref().err().map_or_else(
                || "Image control is unresolved; background indexing remains paused. Recover the recorded mapping before retrying.".into(),
                |error| format!("Image control recovery remains incomplete; indexing stays paused: {error}"),
            );
            intent.record_recovery(db, &message)?;
            result
        }
    }
}

impl Owner {
    pub(super) fn start_image_control(&mut self, envelope: Envelope) {
        let (id, mounting) = match &envelope.request {
            DriveRequest::MountImage { backup } => (backup.clone(), true),
            DriveRequest::UnmountImage { backup } => (backup.clone(), false),
            _ => unreachable!("image control request"),
        };
        let intent = match Intent::begin(&self.db, &id, &envelope.id, mounting) {
            Ok(intent) => intent,
            Err(error) => {
                self.finish_operation(&envelope.id, &envelope.cancel, Err(error));
                return;
            }
        };
        // request() installs the fail-closed policy before returning, including
        // while an older observation is still running. No UDisks call precedes it.
        if let Err(error) = self.publish_policy(false) {
            let cleanup = intent
                .settle(&self.db)
                .err()
                .map(|error| format!("; retained image exclusion: {error}"))
                .unwrap_or_default();
            self.finish_operation(
                &envelope.id,
                &envelope.cancel,
                Err(format!("Could not protect image mount admission: {error}{cleanup}").into()),
            );
            return;
        }
        let action = if mounting {
            self.authority.images.mount.clone()
        } else {
            self.authority.images.unmount.clone()
        };
        let state = self.state.clone();
        let cancel = envelope.cancel.clone();
        self.launch(envelope, None, Vec::new(), None, move || {
            intent.run(state, action, cancel)
        });
    }
}
