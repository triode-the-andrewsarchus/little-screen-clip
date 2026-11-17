//! Journal private capture ownership before writing metadata; publish only sealed databases.
use super::*;
use crate::{Drive, StorageIdentity, root::anchor};
use serde::{Deserialize, Serialize};
use std::{fs, sync::Arc};
mod allocation;
mod publication;
mod retirement;

pub(crate) const KIND: &str = "catalog-capture";
const FILE: &str = "metadata.sqlite";

#[derive(Clone, Serialize, Deserialize)]
struct Ready {
    catalog: Catalog,
    checksum: String,
    bytes: u64,
    #[serde(with = "little_clip_core::path_serde::list")]
    exclusions: Vec<PathBuf>,
}
impl Ready {
    fn seal(
        file: &File,
        catalog: &Catalog,
        exclusions: &[PathBuf],
        cancel: &AtomicBool,
    ) -> Result<Self> {
        let bytes = file.metadata()?.len();
        let limits = catalog.limits.ok_or("Missing catalog capture budgets")?;
        if bytes > limits.bytes {
            return Err("Sealed catalog exceeds its database byte budget".into());
        }
        file.sync_all()?;
        let checksum = FileSource::capture_file(file)?.sha256(file, cancel, Duration::MAX)?;
        Ok(Self {
            catalog: catalog.clone(),
            checksum,
            bytes,
            exclusions: exclusions.to_vec(),
        })
    }
}
#[derive(Clone, Serialize, Deserialize)]
struct Intent {
    id: String,
    drive: String,
    parent: StorageIdentity,
    stage: String,
    directory: StorageIdentity,
    file: StorageIdentity,
    ready: Option<Ready>,
}
pub(super) struct Capture<'a> {
    db: &'a Store,
    state: &'a Root,
    parent: Root,
    stage: Root,
    pin: File,
    intent: Intent,
}
impl<'a> Capture<'a> {
    pub fn new(db: &'a Store, state: &'a Root, drive: &Drive) -> Result<Self> {
        let parent = Root::open(&anchor(&state.directory(Path::new("catalogs"), true)?))?;
        let parent_pin = parent.directory(Path::new(""), false)?;
        let id = uuid::Uuid::new_v4().to_string();
        let allocation::Empty {
            scratch,
            stage,
            pin,
            file,
        } = allocation::Empty::new(&parent_pin, &id)?;
        let intent = Intent {
            id,
            drive: drive.id.clone(),
            parent: parent.storage_identity()?,
            stage: scratch
                .path()
                .file_name()
                .ok_or("Missing capture stage")?
                .to_string_lossy()
                .into(),
            directory: stage.storage_identity()?,
            file,
            ready: None,
        };
        db.put(KIND, &intent.id, &intent)?;
        // From here recovery, rather than TempDir's path-only destructor, owns cleanup.
        let _ = scratch.keep();
        Ok(Self {
            db,
            state,
            parent,
            stage,
            pin,
            intent,
        })
    }
    pub fn id(&self) -> &str {
        &self.intent.id
    }
    pub fn path(&self) -> PathBuf {
        anchor(&self.pin).join(FILE)
    }
    pub fn publish(
        &mut self,
        metadata: Store,
        catalog: &Catalog,
        exclusions: &[PathBuf],
        cancel: &AtomicBool,
    ) -> Result<()> {
        self.seal(metadata, catalog, exclusions, cancel)?;
        // A sealed capture is a completed effect. Finish publication even if a
        // late cancellation arrives; recovery can finish the same transaction.
        publication::publish(self.db, self.state, &self.intent, &AtomicBool::new(false))
    }
    pub(super) fn seal(
        &mut self,
        metadata: Store,
        catalog: &Catalog,
        exclusions: &[PathBuf],
        cancel: &AtomicBool,
    ) -> Result<()> {
        metadata.0.close().map_err(|(_, error)| error)?;
        self.state.validate()?;
        self.parent.validate()?;
        self.stage.validate()?;
        let file = publication::owned_file(&self.stage, Path::new(FILE), &self.intent.file)?;
        let ready = Ready::seal(&file, catalog, exclusions, cancel)?;
        File::open(anchor(&self.pin))?.sync_all()?;
        self.stage.validate()?;
        let mut intent = self.intent.clone();
        intent.ready = Some(ready);
        self.db.put(KIND, &intent.id, &intent)?;
        self.intent = intent;
        Ok(())
    }
    pub fn failed(&self, error: crate::Error) -> Result<Catalog> {
        if self.intent.ready.is_some()
            && self.db.get::<Intent>(KIND, &self.intent.id)?.is_none()
            && let Some(published) = publication::published(self.db, &self.intent)?
        {
            return Ok(published);
        }
        let cleanup = match self.db.get::<Intent>(KIND, &self.intent.id)? {
            Some(intent) if intent.ready.is_none() => {
                retirement::retire(self.db, self.state, &intent)
            }
            Some(_) => Err("Sealed catalog retained for publication recovery".into()),
            None => Err("Capture ownership journal is missing; metadata retained".into()),
        };
        if let Err(cleanup) = cleanup {
            let message = format!("{error}; catalog recovery pending: {cleanup}");
            if let Err(journal) = record_failure(
                self.db,
                self.state,
                &self.intent,
                cleanup.recovery_cause(),
                &message,
            ) {
                return Err(format!("{message}; recovery journal update failed: {journal}").into());
            }
            return Err(message.into());
        }
        Err(error)
    }
}

