//! Background downloader and extractor for offline 3D globe / 2D map basemap assets.

use little_clip_core::{
    BackgroundJobs, CommandRequest,
    config::AppConfig,
    geography::basemap::{AssetStatus, BasemapAssets, storage::AssetDirectory},
    jobs::task::Task,
};
use std::{
    fs,
    io::{Read, Write},
    os::fd::AsRawFd,
    path::{Path, PathBuf},
    sync::{
        Arc, LazyLock, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

pub(super) fn asset_workers() -> &'static BackgroundJobs {
    static WORKERS: LazyLock<BackgroundJobs> = LazyLock::new(|| BackgroundJobs::new(1, 2));
    &WORKERS
}

const GEBCO_FALLBACK_MIRRORS: &[&str] = &[
    "https://data.source.coop/ausantarctic/gebco/GEBCO_2024.tif",
    "https://dap.ceda.ac.uk/bodc/gebco/global/gebco_2024/sub_ice_topography_bathymetry/geotiff/gebco_2024_sub_ice_topo_geotiff.zip",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DownloadPack {
    QuickStart,
    Borders,
    RoadsAndLakes,
    GrwlSummary,
    GrwlMasks,
    WaterOutlines,
    TerrainGebco,
    LandCover,
    HydroRivers,
    All,
}

impl DownloadPack {
    pub fn label(self) -> &'static str {
        match self {
            Self::QuickStart => "Quick Start (Natural Earth II + Cities ≥ 250k)",
            Self::Borders => "Country & State Borders (Natural Earth 10m)",
            Self::RoadsAndLakes => "Major Roads & Lakes (Natural Earth 10m)",
            Self::GrwlSummary => "GRWL River Widths (43 MB centerlines)",
            Self::GrwlMasks => "GRWL 30m Water Masks (394 MB)",
            Self::WaterOutlines => "Water Outlines (GSHHG coastlines & lakes)",
            Self::TerrainGebco => "GEBCO 2025 Terrain (15\" global DEM)",
            Self::LandCover => "Copernicus 300m Land Cover",
            Self::HydroRivers => "HydroRIVERS Global Network",
            Self::All => "All Offline Basemap Assets (< 20 GB)",
        }
    }
}

#[derive(Clone, Debug)]
pub struct DownloadProgress {
    pub fraction: f32,
    pub message: String,
}

const PACKS: [DownloadPack; 9] = [
    DownloadPack::QuickStart,
    DownloadPack::Borders,
    DownloadPack::RoadsAndLakes,
    DownloadPack::GrwlSummary,
    DownloadPack::GrwlMasks,
    DownloadPack::WaterOutlines,
    DownloadPack::TerrainGebco,
    DownloadPack::LandCover,
    DownloadPack::HydroRivers,
];
struct AssetFile<'a> {
    folder: &'static str,
    name: &'static str,
    url: &'a str,
    suffixes: &'static [&'static str],
}
impl DownloadPack {
    fn installed(self, status: &AssetStatus) -> bool {
        match self {
            Self::QuickStart => status.has_quick_start(),
            Self::Borders => status.has_borders,
            Self::RoadsAndLakes => status.has_roads_lakes,
            Self::GrwlSummary => status.has_grwl,
            Self::GrwlMasks => status.has_grwl_masks,
            Self::WaterOutlines => status.has_gshhg,
            Self::TerrainGebco => status.has_full_gebco,
            Self::LandCover => status.has_land_cover,
            Self::HydroRivers => status.has_hydrorivers,
            Self::All => PACKS.into_iter().all(|p| p.installed(status)),
        }
    }
    fn files(self, c: &AppConfig) -> Vec<AssetFile<'_>> {
        match self {
            Self::QuickStart => vec![
                AssetFile {
                    folder: "",
                    name: "ne_10m_populated_places_simple.geojson",
                    url: c.geo_cities_url(),
                    suffixes: &[],
                },
                AssetFile {
                    folder: "",
                    name: "ne2_texture.zip",
                    url: c.geo_ne2_texture_url(),
                    suffixes: &[".tif", ".tiff", ".png", ".jpg", ".jpeg"],
                },
            ],
            Self::Borders => vec![
                AssetFile {
                    folder: "",
                    name: "ne_10m_admin_0_boundary_lines_land.geojson",
                    url: c.geo_countries_url(),
                    suffixes: &[],
                },
                AssetFile {
                    folder: "",
                    name: "ne_10m_admin_1_states_provinces_lines.geojson",
                    url: c.geo_states_url(),
                    suffixes: &[],
                },
            ],
            Self::RoadsAndLakes => vec![
                AssetFile {
                    folder: "",
                    name: "ne_10m_lakes.geojson",
                    url: c.geo_lakes_url(),
                    suffixes: &[],
                },
                AssetFile {
                    folder: "",
                    name: "ne_10m_roads.geojson",
                    url: c.geo_roads_url(),
                    suffixes: &[],
                },
            ],
            Self::GrwlSummary => vec![AssetFile {
                folder: "grwl",
                name: "grwl_summary.zip",
                url: c.geo_grwl_summary_url(),
                suffixes: &[".shp", ".dbf", ".shx"],
            }],
            Self::GrwlMasks => vec![AssetFile {
                folder: "grwl_masks",
                name: "grwl_masks.zip",
                url: c.geo_grwl_mask_url(),
                suffixes: &[".tif", ".tiff"],
            }],
            Self::WaterOutlines => vec![AssetFile {
                folder: "",
                name: "gshhg.zip",
                url: c.geo_gshhg_water_url(),
                suffixes: &["gshhs_c.b", "gshhs_l.b", "gshhs_i.b"],
            }],
            Self::HydroRivers => vec![AssetFile {
                folder: "hydrorivers",
                name: "hydrorivers.zip",
                url: c.geo_hydrorivers_url(),
                suffixes: &[".shp", ".dbf", ".shx"],
            }],
            _ => vec![],
        }
    }
}
struct DownloadSession<'a> {
    root: &'a AssetDirectory,
    config: &'a AppConfig,
    cancelled: &'a AtomicBool,
    progress: &'a Mutex<DownloadProgress>,
}
struct DownloadOutcome {
    assets: Arc<BasemapAssets>,
    disk_usage_bytes: u64,
    errors: Vec<String>,
}
impl DownloadSession<'_> {
    fn download(
        &self,
        queue: &[DownloadPack],
        override_dir: Option<&Path>,
    ) -> Result<DownloadOutcome, String> {
        let mut failures = Vec::new();
        for (i, &pack) in queue.iter().enumerate() {
            if let Err(error) = self.run(
                pack,
                i as f32 / queue.len() as f32,
                0.95 / queue.len() as f32,
            ) {
                failures.push((pack, error));
            }
            if self.cancelled.load(Ordering::Relaxed) {
                return Err("Download cancelled".into());
            }
        }
        self.report(0.96, "Loading offline basemap assets…");
        let assets = Arc::new(BasemapAssets::load_with_cancel(
            &self.root.path(),
            override_dir,
            self.cancelled,
        ));
        if self.cancelled.load(Ordering::Relaxed) {
            return Err("Download cancelled".into());
        }
        let status = assets.status();
        for &pack in queue {
            if !pack.installed(&status) && !failures.iter().any(|(failed, _)| *failed == pack) {
                failures.push((pack, "Download did not produce usable map data".into()));
            }
        }
        Ok(DownloadOutcome {
            assets,
            disk_usage_bytes: dir_size(&self.root.path()),
            errors: failures
                .into_iter()
                .map(|(pack, error)| format!("{}: {error}", pack.label()))
                .collect(),
        })
    }

    fn report(&self, fraction: f32, message: &str) {
        if let Ok(mut p) = self.progress.lock() {
            p.fraction = fraction.clamp(0.0, 1.0);
            p.message = message.into();
        }
    }
    fn remaining(&self) -> u64 {
        self.config
            .geography
            .max_assets_bytes
            .saturating_sub(dir_size(&self.root.path()))
    }
    fn run(&self, pack: DownloadPack, base: f32, span: f32) -> Result<(), String> {
        if self.cancelled.load(Ordering::Relaxed) {
            return Err("Download cancelled".into());
        }
        match pack {
            DownloadPack::TerrainGebco => self.terrain(base, span),
            DownloadPack::LandCover => self.land_cover(base, span),
            _ => {
                let files = pack.files(self.config);
                let n = files.len().max(1) as f32;
                for (i, file) in files.into_iter().enumerate() {
                    let offset = base + span * i as f32 / n;
                    let downloaded = download_to_file(
                        &self.root.path().join(file.folder),
                        file.name,
                        file.url,
                        self.remaining(),
                        self.cancelled,
                        |f, m| self.report(offset + f * span / n, m),
                    )?;
                    if !file.suffixes.is_empty() {
                        extract_matching_from_zip(&downloaded, self.cancelled, file.suffixes)?;
                    }
                }
                Ok(())
            }
        }
    }
    fn terrain(&self, base: f32, span: f32) -> Result<(), String> {
        let mut error = "No GEBCO terrain mirror succeeded".to_string();
        for url in std::iter::once(self.config.geo_gebco_terrain_url())
            .chain(GEBCO_FALLBACK_MIRRORS.iter().copied())
        {
            if self.cancelled.load(Ordering::Relaxed) {
                return Err("Download cancelled".into());
            }
            let zipped = url.to_ascii_lowercase().contains(".zip");
            let name = if zipped {
                "gebco_terrain.zip"
            } else {
                "gebco_terrain.tif"
            };
            let attempt = download_to_file(
                &self.root.path().join("gebco"),
                name,
                url,
                self.remaining(),
                self.cancelled,
                |f, m| self.report(base + f * span, m),
            )
            .and_then(|file| {
                if zipped {
                    extract_matching_from_zip(&file, self.cancelled, &[".tif", ".tiff"])?;
                }
                if zipped {
                    return Ok(());
                }
                little_clip_core::geography::basemap::CogDemPyramid::open(file.as_ref())
                    .map(|_| ())
                    .ok_or_else(|| "GEBCO mirror returned no usable GeoTIFF".to_string())
            });
            match attempt {
                Ok(()) => return Ok(()),
                Err(e) => error = e,
            }
        }
        Err(error)
    }
    fn land_cover(&self, base: f32, span: f32) -> Result<(), String> {
        use little_clip_core::geography::basemap::land_cover::{CONVERSION_SPACE, import_netcdf};
        let tool = CommandRequest::new("gdalwarp")
            .arg("--version")
            .run_cancellable(self.cancelled)
            .map_err(|e| {
                format!(
                    "Install GDAL 3.7+ with NetCDF support (gdalwarp) to import land cover: {e}"
                )
            })?;
        if !tool.status.success() {
            return Err("GDAL land-cover converter is unavailable".into());
        }
        let name = "land_cover_lccs_class_2020_v2_1_1.nc";
        if let Ok(source) = self.root.open_file(std::ffi::OsStr::new(name)) {
            self.report(
                base + 0.2 * span,
                "Converting downloaded Copernicus land cover…",
            );
            import_netcdf(self.root, &source, self.remaining(), self.cancelled)
        } else {
            let limit = self
                .remaining()
                .checked_sub(CONVERSION_SPACE)
                .ok_or("Insufficient disk budget for land-cover import")?;
            let downloaded = download_to_file(
                &self.root.path(),
                name,
                self.config.geo_land_cover_url(),
                limit,
                self.cancelled,
                |f, m| self.report(base + f * span * 0.75, m),
            )?;
            self.report(base + 0.75 * span, "Converting Copernicus land cover…");
            import_netcdf(
                self.root,
                &downloaded.file,
                downloaded.expansion_budget + CONVERSION_SPACE,
                self.cancelled,
            )
        }
    }
}

