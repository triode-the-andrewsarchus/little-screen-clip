//! Background downloader and extractor for offline 3D globe / 2D map basemap assets.

use little_clip_core::{
    CommandRequest,
    config::AppConfig,
    geography::basemap::{AssetStatus, BasemapAssets},
    kernel_interface::create_content_directory,
};
use std::{
    fs,
    io::Read,
    os::fd::AsRawFd,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

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
    rx: Option<mpsc::Receiver<Result<Arc<BasemapAssets>, String>>>,
}

impl AssetDownloader {
    pub fn new(config: &AppConfig) -> Self {
        let assets_dir = config.geo_assets_dir();
        let terrain_override_dir = config.geography.terrain_override_dir.clone();
        let status = BasemapAssets::probe_status(&assets_dir, terrain_override_dir.as_deref());
        let disk_usage_bytes = dir_size(&assets_dir);
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
            rx: None,
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
        self.disk_usage_bytes = dir_size(&self.assets_dir);
        if self.disk_usage_bytes >= self.max_assets_bytes {
            self.last_error = Some(format!(
                "Offline geography disk budget reached ({:.2} GB / {:.2} GB)",
                self.disk_usage_bytes as f64 / (1024.0 * 1024.0 * 1024.0),
                self.max_assets_bytes as f64 / (1024.0 * 1024.0 * 1024.0),
            ));
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
        let terrain_override_dir = self.terrain_override_dir.clone();
        let ne2_url = config.geo_ne2_texture_url().to_string();
        let cities_url = config.geo_cities_url().to_string();
        let countries_url = config.geo_countries_url().to_string();
        let states_url = config.geo_states_url().to_string();
        let roads_url = config.geo_roads_url().to_string();
        let lakes_url = config.geo_lakes_url().to_string();
        let grwl_summary_url = config.geo_grwl_summary_url().to_string();
        let grwl_mask_url = config.geo_grwl_mask_url().to_string();
        let gshhg_url = config.geo_gshhg_water_url().to_string();
        let gebco_url = config.geo_gebco_terrain_url().to_string();
        let lc_url = config.geo_land_cover_url().to_string();
        let rivers_url = config.geo_hydrorivers_url().to_string();
        let status_snapshot = self.status.clone();
        let max_assets_bytes = self.max_assets_bytes;
        let cancelled = self.cancelled.clone();
        let progress = self.progress.clone();
        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);

        std::thread::spawn(move || {
            let report = |fraction: f32, msg: &str| {
                if let Ok(mut p) = progress.lock() {
                    p.fraction = fraction.clamp(0.0, 1.0);
                    p.message = msg.to_string();
                }
            };
            let run_pack = |single: DownloadPack, base: f32, span: f32| -> Result<(), String> {
                let remaining = max_assets_bytes.saturating_sub(dir_size(&assets_dir));
                if remaining == 0 {
                    return Err("Offline geography disk budget reached".into());
                }
                match single {
                    DownloadPack::QuickStart => {
                        report(
                            base + 0.02 * span,
                            "Downloading Natural Earth Populated Places…",
                        );
                        let cities_path = download_to_file(
                            &assets_dir,
                            "ne_10m_populated_places_simple.geojson",
                            &cities_url,
                            remaining,
                            &cancelled,
                            |f, m| report(base + (0.02 + f * 0.13) * span, m),
                        )?;
                        if let Ok(bytes) = fs::read(&cities_path)
                            && let Ok(parsed) =
                                little_clip_core::geography::basemap::parse_cities_geojson(&bytes)
                            && let Ok(encoded) = serde_json::to_vec(&parsed)
                        {
                            let _ = fs::write(assets_dir.join("cities_all.json"), encoded);
                        }
                        let rem = max_assets_bytes.saturating_sub(dir_size(&assets_dir));
                        report(base + 0.16 * span, "Downloading Natural Earth II texture…");
                        let zip_path = download_to_file(
                            &assets_dir,
                            "ne2_texture.zip",
                            &ne2_url,
                            rem,
                            &cancelled,
                            |f, m| report(base + (0.16 + f * 0.74) * span, m),
                        )?;
                        report(base + 0.91 * span, "Extracting Natural Earth II texture…");
                        extract_matching_from_zip(
                            &zip_path,
                            &assets_dir,
                            &[".tif", ".tiff", ".png", ".jpg", ".jpeg"],
                        )?;
                        let _ = fs::remove_file(&zip_path);
                    }
                    DownloadPack::Borders => {
                        report(
                            base + 0.02 * span,
                            "Downloading Natural Earth 10m country borders…",
                        );
                        let c_path = download_to_file(
                            &assets_dir,
                            "ne_10m_admin_0_boundary_lines_land.geojson",
                            &countries_url,
                            remaining,
                            &cancelled,
                            |f, m| report(base + (0.02 + f * 0.25) * span, m),
                        )?;
                        if let Ok(bytes) = fs::read(&c_path)
                            && let Ok(parsed) =
                                little_clip_core::geography::basemap::parse_borders_geojson(&bytes)
                            && let Ok(encoded) = serde_json::to_vec(&parsed)
                        {
                            let _ = fs::write(assets_dir.join("borders_admin0.json"), encoded);
                        }
                        let rem = max_assets_bytes.saturating_sub(dir_size(&assets_dir));
                        report(
                            base + 0.30 * span,
                            "Downloading Natural Earth 10m state/province borders…",
                        );
                        let s_path = download_to_file(
                            &assets_dir,
                            "ne_10m_admin_1_states_provinces_lines.geojson",
                            &states_url,
                            rem,
                            &cancelled,
                            |f, m| report(base + (0.30 + f * 0.60) * span, m),
                        )?;
                        if let Ok(bytes) = fs::read(&s_path)
                            && let Ok(parsed) =
                                little_clip_core::geography::basemap::parse_borders_geojson(&bytes)
                            && let Ok(encoded) = serde_json::to_vec(&parsed)
                        {
                            let _ = fs::write(assets_dir.join("borders_admin1.json"), encoded);
                        }
                    }
                    DownloadPack::RoadsAndLakes => {
                        report(base + 0.02 * span, "Downloading Natural Earth 10m lakes…");
                        let l_path = download_to_file(
                            &assets_dir,
                            "ne_10m_lakes.geojson",
                            &lakes_url,
                            remaining,
                            &cancelled,
                            |f, m| report(base + (0.02 + f * 0.16) * span, m),
                        )?;
                        if let Ok(bytes) = fs::read(&l_path)
                            && let Ok(parsed) =
                                little_clip_core::geography::basemap::parse_lakes_geojson(&bytes)
                            && let Ok(encoded) = serde_json::to_vec(&parsed)
                        {
                            let _ = fs::write(assets_dir.join("lakes_10m.json"), encoded);
                        }
                        let rem = max_assets_bytes.saturating_sub(dir_size(&assets_dir));
                        report(base + 0.20 * span, "Downloading Natural Earth 10m roads…");
                        let r_path = download_to_file(
                            &assets_dir,
                            "ne_10m_roads.geojson",
                            &roads_url,
                            rem,
                            &cancelled,
                            |f, m| report(base + (0.20 + f * 0.70) * span, m),
                        )?;
                        if let Ok(bytes) = fs::read(&r_path)
                            && let Ok(parsed) =
                                little_clip_core::geography::basemap::parse_roads_geojson(&bytes)
                            && let Ok(encoded) = serde_json::to_vec(&parsed)
                        {
                            let _ = fs::write(assets_dir.join("roads_10m.json"), encoded);
                        }
                    }
                    DownloadPack::GrwlSummary => {
                        let grwl_dir = assets_dir.join("grwl");
                        report(
                            base + 0.02 * span,
                            "Downloading GRWL summary river widths (43 MB)…",
                        );
                        let zip_path = download_to_file(
                            &grwl_dir,
                            "grwl_summary.zip",
                            &grwl_summary_url,
                            remaining,
                            &cancelled,
                            |f, m| report(base + (0.02 + f * 0.85) * span, m),
                        )?;
                        report(base + 0.88 * span, "Extracting GRWL centerlines…");
                        extract_matching_from_zip(&zip_path, &grwl_dir, &[".shp", ".dbf", ".shx"])?;
                        let _ = fs::remove_file(&zip_path);
                    }
                    DownloadPack::GrwlMasks => {
                        let mask_dir = assets_dir.join("grwl_masks");
                        report(
                            base + 0.02 * span,
                            "Downloading GRWL 30m water masks (394 MB)…",
                        );
                        let zip_path = download_to_file(
                            &mask_dir,
                            "grwl_masks.zip",
                            &grwl_mask_url,
                            remaining,
                            &cancelled,
                            |f, m| report(base + (0.02 + f * 0.85) * span, m),
                        )?;
                        report(base + 0.88 * span, "Extracting GRWL 30m water masks…");
                        extract_matching_from_zip(&zip_path, &mask_dir, &[".tif", ".tiff"])?;
                        let _ = fs::remove_file(&zip_path);
                    }
                    DownloadPack::WaterOutlines => {
                        report(base + 0.02 * span, "Downloading GSHHG coastlines & lakes…");
                        let zip_path = download_to_file(
                            &assets_dir,
                            "gshhg.zip",
                            &gshhg_url,
                            remaining,
                            &cancelled,
                            |f, m| report(base + (0.02 + f * 0.85) * span, m),
                        )?;
                        report(base + 0.88 * span, "Extracting GSHHG binary shorelines…");
                        extract_matching_from_zip(
                            &zip_path,
                            &assets_dir,
                            &["gshhs_c.b", "gshhs_l.b", "gshhs_i.b"],
                        )?;
                        let _ = fs::remove_file(&zip_path);
                    }
                    DownloadPack::TerrainGebco => {
                        let terrain_dir = assets_dir.join("gebco");
                        let mut candidates = vec![gebco_url.clone()];
                        for &mirror in GEBCO_FALLBACK_MIRRORS {
                            if mirror != gebco_url {
                                candidates.push(mirror.to_string());
                            }
                        }
                        let mut last_err = String::new();
                        let mut ok = false;
                        for (idx, url) in candidates.iter().enumerate() {
                            if cancelled.load(Ordering::Relaxed) {
                                return Err("Download cancelled".into());
                            }
                            let is_zip = url.to_ascii_lowercase().ends_with(".zip");
                            let fname = if is_zip {
                                "gebco_terrain.zip"
                            } else {
                                url.rsplit('/').next().unwrap_or("gebco_terrain.tif")
                            };
                            report(
                                base + 0.02 * span,
                                &format!(
                                    "Downloading GEBCO terrain (mirror {}/{})…",
                                    idx + 1,
                                    candidates.len()
                                ),
                            );
                            match download_to_file(
                                &terrain_dir,
                                fname,
                                url,
                                remaining,
                                &cancelled,
                                |f, m| report(base + (0.02 + f * 0.88) * span, m),
                            ) {
                                Ok(downloaded) => {
                                    if is_zip {
                                        report(
                                            base + 0.92 * span,
                                            "Extracting GEBCO terrain files…",
                                        );
                                        let _ = extract_matching_from_zip(
                                            &downloaded,
                                            &terrain_dir,
                                            &[".tif", ".tiff", ".nc"],
                                        );
                                        let _ = fs::remove_file(&downloaded);
                                    }
                                    ok = true;
                                    break;
                                }
                                Err(err) => {
                                    last_err = err;
                                }
                            }
                        }
                        if !ok {
                            return Err(last_err);
                        }
                    }
                    DownloadPack::LandCover => {
                        report(
                            base + 0.02 * span,
                            "Downloading Copernicus 300m land cover…",
                        );
                        let _ = download_to_file(
                            &assets_dir,
                            "land_cover_lccs_class_2020_v2_1_1.nc",
                            &lc_url,
                            remaining,
                            &cancelled,
                            |f, m| report(base + (0.02 + f * 0.90) * span, m),
                        )?;
                    }
                    DownloadPack::HydroRivers => {
                        let rivers_dir = assets_dir.join("hydrorivers");
                        report(
                            base + 0.02 * span,
                            "Downloading HydroRIVERS shapefile archive…",
                        );
                        let archive = download_to_file(
                            &rivers_dir,
                            "hydrorivers.zip",
                            &rivers_url,
                            remaining,
                            &cancelled,
                            |f, m| report(base + (0.02 + f * 0.88) * span, m),
                        )?;
                        report(base + 0.92 * span, "Extracting HydroRIVERS…");
                        let _ = extract_matching_from_zip(
                            &archive,
                            &rivers_dir,
                            &[".shp", ".dbf", ".shx"],
                        );
                        let _ = fs::remove_file(&archive);
                    }
                    DownloadPack::All => {}
                }
                Ok(())
            };

            let result = (|| -> Result<Arc<BasemapAssets>, String> {
                if pack == DownloadPack::All {
                    let mut queue = Vec::new();
                    if !status_snapshot.has_ne2 || !status_snapshot.has_cities {
                        queue.push(DownloadPack::QuickStart);
                    }
                    if !status_snapshot.has_borders {
                        queue.push(DownloadPack::Borders);
                    }
                    if !status_snapshot.has_roads_lakes {
                        queue.push(DownloadPack::RoadsAndLakes);
                    }
                    if !status_snapshot.has_grwl {
                        queue.push(DownloadPack::GrwlSummary);
                    }
                    if !status_snapshot.has_grwl_masks {
                        queue.push(DownloadPack::GrwlMasks);
                    }
                    if !status_snapshot.has_gshhg {
                        queue.push(DownloadPack::WaterOutlines);
                    }
                    if !status_snapshot.has_gebco {
                        queue.push(DownloadPack::TerrainGebco);
                    }
                    if !status_snapshot.has_land_cover {
                        queue.push(DownloadPack::LandCover);
                    }
                    if !status_snapshot.has_hydrorivers {
                        queue.push(DownloadPack::HydroRivers);
                    }
                    let count = queue.len().max(1) as f32;
                    for (idx, sub) in queue.into_iter().enumerate() {
                        run_pack(sub, idx as f32 / count, 0.95 / count)?;
                    }
                } else {
                    run_pack(pack, 0.0, 0.95)?;
                }
                report(0.96, "Loading offline basemap assets…");
                Ok(Arc::new(BasemapAssets::load(
                    &assets_dir,
                    terrain_override_dir.as_deref(),
                )))
            })();
            let _ = tx.send(result);
        });
    }

