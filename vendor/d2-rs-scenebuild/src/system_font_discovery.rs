// SPDX-License-Identifier: MPL-2.0
//! Bounded host font discovery, following d2fonts/fallback.go fontPaths.

use std::ffi::{CStr, OsString};
use std::io::Read;
use std::os::fd::IntoRawFd;
use std::os::unix::ffi::OsStringExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

/// Work remains charged if a discovery attempt fails and is retried.
#[derive(Debug, Default)]
pub(crate) struct DiscoveryWork {
    pub directory_entries: usize,
    pub files: usize,
}

/// Conventional roots from d2fonts.systemFontRoots, excluding user fonts.
#[allow(dead_code)] // Used by the forthcoming system resolver.
pub(crate) fn system_font_roots() -> Vec<PathBuf> {
    // An explicit directory makes font-dependent exports reproducible. With
    // no override, retain d2fonts.systemFontRoots' conventional host roots.
    if let Some(directory) = std::env::var_os("D2RS_SYSTEM_FONT_DIR") {
        return vec![directory.into()];
    }
    if cfg!(target_os = "macos") {
        vec!["/System/Library/Fonts".into(), "/Library/Fonts".into()]
    } else if cfg!(any(
        target_os = "linux",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "dragonfly"
    )) {
        vec!["/usr/share/fonts".into(), "/usr/local/share/fonts".into()]
    } else {
        Vec::new()
    }
}

/// Open a candidate without accepting a symlink or a replaced file. Go's
/// coverage probe also checks that the size did not change while opening.
#[allow(dead_code)] // Used by the forthcoming bounded coverage probe.
pub(crate) fn open_verified_font(path: &Path, max_file_bytes: u64) -> Option<std::fs::File> {
    let listed = std::fs::symlink_metadata(path).ok()?;
    if !listed.is_file()
        || listed.file_type().is_symlink()
        || listed.len() == 0
        || listed.len() > max_file_bytes
    {
        return None;
    }
    let file = std::fs::File::open(path).ok()?;
    let opened = file.metadata().ok()?;
    if !opened.is_file()
        || (listed.dev(), listed.ino()) != (opened.dev(), opened.ino())
        || opened.len() != listed.len()
    {
        return None;
    }
    Some(file)
}

/// Read a candidate under the remaining scan budget, following
/// d2fonts.readBoundedFontFile. Partial reads are returned for accounting.
#[allow(dead_code)] // Used by the system resolver when it is connected.
pub(crate) fn read_bounded_font_file(
    path: &Path,
    max_file_bytes: u64,
    remaining_scan_bytes: u64,
) -> Result<Vec<u8>, String> {
    if remaining_scan_bytes == 0 {
        return Err("system font scan bytes exceed limit".into());
    }
    let Ok(listed) = std::fs::symlink_metadata(path) else {
        return Ok(Vec::new());
    };
    if !listed.is_file()
        || listed.file_type().is_symlink()
        || listed.len() == 0
        || listed.len() > max_file_bytes
    {
        return Ok(Vec::new());
    }
    if listed.len() > remaining_scan_bytes {
        return Err(format!(
            "system font scan bytes exceed remaining limit {remaining_scan_bytes}"
        ));
    }
    let Ok(file) = std::fs::File::open(path) else {
        return Ok(Vec::new());
    };
    let Ok(opened) = file.metadata() else {
        return Ok(Vec::new());
    };
    if !opened.is_file()
        || (listed.dev(), listed.ino()) != (opened.dev(), opened.ino())
        || opened.len() == 0
    {
        return Ok(Vec::new());
    }
    let limit = max_file_bytes.min(remaining_scan_bytes);
    if opened.len() > limit {
        return Err(format!(
            "system font file exceeds remaining byte limit {limit}"
        ));
    }
    let mut data = Vec::new();
    let _ = file.take(limit.saturating_add(1)).read_to_end(&mut data);
    if data.len() as u64 > limit {
        return Err(format!(
            "system font file grew beyond byte limit {limit} while reading"
        ));
    }
    Ok(data)
}