fn parent(state: &Root, intent: &Intent) -> Result<Root> {
    uuid::Uuid::parse_str(&intent.id).map_err(|_| "Invalid capture identifier")?;
    if Path::new(&intent.stage).components().count() != 1
        || !intent
            .stage
            .starts_with(&format!(".capture-{}-", intent.id))
    {
        return Err("Invalid capture stage name".into());
    }
    let parent = Root::open(&anchor(&state.directory(Path::new("catalogs"), false)?))?;
    if parent.storage_identity()? != intent.parent {
        return Err("Catalog capture parent was replaced; metadata retained".into());
    }
    Ok(parent)
}
fn stage(parent: &Root, intent: &Intent) -> Result<Option<Root>> {
    if parent.metadata(Path::new(&intent.stage))?.is_none() {
        return Ok(None);
    }
    let root = Root::open(&anchor(&parent.directory(Path::new(&intent.stage), false)?))?;
    if root.storage_identity()? != intent.directory {
        return Err("Catalog capture directory was replaced; metadata retained".into());
    }
    Ok(Some(root))
}
fn record_failure(
    db: &Store,
    state: &Root,
    intent: &Intent,
    cause: crate::RecoveryCause,
    message: &str,
) -> Result<()> {
    db.put(
        "recovery",
        &intent.id,
        &crate::DriveRecovery {
            id: intent.id.clone(),
            drive: intent.drive.clone(),
            cause,
            artifact: state.path.join("catalogs").join(if intent.ready.is_some() {
                format!("{}.sqlite", intent.id)
            } else {
                intent.stage.clone()
            }),
            message: message.into(),
        },
    )
}

/// Existing metadata maintenance runs one bounded recovery item per owned task.
pub(crate) fn apply(state: Arc<Root>, id: &str, cancel: &AtomicBool) -> Result<bool> {
    crate::cancelled(cancel)?;
    state.validate()?;
    let db = Store::existing(&state.path.join("drives.sqlite"), false)?;
    let Some(intent) = db.get::<Intent>(KIND, id)? else {
        return Ok(false);
    };
    let result = match &intent.ready {
        Some(_) => publication::publish(&db, &state, &intent, cancel),
        None => retirement::retire(&db, &state, &intent),
    };
    if let Err(error) = &result
        && !cancel.load(std::sync::atomic::Ordering::Acquire)
        && let Err(journal) = record_failure(
            &db,
            &state,
            &intent,
            error.recovery_cause(),
            &error.to_string(),
        )
    {
        return Err(format!("{error}; recovery journal update failed: {journal}").into());
    }
    result.map(|()| true)
}