pub struct AssetDownloader {
    pub assets_dir: PathBuf,
    pub terrain_override_dir: Option<PathBuf>,
    pub max_assets_bytes: u64,
    pub disk_usage_bytes: u64,
    pub status: AssetStatus,
    pub active: Option<DownloadPack>,
    pub last_error: Option<String>,
    progress: Arc<Mutex<DownloadProgress>>,
    cancelled: Arc<AtomicBool>,
    job: Option<Task<Result<DownloadOutcome, String>>>,
}

impl AssetDownloader {
    pub fn new(config: &AppConfig) -> Self {
        let assets_dir = config.geo_assets_dir();
        let terrain_override_dir = config.geography.terrain_override_dir.clone();
        let status = AssetStatus::default();
        let disk_usage_bytes = 0;
        Self {
            assets_dir,
            terrain_override_dir,
            max_assets_bytes: config.geography.max_assets_bytes,
            disk_usage_bytes,
            status,
            active: None,
            last_error: None,
            progress: Arc::new(Mutex::new(DownloadProgress {
                fraction: 0.0,
                message: String::new(),
            })),
            cancelled: Arc::new(AtomicBool::new(false)),
            job: None,
        }
    }

    pub fn progress_snapshot(&self) -> DownloadProgress {
        self.progress
            .lock()
            .map(|p| p.clone())
            .unwrap_or(DownloadProgress {
                fraction: 0.0,
                message: String::new(),
            })
    }

