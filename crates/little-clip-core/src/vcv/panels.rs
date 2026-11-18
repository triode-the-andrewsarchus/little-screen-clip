//! VCV Rack module panel resolution from local Rack screenshots and VCV Library fallback.
//!
//! Priority order per `(plugin, model)`:
//! 1. Local Rack screenshots (`<Rack user folder>/screenshots/<plugin>/<model>.png`),
//!    preserving up to 1520px height (`Rack -t 4`).
//! 2. Local Rack screenshot generation via `Rack -s <sys> -u <staging> -t 4` in an
//!    isolated temporary user directory (when requested for missing installed plugins).
//! 3. Downloaded VCV Library CDN screenshots (`/screenshots/400/...`, fallback `/200/...`)
//!    cached under `~/.local/share/little-screen-clip/vcv_panels`.

use super::catalog::VcvCatalog;
use crate::process::{CommandError, CommandRequest};
use base64::Engine as _;
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use serde::Deserialize;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::ffi::OsString;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

const URL_SEGMENT_ENCODE: &AsciiSet = &NON_ALPHANUMERIC.remove(b'-').remove(b'_').remove(b'.');
/// Bounds encoded PNG/WebP screenshot files; 4x RGBA PNGs from `Rack -t 4` reach ~5 MiB.
const MAX_PANEL_IMAGE_BYTES: usize = 16 * 1024 * 1024;
const MAX_MANIFEST_BYTES: u64 = 2 * 1024 * 1024;
const PANEL_MAX_VIEWER_HEIGHT: u32 = 1520;
const PANEL_MAX_VIEWER_WIDTH: u32 = 4096;
const PANEL_SVG_URI_HEIGHT: u32 = 320;
const RACK_SCREENSHOT_ZOOM: &str = "4";
const LOCAL_SCREENSHOT_BATCH_SIZE: usize = 12;

/// Resolved real panel asset for a `(plugin, model)` tuple.
#[derive(Clone, Debug)]
pub struct ResolvedPanel {
    pub data_uri: String,
    pub aspect: f64,
    pub rgba: Arc<image::RgbaImage>,
}

/// Summary of a local `Rack -t` screenshot generation run.
#[derive(Clone, Debug, Default)]
pub struct ScreenshotBatchResult {
    pub rendered_modules: usize,
    pub rendered_plugins: usize,
    pub skipped_plugins: usize,
    pub failed_plugins: Vec<String>,
    pub cancelled: bool,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct RackEnv {
    pub primary_user_dir: Option<PathBuf>,
    pub screenshot_dirs: Vec<PathBuf>,
    pub plugin_dirs: Vec<PathBuf>,
    pub sys_dir: Option<PathBuf>,
    pub exe_path: Option<PathBuf>,
}

impl RackEnv {
    pub fn can_run_screenshots(&self) -> bool {
        self.exe_path.is_some()
            && self.sys_dir.is_some()
            && self.primary_user_dir.is_some()
            && !self.screenshot_dirs.is_empty()
    }
}

#[derive(Default)]
struct PanelMemoryCache {
    entries: HashMap<(String, String), Option<ResolvedPanel>>,
    missing_library: HashSet<String>,
    attempted_local_plugins: HashSet<String>,
}

static PANEL_CACHE: LazyLock<Mutex<PanelMemoryCache>> =
    LazyLock::new(|| Mutex::new(PanelMemoryCache::default()));
static RACK_SCREENSHOT_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

/// Returns `true` if a local VCV Rack installation capable of `-t` screenshotting is present.
pub fn local_rack_available() -> bool {
    discover_rack_env().can_run_screenshots()
}

/// Counts installed local plugins in `modules` that still lack local Rack screenshots.
pub fn count_missing_local_screenshot_plugins(modules: &[(String, String)]) -> usize {
    let env = discover_rack_env();
    if !env.can_run_screenshots() {
        return 0;
    }
    collect_missing_plugins_for_modules(&env, modules, true).len()
}

/// Resolves a module panel: local Rack screenshots first, then VCV Library cache/CDN.
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

