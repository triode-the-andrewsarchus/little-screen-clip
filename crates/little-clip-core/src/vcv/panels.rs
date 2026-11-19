//! VCV Rack module panel resolution from local Rack screenshots and VCV Library fallback.
//!
//! Passive previews prefer local Rack screenshots and fall back to previously
//! downloaded VCV Library artwork. Explicit viewer actions download missing panels
//! or capture installed modules using `Rack -t 4` in a temporary user directory.

use super::catalog::VcvCatalog;
use crate::process::{CommandError, CommandRequest};
use base64::Engine as _;
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use serde::Deserialize;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::ffi::OsString;
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{LazyLock, Mutex};
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

const THUMBNAIL_BUDGET: usize = 4 * 1024 * 1024;
const VIEWER_BUDGET: usize = 64 * 1024 * 1024;

enum PanelImage {
    Thumbnail(String),
    Viewer(image::RgbaImage),
}

#[derive(Debug)]
struct PanelAsset {
    aspect: f64,
    index: usize,
    image: PanelImage,
}

/// Local panel artwork owned by one preview.
///
/// Thumbnails and viewer images have separate byte budgets and lifetimes.
#[derive(Debug, Default)]
pub struct PanelAssets {
    entries: HashMap<String, HashMap<String, PanelAsset>>,
}

impl PanelAssets {
    pub(super) fn aspect(&self, plugin: &str, model: &str) -> Option<f64> {
        Some(self.entries.get(plugin)?.get(model)?.aspect)
    }

    pub(super) fn thumbnail(&self, plugin: &str, model: &str) -> Option<(usize, f64)> {
        let asset = self.entries.get(plugin)?.get(model)?;
        matches!(asset.image, PanelImage::Thumbnail(_)).then_some((asset.index, asset.aspect))
    }

    pub(super) fn thumbnails(&self) -> impl Iterator<Item = (usize, &str)> {
        self.entries
            .values()
            .flat_map(HashMap::values)
            .filter_map(|asset| match &asset.image {
                PanelImage::Thumbnail(uri) => Some((asset.index, uri.as_str())),
                PanelImage::Viewer(_) => None,
            })
    }

    /// Returns the bitmap for a module in the interactive viewer.
    pub fn viewer_image(&self, plugin: &str, model: &str) -> Option<&image::RgbaImage> {
        match &self.entries.get(plugin)?.get(model)?.image {
            PanelImage::Thumbnail(_) => None,
            PanelImage::Viewer(image) => Some(image),
        }
    }
}

impl std::fmt::Debug for PanelImage {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Thumbnail(uri) => formatter
                .debug_tuple("Thumbnail")
                .field(&uri.len())
                .finish(),
            Self::Viewer(image) => formatter
                .debug_tuple("Viewer")
                .field(&image.dimensions())
                .finish(),
        }
    }
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

// Rack can fail to capture hidden or unlicensed modules. Keep a bounded
// session record so the same missing plugin does not continually prompt a retry.
static ATTEMPTED_LOCAL_PLUGINS: LazyLock<Mutex<HashSet<String>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));
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

/// Reads local artwork at interactive viewer resolution.
pub fn load_viewer_panels(modules: &[(String, String)], cancel: &AtomicBool) -> PanelAssets {
    load_panels(modules, true, cancel)
}

pub(super) fn load_panel_thumbnails(
    modules: &[(String, String)],
    cancel: &AtomicBool,
) -> PanelAssets {
    load_panels(modules, false, cancel)
}

fn load_panels(modules: &[(String, String)], viewer: bool, cancel: &AtomicBool) -> PanelAssets {
    if modules.is_empty() || cancel.load(Ordering::Relaxed) {
        return PanelAssets::default();
    }
    let env = discover_rack_env();
    let cache_dir = default_library_cache_dir();
    load_panels_with_dirs(
        modules,
        viewer,
        &env.screenshot_dirs,
        cache_dir.as_deref(),
        cancel,
    )
}