/// Result of d2fonts.probeFontFileCoverage. Probe reads are charged even
/// when a face is malformed; a matching file is charged in full by the
/// caller instead of charging these reads twice.
#[derive(Debug, Default)]
pub(crate) struct CoverageProbe {
    pub faces: usize,
    pub bytes: u64,
    pub covers: bool,
    pub limit_hit: bool,
}

#[allow(dead_code)] // The system resolver will consume this probe.
pub(crate) fn probe_font_file_coverage(
    path: &Path,
    max_file_bytes: u64,
    remaining_scan_bytes: u64,
    max_faces: usize,
    values: &[char],
) -> CoverageProbe {
    if remaining_scan_bytes == 0 {
        return CoverageProbe {
            limit_hit: true,
            ..CoverageProbe::default()
        };
    }
    let Some(file) = open_verified_font(path, max_file_bytes) else {
        return CoverageProbe::default();
    };
    let mut reader = d2_rs_fontface::sfnt::BoundedReader::new(file, remaining_scan_bytes);
    let mut probe = CoverageProbe::default();
    if let Ok((offsets, dfont)) = reader.collection_offsets() {
        probe.faces = offsets.len();
        if probe.faces <= max_faces {
            'faces: for offset in offsets {
                let Ok(cmap) = reader.font_probe_cmap(offset, dfont) else {
                    if reader.limit_hit() {
                        break;
                    }
                    continue;
                };
                for &value in values {
                    match reader.cmap_glyph_index(&cmap, value) {
                        Ok(glyph) if glyph != 0 => {
                            probe.covers = true;
                            break 'faces;
                        }
                        Err(_) if reader.limit_hit() => break 'faces,
                        _ => {}
                    }
                }
            }
        }
    }
    probe.bytes = reader.bytes_read();
    probe.limit_hit = reader.limit_hit();
    probe
}

// The directory stream owns the file descriptor and closes it on drop.
struct Directory(*mut libc::DIR);

impl Drop for Directory {
    fn drop(&mut self) {
        // SAFETY: fdopendir returned this stream and ownership is unique.
        unsafe { libc::closedir(self.0) };
    }
}

impl Directory {
    fn open(path: &Path) -> std::io::Result<Self> {
        let listed = std::fs::symlink_metadata(path)?;
        if !listed.is_dir() || listed.file_type().is_symlink() {
            return Err(std::io::Error::other("path is not a non-symlink directory"));
        }
        let file = std::fs::File::open(path)?;
        let opened = file.metadata()?;
        if !opened.is_dir() || (listed.dev(), listed.ino()) != (opened.dev(), opened.ino()) {
            return Err(std::io::Error::other(
                "directory identity changed while opening",
            ));
        }
        let fd = file.into_raw_fd();
        // SAFETY: fd is an open directory descriptor; fdopendir takes
        // ownership only when it succeeds.
        let stream = unsafe { libc::fdopendir(fd) };
        if stream.is_null() {
            let error = std::io::Error::last_os_error();
            unsafe { libc::close(fd) };
            return Err(error);
        }
        Ok(Self(stream))
    }

    fn next(&mut self) -> Option<(OsString, u8)> {
        loop {
            // SAFETY: the stream is live and uniquely borrowed. Copy the name
            // before the next readdir call invalidates its returned pointer.
            let entry = unsafe { libc::readdir(self.0) };
            if entry.is_null() {
                return None;
            }
            let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) }.to_bytes();
            if name != b"." && name != b".." {
                return Some((OsString::from_vec(name.to_vec()), unsafe {
                    (*entry).d_type
                }));
            }
        }
    }
}

