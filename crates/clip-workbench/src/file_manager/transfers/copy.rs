use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, Metadata};
use std::io::{Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, symlink};
use std::path::{Path, PathBuf};

use super::{Control, Decision};
use crate::file_manager::filesystem::{Identity, identity, rename_noreplace};

pub(super) fn transfer(
    source: &Path,
    directory: &Path,
    moving: bool,
    control: &Control,
    policy: &mut Option<Decision>,
) -> Result<Option<PathBuf>, String> {
    let (source, target) = plan(source, directory)?;
    if moving && target == source {
        return Ok(None);
    }
    let Some((target, replace)) = destination(&source, target, control, policy)? else {
        return Ok(None);
    };
    if target == source {
        return Err("Source and destination are identical; choose another name".into());
    }
    control.check()?;
    if moving && !replace {
        match rename_noreplace(&source, &target) {
            Ok(()) => return Ok(Some(target)),
            Err(error) if error.raw_os_error() == Some(libc::EXDEV) => {}
            Err(error) => return Err(error.to_string()),
        }
    }
    staged_transfer(&source, &target, moving, replace, control)?;
    Ok(Some(target))
}

fn plan(source: &Path, directory: &Path) -> Result<(PathBuf, PathBuf), String> {
    let directory = directory.canonicalize().map_err(|e| e.to_string())?;
    if !directory.is_dir() {
        return Err("Destination is not a directory".into());
    }
    let source = absolute_entry(source)?;
    let metadata = fs::symlink_metadata(&source).map_err(|e| e.to_string())?;
    if metadata.is_dir() && directory.starts_with(&source) {
        return Err("Destination is inside the source directory".into());
    }
    let target = directory.join(
        source
            .file_name()
            .ok_or("Cannot transfer the filesystem root")?,
    );
    Ok((source, target))
}

fn staged_transfer(
    source: &Path,
    target: &Path,
    moving: bool,
    replace: bool,
    control: &Control,
) -> Result<(), String> {
    let staging = tempfile::Builder::new()
        .prefix(".little-clip-copy-")
        .tempdir_in(target.parent().ok_or("Missing destination folder")?)
        .map_err(|e| e.to_string())?;
    let staged = staging.path().join("item");
    let identities = copy_tree(source, &staged, control)?;
    verify_sources(&identities, control)?;
    control.check()?;
    install(&staged, target, replace)?;
    if moving {
        finish_move(source, target, &identities, control)?;
    }
    Ok(())
}

fn finish_move(
    source: &Path,
    target: &Path,
    identities: &BTreeMap<PathBuf, Identity>,
    control: &Control,
) -> Result<(), String> {
    if control.check().is_err() {
        return Err(format!(
            "Destination copied to {}; source retained after cancellation",
            super::super::display_path(target)
        ));
    }
    remove_verified_source(source, identities, control)
        .map_err(|e| format!("Destination copied; {e}"))
}

// Shared with native outgoing moves: quarantine the exact source before deleting,
// then validate every entry and recover without overwriting a recreated path.
pub(super) fn remove_verified_source(
    source: &Path,
    identities: &BTreeMap<PathBuf, Identity>,
    control: &Control,
) -> Result<(), String> {
    verify_sources(identities, control).map_err(|e| format!("Source retained: {e}"))?;
    let quarantine = private_name(source, "moving")?;
    rename_noreplace(source, &quarantine).map_err(|e| format!("Source retained: {e}"))?;
    let result = verify_relocated(identities, source, &quarantine, control)
        .and_then(|()| remove(&quarantine).map_err(|e| e.to_string()));
    if let Err(error) = result {
        let recovery = match rename_noreplace(&quarantine, source) {
            Ok(()) => format!(
                "remaining source restored at {}",
                super::super::display_path(source)
            ),
            Err(_) => format!(
                "remaining source preserved at {}",
                super::super::display_path(&quarantine)
            ),
        };
        return Err(format!("Source removal failed: {error}; {recovery}"));
    }
    Ok(())
}