pub(super) fn load_panels_with_dirs(
    modules: &[(String, String)],
    viewer: bool,
    screenshot_dirs: &[PathBuf],
    cache_dir: Option<&Path>,
    cancel: &AtomicBool,
) -> PanelAssets {
    collect_panels(modules, viewer, cancel, |plugin, model| {
        read_panel(plugin, model, screenshot_dirs, cache_dir)
    })
}

fn collect_panels(
    modules: &[(String, String)],
    viewer: bool,
    cancel: &AtomicBool,
    mut read: impl FnMut(&str, &str) -> Option<image::DynamicImage>,
) -> PanelAssets {
    let mut assets = PanelAssets::default();
    let mut seen = HashSet::new();
    let mut remaining = if viewer {
        VIEWER_BUDGET
    } else {
        THUMBNAIL_BUDGET
    };
    let bytes_per_pixel = if viewer { 4 } else { 6 };
    let pixel_budget = (remaining / bytes_per_pixel / modules.len().max(1)).max(1);
    for (index, (plugin, model)) in modules.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) || remaining == 0 {
            break;
        }
        if !seen.insert((plugin, model)) {
            continue;
        }
        let Some(image) = read(plugin, model) else {
            continue;
        };
        let aspect = f64::from(image.width()) / f64::from(image.height());
        let image = resize_panel(image, viewer, pixel_budget);
        let panel = if viewer {
            PanelImage::Viewer(image.into_rgba8())
        } else {
            let mut png = Cursor::new(Vec::new());
            if image
                .into_rgba8()
                .write_to(&mut png, image::ImageFormat::Png)
                .is_err()
            {
                continue;
            }
            let base64 = base64::engine::general_purpose::STANDARD.encode(png.into_inner());
            PanelImage::Thumbnail(format!("data:image/png;base64,{base64}"))
        };
        let bytes = match &panel {
            PanelImage::Thumbnail(uri) => uri.capacity(),
            PanelImage::Viewer(image) => image.as_raw().capacity(),
        };
        let Some(budget) = remaining.checked_sub(bytes) else {
            continue;
        };
        remaining = budget;
        assets.entries.entry(plugin.clone()).or_default().insert(
            model.clone(),
            PanelAsset {
                aspect,
                index,
                image: panel,
            },
        );
    }
    assets
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

