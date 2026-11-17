//! Metadata-only admission pins, retained across partition unmount and authorization.
use crate::{Drive, Result};
use little_clip_core::{
    file_inventory::{Identity, identity},
    kernel_interface::{self, MountIdentity, Namespace},
};
use std::{
    fs::File,
    os::{
        fd::AsRawFd,
        unix::fs::{FileTypeExt, MetadataExt},
    },
    path::PathBuf,
};

pub(crate) struct Source {
    path: PathBuf,
    pin: File,
    identity: Identity,
    mount: MountIdentity,
    namespace: Namespace,
    rdev: u64,
    block: bool,
    sequence: u64,
}
impl Source {
    pub fn capture(drive: &Drive) -> Result<Self> {
        let sequence = drive.disk_sequence.filter(|value| *value != 0)
            .ok_or("Kernel disk sequence unavailable; controlled partition backup requires disk lifetime tracking (Linux and lsblk DISK-SEQ support)")?;
        // O_PATH observes metadata only: it neither opens device I/O nor pins the
        // mounted content filesystem, so the accepted unmount can still finish.
        let pin = kernel_interface::pin(&drive.device, true)?;
        let metadata = pin.metadata()?;
        let namespace = Namespace::current();
        if !namespace.known() {
            return Err("Cannot establish device admission namespace".into());
        }
        let source = Self {
            path: drive.device.clone(),
            identity: identity(&metadata),
            mount: MountIdentity::pinned(&pin)?,
            rdev: metadata.rdev(),
            block: metadata.file_type().is_block_device(),
            pin,
            namespace,
            sequence,
        };
        source.validate()?;
        Ok(source)
    }
    fn same_node(&self, metadata: &std::fs::Metadata) -> bool {
        if self.block {
            // UDisks/udev update block-node timestamps during unmount. The held
            // O_PATH pin prevents inode reuse; rdev and BLKGETDISKSEQ additionally
            // validate the backing device before reading and during both passes.
            metadata.file_type().is_block_device()
                && metadata.rdev() == self.rdev
                && identity(metadata).object() == self.identity.object()
        } else {
            identity(metadata) == self.identity
        }
    }
    pub fn validate(&self) -> Result<()> {
        if Namespace::current() != self.namespace {
            return Err("Device namespace changed during partition preparation".into());
        }
        let current = kernel_interface::pin(&self.path, true)?;
        if MountIdentity::pinned(&current)? != self.mount
            || !self.same_node(&current.metadata()?)
            || !self.same_node(&self.pin.metadata()?)
        {
            return Err(
                "Partition device was removed or replaced during partition preparation".into(),
            );
        }
        Ok(())
    }
    pub fn validate_input(&self, input: &File) -> Result<()> {
        self.validate()?;
        let metadata = input.metadata()?;
        if !metadata.file_type().is_block_device() || !self.same_node(&metadata) {
            return Err(
                "Authorized descriptor does not match the admitted partition device".into(),
            );
        }
        let mut sequence = 0_u64;
        // SAFETY: Linux BLKGETDISKSEQ writes one u64 to this live stack value.
        if unsafe {
            libc::ioctl(
                input.as_raw_fd(),
                0x80081280_u64 as libc::c_ulong,
                &mut sequence,
            )
        } != 0
        {
            return Err(format!(
                "Cannot verify authorized disk lifetime: {}",
                std::io::Error::last_os_error()
            )
            .into());
        }
        if sequence != self.sequence {
            return Err("Disk lifetime changed before image publication".into());
        }
        Ok(())
    }
}
