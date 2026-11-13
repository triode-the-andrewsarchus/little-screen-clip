// SPDX-License-Identifier: MPL-2.0
//! Explicit bounded LRU; lib/imageasset/imageasset.go:215–302.
use super::{ResolvedImageAsset, error, limit};
use crate::{ImageKind, ImageResolveError};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

#[derive(Debug, Default)]
struct Entries {
    resources: HashMap<String, Entry>,
    first: Option<String>,
    last: Option<String>,
    bytes: i64,
}

#[derive(Debug)]
struct Entry {
    resource: Arc<ResolvedImageAsset>,
    previous: Option<String>,
    next: Option<String>,
}

impl Entries {
    fn remove(&mut self, key: &str) -> Option<Arc<ResolvedImageAsset>> {
        let entry = self.resources.remove(key)?;
        if let Some(previous) = &entry.previous {
            self.resources.get_mut(previous).unwrap().next = entry.next.clone();
        } else {
            self.first = entry.next.clone();
        }
        if let Some(next) = &entry.next {
            self.resources.get_mut(next).unwrap().previous = entry.previous;
        } else {
            self.last = entry.previous;
        }
        self.bytes -= entry.resource.resource.data.len() as i64;
        Some(entry.resource)
    }

    fn insert_front(&mut self, key: String, resource: Arc<ResolvedImageAsset>) {
        let next = self.first.replace(key.clone());
        if let Some(next) = &next {
            self.resources.get_mut(next).unwrap().previous = Some(key.clone());
        } else {
            self.last = Some(key.clone());
        }
        self.bytes += resource.resource.data.len() as i64;
        self.resources.insert(
            key,
            Entry {
                resource,
                previous: None,
                next,
            },
        );
    }
}

/// Caller-owned, concurrency-safe cache of immutable admitted resources.
/// No global cache is installed. Limits count retained encoded bytes.
#[derive(Debug)]
pub struct ImageMemoryCache {
    max_entries: usize,
    max_bytes: i64,
    entries: Mutex<Entries>,
}

#[derive(Clone, Copy, Debug, serde::Serialize)]
pub struct ImageCacheUsage {
    pub entries: usize,
    pub encoded: i64,
}

impl ImageMemoryCache {
    pub fn new(max_entries: i64, max_bytes: i64) -> Result<Self, ImageResolveError> {
        if max_entries <= 0 || max_bytes <= 0 {
            return Err(error("imageasset: cache limits must be positive"));
        }
        Ok(Self {
            max_entries: usize::try_from(max_entries).unwrap_or(usize::MAX),
            max_bytes,
            entries: Mutex::default(),
        })
    }

    pub fn usage(&self) -> ImageCacheUsage {
        let entries = self.entries.lock().unwrap();
        ImageCacheUsage {
            entries: entries.resources.len(),
            encoded: entries.bytes,
        }
    }

    pub(super) fn get(&self, key: &str) -> Option<Arc<ResolvedImageAsset>> {
        let mut entries = self.entries.lock().unwrap();
        let resource = entries.remove(key)?;
        entries.insert_front(key.to_owned(), resource.clone());
        Some(resource)
    }

    pub(super) fn put(&self, key: String, resource: &ResolvedImageAsset) {
        let mut entries = self.entries.lock().unwrap();
        entries.remove(&key);
        let size = resource.resource.data.len() as i64;
        if size > self.max_bytes {
            return;
        }
        while entries.resources.len() >= self.max_entries || size > self.max_bytes - entries.bytes {
            let oldest = entries.last.clone().expect("LRU entry");
            entries.remove(&oldest).expect("LRU resource");
        }
        entries.insert_front(key, Arc::new(resource.clone()));
    }
}

/// Cache hits revalidate acquisition metadata and every per-resource limit
/// before this resolver independently reserves cumulative capacity.
pub(super) fn validate(
    resource: &ResolvedImageAsset,
    limits: super::ImageAssetLimits,
) -> Result<(), ImageResolveError> {
    let r = &resource.resource;
    // Entries are constructed only by the resolver, so their kind/dimension/
    // footprint invariants cannot be forged through this public cache API.
    limit(
        "fetched bytes",
        resource.fetched_bytes,
        limits.max_fetched_bytes,
    )?;
    limit(
        "encoded bytes",
        r.data.len() as i64,
        limits.max_encoded_bytes,
    )?;
    limit(
        "decompressed bytes",
        resource.decompressed_bytes,
        limits.max_decompressed_bytes,
    )?;
    if r.kind == ImageKind::Svg {
        limit("SVG bytes", r.data.len() as i64, limits.max_svg_bytes)?;
    }
    if r.kind == ImageKind::Raster {
        limit("decoded width", r.pixel_width, limits.max_decoded_width)?;
        limit("decoded height", r.pixel_height, limits.max_decoded_height)?;
        limit(
            "decoded pixels",
            r.pixel_width * r.pixel_height,
            limits.max_decoded_pixels,
        )?;
    }
    limit(
        "decoded bytes",
        r.decoded_bytes,
        limits.max_cumulative_decoded_bytes,
    )
}
