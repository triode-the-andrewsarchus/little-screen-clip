//! Image effect checkpoints shared by UDisks and retained admission.
use super::*;
use serde::{Deserialize, Serialize};

pub(crate) const KIND: &str = "image-control";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum ImagePhase {
    Prepared,
    StartingLoop,
    LoopOwned,
    Mounting,
    Ready,
    Retiring,
    Retired,
    NoEffect,
}
pub(crate) type ImageAction = Arc<
    dyn Fn(&Store, &str, &mut dyn FnMut(ImagePhase) -> Result<()>) -> Result<String> + Send + Sync,
>;
pub(crate) struct ImageAuthority {
    pub mount: ImageAction,
    pub unmount: ImageAction,
}
impl Default for ImageAuthority {
    fn default() -> Self {
        Self {
            mount: Arc::new(mount_image),
            unmount: Arc::new(unmount_image),
        }
    }
}

pub(crate) fn mount_image(
    db: &Store,
    id: &str,
    phase: &mut dyn FnMut(ImagePhase) -> Result<()>,
) -> Result<String> {
    let mut backup: Backup = db.get("backup", id)?.ok_or("Unknown backup version")?;
    if backup.kind != BackupKind::Image {
        return Err("This backup is a file tree, not an image".into());
    }
    if backup.mount.is_some() || backup.loop_object.is_some() {
        return Err("Image already has a loop mapping; unmount it before retrying".into());
    }
    let file = kernel_interface::open_content(&backup.path, false)?;
    let source = little_clip_core::file_source::FileSource::capture_file(&file)?;
    verify_image(&backup, &file)?;
    let connection = system_connection()?;
    let object = setup_loop(&connection, &file, phase)?;
    backup.loop_object = Some(object.to_string());
    let owner = image_loop::Owned::capture(&connection, &backup, &file).map_err(|error| {
        let recorded = db.put("backup", id, &backup).err()
            .map(|error| format!("; loop owner could not be recorded: {error}"))
            .unwrap_or_default();
        crate::Error::from(format!("Loop was created but descriptor admission failed: {error}{recorded}; mapping retained for recovery"))
    })?;
    let result = owner
        .save(db, &backup)
        .and_then(|()| mount_view(db, &connection, &mut backup, &source, &file, &owner, phase));
    result.or_else(|error| {
        // A failed admission may already own a mounted filesystem. Preserve
        // that fact for retry when cleanup is busy, but still attempt cleanup
        // using the retained owner if the journal itself is unavailable.
        let owner_error = db.put("backup", id, &backup).err();
        retire_image(db, backup, true, Some(owner), phase).map_err(|cleanup| {
            let journal = owner_error
                .map(|error| format!("; mounted-image recovery record also failed: {error}"))
                .unwrap_or_default();
            crate::Error::from(format!(
                "{error}; image cleanup also failed: {cleanup}{journal}"
            ))
        })?;
        Err(error)
    })
}

pub(super) fn setup_loop(
    connection: &zbus::blocking::Connection,
    file: &File,
    phase: &mut dyn FnMut(ImagePhase) -> Result<()>,
) -> Result<zbus::zvariant::OwnedObjectPath> {
    let manager = udisks_proxy(
        connection,
        "/org/freedesktop/UDisks2/Manager",
        "org.freedesktop.UDisks2.Manager",
    )?;
    let mut options = HashMap::new();
    options.insert("read-only", zbus::zvariant::Value::from(true));
    phase(ImagePhase::StartingLoop)?;
    Ok(manager
        .call("LoopSetup", &(zbus::zvariant::Fd::from(file), options))
        .map_err(|error| {
            if image_permission_refusal(&error) {
                let _ = phase(ImagePhase::NoEffect);
            }
            error.to_string()
        })?)
}

#[cfg(test)]
pub(crate) fn unrecorded_loop_for_test(
    db: &Store,
    id: &str,
    phase: &mut dyn FnMut(ImagePhase) -> Result<()>,
) -> Result<String> {
    crate::require_disposable_guest();
    let backup: Backup = db.get("backup", id)?.ok_or("Missing owned image")?;
    let file = kernel_interface::open_content(&backup.path, false)?;
    verify_image(&backup, &file)?;
    Ok(setup_loop(&system_connection()?, &file, phase)?.to_string())
}

