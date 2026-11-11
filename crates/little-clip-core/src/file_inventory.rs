//! File inventory observations shared by browser and background search workers.
//!
//! Snapshots preserve unavailable metadata, revisions and path bytes. A snapshot
//! or path hint does not authorize content access; readers retain their guarded
//! root/file handles and verify identity again before decoding or mutation.
use crate::{file_format, path_address::display_path};
use chrono::{DateTime, Local};
use std::{
    hash::{Hash, Hasher},
    os::unix::{ffi::OsStrExt, fs::MetadataExt},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

/// One enumeration snapshot, including metadata failures and source revision.
///
/// Display labels remain in the existing persisted representation. `identity`
/// describes the directory entry (including symlinks); it is not a content pin.
/// A missing warning is not independent permission to open the source.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Entry {
    #[serde(with = "crate::path_serde")]
    pub path: PathBuf,
    pub name: String,
    pub directory: bool,
    pub regular_file: bool,
    pub system: Option<crate::kernel_interface::Kind>,
    pub typescript: bool,
    pub symlink: bool,
    pub hidden: bool,
    pub size: String,
    pub size_bytes: u64,
    pub modified: String,
    pub modified_time: Option<std::time::SystemTime>,
    pub created_time: Option<std::time::SystemTime>,
    pub warning: Option<String>,
    pub revision: u64,
    pub identity: Option<Identity>,
}

impl Entry {
    /// Content with an interactive semantic strip instead of a generic text minimap.
    pub fn has_content_strip(&self) -> bool {
        self.is_playable_media()
            || self.is_scene()
            || self.is_blend()
            || self.is_material()
            || self.is_photo()
            || self.document_format().is_some()
    }

    pub fn document_format(&self) -> Option<file_format::Format> {
        self.regular_file
            .then(|| file_format::Format::for_path(&self.path))
            .flatten()
    }
    pub fn is_subtitles(&self) -> bool {
        self.regular_file && crate::subtitles::is_subtitle_path(&self.path)
    }
    pub fn is_multisample(&self) -> bool {
        self.regular_file
            && file_format::Format::for_path(&self.path) == Some(file_format::Format::Multisample)
    }
    pub fn is_toolpath(&self) -> bool {
        self.regular_file
            && file_format::Format::for_path(&self.path) == Some(file_format::Format::Toolpath)
    }
    pub fn is_geography(&self) -> bool {
        self.regular_file
            && file_format::Format::for_path(&self.path) == Some(file_format::Format::Geography)
    }
    pub fn is_database(&self) -> bool {
        self.regular_file
            && file_format::Format::for_path(&self.path) == Some(file_format::Format::Database)
    }
    pub fn is_calendar(&self) -> bool {
        self.regular_file
            && file_format::Format::for_path(&self.path) == Some(file_format::Format::Calendar)
    }
    pub fn is_mail(&self) -> bool {
        self.regular_file
            && file_format::Format::for_path(&self.path) == Some(file_format::Format::Mail)
    }
    pub fn is_book(&self) -> bool {
        self.regular_file
            && file_format::Format::for_path(&self.path) == Some(file_format::Format::Book)
    }
    pub fn is_notebook(&self) -> bool {
        self.regular_file
            && file_format::Format::for_path(&self.path) == Some(file_format::Format::Notebook)
    }
    pub fn is_paged_document(&self) -> bool {
        self.regular_file
            && (crate::pdf::is_pdf_path(&self.path)
                || crate::document::is_office_document(&self.path))
    }
    pub fn is_blend(&self) -> bool {
        self.regular_file
            && self
                .path
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("blend"))
    }
    pub fn is_material(&self) -> bool {
        self.regular_file && crate::model3d::materials::is_material_file(&self.path)
    }
    pub fn is_video(&self) -> bool {
        self.regular_file && !self.typescript && crate::animation::is_video_path(&self.path)
    }
    pub fn is_playable_media(&self) -> bool {
        self.regular_file
            && (self.is_video()
                || file_format::is_midi(&self.path)
                || crate::audio::is_audio_path(&self.path))
    }
    pub fn is_scene(&self) -> bool {
        self.regular_file
            && (file_format::is_splat_file(&self.path)
                || crate::presentation::model3d::is_3d_model_file(&self.path))
    }
    pub fn is_photo(&self) -> bool {
        self.regular_file
            && (crate::item::is_image_path(&self.path)
                || crate::raw_image::is_raw_image_path(&self.path))
    }
}

