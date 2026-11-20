//! Bounded, read-only inventories. Archive member names never become host paths.
mod compressed;
pub(in crate::file_manager) mod extract;
use std::{
    collections::BTreeSet,
    fs::File,
    io::{Read, Write},
    path::{Component, Path},
    sync::atomic::{AtomicBool, Ordering},
};

use super::super::filesystem::{self, Entry};
use little_clip_core::{file_source::FileSource, kernel_interface};

pub(super) const LIMIT: usize = 32;
pub(super) const ARCHIVE_ENTRIES: usize = 4096;
const ARCHIVE_SIZE: u64 = 512 * 1024 * 1024;

#[derive(Clone, Debug)]
pub(super) enum Child {
    File(Box<Entry>),
    Member {
        ordinal: usize,
        name: String,
        directory: bool,
        size: u64,
    },
}

impl Child {
    pub fn name(&self) -> &str {
        match self {
            Self::File(entry) => &entry.name,
            Self::Member { name, .. } => name,
        }
    }
    pub fn directory(&self) -> bool {
        match self {
            Self::File(entry) => entry.directory,
            Self::Member { directory, .. } => *directory,
        }
    }
}

#[derive(Debug)]
pub(super) struct Inventory {
    pub children: Vec<Child>,
    /// Exact when complete; otherwise only the observed lower bound.
    pub observed: usize,
    pub complete: bool,
    /// Entries observed but deliberately excluded from actionable cards.
    pub omitted: usize,
    pub archive: bool,
    pub notice: Option<String>,
    pub source: Option<clip_search::source::SourceRoot>,
    pub archive_source: Option<FileSource>,
    pub verified: bool,
    pub payload: Option<Vec<u8>>,
}

pub(in crate::file_manager) fn archive(path: &Path) -> bool {
    compressed::format(path).is_some()
        || path.extension().is_some_and(|ext| {
            ["zip", "tar"]
                .iter()
                .any(|known| ext.eq_ignore_ascii_case(known))
        })
}

pub(super) fn load(entry: &Entry, cancel: &AtomicBool) -> Result<Inventory, String> {
    checkpoint(cancel)?;
    filesystem::check_revision(&entry.path, entry.revision)?;
    let result = if entry.directory {
        folder(entry, cancel)
    } else {
        let mut file =
            kernel_interface::open_content(&entry.path, false).map_err(|e| e.to_string())?;
        let metadata = file.metadata().map_err(|e| e.to_string())?;
        if entry.identity != Some(filesystem::identity(&metadata)) {
            return Err("Archive changed before preview".into());
        }
        let source = FileSource::capture_file(&file)?;
        let mut inventory = scan(&mut file, &entry.path, cancel, Members::default())?;
        inventory.archive_source = Some(source.clone());
        source.check_file(&file)?;
        Ok(inventory)
    }?;
    checkpoint(cancel)?;
    filesystem::check_revision(&entry.path, entry.revision)?;
    Ok(result)
}

fn folder(entry: &Entry, cancel: &AtomicBool) -> Result<Inventory, String> {
    // Automatic inspection does not follow directory aliases into another tree.
    if entry.symlink {
        return Err("Open this folder link to browse its contents".into());
    }
    let held = kernel_interface::pin_content_directory(&entry.path).map_err(|e| e.to_string())?;
    if entry.identity
        != Some(filesystem::identity(
            &held.metadata().map_err(|e| e.to_string())?,
        ))
    {
        return Err("Folder changed before preview".into());
    }
    let directory = little_clip_core::file_inventory::read_pinned_directory(
        &entry.path,
        &held,
        cancel,
        LIMIT + 1,
    )?;
    let source = clip_search::source::SourceRoot::from_directory(&directory);
    let observed = directory.entries.len();
    let complete = directory.skipped == 0 && observed <= LIMIT;
    Ok(Inventory {
        children: directory
            .entries
            .into_iter()
            .take(LIMIT)
            .map(|entry| Child::File(Box::new(entry)))
            .collect(),
        observed,
        complete,
        omitted: directory.skipped,
        archive: false,
        notice: None,
        source,
        archive_source: None,
        verified: complete,
        payload: None,
    })
}

fn checkpoint(cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(Ordering::Acquire) {
        Err("Container preview cancelled".into())
    } else {
        Ok(())
    }
}

