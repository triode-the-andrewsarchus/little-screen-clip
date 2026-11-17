//! Bounded, descriptor-relative removal; never follows links or nested mounts.
use super::*;
use std::os::unix::fs::{MetadataExt, PermissionsExt};

pub(super) fn validate(root: &Root, cancel: &AtomicBool) -> Result<()> {
    walk(root, cancel, false)
}
pub(super) fn remove(root: &Root, cancel: &AtomicBool) -> Result<()> {
    walk(root, cancel, true)
}
fn frame(root: &Root, relative: PathBuf, remove: bool) -> Result<scan::Frame> {
    if remove {
        let pin = root.directory(&relative, false)?;
        let mode = pin.metadata()?.mode();
        if mode & 0o700 != 0o700 {
            fs::set_permissions(anchor(&pin), fs::Permissions::from_mode(mode | 0o700))?;
        }
    }
    scan::Frame::new(root, relative)
}
fn child(root: &Root, relative: PathBuf, remove: bool) -> Result<Option<scan::Frame>> {
    let metadata = root
        .metadata(&relative)?
        .ok_or("Retirement entry disappeared")?;
    if metadata.is_dir() {
        return frame(root, relative, remove).map(Some);
    }
    if !metadata.is_file() && !metadata.file_type().is_symlink() {
        return Err("Special entry in backup retirement tree; data retained".into());
    }
    if remove {
        let (_pin, path) = root.parent(&relative, false)?;
        fs::remove_file(path)?;
    }
    Ok(None)
}
fn walk(root: &Root, cancel: &AtomicBool, remove: bool) -> Result<()> {
    let mut frames = vec![frame(root, PathBuf::new(), remove)?];
    while let Some(mut current) = frames.pop() {
        cancelled(cancel)?;
        if let Some(entry) = current.reader.next() {
            let relative = current.relative.join(entry?.file_name());
            let next = child(root, relative, remove)?;
            frames.push(current);
            if let Some(next) = next {
                if frames.len() >= 256 {
                    return Err("Retirement tree exceeds the guarded depth limit".into());
                }
                frames.push(next);
            }
        } else if remove && !current.relative.as_os_str().is_empty() {
            let (_pin, path) = root.parent(&current.relative, false)?;
            fs::remove_dir(path)?;
        }
    }
    root.validate()
}
