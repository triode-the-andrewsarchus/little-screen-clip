//! Local runtime evidence for the exact mounted view created by image admission.
use super::*;
use crate::mounts::Lifetime;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ImageMount {
    backup: String,
    #[serde(with = "little_clip_core::path_serde")]
    image: PathBuf,
    #[serde(with = "little_clip_core::path_serde")]
    path: PathBuf,
    object: String,
    #[serde(flatten)]
    lifetime: Lifetime,
}
impl ImageMount {
    pub(super) fn capture(backup: &Backup, root: &Root) -> Result<Self> {
        if backup.kind != BackupKind::Image {
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
            lifetime: Lifetime::capture(&pin)?,
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
        if self.path != root.path {
            return Err("Backup image mount context is stale".into());
        }
        let pin = root.directory(Path::new(""), false)?;
        self.lifetime.validate(&pin)?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_lifetime_preserves_the_legacy_flat_image_mount_representation() {
        #[derive(Serialize, Deserialize)]
        struct Legacy {
            backup: String,
            image: serde_json::Value,
            path: serde_json::Value,
            object: String,
            identity: serde_json::Value,
            unique_mount: u64,
            namespace: serde_json::Value,
            boot: uuid::Uuid,
        }
        let temp = tempfile::tempdir().unwrap();
        let root = Root::open(temp.path()).unwrap();
        let observed = ImageMount {
            backup: "owned".into(),
            image: "owned.img".into(),
            path: root.path.clone(),
            object: "owned-loop".into(),
            lifetime: Lifetime::capture(&root.directory(Path::new(""), false).unwrap()).unwrap(),
        };
        let json = serde_json::to_value(&observed).unwrap();
        assert!(json.get("lifetime").is_none());
        let old: Legacy = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(serde_json::to_value(old).unwrap(), json);
        assert_eq!(
            serde_json::from_value::<ImageMount>(json).unwrap(),
            observed
        );
    }

    #[test]
    fn writable_directories_cannot_be_published_as_read_only_image_views() {
        let temp = tempfile::tempdir().unwrap();
        let root = Root::open(temp.path()).unwrap();
        let error = read_only(&root.directory(Path::new(""), false).unwrap()).unwrap_err();
        assert!(error.to_string().contains("read-only"));
    }
}