    let env = discover_rack_env();
    let cache_dir = default_library_cache_dir()?;
    let allow_network = !cfg!(test);
    let panel = resolve_panel_with_dirs(
        plugin,
        model,
        &env.screenshot_dirs,
        &cache_dir,
        allow_network,
    );

    let mut guard = PANEL_CACHE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    guard.entries.insert(key, panel.clone());
    panel
}

pub(crate) fn resolve_panel_with_dirs(
    plugin: &str,
    model: &str,
    screenshot_dirs: &[PathBuf],
    cache_dir: &Path,
    allow_network: bool,
) -> Option<ResolvedPanel> {
    if !is_safe_slug(plugin) || !is_safe_slug(model) {
        return None;
    }

    if let Some(bytes) = find_local_screenshot_bytes(plugin, model, screenshot_dirs)
        && let Some(panel) = encode_raster_bytes_as_panel(&bytes)
    {
        return Some(panel);
    }

    let catalog_key = format!("{plugin}/{model}");
    load_or_fetch_library_panel(cache_dir, plugin, model, &catalog_key, allow_network)
        .and_then(|bytes| encode_raster_bytes_as_panel(&bytes))
}

/// Generates local `Rack -t 4` screenshots for installed plugins used in `modules`
/// that are missing from `<Rack user folder>/screenshots/<plugin>/<model>.png`.
pub fn ensure_local_screenshots_for_patch(
    modules: &[(String, String)],
    cancel: &AtomicBool,
) -> ScreenshotBatchResult {
    let env = discover_rack_env();
    if !env.can_run_screenshots() {
        return ScreenshotBatchResult::default();
    }
    let missing = collect_missing_plugins_for_modules(&env, modules, true);
    if missing.is_empty() {
        return ScreenshotBatchResult::default();
    }
    let result = run_resilient_batches(&env, &missing, cancel, &mut |_, _, _| {});
    // Record any plugin whose requested module still has no screenshot (e.g. hidden/unlicensed)
    // so we don't repeatedly re-launch Rack for the same session.
    if !result.cancelled {
        for (plugin, model) in modules {
            if missing.iter().any(|(p, _)| p == plugin)
                && !has_local_screenshot(plugin, model, &env.screenshot_dirs)
            {
                mark_attempted_local_plugin(plugin);
            }
        }
    }
    result
}

/// Captures local `Rack -t 4` screenshots for all installed plugins missing any module PNG.
pub fn generate_all_local_rack_screenshots(
    cancel: &AtomicBool,
    mut on_progress: impl FnMut(usize, usize, usize),
) -> ScreenshotBatchResult {
    let env = discover_rack_env();
    if !env.can_run_screenshots() {
        return ScreenshotBatchResult::default();
    }

    let mut candidates: BTreeMap<String, Option<PathBuf>> = BTreeMap::new();
    let mut skipped_plugins = 0usize;

    let check_entry = |plugin: &str,
                       dir: Option<&Path>,
                       candidates: &mut BTreeMap<String, Option<PathBuf>>,
                       skipped: &mut usize| {
        if plugin_needs_screenshots(plugin, dir, env.sys_dir.as_deref(), &env.screenshot_dirs) {
            candidates
                .entry(plugin.to_string())
                .or_insert_with(|| dir.map(Path::to_path_buf));
        } else {
            *skipped += 1;
        }
    };

    check_entry("Core", None, &mut candidates, &mut skipped_plugins);
    let installed_fund = find_installed_plugin_dir("Fundamental", &env.plugin_dirs);
    check_entry(
        "Fundamental",
        installed_fund.as_deref(),
        &mut candidates,
        &mut skipped_plugins,
    );

    for plugins_root in &env.plugin_dirs {
        let Ok(entries) = std::fs::read_dir(plugins_root) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let Some(slug) = entry.file_name().to_str().map(str::to_string) else {
                continue;
            };
            if !is_safe_slug(&slug) || slug == "Fundamental" {
                continue;
            }
            check_entry(&slug, Some(&path), &mut candidates, &mut skipped_plugins);
        }
    }

    let targets: Vec<(String, Option<PathBuf>)> = candidates.into_iter().collect();
    if targets.is_empty() {
        return ScreenshotBatchResult {
            skipped_plugins,
            ..Default::default()
        };
    }

    let mut out = run_resilient_batches(&env, &targets, cancel, &mut on_progress);
    out.skipped_plugins = skipped_plugins;
    out
}