/// Full bounded lookup uses a captured source, never an archive member as a path.
pub(super) fn read_archive(
    source: &FileSource,
    target: Option<usize>,
    cancel: &AtomicBool,
) -> Result<Inventory, String> {
    checkpoint(cancel)?;
    let mut file =
        kernel_interface::open_content(source.path(), false).map_err(|e| e.to_string())?;
    source.check_file(&file)?;
    let mut result = scan(
        &mut file,
        source.path(),
        cancel,
        Members {
            display_limit: Some(ARCHIVE_ENTRIES),
            target,
            ..Members::default()
        },
    )?;
    checkpoint(cancel)?;
    source.check_file(&file)?;
    if target.is_some() && !result.verified {
        return Err("Archive scan is incomplete; member payloads were not verified".into());
    }
    if target.is_some() && result.payload.is_none() {
        return Err("Archive member is unavailable or is not a regular file".into());
    }
    result.archive_source = Some(source.clone());
    Ok(result)
}
fn scan(
    file: &mut File,
    path: &Path,
    cancel: &AtomicBool,
    members: Members,
) -> Result<Inventory, String> {
    if file.metadata().map_err(|e| e.to_string())?.len() > ARCHIVE_SIZE {
        return Err("Archive inventory exceeds the 512 MiB source limit".into());
    }
    if path
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("zip"))
    {
        zip(file, cancel, members)
    } else if let Some(format) = compressed::format(path) {
        compressed::load_with(file, cancel, format, members)
    } else {
        tar(file, cancel, members)
    }
}

fn zip(input: &mut File, cancel: &AtomicBool, mut members: Members) -> Result<Inventory, String> {
    let mut archive = little_clip_core::zip_content::archive_file(
        input.try_clone().map_err(|e| e.to_string())?,
        ARCHIVE_SIZE,
    )?;
    if archive.len() > ARCHIVE_ENTRIES {
        return Err("Archive exceeds the 4,096-member inventory limit".into());
    }
    for index in 0..archive.len() {
        checkpoint(cancel)?;
        // Raw access reads central-directory metadata without decompressing payloads.
        let member = archive.by_index_raw(index).map_err(|e| e.to_string())?;
        let payload = members.add_zip(&member)?;
        drop(member);
        if let Some(size) = payload {
            let mut member = archive.by_index(index).map_err(|e| e.to_string())?;
            members.read_payload(&mut member, size, cancel)?;
        }
    }
    Ok(members.finish(true))
}

fn tar(input: &mut File, cancel: &AtomicBool, mut members: Members) -> Result<Inventory, String> {
    let length = input.metadata().map_err(|e| e.to_string())?.len();
    let mut archive = tar::Archive::new(input);
    for (index, member) in archive
        .entries_with_seek()
        .map_err(|e| e.to_string())?
        .enumerate()
    {
        checkpoint(cancel)?;
        if index >= ARCHIVE_ENTRIES {
            return Ok(members.finish(false));
        }
        let member = member.map_err(|e| e.to_string())?;
        if member
            .raw_file_position()
            .checked_add(member.size())
            .is_none_or(|end| end > length)
        {
            return Err("Truncated archive member".into());
        }
        members.add_tar(member, cancel)?;
    }
    Ok(members.finish(true))
}

#[derive(Default)]
struct Members {
    names: BTreeSet<String>,
    children: Vec<Child>,
    observed: usize,
    omitted: usize,
    notice: Option<String>,
    display_limit: Option<usize>,
    target: Option<usize>,
    payload: Option<Vec<u8>>,
    output: Option<extract::Output>,
}
impl Members {
    // Header admission selects a regular payload by its stable observed ordinal.
    fn add_zip<R: Read + ?Sized>(
        &mut self,
        member: &zip::read::ZipFile<'_, R>,
    ) -> Result<Option<u64>, String> {
        let kind = member.unix_mode().unwrap_or(0) & 0o170000;
        if member.is_symlink() || !matches!(kind, 0 | 0o100000 | 0o040000) {
            if self.output.is_some() {
                return Err(
                    "Archive contains links or special files; browsing was not opened".into(),
                );
            }
            self.omitted += 1;
            self.observed += 1;
            return Ok(None);
        }
        let ordinal = self.observed;
        let directory = member.is_dir() || kind == 0o040000;
        let name = member.name().map_err(|error| error.to_string())?;
        self.add(&name, directory, member.size())?;
        Ok(((self.output.is_some() || self.target == Some(ordinal))
            && self.observed > ordinal
            && !directory)
            .then_some(member.size()))
    }