    pub fn cancel(&mut self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }

    pub fn start(&mut self, pack: DownloadPack, config: &AppConfig) {
        if self.active.is_some() {
            return;
        }
        self.last_error = None;
        self.active = Some(pack);
        self.cancelled = Arc::new(AtomicBool::new(false));
        if let Ok(mut p) = self.progress.lock() {
            p.fraction = 0.01;
            p.message = format!("Starting {}…", pack.label());
        }

        let assets_dir = self.assets_dir.clone();
        let config = config.clone();
        let progress = self.progress.clone();
        let previous = self.status.clone();
        let override_dir = self.terrain_override_dir.clone();
        let job = Task::start_unbounded(asset_workers(), move |cancelled| {
            let root = AssetDirectory::create(&assets_dir).map_err(|e| e.to_string())?;
            let owner = fs::File::open(root.path()).map_err(|e| e.to_string())?;
            owner
                .try_lock()
                .map_err(|_| "Another geography download owns this asset directory".to_string())?;
            let session = DownloadSession {
                root: &root,
                config: &config,
                cancelled,
                progress: &progress,
            };
            let queue: Vec<_> = if pack == DownloadPack::All {
                PACKS
                    .into_iter()
                    .filter(|p| !p.installed(&previous))
                    .collect()
            } else {
                vec![pack]
            };
            session.download(&queue, override_dir.as_deref())
        });
        self.cancelled = job.token.clone();
        self.job = Some(job);
    }

