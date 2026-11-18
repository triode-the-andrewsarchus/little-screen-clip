//! Metadata-only retirement after a known host has entered a different boot.
use super::*;
use little_clip_core::kernel_interface::Namespace;
use serde::{Deserialize, Serialize};

const LIMIT: u64 = 64 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Origin {
    pub host: Option<[u8; 32]>,
    pub boot: uuid::Uuid,
    pub namespace: Namespace,
}
impl Origin {
    pub(crate) fn capture() -> Result<Self> {
        let namespace = Namespace::current();
        if !namespace.known() {
            return Err("Cannot establish image-control namespace".into());
        }
        let boot = kernel_interface::current_boot_id()?;
        if boot.is_nil() {
            return Err("Image-control boot identity is unknown".into());
        }
        Ok(Self {
            host: machine_id(),
            boot,
            namespace,
        })
    }
    pub(crate) fn previous_boot(&self, current: &Self) -> Result<bool> {
        if self.boot.is_nil() || current.boot.is_nil() {
            return Err("Image-control boot identity is unknown".into());
        }
        if self.boot == current.boot {
            return Ok(false);
        }
        if self.host.is_none()
            || self.host != current.host
            || !self.namespace.known()
            || !current.namespace.known()
        {
            return Err("Old image state belongs to a different or unidentified host; retained for explicit recovery".into());
        }
        Ok(true)
    }
}
fn machine_id() -> Option<[u8; 32]> {
    let bytes = kernel_interface::read_content_bounded(Path::new("/etc/machine-id"), 64).ok()?;
    machine_fingerprint(&bytes)
}
fn machine_fingerprint(bytes: &[u8]) -> Option<[u8; 32]> {
    use sha2::{Digest, Sha256};
    let value = std::str::from_utf8(bytes).ok()?.trim();
    if value.len() != 32 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let id = uuid::Uuid::parse_str(value).ok()?;
    if id.is_nil() {
        return None;
    }
    let mut hash = Sha256::new();
    hash.update(b"little-screen-clip/image-origin/v1\0");
    hash.update(id.as_bytes());
    Some(hash.finalize().into())
}

#[cfg(test)]
mod tests;

pub(crate) struct Evidence {
    pub version: Backup,
    pub header: serde_json::Value,
    pub observation: Option<serde_json::Value>,
    pub attachment: Option<serde_json::Value>,
}
impl Evidence {
    pub(crate) fn capture(db: &Store, id: &str) -> Result<Self> {
        let header = db
            .get_bounded::<serde_json::Value>("backup", id, LIMIT)?
            .ok_or("Image version disappeared during control")?;
        let version: Backup = serde_json::from_value(header.clone())?;
        if version.id != id {
            return Err("Image version key differs from its record".into());
        }
        Ok(Self {
            version,
            header,
            observation: db.get_bounded("image-mount", id, LIMIT)?,
            attachment: db.get_bounded("image-loop", id, LIMIT)?,
        })
    }
    pub(crate) fn check_current(&self, db: &Store) -> Result<()> {
        let id = &self.version.id;
        if db
            .get_bounded::<serde_json::Value>("backup", id, LIMIT)?
            .as_ref()
            != Some(&self.header)
            || db.get_bounded::<serde_json::Value>("image-mount", id, LIMIT)? != self.observation
            || db.get_bounded::<serde_json::Value>("image-loop", id, LIMIT)? != self.attachment
        {
            return Err("Image control evidence changed; retaining exclusions".into());
        }
        Ok(())
    }
    pub(crate) fn check_control_boot(&self, boot: uuid::Uuid) -> Result<()> {
        if let Some(value) = &self.attachment {
            let attachment: image_loop::Attachment = serde_json::from_value(value.clone())?;
            if attachment.origin().boot != boot {
                return Err("Image mapping belongs to a previous boot; allow metadata recovery before retrying".into());
            }
        }
        Ok(())
    }
    pub(crate) fn outlived(&self, origin: Option<&Origin>) -> Result<bool> {
        let Some(origin) = origin else {
            return Ok(false);
        };
        // Avoid host filesystem reads for the common, current-kernel case.
        if origin.boot == kernel_interface::current_boot_id()? {
            return Ok(false);
        }
        self.previous_boot(origin, &Origin::capture()?)
    }
    pub(crate) fn previous_boot(&self, origin: &Origin, current: &Origin) -> Result<bool> {
        if !origin.previous_boot(current)? {
            return Ok(false);
        }
        if self.version.kind != BackupKind::Image || self.version.identity.is_none() {
            return Err("Old image version identity is unavailable".into());
        }
        if let Some(value) = &self.attachment {
            let attachment: image_loop::Attachment = serde_json::from_value(value.clone())?;
            attachment.check_backup(&self.version)?;
            if &attachment.origin() != origin {
                return Err("Image attachment origin differs from its control; retained".into());
            }
        }
        if let Some(value) = &self.observation {
            let view: ImageMount = serde_json::from_value(value.clone())?;
            if !view.matches_retirement(&self.version, origin) {
                return Err("Image view origin differs from its control; retained".into());
            }
        }
        Ok(true)
    }
    /// Applies only inside the caller's short metadata transaction.
    pub(crate) fn retire(&self, db: &Store) -> Result<()> {
        self.check_current(db)?;
        let mut version = self.version.clone();
        version.mount = None;
        version.loop_object = None;
        db.put("backup", &version.id, &version)?;
        db.remove("image-mount", &version.id)?;
        db.remove("image-loop", &version.id)?;
        Ok(())
    }
}