/// Removes the downloaded VCV Library fallback screenshot cache and clears memory cache.
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
    guard.attempted_local_plugins.clear();
    Ok(())
}

pub(crate) fn default_library_cache_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".local/share/little-screen-clip/vcv_panels"))
}

pub(crate) fn discover_rack_env() -> RackEnv {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let xdg_data_home = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from);
    let local_app_data = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
    discover_rack_env_in(
        home.as_deref(),
        xdg_data_home.as_deref(),
        local_app_data.as_deref(),
    )
}

pub(crate) fn discover_rack_env_in(
    home: Option<&Path>,
    xdg_data_home: Option<&Path>,
    local_app_data: Option<&Path>,
) -> RackEnv {
    let mut env = RackEnv::default();
    let mut candidate_user_dirs = Vec::new();
    let mut candidate_sys_dirs = Vec::new();

    if let Some(xdg) = xdg_data_home {
        candidate_user_dirs.push(xdg.join("Rack2"));
    }
    if let Some(local) = local_app_data {
        candidate_user_dirs.push(local.join("Rack2"));
    }
    if let Some(h) = home {
        candidate_user_dirs.push(h.join(".local/share/Rack2"));
        candidate_user_dirs.push(h.join(".Rack2"));
        candidate_user_dirs.push(h.join("Library/Application Support/Rack2"));
        candidate_user_dirs.push(h.join("Documents/Rack2"));
    }

    let mut ordered_user_dirs = Vec::new();
    for udir in &candidate_user_dirs {
        let log_path = udir.join("log.txt");
        if let Ok(log) = std::fs::read_to_string(&log_path) {
            for line in log.lines().take(60) {
                if let Some(idx) = line.find("User directory: ") {
                    let parsed = PathBuf::from(line[idx + "User directory: ".len()..].trim());
                    if parsed.is_dir() {
                        push_unique_dir(&mut ordered_user_dirs, parsed);
                    }
                } else if let Some(idx) = line.find("System directory: ") {
                    let parsed = PathBuf::from(line[idx + "System directory: ".len()..].trim());
                    if parsed.is_dir() {
                        push_unique_dir(&mut candidate_sys_dirs, parsed);
                    }
                }
            }
        }
    }

    for udir in candidate_user_dirs {
        if udir.is_dir() {
            push_unique_dir(&mut ordered_user_dirs, udir);
        }
    }

    env.primary_user_dir = ordered_user_dirs.first().cloned();
    for udir in ordered_user_dirs {
        let shots = udir.join("screenshots");
        if shots.is_dir() || env.screenshot_dirs.is_empty() {
            push_unique_dir(&mut env.screenshot_dirs, shots);
        }
        if let Ok(entries) = std::fs::read_dir(&udir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir()
                    && entry
                        .file_name()
                        .to_str()
                        .is_some_and(|name| name.starts_with("plugins"))
                {
                    push_unique_dir(&mut env.plugin_dirs, path);
                }
            }
        }
    }

    if let Some(h) = home {
        candidate_sys_dirs.push(h.join("Downloads/Rack2Free"));
        candidate_sys_dirs.push(h.join("Downloads/Rack2"));
        candidate_sys_dirs.push(h.join("Applications/VCV Rack 2 Free.app/Contents/Resources"));
        candidate_sys_dirs.push(h.join("Applications/VCV Rack 2 Pro.app/Contents/Resources"));
    }
    for sys in [
        PathBuf::from("/opt/Rack2Free"),
        PathBuf::from("/opt/Rack2"),
        PathBuf::from("/usr/local/share/Rack2"),
        PathBuf::from("/Applications/VCV Rack 2 Free.app/Contents/Resources"),
        PathBuf::from("/Applications/VCV Rack 2 Pro.app/Contents/Resources"),
        PathBuf::from(r"C:\Program Files\VCV\Rack2Free"),
        PathBuf::from(r"C:\Program Files\VCV\Rack2Pro"),
    ] {
        candidate_sys_dirs.push(sys);
    }

    for sys in candidate_sys_dirs {
        if !sys.is_dir() {
            continue;
        }
        if let Some(exe) = rack_executable_in_sys_dir(&sys) {
            env.sys_dir = Some(sys);
            env.exe_path = Some(exe);
            break;
        }
    }

    env
}

