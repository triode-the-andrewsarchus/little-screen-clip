// SPDX-License-Identifier: MPL-2.0
//! System font search and cumulative accounting from d2fonts/fallback.go.
use crate::system_font_discovery::{self as discovery, DiscoveryWork};
use crate::system_font_priority::{FaceProfile, RequestProfile, select_collection_faces};
use crate::{FallbackFont, FallbackRequest, FallbackResolver};
use d2_rs_core::stable_sort::StableSort;
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy, Debug)]
pub struct SystemFallbackLimits {
    pub max_directory_entries: usize,
    pub max_files: usize,
    pub max_faces: usize,
    pub max_requested_runes: usize,
    pub max_coverage_checks: u64,
    pub max_file_bytes: u64,
    pub max_scanned_bytes: u64,
    pub max_resolved_bytes: u64,
}

#[derive(Debug, Default)]
struct Work {
    discovery: DiscoveryWork,
    paths: Option<Vec<PathBuf>>,
    requested_runes: usize,
    faces: usize,
    coverage_checks: u64,
    scanned_bytes: u64,
    resolved_bytes: u64,
}

#[derive(Debug)]
pub struct SystemFallbackResolver {
    limits: SystemFallbackLimits,
    work: Mutex<Work>,
}

impl SystemFallbackResolver {
    pub fn new(limits: SystemFallbackLimits) -> Result<Self, String> {
        if limits.max_directory_entries == 0
            || limits.max_files == 0
            || limits.max_faces == 0
            || limits.max_requested_runes == 0
            || limits.max_coverage_checks == 0
            || limits.max_file_bytes == 0
            || limits.max_scanned_bytes == 0
            || limits.max_resolved_bytes == 0
        {
            return Err("d2fonts: every system fallback limit must be positive".into());
        }
        Ok(Self {
            limits,
            work: Mutex::new(Work::default()),
        })
    }
}

impl FallbackResolver for SystemFallbackResolver {
    fn resolve_fallbacks(&self, request: &FallbackRequest) -> Result<Vec<FallbackFont>, String> {
        let mut work = self
            .work
            .lock()
            .map_err(|_| "d2fonts: poisoned system fallback resolver")?;
        let limits = self.limits;
        if request.family.len() + request.style.len() > 1024 {
            return Err("d2fonts: fallback request font metadata exceeds 1024 bytes".into());
        }
        let count = request.runes.len();
        if count > limits.max_requested_runes {
            return Err(format!(
                "d2fonts: requested fallback rune count {count} exceeds limit {}",
                limits.max_requested_runes
            ));
        }
        if count > limits.max_requested_runes - work.requested_runes {
            return Err(format!(
                "d2fonts: requested fallback rune count exceeds cumulative resolver limit {}",
                limits.max_requested_runes
            ));
        }
        work.requested_runes += count;
        let mut remaining: BTreeSet<char> = request
            .runes
            .iter()
            .copied()
            .filter(|r| {
                let n = *r as u32;
                !(0xfdd0..=0xfdef).contains(&n) && n & 0xffff < 0xfffe
            })
            .collect();
        if remaining.is_empty() {
            return Ok(Vec::new());
        }
        let mut profile_request = request.clone();
        profile_request.runes = remaining.iter().copied().collect();
        let profile = RequestProfile::new(&profile_request);
        if work.paths.is_none() {
            let paths = discovery::font_paths(
                &discovery::system_font_roots(),
                limits.max_directory_entries,
                limits.max_files,
                &mut work.discovery,
            )?;
            work.paths = Some(paths);
        }
        let mut paths = work.paths.clone().unwrap_or_default();
        paths.stable_sort_by(|a, b| {
            profile
                .path_priority(a)
                .cmp(&profile.path_priority(b))
                .then_with(|| a.cmp(b))
        });
        let mut result = Vec::new();
        for path in paths {
            let basename = path.file_name().unwrap_or_default().to_string_lossy();
            if work.scanned_bytes == limits.max_scanned_bytes {
                return Err(format!(
                    "d2fonts: system font {basename:?}: system font scan bytes exceed limit"
                ));
            }
            let probe = discovery::probe_font_file_coverage(
                &path,
                limits.max_file_bytes,
                limits.max_scanned_bytes - work.scanned_bytes,
                limits.max_faces - work.faces,
                &remaining.iter().copied().collect::<Vec<_>>(),
            );
            if probe.limit_hit {
                work.scanned_bytes += probe.bytes;
                return Err(format!(
                    "d2fonts: system font {basename:?}: font cmap probe bytes exceed remaining scan limit"
                ));
            }
            if probe.faces == 0 {
                work.scanned_bytes += probe.bytes;
                continue;
            }
            if probe.faces > limits.max_faces - work.faces {
                work.scanned_bytes += probe.bytes;
                return Err(format!(
                    "d2fonts: system font face count exceeds cumulative resolver limit {} while resolving U+{:04X}",
                    limits.max_faces,
                    *remaining.first().unwrap() as u32
                ));
            }
            if !probe.covers {
                work.faces += probe.faces;
                work.scanned_bytes += probe.bytes;
                continue;
            }
            let data = match discovery::read_bounded_font_file(
                &path,
                limits.max_file_bytes,
                limits.max_scanned_bytes - work.scanned_bytes,
            ) {
                Ok(data) => data,
                Err(err) => {
                    work.scanned_bytes += probe.bytes;
                    return Err(format!("d2fonts: system font {basename:?}: {err}"));
                }
            };
            if data.is_empty() {
                work.scanned_bytes += probe.bytes;
                continue;
            }
            work.scanned_bytes += data.len() as u64;
            let collection = match d2_rs_fontface::parse_face_collection_with_limit(
                &data,
                limits.max_faces - work.faces,
            ) {
                Ok(collection) => collection,
                Err(error @ d2_rs_fontface::Error::FaceCountLimit { .. }) => {
                    return Err(format!("d2fonts: system font {basename:?}: {error}"));
                }
                Err(_) => continue,
            };
            work.faces += collection.num_faces();
            let data: Arc<[u8]> = Arc::from(data);
            let mime = match path
                .extension()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_ascii_lowercase()
                .as_str()
            {
                "ttc" | "otc" => "font/collection",
                "otf" => "font/otf",
                _ => "font/ttf",
            };
            let mut retained = work.resolved_bytes;
            select_collection_faces(&collection, &mut remaining, &profile, &mut work.coverage_checks, limits.max_coverage_checks, FaceProfile::from_face, |index| {
                if data.len() as u64 > limits.max_resolved_bytes - retained {
                    return Err(format!("d2fonts: resolved fallback font bytes exceed limit {} while retaining face {index} from {basename:?}", limits.max_resolved_bytes));
                }
                retained += data.len() as u64;
                result.push(FallbackFont { name: basename.to_string(), mime_type: mime.into(), data: data.clone(), face_index: index as u16 });
                Ok(())
            }).inspect_err(|_e| { work.resolved_bytes = retained; })?;
            work.resolved_bytes = retained;
            if remaining.is_empty() {
                break;
            }
        }
        Ok(result)
    }
}
