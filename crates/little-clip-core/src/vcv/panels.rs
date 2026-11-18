//! VCV Library panel screenshot resolution and local disk/memory caching.
//!
//! Fetches rendered module screenshots from the VCV Library CDN
//! (`/screenshots/400/...`, falling back to `/200/...`) into a local cache
//! directory and preserves up to 1520px native vertical resolution for crisp
//! rail viewer zoom.

use super::catalog::VcvCatalog;
use crate::process::CommandRequest;
use base64::Engine as _;
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use std::collections::{HashMap, HashSet};
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

const URL_SEGMENT_ENCODE: &AsciiSet = &NON_ALPHANUMERIC.remove(b'-').remove(b'_').remove(b'.');
const MAX_PANEL_IMAGE_BYTES: usize = 4 * 1024 * 1024;
/// VCV Library `/screenshots/400/` WebPs are lossless 4x screenshots (up to 1520px tall).
const PANEL_MAX_VIEWER_HEIGHT: u32 = 1520;
const PANEL_MAX_VIEWER_WIDTH: u32 = 4096;
/// Bounded PNG data-URI height for SVG strip/hover embedding so multi-row racks stay within budget.
const PANEL_SVG_URI_HEIGHT: u32 = 320;

/// Resolved real panel asset for a `(plugin, model)` tuple.
#[derive(Clone, Debug)]
pub struct ResolvedPanel {
    pub data_uri: String,
    pub aspect: f64,
    pub rgba: Arc<image::RgbaImage>,
}

#[derive(Default)]
struct PanelMemoryCache {
    entries: HashMap<(String, String), Option<ResolvedPanel>>,
    missing_library: HashSet<String>,
}

static PANEL_CACHE: LazyLock<Mutex<PanelMemoryCache>> =
    LazyLock::new(|| Mutex::new(PanelMemoryCache::default()));

/// Resolves a real module panel screenshot from the local VCV Library cache or CDN.
pub fn resolve_panel(plugin: &str, model: &str) -> Option<ResolvedPanel> {
    if !is_safe_slug(plugin) || !is_safe_slug(model) {
        return None;
    }
    let key = (plugin.to_string(), model.to_string());
    {
        let guard = PANEL_CACHE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(cached) = guard.entries.get(&key) {
            return cached.clone();
        }
    }

    let cache_dir = default_library_cache_dir()?;
    let allow_network = !cfg!(test);
    let panel = resolve_panel_in_dir(plugin, model, &cache_dir, allow_network);

    let mut guard = PANEL_CACHE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    guard.entries.insert(key, panel.clone());
    panel
}

pub(crate) fn resolve_panel_in_dir(
    plugin: &str,
    model: &str,
    cache_dir: &Path,
    allow_network: bool,
) -> Option<ResolvedPanel> {
    if !is_safe_slug(plugin) || !is_safe_slug(model) {
        return None;
    }
    let catalog_key = format!("{plugin}/{model}");
    load_or_fetch_library_panel(cache_dir, plugin, model, &catalog_key, allow_network)
        .and_then(|bytes| encode_raster_bytes_as_panel(&bytes))
}

/// Removes the entire downloaded VCV Library screenshot cache directory and clears memory cache.
pub fn clear_downloaded_panel_cache() -> Result<(), String> {
    let Some(cache_dir) = default_library_cache_dir() else {
        return Ok(());
    };
    if cache_dir.exists() {
        std::fs::remove_dir_all(&cache_dir)
            .map_err(|e| format!("Cannot remove VCV panel cache: {e}"))?;
    }
    let mut guard = PANEL_CACHE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    guard.entries.clear();
    guard.missing_library.clear();
    Ok(())
}

pub(crate) fn default_library_cache_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".local/share/little-screen-clip/vcv_panels"))
}