fn rack_executable_in_sys_dir(sys_dir: &Path) -> Option<PathBuf> {
    let candidates = if cfg!(target_os = "macos") {
        vec![
            sys_dir.join("../MacOS/Rack"),
            sys_dir.join("Rack"),
            sys_dir.join("Rack.exe"),
        ]
    } else if cfg!(target_os = "windows") {
        vec![sys_dir.join("Rack.exe"), sys_dir.join("Rack")]
    } else {
        vec![sys_dir.join("Rack"), sys_dir.join("../MacOS/Rack")]
    };
    candidates.into_iter().find(|p| p.is_file())
}

fn push_unique_dir(list: &mut Vec<PathBuf>, dir: PathBuf) {
    if !list.iter().any(|existing| existing == &dir) {
        list.push(dir);
    }
}

fn find_local_screenshot_bytes(
    plugin: &str,
    model: &str,
    screenshot_dirs: &[PathBuf],
) -> Option<Vec<u8>> {
    for dir in screenshot_dirs {
        let plugin_dir = dir.join(plugin);
        if !plugin_dir.is_dir() {
            continue;
        }
        for ext in ["png", "webp", "jpg"] {
            let path = plugin_dir.join(format!("{model}.{ext}"));
            if path.is_file()
                && let Ok(meta) = std::fs::metadata(&path)
                && meta.len() > 0
                && meta.len() <= MAX_PANEL_IMAGE_BYTES as u64
                && let Ok(bytes) = std::fs::read(&path)
            {
                return Some(bytes);
            }
        }
    }
    None
}

fn has_local_screenshot(plugin: &str, model: &str, screenshot_dirs: &[PathBuf]) -> bool {
    screenshot_dirs.iter().any(|dir| {
        let plugin_dir = dir.join(plugin);
        ["png", "webp", "jpg"]
            .iter()
            .any(|ext| plugin_dir.join(format!("{model}.{ext}")).is_file())
    })
}

fn find_installed_plugin_dir(plugin: &str, plugin_dirs: &[PathBuf]) -> Option<PathBuf> {
    plugin_dirs
        .iter()
        .map(|root| root.join(plugin))
        .find(|p| p.is_dir())
}

fn collect_missing_plugins_for_modules(
    env: &RackEnv,
    modules: &[(String, String)],
    respect_attempted: bool,
) -> Vec<(String, Option<PathBuf>)> {
    let attempted = if respect_attempted {
        PANEL_CACHE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .attempted_local_plugins
            .clone()
    } else {
        HashSet::new()
    };

    let mut needed: BTreeMap<String, Option<PathBuf>> = BTreeMap::new();
    for (plugin, model) in modules {
        if !is_safe_slug(plugin) || !is_safe_slug(model) {
            continue;
        }
        if needed.contains_key(plugin) || attempted.contains(plugin) {
            continue;
        }
        if has_local_screenshot(plugin, model, &env.screenshot_dirs) {
            continue;
        }
        if matches!(plugin.as_str(), "Core" | "Fundamental") {
            let dir = find_installed_plugin_dir(plugin, &env.plugin_dirs);
            needed.insert(plugin.clone(), dir);
        } else if let Some(dir) = find_installed_plugin_dir(plugin, &env.plugin_dirs) {
            needed.insert(plugin.clone(), Some(dir));
        }
    }
    needed.into_iter().collect()
}