    fn add_tar<R: std::io::Read>(
        &mut self,
        mut member: tar::Entry<'_, R>,
        cancel: &AtomicBool,
    ) -> Result<(), String> {
        let kind = member.header().entry_type();
        if !kind.is_file() && !kind.is_dir() {
            if self.output.is_some() {
                return Err(
                    "Archive contains links or special files; browsing was not opened".into(),
                );
            }
            self.omitted += 1;
            self.observed += 1;
            return Ok(());
        }
        let path = member.path().map_err(|e| e.to_string())?;
        let name = path.to_str().ok_or("Archive member name is not UTF-8")?;
        let ordinal = self.observed;
        self.add(name, kind.is_dir(), member.size())?;
        if (self.output.is_some() || self.target == Some(ordinal))
            && self.observed > ordinal
            && kind.is_file()
        {
            let size = member.size();
            self.read_payload(&mut member, size, cancel)?;
        }
        Ok(())
    }

    fn read_payload(
        &mut self,
        input: &mut impl Read,
        size: u64,
        cancel: &AtomicBool,
    ) -> Result<(), String> {
        if let Some(output) = &mut self.output {
            return output.copy(input, size, cancel);
        }
        const BYTES: usize = 8 * 1024 * 1024;
        if size > BYTES as u64 {
            return Err("Member exceeds the 8 MiB preview limit".into());
        }
        let mut bytes = Vec::with_capacity(size as usize);
        copy_payload(input, &mut bytes, size, cancel)?;
        self.payload = Some(bytes);
        Ok(())
    }

    fn add(&mut self, name: &str, directory: bool, size: u64) -> Result<(), String> {
        // Retain full member provenance; do not join it onto the archive's parent.
        if name.len() > 4096 || name.contains(['\\', '\0']) || name.chars().any(char::is_control) {
            return Err("Unsafe archive member name".into());
        }
        let path = Path::new(name);
        if path.is_absolute()
            || path
                .components()
                .any(|c| !matches!(c, Component::Normal(_) | Component::CurDir))
        {
            return Err("Unsafe archive member path".into());
        }
        if path.components().count() == 0 {
            return Ok(());
        }
        let canonical = path
            .components()
            .filter_map(|c| match c {
                Component::Normal(name) => name.to_str(),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("/");
        if canonical.is_empty() {
            return Ok(());
        }
        if !self.names.insert(canonical.clone()) {
            return Err("Duplicate archive member path".into());
        }
        if let Some(output) = &mut self.output {
            output.admit(&canonical, directory, size)?;
        }
        self.observed += 1;
        if self.children.len() < self.display_limit.unwrap_or(LIMIT) {
            self.children.push(Child::Member {
                ordinal: self.observed - 1,
                name: canonical,
                directory,
                size,
            });
        }
        Ok(())
    }
    fn finish(mut self, scanned_all: bool) -> Inventory {
        self.children.sort_by(|a, b| {
            b.directory()
                .cmp(&a.directory())
                .then_with(|| a.name().cmp(b.name()))
        });
        Inventory {
            complete: scanned_all && self.omitted == 0 && self.observed <= LIMIT,
            omitted: self.omitted,
            children: self.children,
            observed: self.observed,
            archive: true,
            notice: self.notice,
            source: None,
            archive_source: None,
            verified: scanned_all,
            payload: self.payload,
        }
    }
}

// Preview memory and browsing files use the same exact-length, cancellable read.
fn copy_payload(
    input: &mut impl Read,
    output: &mut impl Write,
    size: u64,
    cancel: &AtomicBool,
) -> Result<(), String> {
    let mut copied = 0_u64;
    let mut buffer = [0; 64 * 1024];
    loop {
        checkpoint(cancel)?;
        let count = input.read(&mut buffer).map_err(|e| e.to_string())?;
        if count == 0 {
            break;
        }
        copied = copied
            .checked_add(count as u64)
            .filter(|count| *count <= size)
            .ok_or("Archive member exceeded its declared size")?;
        output
            .write_all(&buffer[..count])
            .map_err(|e| e.to_string())?;
    }
    if copied != size {
        return Err("Truncated archive member".into());
    }
    Ok(())
}
