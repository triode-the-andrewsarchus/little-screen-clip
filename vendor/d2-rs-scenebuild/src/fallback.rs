// SPDX-License-Identifier: MPL-2.0
//! d2fonts/bundled_fallback.go: bounded, deterministic native fallback.
use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

/// Missing code points and the original scene font style.
#[derive(Clone, Debug)]
pub struct FallbackRequest {
    pub runes: Vec<char>,
    pub family: String,
    pub style: String,
    pub weight: i64,
}

/// An immutable resolver result. The builder validates and owns its scene copy.
#[derive(Clone, Debug)]
pub struct FallbackFont {
    pub name: String,
    pub mime_type: String,
    pub data: Arc<[u8]>,
    pub face_index: u16,
}

/// Scene-builder resolver handoff. Bundled results use upstream's internal
/// shared-resource accounting, rather than its mutable public-result copies.
/// Cancellation and operating-system font discovery are not exposed here.
pub trait FallbackResolver: std::fmt::Debug + Send + Sync {
    fn resolve_fallbacks(&self, request: &FallbackRequest) -> Result<Vec<FallbackFont>, String>;
}

/// Cumulative limits over one bundled resolver's lifetime.
#[derive(Clone, Copy, Debug)]
pub struct BundledFallbackLimits {
    pub max_requested_runes: i64,
    pub max_bundled_bytes: i64,
    pub max_resolved_bytes: i64,
}

#[derive(Default, Debug)]
struct Work {
    requested_runes: i64,
    retained: Option<Arc<[u8]>>,
}

/// Bundled Noto resolution, optionally followed by another bounded resolver.
/// It never scans the host or accesses the network. Native builds authenticate
/// the pinned font; WASM omits it, matching upstream's platform boundary.
#[derive(Debug)]
pub struct BundledFallbackResolver {
    limits: BundledFallbackLimits,
    next: Option<Arc<dyn FallbackResolver>>,
    work: Mutex<Work>,
    resolved_bytes: Mutex<i64>,
}

impl BundledFallbackResolver {
    pub fn new(
        next: Option<Arc<dyn FallbackResolver>>,
        mut limits: BundledFallbackLimits,
    ) -> Result<Self, String> {
        if limits.max_bundled_bytes == 0 {
            limits.max_bundled_bytes = limits.max_resolved_bytes;
        }
        if limits.max_requested_runes <= 0
            || limits.max_bundled_bytes <= 0
            || limits.max_resolved_bytes <= 0
        {
            return Err("d2fonts: every bundled fallback limit must be positive".into());
        }
        if limits.max_bundled_bytes > limits.max_resolved_bytes {
            return Err(format!(
                "d2fonts: bundled fallback byte limit {} exceeds aggregate resolved byte limit {}",
                limits.max_bundled_bytes, limits.max_resolved_bytes
            ));
        }
        Ok(Self {
            limits,
            next,
            work: Mutex::default(),
            resolved_bytes: Mutex::default(),
        })
    }
}