#[derive(Deserialize)]
struct ManifestLite {
    #[serde(default)]
    modules: Vec<ManifestModuleLite>,
}

#[derive(Deserialize)]
struct ManifestModuleLite {
    slug: String,
    #[serde(default)]
    hidden: bool,
    #[serde(default)]
    disabled: bool,
}

pub(crate) fn plugin_needs_screenshots(
    plugin: &str,
    plugin_dir: Option<&Path>,
    sys_dir: Option<&Path>,
    screenshot_dirs: &[PathBuf],
) -> bool {
    let manifest_path = if plugin == "Core" {
        sys_dir.map(|d| d.join("Core.json"))
    } else {
        plugin_dir.map(|d| d.join("plugin.json"))
    };

    if let Some(mpath) = manifest_path
        && let Ok(meta) = std::fs::metadata(&mpath)
        && meta.len() > 0
        && meta.len() <= MAX_MANIFEST_BYTES
        && let Ok(bytes) = std::fs::read(&mpath)
        && let Ok(manifest) = serde_json::from_slice::<ManifestLite>(&bytes)
        && !manifest.modules.is_empty()
    {
        let visible: Vec<_> = manifest
            .modules
            .iter()
            .filter(|m| !m.hidden && !m.disabled && is_safe_slug(&m.slug))
            .collect();
        if !visible.is_empty() {
            return visible
                .iter()
                .any(|m| !has_local_screenshot(plugin, &m.slug, screenshot_dirs));
        }
    }

    !screenshot_dirs.iter().any(|dir| {
        let pdir = dir.join(plugin);
        pdir.is_dir()
            && std::fs::read_dir(&pdir).ok().is_some_and(|mut it| {
                it.any(|entry| {
                    entry.ok().is_some_and(|e| {
                        e.path()
                            .extension()
                            .and_then(|s| s.to_str())
                            .is_some_and(|ext| {
                                matches!(ext.to_ascii_lowercase().as_str(), "png" | "webp" | "jpg")
                            })
                    })
                })
            })
    })
}

fn run_resilient_batches(
    env: &RackEnv,
    targets: &[(String, Option<PathBuf>)],
    cancel: &AtomicBool,
    on_progress: &mut impl FnMut(usize, usize, usize),
) -> ScreenshotBatchResult {
    let mut summary = ScreenshotBatchResult::default();
    let total = targets.len();
    let mut done = 0usize;
    on_progress(0, total, 0);

    for chunk in targets.chunks(LOCAL_SCREENSHOT_BATCH_SIZE) {
        if cancel.load(Ordering::Relaxed) {
            summary.cancelled = true;
            break;
        }
        let outcome = run_rack_screenshot_batch(env, chunk, cancel);
        summary.rendered_modules += outcome.copied_modules;
        if outcome.cancelled {
            summary.cancelled = true;
            break;
        }
        if outcome.ok {
            summary.rendered_plugins += chunk.len();
            done += chunk.len();
            on_progress(done, total, summary.rendered_modules);
            continue;
        }

        if chunk.len() == 1 {
            let slug = &chunk[0].0;
            mark_attempted_local_plugin(slug);
            summary.failed_plugins.push(slug.clone());
            done += 1;
            on_progress(done, total, summary.rendered_modules);
            continue;
        }

        // Isolate broken/crashing plugins by retrying each target one by one.
        for item in chunk {
            if cancel.load(Ordering::Relaxed) {
                summary.cancelled = true;
                break;
            }
            let single = run_rack_screenshot_batch(env, std::slice::from_ref(item), cancel);
            summary.rendered_modules += single.copied_modules;
            if single.cancelled {
                summary.cancelled = true;
                break;
            }
            if single.ok {
                summary.rendered_plugins += 1;
            } else {
                mark_attempted_local_plugin(&item.0);
                summary.failed_plugins.push(item.0.clone());
            }
            done += 1;
            on_progress(done, total, summary.rendered_modules);
        }
        if summary.cancelled {
            break;
        }
    }

    summary
}