fn load_or_fetch_library_panel(
    cache_dir: &Path,
    plugin: &str,
    model: &str,
    catalog_key: &str,
    allow_network: bool,
) -> Option<Vec<u8>> {
    let plugin_dir = cache_dir.join(plugin);
    let mut cached_low_res: Option<Vec<u8>> = None;
    for ext in ["webp", "png", "jpg"] {
        let p = plugin_dir.join(format!("{model}.{ext}"));
        if p.is_file()
            && let Ok(meta) = std::fs::metadata(&p)
            && meta.len() <= MAX_PANEL_IMAGE_BYTES as u64
            && let Ok(bytes) = std::fs::read(&p)
        {
            if let Ok(reader) = image::ImageReader::new(Cursor::new(&bytes)).with_guessed_format()
                && let Ok((_, h)) = reader.into_dimensions()
                && h >= 280
            {
                return Some(bytes);
            }
            cached_low_res = Some(bytes);
            break;
        }
    }

    if !allow_network || !VcvCatalog::embedded().is_listed_in_library(plugin) {
        return cached_low_res;
    }
    {
        let guard = PANEL_CACHE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if guard.missing_library.contains(catalog_key) {
            return cached_low_res;
        }
    }

    let enc_plugin = utf8_percent_encode(plugin, URL_SEGMENT_ENCODE);
    let enc_model = utf8_percent_encode(model, URL_SEGMENT_ENCODE);
    for size in [400, 200] {
        let url =
            format!("https://library.vcvrack.com/screenshots/{size}/{enc_plugin}/{enc_model}.webp");
        let output = CommandRequest::new("curl")
            .arg("-fsSL")
            .arg("--max-time")
            .arg("4")
            .arg("--connect-timeout")
            .arg("2")
            .arg(&url)
            .timeout(Duration::from_secs(5))
            .max_output_bytes(MAX_PANEL_IMAGE_BYTES)
            .run();
        let Ok(output) = output else {
            continue;
        };
        if !output.status.success() || output.stdout.is_empty() {
            continue;
        }
        if image::load_from_memory(&output.stdout).is_err() {
            continue;
        }
        if std::fs::create_dir_all(&plugin_dir).is_ok() {
            let target = plugin_dir.join(format!("{model}.webp"));
            let tmp = plugin_dir.join(format!("{model}.webp.tmp"));
            if std::fs::write(&tmp, &output.stdout).is_ok() {
                let _ = std::fs::rename(&tmp, &target);
            }
        }
        return Some(output.stdout);
    }

    if cached_low_res.is_none() {
        mark_missing_library(catalog_key);
    }
    cached_low_res
}

fn mark_missing_library(catalog_key: &str) {
    let mut guard = PANEL_CACHE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    guard.missing_library.insert(catalog_key.to_string());
}

pub(crate) fn encode_raster_bytes_as_panel(bytes: &[u8]) -> Option<ResolvedPanel> {
    let img = image::load_from_memory(bytes).ok()?;
    let (w, h) = (img.width(), img.height());
    if w == 0 || h == 0 {
        return None;
    }
    let aspect = f64::from(w) / f64::from(h);

    let full_rgba = if h > PANEL_MAX_VIEWER_HEIGHT || w > PANEL_MAX_VIEWER_WIDTH {
        let scale = (f64::from(PANEL_MAX_VIEWER_HEIGHT) / f64::from(h))
            .min(f64::from(PANEL_MAX_VIEWER_WIDTH) / f64::from(w));
        let target_w = (f64::from(w) * scale)
            .round()
            .clamp(8.0, f64::from(PANEL_MAX_VIEWER_WIDTH)) as u32;
        let target_h = (f64::from(h) * scale)
            .round()
            .clamp(16.0, f64::from(PANEL_MAX_VIEWER_HEIGHT)) as u32;
        img.resize_exact(target_w, target_h, image::imageops::FilterType::Lanczos3)
            .to_rgba8()
    } else {
        img.to_rgba8()
    };
    let rgba = Arc::new(full_rgba);

    let uri_img = if h > PANEL_SVG_URI_HEIGHT {
        let target_w = ((aspect * f64::from(PANEL_SVG_URI_HEIGHT)).round() as u32).clamp(8, 1024);
        img.resize_exact(
            target_w,
            PANEL_SVG_URI_HEIGHT,
            image::imageops::FilterType::Lanczos3,
        )
    } else {
        img
    };
    let mut buf = Cursor::new(Vec::new());
    uri_img.write_to(&mut buf, image::ImageFormat::Png).ok()?;
    let b64 = base64::engine::general_purpose::STANDARD.encode(buf.into_inner());
    Some(ResolvedPanel {
        data_uri: format!("data:image/png;base64,{b64}"),
        aspect,
        rgba,
    })
}

fn is_safe_slug(s: &str) -> bool {
    !s.is_empty() && !s.contains('/') && !s.contains('\\') && !s.contains("..") && !s.contains('\0')
}
