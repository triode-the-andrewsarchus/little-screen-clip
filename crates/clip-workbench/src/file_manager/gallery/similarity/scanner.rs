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

/// Recursively scans a directory for image files.
///
/// Avoids symlink cycles, bind mount recursion, and kernel filesystems. Returns early
/// with collected entries if `cancelled` is set or if the recursive item budget is reached.
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
        if depth > MAX_DIRECTORY_DEPTH {
            continue;
        }
        if results.len() >= MAX_RECURSIVE_ITEMS {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recursive_scanner_skips_over_depth_branches_without_abandoning_siblings() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();

        // Branch A: shallow sibling with an image
        let branch_a = root.join("branch_a");
        std::fs::create_dir_all(&branch_a).unwrap();
        std::fs::write(branch_a.join("photo_a.jpg"), b"fake_jpeg").unwrap();

        // Branch B: deep branch exceeding MAX_DIRECTORY_DEPTH
        let mut deep = root.join("branch_b");
        for i in 0..=MAX_DIRECTORY_DEPTH + 2 {
            deep = deep.join(format!("depth_{i}"));
        }
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::write(deep.join("deep_photo.jpg"), b"fake_jpeg").unwrap();

        // Branch C: another shallow sibling with an image
        let branch_c = root.join("branch_c");
        std::fs::create_dir_all(&branch_c).unwrap();
        std::fs::write(branch_c.join("photo_c.png"), b"fake_png").unwrap();

        let cancelled = AtomicBool::new(false);
        let photos = scan_recursive_photos(root, false, &cancelled);

        let names: Vec<String> = photos.into_iter().map(|e| e.name).collect();
        assert!(names.contains(&"photo_a.jpg".to_string()));
        assert!(names.contains(&"photo_c.png".to_string()));
        assert!(!names.contains(&"deep_photo.jpg".to_string()));
    }

    #[test]
    fn recursive_scanner_respects_hidden_toggle() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();

        std::fs::write(root.join("visible.jpg"), b"fake_jpeg").unwrap();
        std::fs::write(root.join(".hidden.jpg"), b"fake_jpeg").unwrap();

        let cancelled = AtomicBool::new(false);
        let without_hidden = scan_recursive_photos(root, false, &cancelled);
        assert_eq!(without_hidden.len(), 1);
        assert_eq!(without_hidden[0].name, "visible.jpg");

        let with_hidden = scan_recursive_photos(root, true, &cancelled);
        assert_eq!(with_hidden.len(), 2);
    }
}