struct SingleBatchRun {
    ok: bool,
    cancelled: bool,
    copied_modules: usize,
}

fn run_rack_screenshot_batch(
    env: &RackEnv,
    plugin_entries: &[(String, Option<PathBuf>)],
    cancel: &AtomicBool,
) -> SingleBatchRun {
    let (Some(exe), Some(sys_dir), Some(user_dir), Some(dest_screenshots)) = (
        env.exe_path.as_ref(),
        env.sys_dir.as_ref(),
        env.primary_user_dir.as_ref(),
        env.screenshot_dirs.first(),
    ) else {
        return SingleBatchRun {
            ok: false,
            cancelled: false,
            copied_modules: 0,
        };
    };

    let _lock = RACK_SCREENSHOT_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if cancel.load(Ordering::Relaxed) {
        return SingleBatchRun {
            ok: false,
            cancelled: true,
            copied_modules: 0,
        };
    }

    let Ok(staging) = tempfile::tempdir() else {
        return SingleBatchRun {
            ok: false,
            cancelled: false,
            copied_modules: 0,
        };
    };

    let staging_settings = staging.path().join("settings.json");
    let live_settings = user_dir.join("settings.json");
    if live_settings.is_file() {
        let _ = write_staging_settings_json(&live_settings, &staging_settings);
    }

    let live_licenses = user_dir.join("licenses");
    if live_licenses.is_dir() {
        let _ = create_dir_link(&live_licenses, &staging.path().join("licenses"));
    }

    let plugins_subdir_name = env
        .plugin_dirs
        .first()
        .and_then(|p| p.file_name())
        .and_then(|s| s.to_str())
        .unwrap_or(default_plugins_subdir_name());
    let staging_plugins_dir = staging.path().join(plugins_subdir_name);
    if std::fs::create_dir_all(&staging_plugins_dir).is_err() {
        return SingleBatchRun {
            ok: false,
            cancelled: false,
            copied_modules: 0,
        };
    }

    for (slug, maybe_dir) in plugin_entries {
        if slug == "Core" {
            continue;
        }
        if let Some(src_dir) = maybe_dir
            && src_dir.is_dir()
        {
            let _ = create_dir_link(src_dir, &staging_plugins_dir.join(slug));
        }
    }

    let mut request = CommandRequest::new(exe)
        .current_dir(sys_dir)
        .args(["-s"])
        .arg(sys_dir)
        .args(["-u"])
        .arg(staging.path())
        .args(["-t", RACK_SCREENSHOT_ZOOM])
        .own_process_group()
        .die_with_parent()
        .discard_output()
        .timeout(Duration::from_secs(60));

    if cfg!(target_os = "linux") {
        let mut ld_path = OsString::from(sys_dir.as_os_str());
        if let Some(existing) = std::env::var_os("LD_LIBRARY_PATH")
            && !existing.is_empty()
        {
            ld_path.push(":");
            ld_path.push(existing);
        }
        request = request.env("LD_LIBRARY_PATH", ld_path);
    }

    let run_res = request.run_cancellable(cancel);
    let copied_modules =
        copy_generated_screenshots(&staging.path().join("screenshots"), dest_screenshots);

    match run_res {
        Ok(out) => SingleBatchRun {
            ok: out.status.success(),
            cancelled: false,
            copied_modules,
        },
        Err(CommandError::Cancelled) => SingleBatchRun {
            ok: false,
            cancelled: true,
            copied_modules,
        },
        Err(_) => SingleBatchRun {
            ok: false,
            cancelled: false,
            copied_modules,
        },
    }
}