/// A directory snapshot with partial enumeration and observed mount identity.
///
/// `skipped` retains known incomplete coverage; missing object/mount metadata
/// remains unknown and must not authorize cache reuse or content reads.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Directory {
    pub system: bool,
    #[serde(with = "crate::path_serde")]
    pub path: PathBuf,
    pub entries: Vec<Entry>,
    pub skipped: usize,
    pub signature: u64,
    pub object: Option<(u64, u64)>,
    #[serde(default)]
    pub mount: Option<u64>,
}

/// Enumerates a canonical directory on a background worker.
///
/// Canonical locations make navigation and ancestor revelation agree. Entry
/// metadata failures remain in the snapshot rather than hiding those entries.
///
/// # Errors
/// Returns cancellation, directory I/O or observed replacement/mount changes.
pub fn read_directory(path: &Path, cancelled: &AtomicBool) -> Result<Directory, String> {
    let path = path.canonicalize().map_err(|error| error.to_string())?;
    let reader = std::fs::read_dir(&path).map_err(|error| error.to_string())?;
    collect_directory(path, reader, cancelled, usize::MAX, None)
}

/// Enumerates at most `limit` entries through a retained directory handle.
///
/// Metadata uses descriptor-relative paths, remapped only after observation.
/// Callers retain source admission and validate the requested root separately.
///
/// # Errors
/// Returns cancellation, directory I/O or observed replacement/mount changes.
pub fn read_pinned_directory(
    path: &Path,
    directory: &std::fs::File,
    cancelled: &AtomicBool,
    limit: usize,
) -> Result<Directory, String> {
    use std::os::fd::AsRawFd;
    let held = PathBuf::from(format!("/proc/self/fd/{}", directory.as_raw_fd()));
    let reader = std::fs::read_dir(&held).map_err(|error| error.to_string())?;
    collect_directory(path.into(), reader, cancelled, limit, Some(&held))
}

fn collect_directory(
    path: PathBuf,
    reader: std::fs::ReadDir,
    cancelled: &AtomicBool,
    limit: usize,
    held: Option<&Path>,
) -> Result<Directory, String> {
    let object = std::fs::metadata(held.unwrap_or(&path))
        .ok()
        .map(|metadata| (metadata.dev(), metadata.ino()));
    let mount = crate::kernel_interface::MountIdentity::directory(held.unwrap_or(&path))
        .ok()
        .map(|identity| identity.mount);
    let mut entries = Vec::new();
    let mut skipped = 0;
    for child in reader {
        if cancelled.load(Ordering::Relaxed) {
            return Err("Directory read cancelled".into());
        }
        if entries.len() >= limit {
            skipped += 1;
            break;
        }
        match child {
            Ok(child) => {
                let mut entry = read_entry(child, held.is_none());
                if let Some(held) = held {
                    entry.path =
                        path.join(entry.path.strip_prefix(held).map_err(|e| e.to_string())?);
                }
                entries.push(entry);
            }
            Err(_) => skipped += 1,
        }
    }
    entries.sort_by(|a, b| {
        b.directory
            .cmp(&a.directory)
            .then_with(|| a.path.file_name().cmp(&b.path.file_name()))
    });
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for entry in &entries {
        entry.path.hash(&mut hasher);
        entry.revision.hash(&mut hasher);
    }
    skipped.hash(&mut hasher);
    object.hash(&mut hasher);
    mount.hash(&mut hasher);
    if mount
        != crate::kernel_interface::MountIdentity::directory(held.unwrap_or(&path))
            .ok()
            .map(|identity| identity.mount)
    {
        return Err("Directory mount changed while reading".into());
    }
    if object
        != std::fs::metadata(held.unwrap_or(&path))
            .ok()
            .map(|metadata| (metadata.dev(), metadata.ino()))
    {
        return Err("Directory was replaced while reading".into());
    }
    let system = !crate::kernel_interface::classify(&path).is_ok_and(|kind| !kind.protected());
    Ok(Directory {
        system,
        signature: hasher.finish(),
        path,
        entries,
        skipped,
        object,
        mount,
    })
}