/// Collect candidate paths from verified directory handles. The caller sorts
/// the resulting paths for each request using the Go fallback priority.
#[allow(dead_code)] // Wired into the system resolver when its probe is complete.
pub(crate) fn font_paths(
    roots: &[PathBuf],
    max_entries: usize,
    max_files: usize,
    work: &mut DiscoveryWork,
) -> Result<Vec<PathBuf>, String> {
    let mut paths = Vec::new();
    for root in roots {
        if !std::fs::symlink_metadata(root).is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink())
        {
            continue;
        }
        if work.directory_entries >= max_entries {
            return Err(format!(
                "d2fonts: system font directory entries exceed limit {max_entries}"
            ));
        }
        work.directory_entries += 1;
        let mut stack = vec![root.clone()];
        while let Some(directory) = stack.pop() {
            let Ok(mut handle) = Directory::open(&directory) else {
                continue;
            };
            while let Some((name, kind)) = handle.next() {
                if work.directory_entries >= max_entries {
                    return Err(format!(
                        "d2fonts: system font directory entries exceed limit {max_entries}"
                    ));
                }
                work.directory_entries += 1;
                let path = directory.join(name);
                if kind == libc::DT_LNK {
                    continue;
                }
                if kind == libc::DT_DIR {
                    stack.push(path);
                    continue;
                }
                let extension = path.extension().and_then(|s| s.to_str()).unwrap_or("");
                if !["ttf", "otf", "ttc", "otc"]
                    .iter()
                    .any(|ext| extension.eq_ignore_ascii_case(ext))
                {
                    continue;
                }
                if work.files >= max_files {
                    return Err(format!(
                        "d2fonts: system font candidate count exceeds limit {max_files}; raise the bounded fallback search limit or configure a custom font"
                    ));
                }
                work.files += 1;
                paths.push(path);
            }
        }
    }
    Ok(paths)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coverage_probe_reports_faces_and_charges_bounded_reads() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../d2-rs-text/assets/SourceSansPro-Regular.ttf");
        let probe = probe_font_file_coverage(&path, u64::MAX, u64::MAX, 1, &['A']);
        assert_eq!(probe.faces, 1);
        assert!(probe.covers && probe.bytes > 0 && !probe.limit_hit);
        let limited = probe_font_file_coverage(&path, u64::MAX, 16, 1, &['A']);
        assert!(limited.limit_hit && !limited.covers);
        assert!(limited.bytes <= 16);
        let no_faces = probe_font_file_coverage(&path, u64::MAX, u64::MAX, 0, &['A']);
        assert_eq!(no_faces.faces, 1);
        assert!(!no_faces.covers && !no_faces.limit_hit);
    }

    #[test]
    fn discovery_counts_entries_and_never_follows_symlinks() {
        let root = std::env::temp_dir().join(format!("d2rs-font-discovery-{}", std::process::id()));
        std::fs::create_dir_all(root.join("nested")).unwrap();
        std::fs::write(root.join("nested/face.TTF"), b"font").unwrap();
        std::fs::write(root.join("ignore.txt"), b"other").unwrap();
        std::os::unix::fs::symlink(root.join("nested"), root.join("linked")).unwrap();
        assert_eq!(
            read_bounded_font_file(&root.join("nested/face.TTF"), 4, 4).unwrap(),
            b"font"
        );
        assert!(
            read_bounded_font_file(&root.join("nested/face.TTF"), 4, 3)
                .unwrap_err()
                .contains("remaining limit 3")
        );
        assert!(
            read_bounded_font_file(&root.join("nested/face.TTF"), 3, 4)
                .unwrap()
                .is_empty()
        );
        assert!(open_verified_font(&root.join("nested/face.TTF"), 4).is_some());
        assert!(open_verified_font(&root.join("nested/face.TTF"), 3).is_none());
        assert!(open_verified_font(&root.join("linked"), 100).is_none());
        std::os::unix::fs::symlink(root.join("nested/face.TTF"), root.join("link.ttf")).unwrap();
        assert!(open_verified_font(&root.join("link.ttf"), 100).is_none());
        std::fs::remove_file(root.join("link.ttf")).unwrap();
        let mut work = DiscoveryWork::default();
        let paths = font_paths(std::slice::from_ref(&root), 5, 1, &mut work).unwrap();
        assert_eq!(paths, vec![root.join("nested/face.TTF")]);
        assert_eq!((work.directory_entries, work.files), (5, 1));
        let mut work = DiscoveryWork::default();
        assert!(
            font_paths(std::slice::from_ref(&root), 4, 1, &mut work)
                .unwrap_err()
                .contains("directory entries exceed limit 4")
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