/// Removes downloaded VCV Library artwork from disk.
pub fn clear_downloaded_panel_cache() -> Result<(), String> {
    let Some(cache_dir) = default_library_cache_dir() else {
        return Ok(());
    };
    match std::fs::remove_dir_all(cache_dir) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        result => result.map_err(|error| format!("Cannot remove VCV panel cache: {error}")),
    }
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
        if let Ok(bytes) = crate::file_source::read_file_bytes(&log_path, 128 * 1024, false, true) {
            let log = String::from_utf8_lossy(&bytes);
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

fn read_panel(
    plugin: &str,
    model: &str,
    screenshot_dirs: &[PathBuf],
    cache_dir: Option<&Path>,
) -> Option<image::DynamicImage> {
    if !is_safe_slug(plugin) || !is_safe_slug(model) {
        return None;
    }
    screenshot_dirs
        .iter()
        .map(PathBuf::as_path)
        .chain(cache_dir)
        .find_map(|dir| {
            ["png", "webp", "jpg"].iter().find_map(|extension| {
                let path = dir.join(plugin).join(format!("{model}.{extension}"));
                let bytes =
                    crate::file_source::read_file_bytes(&path, MAX_PANEL_IMAGE_BYTES, true, true)
                        .ok()?;
                decode_panel(&bytes, None)
            })
        })
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
        ATTEMPTED_LOCAL_PLUGINS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
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
        && let Ok(bytes) =
            crate::file_source::read_file_bytes(&mpath, MAX_MANIFEST_BYTES as usize, true, true)
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
        let _ = create_symlink(&live_licenses, &staging.path().join("licenses"));
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

    // Seed existing Core/Fundamental screenshots as symlinks so Rack skips
    // re-rendering all 51 built-in modules during third-party plugin batches.
    let staging_shots_dir = staging.path().join("screenshots");
    for builtin in ["Core", "Fundamental"] {
        seed_existing_screenshot_links(
            &dest_screenshots.join(builtin),
            &staging_shots_dir.join(builtin),
        );
    }

    for (slug, maybe_dir) in plugin_entries {
        if slug == "Core" {
            continue;
        }
        if let Some(src_dir) = maybe_dir
            && src_dir.is_dir()
        {
            let _ = create_symlink(src_dir, &staging_plugins_dir.join(slug));
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

pub(crate) fn copy_generated_screenshots(staging_shots: &Path, dest_shots: &Path) -> usize {
    let Ok(plugin_entries) = std::fs::read_dir(staging_shots) else {
        return 0;
    };
    let mut copied = 0usize;

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
            // Skip seeded symlinks; only copy freshly generated regular PNG files.
            let Ok(meta) = std::fs::symlink_metadata(&src_file) else {
                continue;
            };
            if !meta.file_type().is_file() {
                continue;
            }
            let Some(file_name) = file_entry.file_name().to_str().map(str::to_string) else {
                continue;
            };
            if !file_name.to_ascii_lowercase().ends_with(".png") {
                continue;
            }
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
            let Ok(mut staged) = tempfile::NamedTempFile::new_in(&dst_plugin_dir) else {
                continue;
            };
            let Ok(source) = crate::file_source::open_regular_file(&src_file, false) else {
                continue;
            };
            let mut source = std::io::Read::take(source, MAX_PANEL_IMAGE_BYTES as u64 + 1);
            if std::io::copy(&mut source, &mut staged)
                .is_ok_and(|size| size > 0 && size <= MAX_PANEL_IMAGE_BYTES as u64)
                && staged.persist_noclobber(&target).is_ok()
            {
                copied += 1;
            }
        }
    }
    copied
}

pub(crate) fn write_staging_settings_json(src: &Path, dst: &Path) -> Result<(), String> {
    let bytes = crate::file_source::read_file_bytes(src, MAX_MANIFEST_BYTES as usize, true, true)
        .map_err(|e| format!("read settings: {e}"))?;
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

pub(crate) fn seed_existing_screenshot_links(src_dir: &Path, dst_dir: &Path) {
    let Ok(entries) = std::fs::read_dir(src_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let src = entry.path();
        if !src.is_file() {
            continue;
        }
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        if !name.to_ascii_lowercase().ends_with(".png") {
            continue;
        }
        if std::fs::create_dir_all(dst_dir).is_err() {
            return;
        }
        let _ = create_symlink(&src, &dst_dir.join(name));
    }
}

#[cfg(unix)]
fn create_symlink(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(src, dst)
}

#[cfg(not(unix))]
fn create_symlink(_src: &Path, _dst: &Path) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "symlinks unavailable",
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
    let mut attempted = ATTEMPTED_LOCAL_PLUGINS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if attempted.len() < 1024 {
        attempted.insert(plugin.to_string());
    }
}

/// Loads viewer artwork and downloads missing panels after an explicit user request.
///
/// The count reports files installed, including partial progress on cancellation.
/// The second result contains a cache error if downloaded artwork could not be saved.
pub fn download_viewer_panels(
    modules: &[(String, String)],
    cancel: &AtomicBool,
) -> (PanelAssets, Result<usize, String>) {
    let cache_dir = default_library_cache_dir();
    let env = discover_rack_env();
    let mut result = cache_dir
        .as_ref()
        .map(|_| 0)
        .ok_or("Panel cache directory is unavailable".into());
    let panels = collect_panels(modules, true, cancel, |plugin, model| {
        read_panel(plugin, model, &env.screenshot_dirs, cache_dir.as_deref()).or_else(|| {
            let directory = cache_dir.as_ref()?.join(plugin);
            let downloaded = result.as_mut().ok()?;
            if !is_safe_slug(plugin)
                || !is_safe_slug(model)
                || !VcvCatalog::embedded().is_listed_in_library(plugin)
            {
                return None;
            }
            let (bytes, image) = download_panel(plugin, model, cancel)?;
            match save_download(&directory, model, &bytes) {
                Ok(()) => *downloaded += 1,
                Err(error) => result = Err(format!(
                    "Could not cache {plugin}/{model} after downloading {downloaded} panels: {error}"
                )),
            }
            Some(image)
        })
    });
    (panels, result)
}

fn save_download(directory: &Path, model: &str, bytes: &[u8]) -> std::io::Result<()> {
    std::fs::create_dir_all(directory)?;
    let mut staged = tempfile::NamedTempFile::new_in(directory)?;
    staged.write_all(bytes)?;
    staged.persist(directory.join(format!("{model}.webp")))?;
    Ok(())
}

fn download_panel(
    plugin: &str,
    model: &str,
    cancel: &AtomicBool,
) -> Option<(Vec<u8>, image::DynamicImage)> {
    let plugin = utf8_percent_encode(plugin, URL_SEGMENT_ENCODE);
    let model = utf8_percent_encode(model, URL_SEGMENT_ENCODE);
    for size in [400, 200] {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let url = format!("https://library.vcvrack.com/screenshots/{size}/{plugin}/{model}.webp");
        let output = CommandRequest::new("curl")
            .args([
                "-q",
                "-fsSL",
                "--proto",
                "=https",
                "--max-time",
                "4",
                "--connect-timeout",
                "2",
            ])
            .arg(url)
            .own_process_group()
            .die_with_parent()
            .timeout(Duration::from_secs(5))
            .max_output_bytes(MAX_PANEL_IMAGE_BYTES)
            .run_cancellable(cancel);
        if let Ok(output) = output
            && output.status.success()
            && let Some(image) = decode_panel(&output.stdout, Some(image::ImageFormat::WebP))
        {
            return Some((output.stdout, image));
        }
    }
    None
}

fn decode_panel(bytes: &[u8], format: Option<image::ImageFormat>) -> Option<image::DynamicImage> {
    let mut reader = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .ok()?;
    let detected = reader.format()?;
    if !matches!(
        detected,
        image::ImageFormat::Png | image::ImageFormat::Jpeg | image::ImageFormat::WebP
    ) || format.is_some_and(|expected| expected != detected)
    {
        return None;
    }
    let mut limits = crate::item::raster_preview_limits();
    limits.max_image_width = Some(8192);
    limits.max_image_height = Some(4096);
    limits.max_alloc = Some(32 * 1024 * 1024);
    reader.limits(limits);
    reader.decode().ok()
}

fn resize_panel(
    image: image::DynamicImage,
    viewer: bool,
    pixel_budget: usize,
) -> image::DynamicImage {
    let (max_w, max_h) = if viewer {
        (PANEL_MAX_VIEWER_WIDTH, PANEL_MAX_VIEWER_HEIGHT)
    } else {
        (640, PANEL_SVG_URI_HEIGHT)
    };
    let (width, height) = (f64::from(image.width()), f64::from(image.height()));
    let scale = (f64::from(max_w) / width)
        .min(f64::from(max_h) / height)
        .min((pixel_budget as f64 / (width * height)).sqrt());
    if scale >= 1.0 {
        return image;
    }
    image.resize_exact(
        (width * scale).floor().clamp(1.0, pixel_budget as f64) as u32,
        (height * scale).floor().clamp(1.0, pixel_budget as f64) as u32,
        image::imageops::FilterType::Lanczos3,
    )
}

fn is_safe_slug(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 256
        && s != "."
        && !s.contains("..")
        && s.bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}