impl FallbackResolver for BundledFallbackResolver {
    fn resolve_fallbacks(&self, request: &FallbackRequest) -> Result<Vec<FallbackFont>, String> {
        // Hold the gate through resolution, as acquireResolve does upstream.
        let mut work = self
            .work
            .lock()
            .map_err(|_| "d2fonts: poisoned bundled fallback resolver")?;
        let mut resolved_bytes = self
            .resolved_bytes
            .lock()
            .map_err(|_| "d2fonts: poisoned bundled fallback resolver")?;
        if request.family.len() + request.style.len() > 1024 {
            return Err("d2fonts: fallback request font metadata exceeds 1024 bytes".into());
        }
        let count = request.runes.len() as i64;
        if count > self.limits.max_requested_runes {
            return Err(format!(
                "d2fonts: requested fallback rune count {count} exceeds limit {}",
                self.limits.max_requested_runes
            ));
        }
        if count > self.limits.max_requested_runes - work.requested_runes {
            return Err(format!(
                "d2fonts: requested fallback rune count exceeds cumulative resolver limit {}",
                self.limits.max_requested_runes
            ));
        }
        work.requested_runes += count;
        let mut remaining: BTreeSet<char> = request
            .runes
            .iter()
            .copied()
            .filter(|r| {
                let value = *r as u32;
                !(0xfdd0..=0xfdef).contains(&value) && value & 0xffff < 0xfffe
            })
            .collect();
        let mut result = Vec::new();
        if remaining
            .iter()
            .copied()
            .any(d2_rs_fontface::bundled_noto_color_emoji_covers_rune)
        {
            if let Some(data) = bundled_data()? {
                let source = d2_rs_fontface::registered_bundled_noto_color_emoji(&data, 0)
                    .map_err(|e| e.to_string())?
                    .ok_or("d2fonts: bundled Noto Color Emoji face is not authenticated")?;
                let mut covered = Vec::new();
                for &r in &remaining {
                    if source
                        .supports_renderable_rune(r)
                        .map_err(|e| e.to_string())?
                    {
                        covered.push(r);
                    }
                }
                if !covered.is_empty() {
                    if work.retained.is_none() {
                        let size = data.len() as i64;
                        if size > self.limits.max_bundled_bytes {
                            return Err(format!(
                                "d2fonts: bundled fallback font bytes exceed limit {} while retaining Noto Color Emoji",
                                self.limits.max_bundled_bytes
                            ));
                        }
                        if size > self.limits.max_resolved_bytes - *resolved_bytes {
                            return Err(format!(
                                "d2fonts: resolved fallback font bytes exceed limit {} while retaining bundled Noto Color Emoji",
                                self.limits.max_resolved_bytes
                            ));
                        }
                        work.retained = Some(data.clone());
                        *resolved_bytes += size;
                    }
                    result.push(FallbackFont {
                        name: "NotoColorEmoji-COLRv1-v2.051.ttf".into(),
                        mime_type: "font/ttf".into(),
                        data,
                        face_index: 0,
                    });
                    for r in covered {
                        remaining.remove(&r);
                    }
                }
            }
        }
        if !remaining.is_empty() {
            if let Some(next) = &self.next {
                let mut downstream_request = request.clone();
                downstream_request.runes = remaining.into_iter().collect();
                for (index, font) in next
                    .resolve_fallbacks(&downstream_request)?
                    .into_iter()
                    .enumerate()
                {
                    if font.data.len() as i64 > self.limits.max_resolved_bytes - *resolved_bytes {
                        return Err(format!(
                            "d2fonts: resolved fallback font bytes exceed limit {} at downstream resource {index}",
                            self.limits.max_resolved_bytes
                        ));
                    }
                    *resolved_bytes += font.data.len() as i64;
                    result.push(font);
                }
            }
        }
        Ok(result)
    }
}

#[cfg(target_arch = "wasm32")]
fn bundled_data() -> Result<Option<Arc<[u8]>>, String> {
    Ok(None)
}

#[cfg(not(target_arch = "wasm32"))]
fn bundled_data() -> Result<Option<Arc<[u8]>>, String> {
    // d2-rs-fontface embeds the face decoded and authenticated at build time
    // (both pinned digests), registered once for the whole process.
    d2_rs_fontface::bundled_noto_color_emoji()
        .map(Some)
        .map_err(|e| e.to_string())
}

/// fontfallback.AssetID: MIME, NUL, big-endian face index, then exact bytes.
pub(crate) fn asset_id(font: &FallbackFont) -> String {
    use sha2::{Digest, Sha256};
    let mut hash = Sha256::new();
    hash.update(font.mime_type.as_bytes());
    hash.update([0]);
    hash.update(font.face_index.to_be_bytes());
    hash.update(&font.data);
    format!("font:fallback:{:x}", hash.finalize())
}
