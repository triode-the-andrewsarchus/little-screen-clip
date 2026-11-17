//! Local runtime evidence for the exact mounted view created by image admission.
use super::*;
use little_clip_core::kernel_interface::{MountIdentity, Namespace};
use serde::{Deserialize, Serialize};
use std::io::Read;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ImageMount {
    backup: String,
    #[serde(with = "little_clip_core::path_serde")]
    image: PathBuf,
    #[serde(with = "little_clip_core::path_serde")]
    path: PathBuf,
    object: String,
    identity: MountIdentity,
    unique_mount: u64,
    namespace: Namespace,
    boot: uuid::Uuid,
}
impl ImageMount {
    pub(super) fn capture(backup: &Backup, root: &Root) -> Result<Self> {
        let namespace = Namespace::current();
        if !namespace.known() || backup.kind != BackupKind::Image {
            return Err("Cannot establish image mount context".into());
        }
        let pin = root.directory(Path::new(""), false)?;
        read_only(&pin)?;
        let observation = Self {
            backup: backup.id.clone(),
            image: backup.path.clone(),
            path: root.path.clone(),
            object: backup
                .loop_object
                .clone()
                .ok_or("Missing image loop owner")?,
            identity: MountIdentity::pinned(&pin)?,
            unique_mount: unique_mount(&pin)?,
            namespace,
            boot: boot()?,
        };
        root.validate()?;
        Ok(observation)
    }
    pub(crate) fn load(db: &Store, backup: &Backup) -> Result<Option<Self>> {
        Ok(db.get::<Self>("image-mount", &backup.id)?.filter(|mount| {
            mount.backup == backup.id
                && mount.image == backup.path
                && Some(&mount.path) == backup.mount.as_ref()
                && Some(&mount.object) == backup.loop_object.as_ref()
                && backup.kind == BackupKind::Image
        }))
    }
    pub(crate) fn validate(&self, root: &Root) -> Result<()> {
        if !self.namespace.known()
            || self.path != root.path
            || Namespace::current() != self.namespace
            || boot()? != self.boot
        {
            return Err("Backup image mount context is stale".into());
        }
        let pin = root.directory(Path::new(""), false)?;
        if MountIdentity::pinned(&pin)? != self.identity || unique_mount(&pin)? != self.unique_mount
        {
            return Err("Backup image mount was replaced".into());
        }
        read_only(&pin)?;
        root.validate()
    }
}
fn read_only(pin: &File) -> Result<()> {
    if !rustix::fs::fstatvfs(pin)?
        .f_flag
        .contains(rustix::fs::StatVfsMountFlags::RDONLY)
    {
        return Err("Image mount did not remain read-only".into());
    }
    Ok(())
}
fn boot() -> Result<uuid::Uuid> {
    let mut value = String::new();
    File::open("/proc/sys/kernel/random/boot_id")?
        .take(64)
        .read_to_string(&mut value)?;
    Ok(uuid::Uuid::parse_str(value.trim()).map_err(|error| error.to_string())?)
}
fn unique_mount(pin: &File) -> Result<u64> {
    let mut stats = std::mem::MaybeUninit::<libc::statx>::zeroed();
    // SAFETY: statx borrows a live descriptor and writes its fixed-size output.
    // Request only the known mount-ID field; rustix currently masks this bit out.
    let result = unsafe {
        libc::statx(
            pin.as_raw_fd(),
            c"".as_ptr(),
            libc::AT_EMPTY_PATH,
            libc::STATX_MNT_ID_UNIQUE,
            stats.as_mut_ptr(),
        )
    };
    if result != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    // SAFETY: the successful statx call initialized the entire output structure.
    let stats = unsafe { stats.assume_init() };
    if stats.stx_mask & libc::STATX_MNT_ID_UNIQUE == 0 || stats.stx_mnt_id == 0 {
        return Err("Unique image mount identity requires Linux 6.8 or later".into());
    }
    Ok(stats.stx_mnt_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unique_mount_observation_is_stable_and_boot_identity_is_parseable() {
        let temp = tempfile::tempdir().unwrap();
        let pin = kernel_interface::pin_content_directory(temp.path()).unwrap();
        let first = unique_mount(&pin).unwrap();
        assert!(first > 0);
        assert_eq!(first, unique_mount(&pin).unwrap());
        assert_eq!(boot().unwrap(), boot().unwrap());
    }

    #[test]
    fn writable_directories_cannot_be_published_as_read_only_image_views() {
        let temp = tempfile::tempdir().unwrap();
        let root = Root::open(temp.path()).unwrap();
        let error = read_only(&root.directory(Path::new(""), false).unwrap()).unwrap_err();
        assert!(error.to_string().contains("read-only"));
    }
}
