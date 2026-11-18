//! Authorized loop descriptors bind native effects to one persisted attachment.
use super::*;
use crate::{StorageIdentity, mounts::Lifetime};
use serde::{Deserialize, Serialize};
use std::os::unix::{ffi::OsStringExt, fs::MetadataExt};

pub(super) const KIND: &str = "image-loop";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Attachment {
    backup: String,
    #[serde(with = "little_clip_core::path_serde")]
    image: PathBuf,
    identity: StorageIdentity,
    object: String,
    #[serde(with = "little_clip_core::path_serde")]
    device: PathBuf,
    node: Lifetime,
    rdev: u64,
    sequence: u64,
    backing_device: u64,
    backing_inode: u64,
}
impl Attachment {
    pub(super) fn load(db: &Store, backup: &Backup) -> Result<Option<Self>> {
        let attachment: Option<Self> = db.get_bounded(KIND, &backup.id, 64 * 1024)?;
        if let Some(attachment) = &attachment {
            attachment.check_backup(backup)?;
        }
        Ok(attachment)
    }
    fn check_backup(&self, backup: &Backup) -> Result<()> {
        if backup.kind != BackupKind::Image
            || self.backup != backup.id
            || self.image != backup.path
            || Some(&self.identity) != backup.identity.as_ref()
            || Some(&self.object) != backup.loop_object.as_ref()
        {
            return Err(
                "Loop attachment belongs to a different backup; retained for recovery".into(),
            );
        }
        Ok(())
    }
    pub(super) fn reopen(
        &self,
        connection: &zbus::blocking::Connection,
        backup: &Backup,
    ) -> Result<Option<Owned>> {
        self.check_backup(backup)?;
        // Reject a changed boot/namespace/node before asking for device I/O.
        self.node
            .validate(&kernel_interface::pin(&self.device, true)?)?;
        let (file, device, node) = open(connection, &self.object)?;
        self.node.validate(&node)?;
        if device != self.device || file.metadata()?.rdev() != self.rdev {
            return Err("Image loop device identity changed; mapping retained".into());
        }
        match Info::read(&file) {
            // An unbound device with an advanced generation proves the old
            // attachment ended. No native mutation is necessary on this retry.
            Err(error)
                if error.raw_os_error() == Some(libc::ENXIO)
                    && crate::device::disk_sequence(&file)? != self.sequence =>
            {
                Ok(None)
            }
            Err(error) => Err(error.into()),
            Ok(info) => {
                self.check_status(crate::device::disk_sequence(&file)?, &info)?;
                Ok(Some(Owned {
                    attachment: self.clone(),
                    file,
                }))
            }
        }
    }
    fn check_status(&self, sequence: u64, info: &Info) -> Result<()> {
        if sequence != self.sequence
            || info.device != self.backing_device
            || info.inode != self.backing_inode
            || info.rdevice != 0
            || info.offset != 0
            || info.size_limit != 0
            || info.flags & 1 == 0
        {
            return Err("Image loop attachment changed; refusing native cleanup".into());
        }
        Ok(())
    }
}

