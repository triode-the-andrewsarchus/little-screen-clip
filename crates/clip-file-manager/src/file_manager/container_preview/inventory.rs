//! Bounded, read-only inventories. Archive member names never become host paths.
use std::{
    collections::BTreeSet,
    fs::File,
    path::{Component, Path},
    sync::atomic::{AtomicBool, Ordering},
};

use super::super::filesystem::{self, Entry};
use little_clip_core::{file_source::FileSource, kernel_interface};

pub(super) const LIMIT: usize = 32;
const ARCHIVE_ENTRIES: usize = 4096;
const ARCHIVE_SIZE: u64 = 512 * 1024 * 1024;

#[derive(Clone, Debug)]
pub(super) enum Child {
    File(Box<Entry>),
    Member {
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
    pub archive: bool,
}

pub(super) fn archive(path: &Path) -> bool {
    path.extension().is_some_and(|ext| {
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
        let source = FileSource::capture_file(&file)?;
        if file.metadata().map_err(|e| e.to_string())?.len() > ARCHIVE_SIZE {
            return Err("Archive inventory exceeds the 512 MiB source limit".into());
        }
        let inventory = if entry
            .path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("zip"))
        {
            zip(&mut file, cancel)
        } else {
            tar(&mut file, cancel)
        }?;
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
        archive: false,
    })
}

fn checkpoint(cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(Ordering::Acquire) {
        Err("Container preview cancelled".into())
    } else {
        Ok(())
    }
}

fn zip(input: &mut File, cancel: &AtomicBool) -> Result<Inventory, String> {
    let mut archive = little_clip_core::zip_content::archive_file(
        input.try_clone().map_err(|e| e.to_string())?,
        ARCHIVE_SIZE,
    )?;
    if archive.len() > ARCHIVE_ENTRIES {
        return Err("Archive exceeds the 4,096-member inventory limit".into());
    }
    let mut members = Members::default();
    for index in 0..archive.len() {
        checkpoint(cancel)?;
        // Raw access reads central-directory metadata without decompressing payloads.
        let member = archive.by_index_raw(index).map_err(|e| e.to_string())?;
        if member.is_symlink() {
            continue;
        }
        members.add(member.name(), member.is_dir(), member.size())?;
    }
    Ok(members.finish(true))
}

fn tar(input: &mut File, cancel: &AtomicBool) -> Result<Inventory, String> {
    let length = input.metadata().map_err(|e| e.to_string())?.len();
    let mut archive = tar::Archive::new(input);
    let mut members = Members::default();
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
        let kind = member.header().entry_type();
        if !kind.is_file() && !kind.is_dir() {
            continue;
        }
        let path = member.path().map_err(|e| e.to_string())?;
        let name = path.to_str().ok_or("Archive member name is not UTF-8")?;
        members.add(name, kind.is_dir(), member.size())?;
    }
    Ok(members.finish(true))
}

#[derive(Default)]
struct Members {
    names: BTreeSet<String>,
    children: Vec<Child>,
    observed: usize,
}
impl Members {
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
        if canonical.is_empty() || !self.names.insert(canonical.clone()) {
            return Ok(());
        }
        self.observed += 1;
        if self.children.len() < LIMIT {
            self.children.push(Child::Member {
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
            complete: scanned_all && self.observed <= LIMIT,
            children: self.children,
            observed: self.observed,
            archive: true,
        }
    }
}
