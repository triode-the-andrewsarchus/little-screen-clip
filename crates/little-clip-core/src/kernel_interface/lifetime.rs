//! Shared local mount continuity for image providers and persisted scan observations.
use super::{MountIdentity, Namespace};
use serde::{Deserialize, Serialize};
use std::io::{self, ErrorKind, Result};
use std::{fs::File, io::Read, os::fd::AsRawFd};

/// One boot- and namespace-scoped observation of a non-reused mount lifetime.
///
/// Retain the original serialized fields when persisting this observation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MountLifetime {
    /// Pinned object and reusable mount observations.
    pub identity: MountIdentity,
    /// Kernel mount ID that is not reused within this boot.
    pub unique_mount: u64,
    /// Namespaces in which the mount was observed.
    pub namespace: Namespace,
    /// Boot identity accompanying the unique mount ID.
    pub boot: uuid::Uuid,
}
impl MountLifetime {
    /// Returns whether two observations belong to the same known mount lifetime.
    ///
    /// Objects may have different inodes and Btrfs subvolume device numbers.
    /// This is not a payload admission check.
    pub fn same_mount(&self, other: &Self) -> bool {
        self.namespace.known()
            && other.namespace.known()
            && self.namespace == other.namespace
            && self.boot == other.boot
            && self.unique_mount != 0
            && self.unique_mount == other.unique_mount
            && self.identity.mount == other.identity.mount
    }

    /// Observes a pinned object's boot, namespace and unique mount ID.
    ///
    /// # Errors
    /// Returns metadata failures, unknown namespace context or missing Linux 6.8
    /// unique mount-ID support. This does not read object payload bytes.
    pub fn capture(pin: &File) -> Result<Self> {
        let namespace = Namespace::current();
        if !namespace.known() {
            return Err(io::Error::new(
                ErrorKind::Unsupported,
                "Cannot establish mount namespace",
            ));
        }
        Ok(Self {
            identity: MountIdentity::pinned(pin)?,
            unique_mount: unique_mount_id(pin)?,
            namespace,
            boot: current_boot_id()?,
        })
    }
    /// Revalidates the same pinned object and mount lifetime.
    ///
    /// # Errors
    /// Returns an error when boot, namespace, mount or object identity changes.
    pub fn validate(&self, pin: &File) -> Result<()> {
        if !self.namespace.known()
            || Namespace::current() != self.namespace
            || current_boot_id()? != self.boot
        {
            return Err(io::Error::new(
                ErrorKind::Unsupported,
                "Mount context is stale",
            ));
        }
        if MountIdentity::pinned(pin)? != self.identity
            || unique_mount_id(pin)? != self.unique_mount
        {
            return Err(io::Error::new(ErrorKind::Unsupported, "Mount was replaced"));
        }
        Ok(())
    }
}
/// Reads the current boot identity from the fixed, verified procfs interface.
///
/// # Errors
/// Returns bounded procfs-read or invalid identity failures.
pub fn current_boot_id() -> Result<uuid::Uuid> {
    let mut value = String::new();
    // Fixed non-consuming procfs identity, never selected by a content path.
    let file = super::proc_metadata(super::ProcMetadata::Boot)?;
    file.take(64).read_to_string(&mut value)?;
    uuid::Uuid::parse_str(value.trim())
        .map_err(|error| io::Error::new(ErrorKind::InvalidData, error))
}
/// Observes the non-reused mount ID of one retained metadata descriptor.
///
/// # Errors
/// Returns `statx` failures or missing Linux 6.8 unique mount-ID support.
pub fn unique_mount_id(pin: &File) -> Result<u64> {
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
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: the successful statx call initialized the entire output structure.
    let stats = unsafe { stats.assume_init() };
    if stats.stx_mask & libc::STATX_MNT_ID_UNIQUE == 0 || stats.stx_mnt_id == 0 {
        return Err(io::Error::new(
            ErrorKind::Unsupported,
            "Unique mount identity requires Linux 6.8 or later",
        ));
    }
    Ok(stats.stx_mnt_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unique_mount_observation_is_stable_and_preserves_flat_image_fields() {
        let temp = tempfile::tempdir().unwrap();
        let pin = crate::kernel_interface::pin_content_directory(temp.path()).unwrap();
        let observed = MountLifetime::capture(&pin).unwrap();
        assert_eq!(observed, MountLifetime::capture(&pin).unwrap());
        observed.validate(&pin).unwrap();
        let value = serde_json::to_value(&observed).unwrap();
        assert_eq!(value.as_object().unwrap().len(), 4);
        for key in ["identity", "unique_mount", "namespace", "boot"] {
            assert!(value.get(key).is_some());
        }
        assert_eq!(
            serde_json::from_value::<MountLifetime>(value).unwrap(),
            observed
        );
    }
}
