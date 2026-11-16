//! Safe recursive directory traversal for image files.
use std::{
    collections::HashSet,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

use super::super::super::filesystem::{Entry, read_entry};

/// Recursively scans a directory for image files.
///
/// Avoids symlink cycles, bind mount recursion, and kernel filesystems. Returns early
/// with collected entries if `cancelled` is set or if `photo_limit` is reached.
pub fn scan_recursive_photos(
    root: &Path,
    show_hidden: bool,
    photo_limit: usize,
    max_depth: usize,
    cancelled: &AtomicBool,
) -> Vec<Entry> {
    let mut results = Vec::new();
    let mut visited_dirs = HashSet::new();

    if little_clip_core::drives::index_excluded(root) {
        return results;
    }
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
        if little_clip_core::drives::index_excluded(&dir_path) {
            continue;
        }
        if depth > max_depth {
            continue;
        }
        if results.len() >= photo_limit {
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
            if cancelled.load(Ordering::Relaxed) || results.len() >= photo_limit {
                break;
            }
            let file_name = child.file_name();
            let is_hidden = file_name.as_encoded_bytes().starts_with(b".");
            if is_hidden && !show_hidden {
                continue;
            }

            // Recursive discovery must not enter an excluded tree through a link
            // or decode a linked payload under an unrelated policy pathname.
            if !child
                .file_type()
                .is_ok_and(|kind| kind.is_dir() || kind.is_file())
            {
                continue;
            }
            if little_clip_core::drives::index_excluded(&dir_path.join(&file_name)) {
                continue;
            }
            let entry = read_entry(child, true);
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

    const DEFAULT_TEST_DIRECTORY_DEPTH: usize = 16;

    #[test]
    fn drive_no_index_policy_prunes_photo_subtrees_before_discovery() {
        use crate::file_manager::tests::{with_drive_policy, write_drive_policy};
        with_drive_policy(|root| {
            let excluded = root.join("excluded");
            std::fs::create_dir(&excluded).unwrap();
            std::fs::write(excluded.join("hidden.jpg"), b"fixture").unwrap();
            std::fs::write(root.join("visible.jpg"), b"fixture").unwrap();
            std::os::unix::fs::symlink(&excluded, root.join("directory-alias")).unwrap();
            std::os::unix::fs::symlink(excluded.join("hidden.jpg"), root.join("file-alias.jpg"))
                .unwrap();
            write_drive_policy(vec![excluded.clone()]);
            let photos = scan_recursive_photos(root, true, 10, 5, &AtomicBool::new(false));
            assert_eq!(photos.len(), 1);
            assert_eq!(photos[0].name, "visible.jpg");
            assert!(
                scan_recursive_photos(&excluded, true, 10, 5, &AtomicBool::new(false)).is_empty()
            );
            write_drive_policy(vec![]);
            assert_eq!(
                scan_recursive_photos(root, true, 10, 5, &AtomicBool::new(false)).len(),
                2
            );
        });
    }

    #[test]
    fn recursive_scanner_skips_over_depth_branches_without_abandoning_siblings() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();

        // Branch A: shallow sibling with an image
        let branch_a = root.join("branch_a");
        std::fs::create_dir_all(&branch_a).unwrap();
        std::fs::write(branch_a.join("photo_a.jpg"), b"fake_jpeg").unwrap();

        // Branch B: deep branch exceeding the default configured depth.
        let mut deep = root.join("branch_b");
        for i in 0..=DEFAULT_TEST_DIRECTORY_DEPTH + 2 {
            deep = deep.join(format!("depth_{i}"));
        }
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::write(deep.join("deep_photo.jpg"), b"fake_jpeg").unwrap();

        // Branch C: another shallow sibling with an image
        let branch_c = root.join("branch_c");
        std::fs::create_dir_all(&branch_c).unwrap();
        std::fs::write(branch_c.join("photo_c.png"), b"fake_png").unwrap();

        let cancelled = AtomicBool::new(false);
        let photos = scan_recursive_photos(
            root,
            false,
            10_000,
            DEFAULT_TEST_DIRECTORY_DEPTH,
            &cancelled,
        );

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
        let without_hidden = scan_recursive_photos(
            root,
            false,
            10_000,
            DEFAULT_TEST_DIRECTORY_DEPTH,
            &cancelled,
        );
        assert_eq!(without_hidden.len(), 1);
        assert_eq!(without_hidden[0].name, "visible.jpg");

        let with_hidden =
            scan_recursive_photos(root, true, 10_000, DEFAULT_TEST_DIRECTORY_DEPTH, &cancelled);
        assert_eq!(with_hidden.len(), 2);
    }

    #[test]
    fn recursive_scanner_stops_at_configured_photo_limit() {
        let temp = tempfile::tempdir().unwrap();
        for index in 0..5 {
            std::fs::write(temp.path().join(format!("photo_{index}.jpg")), b"fixture").unwrap();
        }

        let photos = scan_recursive_photos(
            temp.path(),
            false,
            3,
            DEFAULT_TEST_DIRECTORY_DEPTH,
            &AtomicBool::new(false),
        );
        assert_eq!(photos.len(), 3);
    }

    #[test]
    fn recursive_scanner_respects_configured_max_depth() {
        let temp = tempfile::tempdir().unwrap();
        let nested = temp.path().join("one").join("two").join("three");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(nested.join("photo.jpg"), b"fixture").unwrap();

        let shallow = scan_recursive_photos(temp.path(), false, 10, 2, &AtomicBool::new(false));
        assert!(shallow.is_empty());
        let deep = scan_recursive_photos(temp.path(), false, 10, 3, &AtomicBool::new(false));
        assert_eq!(deep.len(), 1);
    }
}