    pub fn poll(&mut self, ctx: &egui::Context) -> Option<Arc<BasemapAssets>> {
        if self.active.is_some() {
            ctx.request_repaint_after(Duration::from_millis(80));
        }
        let rx = self.rx.as_ref()?;
        match rx.try_recv() {
            Ok(Ok(assets)) => {
                self.active = None;
                self.rx = None;
                self.disk_usage_bytes = dir_size(&self.assets_dir);
                self.status = BasemapAssets::probe_status(
                    &self.assets_dir,
                    self.terrain_override_dir.as_deref(),
                );
                Some(assets)
            }
            Ok(Err(err)) => {
                self.active = None;
                self.rx = None;
                self.last_error = Some(err);
                None
            }
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => {
                self.active = None;
                self.rx = None;
                self.last_error = Some("Download worker terminated unexpectedly".into());
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
) -> Result<PathBuf, String> {
    if cancelled.load(Ordering::Relaxed) {
        return Err("Download cancelled".into());
    }
    let directory = create_content_directory(dir).map_err(|e| e.to_string())?;
    let anchor = PathBuf::from(format!("/proc/self/fd/{}", directory.as_raw_fd()));
    let staged = tempfile::NamedTempFile::new_in(&anchor).map_err(|e| e.to_string())?;
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
    let target = anchor.join(filename);
    if target.exists() {
        let _ = fs::remove_file(&target);
    }
    staged.persist(&target).map_err(|e| e.error.to_string())?;
    Ok(dir.join(filename))
}

fn extract_matching_from_zip(
    zip_path: &Path,
    dest_dir: &Path,
    suffixes: &[&str],
) -> Result<(), String> {
    let file = fs::File::open(zip_path).map_err(|e| e.to_string())?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;
    for i in 0..archive.len() {
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
            let mut buf = Vec::new();
            entry.read_to_end(&mut buf).map_err(|e| e.to_string())?;
            fs::write(dest_dir.join(&name), &buf).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

fn dir_size(dir: &Path) -> u64 {
    let Ok(entries) = fs::read_dir(dir) else {
        return 0;
    };
    let mut total = 0u64;
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            total = total.saturating_add(dir_size(&p));
        } else if let Ok(meta) = entry.metadata() {
            total = total.saturating_add(meta.len());
        }
    }
    total
}
