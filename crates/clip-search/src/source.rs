//! Serializable root observations, revalidated before content access.
use crate::{policy::Root, session::SourceSession};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// A root observation sent across worker requests and persisted index records.
///
/// Deserialization alone never authorizes access. [`Self::validate`] must obtain
/// a current guarded root before content access; legacy missing mount identity
/// is deliberately unavailable rather than inferred.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SourceRoot {
    #[serde(with = "little_clip_core::path_serde")]
    /// Losslessly encoded root address.
    pub path: PathBuf,
    /// Observed filesystem device.
    pub device: u64,
    /// Observed root inode.
    pub inode: u64,
    #[serde(default)]
    /// Observed mount ID; older records may lack it.
    pub mount: Option<u64>,
    #[serde(default)]
    /// Optional mounted-source continuity observation.
    pub session: Option<SourceSession>,
}
impl SourceRoot {
    /// Admits a root and captures its mounted-source observations on a worker.
    ///
    /// # Errors
    /// Returns admission, pinning or identity-observation failures.
    pub fn capture(path: &Path) -> Result<Self, String> {
        let root = Root::open(path)?;
        Ok(Self::from_root(&root))
    }
    /// Captures a descriptor from an admitted, retained root on a worker.
    pub fn from_root(root: &Root) -> Self {
        Self {
            path: root.path.clone(),
            device: root.device,
            inode: root.inode,
            mount: Some(root.mount),
            session: SourceSession::capture(root),
        }
    }
    /// Reopens and validates the root, retaining its pin on success.
    ///
    /// # Errors
    /// Returns an error for missing mount observations, changed identity or
    /// continuity, and any current admission or I/O failure.
    pub fn validate(&self) -> Result<Root, String> {
        let expected_mount = self
            .mount
            .ok_or("Indexed source needs a current mount observation before content access")?;
        let root = Root::open(&self.path)?;
        if (root.device, root.inode, root.mount) != (self.device, self.inode, expected_mount) {
            return Err("Indexed root was replaced or remounted".into());
        }
        if self.session.is_some() && self.session != SourceSession::capture(&root) {
            return Err("Indexed source session changed".into());
        }
        Ok(root)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::ffi::OsStringExt;

    #[test]
    fn descriptors_preserve_path_bytes_and_require_current_mount_and_session_identity() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp
            .path()
            .join(std::ffi::OsString::from_vec(b"source-\xff".to_vec()));
        std::fs::create_dir(&path).unwrap();
        let observed = SourceRoot::capture(&path).unwrap();
        observed.validate().unwrap();
        let mut json = serde_json::to_value(&observed).unwrap();
        assert!(json["path"].is_array());
        let round_trip: SourceRoot = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(round_trip.path, observed.path);
        round_trip.validate().unwrap();
        json.as_object_mut().unwrap().remove("mount");
        json.as_object_mut().unwrap().remove("session");
        let legacy: SourceRoot = serde_json::from_value(json).unwrap();
        assert!(legacy.mount.is_none() && legacy.session.is_none());
        assert!(legacy.validate().err().unwrap().contains("current mount"));
        let mut changed = observed.clone();
        changed.mount = Some(changed.mount.unwrap() + 1);
        assert!(changed.validate().is_err());
        // Keep the old directory alive so inode reuse cannot make this flaky.
        std::fs::rename(&path, temp.path().join("retired")).unwrap();
        std::fs::create_dir(&path).unwrap();
        assert!(observed.validate().is_err());
    }
}