pub(super) struct Owned {
    attachment: Attachment,
    file: File,
}
impl Owned {
    pub(super) fn adopt_ready(
        connection: &zbus::blocking::Connection,
        db: &Store,
        backup: &Backup,
    ) -> Result<Self> {
        // A legacy ready mount can be upgraded only through its current
        // observed filesystem and the exact image backing the authorized FD.
        ImageMount::validate_view(db, backup)?;
        let image = kernel_interface::open_content(&backup.path, false)?;
        let owner = Self::capture(connection, backup, &image)?;
        owner.matches_mount(backup)?;
        ImageMount::validate_view(db, backup)?;
        owner.save(db, backup)?;
        Ok(owner)
    }
    pub(super) fn capture(
        connection: &zbus::blocking::Connection,
        backup: &Backup,
        image: &File,
    ) -> Result<Self> {
        let identity = StorageIdentity::pinned(image)?;
        if Some(&identity) != backup.identity.as_ref() || !image.metadata()?.is_file() {
            return Err("Image identity changed before loop admission".into());
        }
        let object = backup
            .loop_object
            .as_deref()
            .ok_or("Missing image loop owner")?;
        let (file, device, node) = open(connection, object)?;
        let metadata = image.metadata()?;
        let attachment = Attachment {
            backup: backup.id.clone(),
            image: backup.path.clone(),
            identity,
            object: object.into(),
            device,
            node: Lifetime::capture(&node)?,
            rdev: file.metadata()?.rdev(),
            sequence: crate::device::disk_sequence(&file)?,
            backing_device: metadata.dev(),
            backing_inode: metadata.ino(),
        };
        let owned = Self { attachment, file };
        owned.validate()?;
        Ok(owned)
    }
    pub(super) fn save(&self, db: &Store, backup: &Backup) -> Result<()> {
        self.attachment.check_backup(backup)?;
        let transaction = db.0.unchecked_transaction()?;
        db.put(KIND, &backup.id, &self.attachment)?;
        db.put("backup", &backup.id, backup)?;
        transaction.commit()?;
        Ok(())
    }
    pub(super) fn validate(&self) -> Result<()> {
        self.attachment.node.validate(&self.file)?;
        self.attachment
            .node
            .validate(&kernel_interface::pin(&self.attachment.device, true)?)?;
        let info = Info::read(&self.file)?;
        self.attachment
            .check_status(crate::device::disk_sequence(&self.file)?, &info)
    }
    pub(super) fn mounted(&self) -> Result<bool> {
        self.validate()?;
        let observed = little_clip_core::mount_inventory::Snapshot::capture()?;
        if observed.namespace != self.attachment.node.namespace
            || observed.boot != self.attachment.node.boot
        {
            return Err("Image loop mount namespace changed".into());
        }
        let mounted = observed
            .mounts
            .iter()
            .any(|mount| mount.device == self.attachment.rdev);
        self.validate()?;
        Ok(mounted)
    }
    pub(super) fn matches_mount(&self, backup: &Backup) -> Result<()> {
        let root = Root::open(backup.mount.as_deref().ok_or("Image has no mounted view")?)?;
        let directory = root.directory(Path::new(""), false)?;
        if directory.metadata()?.dev() != self.attachment.rdev {
            return Err("Mounted image is backed by a different loop device".into());
        }
        self.validate()
    }
    pub(super) fn retire(self) -> Result<()> {
        self.validate()?;
        // SAFETY: LOOP_CLR_FD acts on our authorized, identity-checked loop
        // descriptor. It never resolves a recycled UDisks object/path name.
        if unsafe { libc::ioctl(self.file.as_raw_fd(), 0x4c01 as libc::c_ulong) } != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        match Info::read(&self.file) {
            // Lo_rundown rejects new opens; dropping our last descriptor
            // completes release before this worker can publish retirement.
            Err(error) if error.raw_os_error() == Some(libc::ENXIO) => Ok(()),
            Err(error) => Err(error.into()),
            Ok(_) => Err(
                "Loop retirement is deferred by open device readers; close them and retry cleanup"
                    .into(),
            ),
        }
    }
}

fn open(connection: &zbus::blocking::Connection, object: &str) -> Result<(File, PathBuf, File)> {
    let block = udisks_proxy(connection, object, "org.freedesktop.UDisks2.Block")?;
    let bytes: Vec<u8> = block
        .get_property("Device")
        .map_err(|error| error.to_string())?;
    let device = PathBuf::from(std::ffi::OsString::from_vec(
        bytes.strip_suffix(&[0]).unwrap_or(&bytes).to_vec(),
    ));
    let node = kernel_interface::pin(&device, true)?;
    if !device.is_absolute()
        || !matches!(
            kernel_interface::classify_handle(&node)?,
            kernel_interface::Kind::Block { major: 7, .. }
        )
    {
        return Err("Image control requires a registered loop block device".into());
    }
    let lifetime = Lifetime::capture(&node)?;
    let mut options = HashMap::new();
    options.insert(
        "flags",
        zbus::zvariant::Value::from(libc::O_CLOEXEC | libc::O_NONBLOCK),
    );
    let fd: zbus::zvariant::OwnedFd = block
        .call("OpenDevice", &("r", options))
        .map_err(|error| error.to_string())?;
    let fd: OwnedFd = fd.into();
    let file = File::from(fd);
    lifetime.validate(&file)?;
    if file.metadata()?.rdev() != node.metadata()?.rdev() {
        return Err("Authorized image descriptor differs from its device node".into());
    }
    Ok((file, device, node))
}

// Linux UAPI loop_info64. No payload reads or write-mode device opens are used.
#[repr(C)]
#[derive(Clone, Copy)]
struct Info {
    device: u64,
    inode: u64,
    rdevice: u64,
    offset: u64,
    size_limit: u64,
    number: u32,
    encryption: u32,
    key_size: u32,
    flags: u32,
    filename: [u8; 64],
    crypt_name: [u8; 64],
    key: [u8; 32],
    init: [u64; 2],
}
impl Info {
    fn read(file: &File) -> std::io::Result<Self> {
        // SAFETY: every field is an integer or a fixed byte array.
        let mut info: Self = unsafe { std::mem::zeroed() };
        // SAFETY: LOOP_GET_STATUS64 fills the live repr(C) UAPI buffer.
        if unsafe { libc::ioctl(file.as_raw_fd(), 0x4c05 as libc::c_ulong, &mut info) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(info)
    }
}

#[cfg(test)]
mod tests;
