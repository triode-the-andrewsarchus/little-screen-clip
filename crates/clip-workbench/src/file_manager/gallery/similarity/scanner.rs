//! Safe recursive directory traversal for image files.
use std::{
    collections::HashSet,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

use super::super::super::filesystem::{Entry, read_entry};

const MAX_RECURSIVE_ITEMS: usize = 10_000;
const MAX_DIRECTORY_DEPTH: usize = 16;

/// Recursively scans `root` for image files (`entry.is_photo()`).
///
/// Avoids symlink cycles, bind mount recursion, and kernel filesystems.
pub fn scan_recursive_photos(root: &Path, show_hidden: bool, cancelled: &AtomicBool) -> Vec<Entry> {
    let mut results = Vec::new();
    let mut visited_dirs = HashSet::new();

    let root_canon = match root.canonicalize() {
        Ok(p) => p,
        Err(_) => root.to_path_buf(),
    };

    if let Ok(meta) = std::fs::metadata(&root_canon) {
        visited_dirs.insert((meta.dev(), meta.ino()));
    }

    let mut dir_queue: Vec<(PathBuf, usize)> = vec![(root_canon, 0)];

    while let Some((dir_path, depth)) = dir_queue.pop() {
        if cancelled.load(Ordering::Relaxed) {
            break;
        }
        if depth > MAX_DIRECTORY_DEPTH || results.len() >= MAX_RECURSIVE_ITEMS {
            break;
        }

        // Validate that this path is not a kernel/pseudo interface
        if !little_clip_core::kernel_interface::classify(&dir_path)
            .is_ok_and(|kind| !kind.protected())
        {
            continue;
        }

        let Ok(reader) = std::fs::read_dir(&dir_path) else {
            continue;
        };

        for child in reader.flatten() {
            if cancelled.load(Ordering::Relaxed) || results.len() >= MAX_RECURSIVE_ITEMS {
                break;
            }
            let file_name = child.file_name();
            let is_hidden = file_name.as_encoded_bytes().starts_with(b".");
            if is_hidden && !show_hidden {
                continue;
            }

            let entry = read_entry(child);
            if entry.directory {
                if let Ok(meta) = std::fs::metadata(&entry.path) {
                    let key = (meta.dev(), meta.ino());
                    if visited_dirs.insert(key) {
                        dir_queue.push((entry.path.clone(), depth + 1));
                    }
                }
            } else if entry.is_photo() {
                results.push(entry);
            }
        }
    }

    results
}