fn copy_generated_screenshots(staging_shots: &Path, dest_shots: &Path) -> usize {
    let Ok(plugin_entries) = std::fs::read_dir(staging_shots) else {
        return 0;
    };
    let mut copied = 0usize;
    let mut invalidated = Vec::new();

    for plugin_entry in plugin_entries.flatten() {
        let src_plugin_dir = plugin_entry.path();
        if !src_plugin_dir.is_dir() {
            continue;
        }
        let Some(plugin_slug) = plugin_entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        if !is_safe_slug(&plugin_slug) {
            continue;
        }
        let Ok(model_entries) = std::fs::read_dir(&src_plugin_dir) else {
            continue;
        };
        let dst_plugin_dir = dest_shots.join(&plugin_slug);
        for file_entry in model_entries.flatten() {
            let src_file = file_entry.path();
            if !src_file.is_file() {
                continue;
            }
            let Some(file_name) = file_entry.file_name().to_str().map(str::to_string) else {
                continue;
            };
            if !file_name.to_ascii_lowercase().ends_with(".png") {
                continue;
            }
            let Ok(meta) = std::fs::metadata(&src_file) else {
                continue;
            };
            if meta.len() == 0 || meta.len() > MAX_PANEL_IMAGE_BYTES as u64 {
                continue;
            }
            let target = dst_plugin_dir.join(&file_name);
            if target.exists() && target.metadata().is_ok_and(|m| m.len() > 0) {
                continue;
            }
            if std::fs::create_dir_all(&dst_plugin_dir).is_err() {
                continue;
            }
            let tmp_target = dst_plugin_dir.join(format!("{file_name}.tmp"));
            if std::fs::copy(&src_file, &tmp_target).is_ok()
                && std::fs::rename(&tmp_target, &target).is_ok()
            {
                if let Some(model_slug) = target.file_stem().and_then(|s| s.to_str()) {
                    invalidated.push((plugin_slug.clone(), model_slug.to_string()));
                }
                copied += 1;
            } else {
                let _ = std::fs::remove_file(&tmp_target);
            }
        }
    }

    if !invalidated.is_empty() {
        let mut guard = PANEL_CACHE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for key in invalidated {
            guard.entries.remove(&key);
        }
    }
    copied
}

pub(crate) fn write_staging_settings_json(src: &Path, dst: &Path) -> Result<(), String> {
    let bytes = std::fs::read(src).map_err(|e| format!("read settings: {e}"))?;
    let mut doc: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|e| format!("parse settings: {e}"))?;
    if let Some(obj) = doc.as_object_mut() {
        obj.insert(
            "windowMaximized".to_string(),
            serde_json::Value::Bool(false),
        );
        obj.insert("windowSize".to_string(), serde_json::json!([128.0, 128.0]));
    }
    let out = serde_json::to_vec(&doc).map_err(|e| format!("serialize settings: {e}"))?;
    std::fs::write(dst, out).map_err(|e| format!("write staging settings: {e}"))
}

#[cfg(unix)]
fn create_dir_link(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(src, dst)
}

#[cfg(not(unix))]
fn create_dir_link(_src: &Path, _dst: &Path) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "directory symlinks unavailable",
    ))
}

fn default_plugins_subdir_name() -> &'static str {
    if cfg!(target_os = "macos") {
        if cfg!(target_arch = "aarch64") {
            "plugins-mac-arm64"
        } else {
            "plugins-mac-x64"
        }
    } else if cfg!(target_os = "windows") {
        "plugins-win-x64"
    } else if cfg!(target_arch = "aarch64") {
        "plugins-lin-arm64"
    } else {
        "plugins-lin-x64"
    }
}

fn mark_attempted_local_plugin(plugin: &str) {
    let mut guard = PANEL_CACHE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    guard.attempted_local_plugins.insert(plugin.to_string());
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