/// Observes an entry; optional TypeScript probing belongs only on workers.
///
/// Protected/nonregular sources never enter that payload probe. Failed metadata
/// remains a warning or unavailable observation in the returned snapshot.
pub fn read_entry(child: std::fs::DirEntry, inspect_typescript: bool) -> Entry {
    let path = child.path();
    let symlink = child.file_type().is_ok_and(|kind| kind.is_symlink());
    let metadata = std::fs::metadata(&path);
    let directory = metadata.as_ref().is_ok_and(|data| data.is_dir());
    let route = crate::kernel_interface::classify(&path).ok();
    let system = route.filter(|kind| kind.semantic());
    let regular_file = route == Some(crate::kernel_interface::Kind::Content)
        && metadata.as_ref().is_ok_and(|data| data.is_file());
    let modified = metadata.as_ref().ok().and_then(|data| data.modified().ok());
    let warning = metadata.as_ref().err().map(|error| error.to_string());
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    if let Ok(data) = &metadata {
        identity(data).hash(&mut hasher);
    }
    let link_metadata = std::fs::symlink_metadata(&path);
    let entry_identity = link_metadata.as_ref().ok().map(identity);
    entry_identity.hash(&mut hasher);
    Entry {
        typescript: inspect_typescript
            && regular_file
            && crate::text_format::is_typescript_source(&path),
        revision: hasher.finish(),
        identity: entry_identity,
        hidden: child.file_name().as_bytes().starts_with(b"."),
        name: display_path(Path::new(&child.file_name())),
        path,
        directory,
        regular_file,
        system,
        symlink,
        size_bytes: metadata.as_ref().map_or(0, |data| data.len()),
        size: if directory {
            "—".into()
        } else {
            metadata
                .as_ref()
                .map(|data| format_size(data.len()))
                .unwrap_or_else(|_| "—".into())
        },
        modified: modified
            .map(|time| {
                DateTime::<Local>::from(time)
                    .format("%Y-%m-%d %H:%M")
                    .to_string()
            })
            .unwrap_or_else(|| "—".into()),
        modified_time: modified,
        created_time: metadata.as_ref().ok().and_then(|data| data.created().ok()),
        warning,
    }
}

/// Formats the existing inventory byte-size label in binary units.
pub fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
/// Metadata fingerprint preserving object, size, mtime, ctime and mode.
///
/// Its tuple serialization is retained for existing inventories and indexes.
pub struct Identity(u64, u64, u64, i64, i64, i64, i64, u32);

/// Captures the existing full metadata fingerprint.
pub fn identity(data: &std::fs::Metadata) -> Identity {
    Identity(
        data.dev(),
        data.ino(),
        data.len(),
        data.mtime(),
        data.mtime_nsec(),
        data.ctime(),
        data.ctime_nsec(),
        data.mode(),
    )
}

impl Identity {
    /// Returns the observed device and inode pair.
    pub fn object(self) -> (u64, u64) {
        (self.0, self.1)
    }
    /// Checks identity after rename, allowing only the expected ctime change.
    ///
    /// Object identity, contents, mode and mtime must match before removing a
    /// quarantined move source; the caller still owns ordinary-source admission.
    pub fn after_rename_matches(self, actual: Self) -> bool {
        (self.0, self.1, self.2, self.3, self.4, self.7)
            == (actual.0, actual.1, actual.2, actual.3, actual.4, actual.7)
    }
}

/// Rechecks the existing enumeration revision against current metadata.
///
/// # Errors
/// Returns metadata failures or a changed file/directory-entry fingerprint.
pub fn check_revision(path: &Path, revision: u64) -> Result<(), String> {
    if current_revision(path)? != revision {
        return Err("File changed since enumeration".into());
    }
    Ok(())
}

pub(crate) fn current_revision(path: &Path) -> Result<u64, String> {
    let mut version = std::collections::hash_map::DefaultHasher::new();
    identity(&std::fs::metadata(path).map_err(|e| e.to_string())?).hash(&mut version);
    Some(identity(
        &std::fs::symlink_metadata(path).map_err(|e| e.to_string())?,
    ))
    .hash(&mut version);
    Ok(version.finish())
}