fn mount_view(
    db: &Store,
    connection: &zbus::blocking::Connection,
    backup: &mut Backup,
    source: &little_clip_core::file_source::FileSource,
    file: &File,
    owner: &image_loop::Owned,
    phase: &mut dyn FnMut(ImagePhase) -> Result<()>,
) -> Result<String> {
    let id = &backup.id;
    phase(ImagePhase::LoopOwned)?;
    let filesystem = udisks_proxy(
        connection,
        backup
            .loop_object
            .as_deref()
            .ok_or("Missing image loop owner")?,
        "org.freedesktop.UDisks2.Filesystem",
    )?;
    let mut options = HashMap::new();
    options.insert(
        "options",
        zbus::zvariant::Value::from("ro,nosuid,nodev,noexec"),
    );
    phase(ImagePhase::Mounting)?;
    owner.validate()?;
    let mounted: String = filesystem
        .call("Mount", &(options,))
        .map_err(|e| e.to_string())?;
    // Retain the actual successful mount even if later validation/publication
    // fails. Cleanup must use this owner, not reload an older partial record.
    backup.mount = Some(mounted.clone().into());
    owner.validate()?;
    let root = Root::open(Path::new(&mounted))?;
    let observation = ImageMount::capture(backup, &root)?;
    source.check_file(file)?;
    let transaction = db.0.unchecked_transaction()?;
    db.put("image-mount", id, &observation)?;
    db.put("backup", id, &backup)?;
    transaction.commit()?;
    let checkpoint = completed_checkpoint(ImagePhase::Ready, phase);
    Ok(format!("Image mounted read-only at {mounted}{checkpoint}"))
}
pub(crate) fn unmount_image(
    db: &Store,
    id: &str,
    phase: &mut dyn FnMut(ImagePhase) -> Result<()>,
) -> Result<String> {
    let backup: Backup = db.get("backup", id)?.ok_or("Unknown backup version")?;
    let connection = system_connection()?;
    let owner = match image_loop::Attachment::load(db, &backup)? {
        Some(attachment) => attachment.reopen(&connection, &backup)?,
        None => Some(image_loop::Owned::adopt_ready(&connection, db, &backup)?),
    };
    retire_image(db, backup, false, owner, phase)
}
fn retire_image(
    db: &Store,
    mut backup: Backup,
    fresh: bool,
    owner: Option<image_loop::Owned>,
    phase: &mut dyn FnMut(ImagePhase) -> Result<()>,
) -> Result<String> {
    let object = backup.loop_object.clone();
    // Cleanup already owns an effect: journal failure must not discard its owner.
    if fresh {
        let _ = phase(ImagePhase::Retiring);
    } else {
        phase(ImagePhase::Retiring)?;
    }
    if let Some(owner) = owner {
        unmount_view(db, &mut backup, fresh, &owner, phase)?;
        owner.retire()?;
    }
    backup.mount = None;
    backup.loop_object = None;
    if let Err(error) = retire_metadata(db, &backup, object.as_deref()) {
        return Ok(format!(
            "Image loop detached; local retirement metadata needs recovery: {error}"
        ));
    }
    let checkpoint = completed_checkpoint(ImagePhase::Retired, phase);
    Ok(format!("Read-only backup image unmounted{checkpoint}"))
}

fn retire_metadata(db: &Store, backup: &Backup, object: Option<&str>) -> Result<()> {
    let transaction = db.0.unchecked_transaction()?;
    let current: Backup = db
        .get("backup", &backup.id)?
        .ok_or("Image version disappeared during retirement")?;
    if current.id != backup.id
        || current.path != backup.path
        || current.identity != backup.identity
        || current.kind != backup.kind
        || (current.loop_object.is_some() && current.loop_object.as_deref() != object)
    {
        return Err(
            "Image retirement record changed; native completion retained for recovery".into(),
        );
    }
    db.remove("image-mount", &backup.id)?;
    db.remove(image_loop::KIND, &backup.id)?;
    db.put("backup", &backup.id, backup)?;
    transaction.commit()?;
    Ok(())
}

fn unmount_view(
    db: &Store,
    backup: &mut Backup,
    fresh: bool,
    owner: &image_loop::Owned,
    phase: &mut dyn FnMut(ImagePhase) -> Result<()>,
) -> Result<()> {
    if !owner.mounted()? {
        return Ok(());
    }
    if !fresh {
        ImageMount::validate_view(db, backup)?;
    }
    owner.matches_mount(backup)?;
    let connection = system_connection()?;
    let filesystem = udisks_proxy(
        &connection,
        backup
            .loop_object
            .as_deref()
            .ok_or("Missing image loop owner")?,
        "org.freedesktop.UDisks2.Filesystem",
    )?;
    let options: HashMap<&str, zbus::zvariant::Value<'_>> = HashMap::new();
    filesystem
        .call::<_, _, ()>("Unmount", &(options,))
        .map_err(|error| {
            if !fresh && (image_permission_refusal(&error) || matches!(&error, zbus::Error::MethodError(name, _, _) if name.as_str() == "org.freedesktop.UDisks2.Error.DeviceBusy")) {
                let _ = phase(ImagePhase::NoEffect);
            }
            error.to_string()
        })?;
    backup.mount = None;
    db.put("backup", &backup.id, &backup)?;
    Ok(())
}

fn completed_checkpoint(
    completed: ImagePhase,
    phase: &mut dyn FnMut(ImagePhase) -> Result<()>,
) -> String {
    phase(completed)
        .err()
        .map(|error| format!("; image-control checkpoint could not be updated: {error}"))
        .unwrap_or_default()
}

fn image_permission_refusal(error: &zbus::Error) -> bool {
    matches!(error, zbus::Error::MethodError(name, _, _) if matches!(name.as_str(),
        "org.freedesktop.UDisks2.Error.NotAuthorized" | "org.freedesktop.UDisks2.Error.NotAuthorizedCanObtain" | "org.freedesktop.UDisks2.Error.NotAuthorizedDismissed"))
}
