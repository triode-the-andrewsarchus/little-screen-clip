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
    let result = mount_view(db, &connection, &mut backup, &source, &file, phase);
    result.or_else(|error| {
        // A failed admission may already own a mounted filesystem. Preserve
        // that fact for retry when cleanup is busy, but still attempt cleanup
        // using the retained owner if the journal itself is unavailable.
        let owner_error = db.put("backup", id, &backup).err();
        retire_image(db, backup, true, phase).map_err(|cleanup| {
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

fn setup_loop(
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

fn mount_view(
    db: &Store,
    connection: &zbus::blocking::Connection,
    backup: &mut Backup,
    source: &little_clip_core::file_source::FileSource,
    file: &File,
    phase: &mut dyn FnMut(ImagePhase) -> Result<()>,
) -> Result<String> {
    let id = &backup.id;
    db.put("backup", id, &backup)?;
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
    let mounted: String = filesystem
        .call("Mount", &(options,))
        .map_err(|e| e.to_string())?;
    // Retain the actual successful mount even if later validation/publication
    // fails. Cleanup must use this owner, not reload an older partial record.
    backup.mount = Some(mounted.clone().into());
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
    retire_image(
        db,
        db.get("backup", id)?.ok_or("Unknown backup version")?,
        false,
        phase,
    )
}
fn retire_image(
    db: &Store,
    mut backup: Backup,
    fresh: bool,
    phase: &mut dyn FnMut(ImagePhase) -> Result<()>,
) -> Result<String> {
    let id = backup.id.clone();
    let object = backup
        .loop_object
        .clone()
        .ok_or("Image has no managed loop mapping")?;
    if !fresh {
        ImageMount::validate_view(db, &backup)?;
    }
    // Cleanup already owns an effect: journal failure must not discard its owner.
    if fresh {
        let _ = phase(ImagePhase::Retiring);
    } else {
        phase(ImagePhase::Retiring)?;
    }
    let connection = system_connection()?;
    unmount_view(db, &connection, &mut backup, fresh, phase)?;
    let proxy = udisks_proxy(&connection, object.as_str(), "org.freedesktop.UDisks2.Loop")?;
    let options: HashMap<&str, zbus::zvariant::Value<'_>> = HashMap::new();
    proxy
        .call::<_, _, ()>("Delete", &(options,))
        .map_err(|e| e.to_string())?;
    backup.loop_object = None;
    let transaction = db.0.unchecked_transaction()?;
    db.remove("image-mount", &id)?;
    db.put("backup", &id, &backup)?;
    transaction.commit()?;
    let checkpoint = completed_checkpoint(ImagePhase::Retired, phase);
    Ok(format!("Read-only backup image unmounted{checkpoint}"))
}

fn unmount_view(
    db: &Store,
    connection: &zbus::blocking::Connection,
    backup: &mut Backup,
    fresh: bool,
    phase: &mut dyn FnMut(ImagePhase) -> Result<()>,
) -> Result<()> {
    if backup.mount.is_some() {
        let filesystem = udisks_proxy(
            connection,
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
    }
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