fn verify_relocated(
    identities: &BTreeMap<PathBuf, Identity>,
    source: &Path,
    quarantine: &Path,
    control: &Control,
) -> Result<(), String> {
    for (path, expected) in identities {
        control.check()?;
        let moved = quarantine.join(path.strip_prefix(source).map_err(|e| e.to_string())?);
        let actual = identity(&fs::symlink_metadata(&moved).map_err(|e| e.to_string())?);
        let unchanged = if path == source {
            expected.after_rename_matches(actual)
        } else {
            *expected == actual
        };
        if !unchanged {
            return Err("Source changed before removal".into());
        }
    }
    Ok(())
}

fn private_name(path: &Path, action: &str) -> Result<PathBuf, String> {
    let mut name =
        std::ffi::OsString::from(format!(".little-clip-{action}-{}-", uuid::Uuid::new_v4()));
    let original = path.file_name().ok_or("Missing filename")?;
    if name.as_bytes().len() + original.as_bytes().len() <= 255 {
        name.push(original);
    }
    Ok(path.parent().ok_or("Missing parent folder")?.join(name))
}

fn absolute_entry(path: &Path) -> Result<PathBuf, String> {
    let parent = path
        .parent()
        .ok_or("Cannot operate on filesystem root")?
        .canonicalize()
        .map_err(|e| e.to_string())?;
    Ok(parent.join(path.file_name().ok_or("Missing filename")?))
}

fn destination(
    source: &Path,
    mut target: PathBuf,
    control: &Control,
    policy: &mut Option<Decision>,
) -> Result<Option<(PathBuf, bool)>, String> {
    loop {
        control.check()?;
        match fs::symlink_metadata(&target) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Some((target, false)));
            }
            Err(error) => return Err(error.to_string()),
            Ok(_) => {}
        }
        let decision = if let Some(decision) = policy.as_ref() {
            decision.clone()
        } else {
            let (decision, all) = control.conflict(source, &target)?;
            if all && !matches!(decision, Decision::Name(_)) {
                *policy = Some(decision.clone());
            }
            decision
        };
        match decision {
            Decision::Skip => return Ok(None),
            Decision::Replace => return Ok(Some((target, true))),
            Decision::Name(name) => {
                target = target
                    .parent()
                    .ok_or("Missing destination folder")?
                    .join(name)
            }
        }
    }
}

fn copy_tree(
    source: &Path,
    target: &Path,
    control: &Control,
) -> Result<BTreeMap<PathBuf, Identity>, String> {
    let mut identities = BTreeMap::new();
    let mut visited = BTreeSet::new();
    let mut stack = vec![(source.to_path_buf(), target.to_path_buf(), false)];
    while let Some((source, target, finish)) = stack.pop() {
        control.check()?;
        let metadata = fs::symlink_metadata(&source).map_err(|e| e.to_string())?;
        if finish {
            let directory = fs::File::open(&target).map_err(|e| e.to_string())?;
            preserve_metadata(&directory, &metadata)?;
            continue;
        }
        identities.insert(source.clone(), identity(&metadata));
        control
            .progress
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .current = source.clone();
        if metadata.file_type().is_symlink() {
            symlink(fs::read_link(&source).map_err(|e| e.to_string())?, &target)
                .map_err(|e| e.to_string())?;
        } else if metadata.is_dir() {
            if !visited.insert((metadata.dev(), metadata.ino())) {
                return Err(
                    "Repeated directory identity; filesystem alias traversal stopped".into(),
                );
            }
            enqueue_directory(&source, &target, &mut stack, control)?;
        } else if metadata.is_file() {
            copy_file(&source, &target, &metadata, control)?;
        } else {
            return Err("Special files (devices, sockets and FIFOs) cannot be copied".into());
        }
    }
    Ok(identities)
}

fn enqueue_directory(
    source: &Path,
    target: &Path,
    stack: &mut Vec<(PathBuf, PathBuf, bool)>,
    control: &Control,
) -> Result<(), String> {
    fs::create_dir(target).map_err(|e| e.to_string())?;
    stack.push((source.into(), target.into(), true));
    for child in fs::read_dir(source).map_err(|e| e.to_string())? {
        control.check()?;
        let child = child.map_err(|e| e.to_string())?;
        stack.push((child.path(), target.join(child.file_name()), false));
    }
    Ok(())
}