    pub fn poll(&mut self, ctx: &egui::Context) -> Option<Arc<BasemapAssets>> {
        if self.active.is_some() {
            ctx.request_repaint_after(Duration::from_millis(80));
        }
        let result = self.job.as_ref()?.poll()?;
        self.active = None;
        self.job = None;
        match result {
            Ok(Ok(outcome)) => {
                self.disk_usage_bytes = outcome.disk_usage_bytes;
                self.status = outcome.assets.status();
                self.last_error = (!outcome.errors.is_empty())
                    .then(|| format!("Unavailable map layers:\n{}", outcome.errors.join("\n")));
                Some(outcome.assets)
            }
            Ok(Err(error)) => {
                self.last_error = Some(error);
                None
            }
            Err(error) => {
                self.last_error = Some(format!("Asset worker failed: {error:?}"));
                None
            }
        }
    }
}

fn download_to_file(
    dir: &Path,
    filename: &str,
    url: &str,
    max_bytes: u64,
    cancelled: &AtomicBool,
    on_progress: impl Fn(f32, &str),
) -> Result<Downloaded, String> {
    if cancelled.load(Ordering::Relaxed) {
        return Err("Download cancelled".into());
    }
    let directory = AssetDirectory::create(dir).map_err(|e| e.to_string())?;
    let staged = directory.stage().map_err(|e| e.to_string())?;
    let (request, fd) = CommandRequest::new("curl")
        .args([
            "--disable",
            "--location",
            "--fail",
            "--silent",
            "--show-error",
            "--connect-timeout",
            "15",
            "--speed-limit",
            "4096",
            "--speed-time",
            "20",
        ])
        .resource_limit(rustix::process::Resource::Fsize, max_bytes)
        .timeout(Duration::from_secs(3600))
        .own_process_group()
        .die_with_parent()
        .inherit_handle(staged.as_file())
        .map_err(|e| e.to_string())?;

    on_progress(0.02, &format!("Downloading {filename}…"));
    let budget_exceeded = AtomicBool::new(false);
    let stalled = AtomicBool::new(false);
    let attempt_cancel = AtomicBool::new(false);
    let last_bytes = std::sync::atomic::AtomicU64::new(0);
    let last_advance = Mutex::new(Instant::now());
    let output = request
        .arg("--output")
        .arg(format!("/proc/self/fd/{fd}"))
        .args(["--", url])
        .run_cancellable_with_progress(&attempt_cancel, || {
            if cancelled.load(Ordering::Relaxed) {
                attempt_cancel.store(true, Ordering::Relaxed);
                return;
            }
            let bytes = staged.as_file().metadata().map_or(0, |m| m.len());
            if bytes > max_bytes {
                budget_exceeded.store(true, Ordering::Relaxed);
                attempt_cancel.store(true, Ordering::Relaxed);
                return;
            }
            let prev = last_bytes.load(Ordering::Relaxed);
            if bytes >= prev + 4096 {
                last_bytes.store(bytes, Ordering::Relaxed);
                if let Ok(mut t) = last_advance.lock() {
                    *t = Instant::now();
                }
            } else if let Ok(t) = last_advance.lock()
                && t.elapsed() >= Duration::from_secs(25)
            {
                stalled.store(true, Ordering::Relaxed);
                attempt_cancel.store(true, Ordering::Relaxed);
                return;
            }
            let mb = bytes as f32 / (1024.0 * 1024.0);
            let approx = (mb / (mb + 80.0)).clamp(0.03, 0.95);
            on_progress(approx, &format!("Downloading {filename} ({mb:.1} MB)…"));
        })
        .map_err(|e| e.to_string())?;
    if budget_exceeded.load(Ordering::Relaxed) {
        return Err("Download aborted: geography dataset disk budget exceeded".into());
    }
    if stalled.load(Ordering::Relaxed) {
        return Err(format!(
            "Download stalled for {filename} (< 4 KB/s for 25s)"
        ));
    }
    if cancelled.load(Ordering::Relaxed) {
        return Err("Download cancelled".into());
    }
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("curl failed for {filename}: {}", stderr.trim()));
    }
    let downloaded_bytes = staged
        .as_file()
        .metadata()
        .map_err(|e| e.to_string())?
        .len();
    if downloaded_bytes > max_bytes {
        return Err("Download exceeded the geography disk budget".into());
    }
    let (file, staging) = if filename.to_ascii_lowercase().ends_with(".zip") {
        (
            staged.as_file().try_clone().map_err(|e| e.to_string())?,
            Some(staged),
        )
    } else {
        (
            directory
                .install(staged, std::ffi::OsStr::new(filename))
                .map_err(|e| e.to_string())?,
            None,
        )
    };
    let path = PathBuf::from(format!("/proc/self/fd/{}", file.as_raw_fd()));
    Ok(Downloaded {
        expansion_budget: max_bytes - downloaded_bytes,
        _staging: staging,
        file,
        directory,
        path,
    })
}

