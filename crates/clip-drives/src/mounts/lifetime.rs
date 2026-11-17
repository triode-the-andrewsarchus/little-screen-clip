//! Shared local mount continuity for image providers and persisted scan observations.
use crate::Result;
use little_clip_core::kernel_interface::{MountIdentity, Namespace};
use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    io::Read,
    os::{fd::AsRawFd, unix::fs::OpenOptionsExt},
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Lifetime {
    identity: MountIdentity,
    unique_mount: u64,
    namespace: Namespace,
    boot: uuid::Uuid,
}
impl Lifetime {
    pub fn capture(pin: &File) -> Result<Self> {
        let namespace = Namespace::current();
        if !namespace.known() {
            return Err("Cannot establish mount namespace".into());
        }
        Ok(Self {
            identity: MountIdentity::pinned(pin)?,
            unique_mount: unique_mount(pin)?,
            namespace,
            boot: boot()?,
        })
    }
    pub fn validate(&self, pin: &File) -> Result<()> {
        if !self.namespace.known() || Namespace::current() != self.namespace || boot()? != self.boot
        {
            return Err("Mount context is stale".into());
        }
        if MountIdentity::pinned(pin)? != self.identity || unique_mount(pin)? != self.unique_mount {
            return Err("Mount was replaced".into());
        }
        Ok(())
    }
}
fn boot() -> Result<uuid::Uuid> {
    let mut value = String::new();
    // Fixed non-consuming procfs identity, never selected by a content path.
    let file = File::options()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open("/proc/sys/kernel/random/boot_id")?;
    if rustix::fs::fstatfs(&file)?.f_type != libc::PROC_SUPER_MAGIC {
        return Err("Boot identity is not on procfs".into());
    }
    file.take(64).read_to_string(&mut value)?;
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
        return Err("Unique mount identity requires Linux 6.8 or later".into());
    }
    Ok(stats.stx_mnt_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unique_mount_observation_is_stable_and_preserves_flat_image_fields() {
        let temp = tempfile::tempdir().unwrap();
        let pin = little_clip_core::kernel_interface::pin_content_directory(temp.path()).unwrap();
        let observed = Lifetime::capture(&pin).unwrap();
        assert_eq!(observed, Lifetime::capture(&pin).unwrap());
        observed.validate(&pin).unwrap();
        let value = serde_json::to_value(&observed).unwrap();
        assert_eq!(value.as_object().unwrap().len(), 4);
        for key in ["identity", "unique_mount", "namespace", "boot"] {
            assert!(value.get(key).is_some());
        }
        assert_eq!(serde_json::from_value::<Lifetime>(value).unwrap(), observed);
    }
}