fn copy_file(
    source: &Path,
    target: &Path,
    metadata: &Metadata,
    control: &Control,
) -> Result<(), String> {
    let mut input = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(source)
        .map_err(|e| e.to_string())?;
    if identity(&input.metadata().map_err(|e| e.to_string())?) != identity(metadata) {
        return Err("Source changed before copying".into());
    }
    let mut output = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(target)
        .map_err(|e| e.to_string())?;
    let mut buffer = vec![0; 256 * 1024];
    loop {
        control.check()?;
        let read = input.read(&mut buffer).map_err(|e| e.to_string())?;
        if read == 0 {
            break;
        }
        output
            .write_all(&buffer[..read])
            .map_err(|e| e.to_string())?;
        control
            .progress
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .bytes += read as u64;
    }
    preserve_metadata(&output, metadata)?;
    output.sync_all().map_err(|e| e.to_string())?;
    Ok(())
}

fn preserve_metadata(file: &fs::File, metadata: &Metadata) -> Result<(), String> {
    file.set_permissions(metadata.permissions())
        .map_err(|e| e.to_string())?;
    let mut times = fs::FileTimes::new();
    if let Ok(time) = metadata.modified() {
        times = times.set_modified(time);
    }
    if let Ok(time) = metadata.accessed() {
        times = times.set_accessed(time);
    }
    file.set_times(times).map_err(|e| e.to_string())
}

pub(super) fn verify_sources<'a>(
    identities: impl IntoIterator<Item = (&'a PathBuf, &'a Identity)>,
    control: &Control,
) -> Result<(), String> {
    for (path, expected) in identities {
        control.check()?;
        let metadata = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
        if identity(&metadata) != *expected {
            return Err(format!(
                "Source changed during transfer: {}",
                super::super::display_path(path)
            ));
        }
    }
    Ok(())
}

fn install(staged: &Path, target: &Path, replace: bool) -> Result<(), String> {
    if !replace {
        return rename_noreplace(staged, target).map_err(|e| e.to_string());
    }
    let backup = private_name(target, "replaced")?;
    rename_noreplace(target, &backup).map_err(|e| e.to_string())?;
    if let Err(error) = rename_noreplace(staged, target) {
        let rollback = rename_noreplace(&backup, target);
        return Err(format!(
            "Installation failed: {error}. Previous destination {}",
            match rollback {
                Ok(()) => "was restored".into(),
                Err(error) => format!(
                    "is preserved at {} (restore failed: {error})",
                    super::super::display_path(&backup)
                ),
            }
        ));
    }
    remove(&backup).map_err(|e| {
        format!(
            "Destination installed; previous destination remains at {}: {e}",
            super::super::display_path(&backup)
        )
    })
}

pub(super) fn remove(path: &Path) -> std::io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cancellation_after_installation_keeps_both_completed_destination_and_source() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source");
        let target = root.path().join("installed");
        fs::write(&source, b"source data").unwrap();
        fs::write(&target, b"source data").unwrap();
        let identities = BTreeMap::from([(
            source.clone(),
            identity(&fs::symlink_metadata(&source).unwrap()),
        )]);
        let control = Control::default();
        control
            .cancelled
            .store(true, std::sync::atomic::Ordering::Relaxed);
        let error = finish_move(&source, &target, &identities, &control).unwrap_err();
        assert!(error.contains("source retained"));
        assert_eq!(fs::read(source).unwrap(), b"source data");
        assert_eq!(fs::read(target).unwrap(), b"source data");
    }
    #[test]
    fn failed_replacement_installation_restores_previous_destination() {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("target");
        fs::write(&target, b"previous contents").unwrap();
        let error = install(&root.path().join("missing-stage"), &target, true).unwrap_err();
        assert!(error.contains("restored"));
        assert_eq!(fs::read(target).unwrap(), b"previous contents");
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
    }
}