// Declaration order keeps the parent descriptor alive through temporary cleanup.
struct Downloaded {
    expansion_budget: u64,
    _staging: Option<tempfile::NamedTempFile>,
    file: fs::File,
    directory: AssetDirectory,
    path: PathBuf,
}
impl AsRef<Path> for Downloaded {
    fn as_ref(&self) -> &Path {
        &self.path
    }
}

fn extract_matching_from_zip(
    zip_path: &Downloaded,
    cancelled: &AtomicBool,
    suffixes: &[&str],
) -> Result<(), String> {
    let file = zip_path.file.try_clone().map_err(|e| e.to_string())?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;
    if archive.len() > 65_536 {
        return Err("Archive has too many entries".into());
    }
    let mut remaining = zip_path.expansion_budget;
    let mut buffer = [0u8; 64 * 1024];
    let mut extracted = 0;
    for i in 0..archive.len() {
        if cancelled.load(Ordering::Relaxed) {
            return Err("Extraction cancelled".into());
        }
        let mut entry = archive.by_index(i).map_err(|e| e.to_string())?;
        if entry.is_dir() {
            continue;
        }
        let Some(name) = Path::new(entry.name())
            .file_name()
            .and_then(|n| n.to_str())
            .map(str::to_string)
        else {
            continue;
        };
        let lower = name.to_ascii_lowercase();
        if suffixes.iter().any(|s| lower.ends_with(s)) {
            if entry.size() > remaining {
                return Err("Expanded archive exceeds the geography disk budget".into());
            }
            let mut staged = zip_path.directory.stage().map_err(|e| e.to_string())?;
            loop {
                if cancelled.load(Ordering::Relaxed) {
                    return Err("Extraction cancelled".into());
                }
                let count = entry.read(&mut buffer).map_err(|e| e.to_string())?;
                if count == 0 {
                    break;
                }
                remaining = remaining
                    .checked_sub(count as u64)
                    .ok_or("Expanded archive exceeds the geography disk budget")?;
                staged
                    .write_all(&buffer[..count])
                    .map_err(|e| e.to_string())?;
            }
            if cancelled.load(Ordering::Relaxed) {
                return Err("Extraction cancelled".into());
            }
            extracted += 1;
            zip_path
                .directory
                .install(staged, std::ffi::OsStr::new(&name))
                .map_err(|e| e.to_string())?;
        }
    }
    if extracted == 0 {
        return Err("Archive contains no supported map assets".into());
    }
    Ok(())
}

// Conservative accounting: incomplete or protected inventories disable admission.
// Symlinks are never traversed and depth/entry limits prevent cycles or huge scans.
fn dir_size(dir: &Path) -> u64 {
    if !dir.exists() {
        return 0;
    }
    let Ok(root) = AssetDirectory::open(dir) else {
        return u64::MAX;
    };
    let mut pending = vec![(root, 0)];
    let mut total = 0u64;
    let mut visited = 0;
    while let Some((directory, depth)) = pending.pop() {
        let Ok(entries) = fs::read_dir(directory.path()) else {
            return u64::MAX;
        };
        for entry in entries {
            let Ok(entry) = entry else {
                return u64::MAX;
            };
            visited += 1;
            if visited > 100_000 {
                return u64::MAX;
            }
            let Ok(kind) = entry.file_type() else {
                return u64::MAX;
            };
            if kind.is_symlink() {
                continue;
            }
            if kind.is_dir() {
                if depth >= 16 {
                    return u64::MAX;
                }
                // Keep a pin alive for each queued child, rather than an fd pathname
                // whose parent would be dropped at the end of this iteration.
                let Ok(child) = AssetDirectory::open(&entry.path()) else {
                    return u64::MAX;
                };
                pending.push((child, depth + 1));
            } else {
                let Ok(meta) = entry.metadata() else {
                    return u64::MAX;
                };
                total = total.saturating_add(meta.len());
            }
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{ffi::OsStr, io::Write, os::unix::fs::symlink};

    #[test]
    fn an_overview_does_not_satisfy_the_full_terrain_download() {
        let mut status = AssetStatus {
            has_gebco: true,
            ..AssetStatus::default()
        };
        assert!(!DownloadPack::TerrainGebco.installed(&status));
        status.has_full_gebco = true;
        assert!(DownloadPack::TerrainGebco.installed(&status));
    }

    fn download_fixture(config: &AppConfig, queue: &[DownloadPack]) -> DownloadOutcome {
        let root = AssetDirectory::create(&config.geo_assets_dir()).unwrap();
        let cancelled = AtomicBool::new(false);
        let progress = Mutex::new(DownloadProgress {
            fraction: 0.0,
            message: String::new(),
        });
        DownloadSession {
            root: &root,
            config,
            cancelled: &cancelled,
            progress: &progress,
        }
        .download(queue, None)
        .unwrap()
    }

    #[test]
    fn failed_pack_keeps_and_publishes_layers_downloaded_before_and_after_it() {
        let temp = tempfile::tempdir().unwrap();
        let line = temp.path().join("line.geojson");
        fs::write(
            &line,
            br#"{"type":"FeatureCollection","features":[{"type":"Feature","properties":{},"geometry":{"type":"LineString","coordinates":[[0,0],[1,1]]}}]}"#,
        )
        .unwrap();
        let lake = temp.path().join("lake.geojson");
        fs::write(
            &lake,
            br#"{"type":"FeatureCollection","features":[{"type":"Feature","properties":{},"geometry":{"type":"Polygon","coordinates":[[[0,0],[1,0],[1,1],[0,0]]]}}]}"#,
        )
        .unwrap();
        let mut config = AppConfig::default();
        config.geography.assets_dir = Some(temp.path().join("assets"));
        let line_url = format!("file://{}", line.display());
        config.geography.countries_url = Some(line_url.clone());
        config.geography.states_url = Some(line_url.clone());
        config.geography.roads_url = Some(line_url);
        config.geography.lakes_url = Some(format!("file://{}", lake.display()));
        // With GDAL absent this fails at the dependency check; with it installed
        // the deliberately unsupported URL fails without contacting a server.
        config.geography.land_cover_url = Some("unavailable-test://land-cover".into());
        let outcome = download_fixture(
            &config,
            &[
                DownloadPack::Borders,
                DownloadPack::LandCover,
                DownloadPack::RoadsAndLakes,
            ],
        );
        assert_eq!(outcome.errors.len(), 1);
        assert!(outcome.errors[0].contains(DownloadPack::LandCover.label()));
        let pool = BackgroundJobs::new(1, 1);
        let mut downloader = AssetDownloader::new(&config);
        downloader.active = Some(DownloadPack::All);
        downloader.job = Some(Task::start_unbounded(&pool, move |_| Ok(outcome)));
        let ctx = egui::Context::default();
        let deadline = Instant::now() + Duration::from_secs(5);
        let assets = loop {
            if let Some(assets) = downloader.poll(&ctx) {
                break assets;
            }
            assert!(
                Instant::now() < deadline,
                "Partial download was not published"
            );
            std::thread::sleep(Duration::from_millis(1));
        };
        assert!(assets.status().has_borders);
        assert!(downloader.status.has_roads_lakes);
        assert!(!downloader.status.has_land_cover);
        assert!(downloader.disk_usage_bytes > 0);
        assert!(downloader.last_error.unwrap().contains("Land Cover"));
    }

    #[test]
    fn successful_transfer_of_unusable_data_is_reported() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("empty.geojson");
        fs::write(&source, br#"{"type":"FeatureCollection","features":[]}"#).unwrap();
        let mut config = AppConfig::default();
        config.geography.assets_dir = Some(temp.path().join("assets"));
        let url = format!("file://{}", source.display());
        config.geography.countries_url = Some(url.clone());
        config.geography.states_url = Some(url);
        let outcome = download_fixture(&config, &[DownloadPack::Borders]);
        assert!(!outcome.assets.status().has_borders);
        assert_eq!(outcome.errors.len(), 1);
        assert!(outcome.errors[0].contains("usable map data"));
    }

    #[test]
    fn cancelling_an_accepted_download_retains_its_owner_until_worker_exit() {
        let pool = BackgroundJobs::new(1, 1);
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let mut downloader = AssetDownloader::new(&AppConfig::default());
        let job = Task::start_unbounded(&pool, move |cancel| {
            started_tx.send(()).unwrap();
            release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            assert!(cancel.load(Ordering::Relaxed));
            Err("Download cancelled".to_string())
        });
        downloader.cancelled = job.token.clone();
        downloader.job = Some(job);
        downloader.active = Some(DownloadPack::QuickStart);
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        downloader.cancel();
        let ctx = egui::Context::default();
        assert!(downloader.poll(&ctx).is_none());
        assert!(downloader.active.is_some());
        release_tx.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while downloader.active.is_some() && Instant::now() < deadline {
            downloader.poll(&ctx);
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(downloader.active.is_none());
        assert_eq!(downloader.last_error.as_deref(), Some("Download cancelled"));
    }

    #[test]
    fn extraction_retains_its_directory_and_rejects_symlink_destinations() {
        let root = tempfile::tempdir().unwrap();
        let original = root.path().join("assets");
        let directory = AssetDirectory::create(&original).unwrap();
        let mut staged = directory.stage().unwrap();
        {
            let mut zip = zip::ZipWriter::new(staged.as_file_mut());
            zip.start_file("nested/tile.tif", zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(b"fixture raster").unwrap();
            zip.finish().unwrap();
        }
        let file = staged.as_file().try_clone().unwrap();
        let mut downloaded = Downloaded {
            path: PathBuf::from(format!("/proc/self/fd/{}", file.as_raw_fd())),
            file,
            expansion_budget: 1024,
            _staging: Some(staged),
            directory,
        };
        let cancelled = AtomicBool::new(false);
        assert!(
            extract_matching_from_zip(&downloaded, &cancelled, &[".geojson"])
                .unwrap_err()
                .contains("no supported")
        );
        cancelled.store(true, Ordering::Relaxed);
        assert!(
            extract_matching_from_zip(&downloaded, &cancelled, &[".tif"])
                .unwrap_err()
                .contains("cancelled")
        );
        cancelled.store(false, Ordering::Relaxed);
        downloaded.expansion_budget = 5;
        assert!(
            extract_matching_from_zip(&downloaded, &cancelled, &[".tif"])
                .unwrap_err()
                .contains("budget")
        );
        assert!(!original.join("tile.tif").exists());
        downloaded.expansion_budget = 1024;
        let moved = root.path().join("moved");
        fs::rename(&original, &moved).unwrap();
        fs::create_dir(&original).unwrap();
        let outside = root.path().join("outside");
        fs::write(&outside, b"keep original").unwrap();
        symlink(&outside, moved.join("tile.tif")).unwrap();
        assert!(extract_matching_from_zip(&downloaded, &cancelled, &[".tif"]).is_err());
        assert_eq!(fs::read(&outside).unwrap(), b"keep original");
        fs::remove_file(moved.join("tile.tif")).unwrap();
        extract_matching_from_zip(&downloaded, &cancelled, &[".tif"]).unwrap();
        assert_eq!(fs::read(moved.join("tile.tif")).unwrap(), b"fixture raster");
        assert!(!original.join("tile.tif").exists());
        // Raw installation uses the same held parent and atomically replaces a file.
        downloaded
            .directory
            .write(OsStr::new("tile.tif"), b"next revision")
            .unwrap();
        assert_eq!(fs::read(moved.join("tile.tif")).unwrap(), b"next revision");
    }
}

#[cfg(test)]
#[test]
fn disk_accounting_does_not_follow_directory_symlink_cycles() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("asset"), b"12345").unwrap();
    std::os::unix::fs::symlink(root.path(), root.path().join("cycle")).unwrap();
    assert_eq!(dir_size(root.path()), 5);
}
