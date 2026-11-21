//! Background downloader and extractor for offline 3D globe / 2D map basemap assets.
pub(crate) mod nasadem;
pub(super) mod viewfinder;

use super::geography::{detail, lunar_terrain, sky, solar};

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
    Viewfinder,
    LandCover,
    HydroRivers,
    Sky,
    StarCatalog,
    MoonRelief,
    SolarSystem,
    World(solar::Id),
    DetailedTransportation,
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
            Self::Viewfinder => "Viewfinder 1″ Regional Terrain",
            Self::TerrainGebco => "GEBCO 2025 Terrain (15\" global DEM)",
            Self::LandCover => "Copernicus 300m Land Cover",
            Self::HydroRivers => "HydroRIVERS Global Network",
            Self::Sky => "Sky and Earth Lighting",
            Self::StarCatalog => "Constellations and Star Catalog",
            Self::MoonRelief => "Moon Elevation (NASA LOLA)",
            Self::SolarSystem => "Solar System Offline Data",
            Self::World(id) => solar::assets::name(id).unwrap_or("World Assets"),
            Self::DetailedTransportation => "Detailed Transportation",
            Self::All => "Base Offline Maps",
        }
    }
}

#[derive(Clone, Debug)]
pub struct DownloadProgress {
    pub fraction: f32,
    pub message: String,
}

const BASE_PACKS: [DownloadPack; 9] = [
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
fn solar_packs() -> &'static [(DownloadPack, u64)] {
    static PACKS: LazyLock<Vec<(DownloadPack, u64)>> = LazyLock::new(|| {
        [
            (DownloadPack::Sky, sky::PACK_BYTES),
            (
                DownloadPack::StarCatalog,
                sky::catalog::DOWNLOAD_BYTES as u64,
            ),
            (DownloadPack::MoonRelief, lunar_terrain::BYTES),
        ]
        .into_iter()
        .chain(solar::assets::bulk_worlds().map(|(id, bytes)| (DownloadPack::World(id), bytes)))
        .collect()
    });
    &PACKS
}

pub(super) fn solar_download_bytes() -> u64 {
    solar_packs().iter().map(|(_, bytes)| bytes).sum()
}

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
            Self::Viewfinder => status.local_hgt_count > 0,
            Self::LandCover => status.has_land_cover,
            Self::HydroRivers => status.has_hydrorivers,
            Self::Sky
            | Self::StarCatalog
            | Self::MoonRelief
            | Self::SolarSystem
            | Self::World(_)
            | Self::DetailedTransportation => true, // Verified by their own loaders.
            Self::All => BASE_PACKS.into_iter().all(|p| p.installed(status)),
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
            Self::Sky => [
                (
                    sky::KERNEL,
                    "https://naif.jpl.nasa.gov/pub/naif/generic_kernels/spk/planets/de440.bsp",
                ),
                (sky::EOP, "https://maia.usno.navy.mil/ser7/finals2000A.all"),
                (
                    sky::STARS,
                    "https://svs.gsfc.nasa.gov/vis/a000000/a004800/a004851/starmap_2020_4k.exr",
                ),
                (
                    sky::MOON,
                    "https://svs.gsfc.nasa.gov/vis/a000000/a004700/a004720/lroc_color_poles_2k.tif",
                ),
                (
                    sky::NIGHT,
                    "https://assets.science.nasa.gov/content/dam/science/esd/eo/images/imagerecords/144000/144898/BlackMarble_2016_01deg.jpg",
                ),
            ]
            .into_iter()
            .map(|(name, url)| AssetFile {
                folder: sky::DIRECTORY,
                name,
                url,
                suffixes: &[],
            })
            .collect(),
            Self::StarCatalog => sky::catalog::SOURCES
                .into_iter()
                .map(|(name, url, _)| AssetFile {
                    folder: sky::DIRECTORY,
                    name,
                    url,
                    suffixes: &[],
                })
                .collect(),
            _ => vec![],
        }
    }
}
struct DownloadSession<'a> {
    root: &'a AssetDirectory,
    config: &'a AppConfig,
    cancelled: &'a AtomicBool,
    progress: &'a Mutex<DownloadProgress>,
    viewfinder: Option<&'a viewfinder::Plan>,
    all_ceiling: Option<u64>,
}
struct DownloadOutcome {
    assets: Arc<BasemapAssets>,
    disk_usage_bytes: u64,
    errors: Vec<String>,
    transport: detail::Status,
}

enum DownloadResult {
    Earth(DownloadOutcome),
    World {
        disk_usage_bytes: u64,
        solar_errors: Option<Vec<String>>,
    },
}
impl DownloadSession<'_> {
    fn download(
        &self,
        queue: &[DownloadPack],
        override_dir: Option<&Path>,
    ) -> Result<DownloadOutcome, String> {
        let mut failures =
            self.transfer_queue(queue.iter().map(|&pack| (pack, 0.95 / queue.len() as f32)))?;
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
            transport: detail::Status::load(self.root.display_path()),
            disk_usage_bytes: dir_size(&self.root.path()),
            errors: failures
                .into_iter()
                .map(|(pack, error)| format!("{}: {error}", pack.label()))
                .collect(),
        })
    }

    fn transfer_queue(
        &self,
        queue: impl IntoIterator<Item = (DownloadPack, f32)>,
    ) -> Result<Vec<(DownloadPack, String)>, String> {
        let mut failures = Vec::new();
        let mut base = 0.0;
        for (pack, span) in queue {
            if self.cancelled.load(Ordering::Relaxed) {
                return Err("Download cancelled".into());
            }
            self.report(base, pack.label());
            if let Err(error) = self.run(pack, base, span) {
                failures.push((pack, error));
            }
            base += span;
        }
        if self.cancelled.load(Ordering::Relaxed) {
            return Err("Download cancelled".into());
        }
        Ok(failures)
    }

    fn solar(&self) -> Result<DownloadResult, String> {
        let bytes = solar_download_bytes() as f32;
        let errors = self
            .transfer_queue(
                solar_packs()
                    .iter()
                    .map(|&(pack, size)| (pack, 0.98 * size as f32 / bytes)),
            )?
            .into_iter()
            .map(|(pack, error)| format!("{} · {error}", pack.label()))
            .collect();
        Ok(DownloadResult::World {
            disk_usage_bytes: dir_size(&self.root.path()),
            solar_errors: Some(errors),
        })
    }

    fn report(&self, fraction: f32, message: &str) {
        if let Ok(mut p) = self.progress.lock() {
            p.fraction = fraction.clamp(0.0, 1.0);
            p.message = message.into();
        }
    }
    fn remaining(&self) -> Result<u64, String> {
        let available = free_space(self.root)?;
        Ok(self.all_ceiling.map_or(available, |ceiling| {
            available.min(ceiling.saturating_sub(dir_size(&self.root.path())))
        }))
    }
    fn run(&self, pack: DownloadPack, base: f32, span: f32) -> Result<(), String> {
        if self.cancelled.load(Ordering::Relaxed) {
            return Err("Download cancelled".into());
        }
        match pack {
            DownloadPack::TerrainGebco => self.terrain(base, span),
            DownloadPack::Viewfinder => viewfinder::download(
                self,
                self.viewfinder.ok_or("Missing regional terrain plan")?,
            ),
            DownloadPack::LandCover => self.land_cover(base, span),
            DownloadPack::DetailedTransportation => self.transportation(base, span),
            DownloadPack::MoonRelief => self.download_checked_files(
                Path::new(lunar_terrain::DIRECTORY),
                lunar_terrain::FILES
                    .into_iter()
                    .map(|(name, url, size)| (name, url, size..=size)),
                |_, _| true,
                base,
                span,
            ),
            DownloadPack::World(id) => {
                let asset =
                    solar::assets::Download::for_body(id).ok_or("No dataset for this body")?;
                self.download_checked_files(
                    &asset.folder,
                    [(asset.name, asset.url.as_str(), asset.size)],
                    solar::assets::verify,
                    base,
                    span,
                )
            }
            DownloadPack::Sky | DownloadPack::StarCatalog => self.download_sky(
                pack.files(self.config),
                pack == DownloadPack::StarCatalog,
                base,
                span,
            ),
            _ => {
                let files = pack.files(self.config);
                let n = files.len().max(1) as f32;
                for (i, file) in files.into_iter().enumerate() {
                    let offset = base + span * i as f32 / n;
                    let downloaded = download_to_file(
                        &self.root.path().join(file.folder),
                        file.name,
                        file.url,
                        self.remaining()?,
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
    fn download_sky(
        &self,
        assets: Vec<AssetFile<'_>>,
        verify_catalog: bool,
        base: f32,
        span: f32,
    ) -> Result<(), String> {
        self.download_checked_files(
            Path::new(sky::DIRECTORY),
            assets
                .into_iter()
                .filter_map(|file| Some((file.name, file.url, sky::asset_size(file.name)?))),
            |name, file| !verify_catalog || sky::catalog::verified(name, file),
            base,
            span,
        )
    }

    fn download_checked_files<'a>(
        &self,
        directory: &Path,
        files: impl IntoIterator<Item = (&'a str, &'a str, std::ops::RangeInclusive<u64>)>,
        verify: impl Fn(&str, &fs::File) -> bool,
        base: f32,
        span: f32,
    ) -> Result<(), String> {
        let root = self.root.path().join(directory);
        let files: Vec<_> = files
            .into_iter()
            .filter(|(name, _, sizes)| {
                let present =
                    little_clip_core::kernel_interface::open_content(&root.join(name), false)
                        .ok()
                        .is_some_and(|existing| {
                            existing
                                .metadata()
                                .is_ok_and(|meta| sizes.contains(&meta.len()))
                                && verify(name, &existing)
                        });
                !present
            })
            .collect();
        let required: u64 = files.iter().map(|(_, _, sizes)| *sizes.end()).sum();
        require_space(self.root, required)?;
        let count = files.len().max(1) as f32;
        let mut failures = Vec::new();
        for (i, (name, url, sizes)) in files.into_iter().enumerate() {
            if self.cancelled.load(Ordering::Relaxed) {
                return Err("Download cancelled".into());
            }
            let frame = |f, msg: &str| self.report(base + span * (i as f32 + f) / count, msg);
            let transfer = download_to_file(&root, name, url, *sizes.end(), self.cancelled, frame)
                .and_then(|downloaded| {
                    let bytes = downloaded.file.metadata().map_err(|e| e.to_string())?.len();
                    if !sizes.contains(&bytes) {
                        return Err("Unexpected source size".into());
                    }
                    if !verify(name, &downloaded.file) {
                        return Err("Unexpected source contents".into());
                    }
                    Ok(())
                });
            if let Err(error) = transfer {
                failures.push(format!("{name} · {error}"));
            }
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(failures.join("\n"))
        }
    }

    fn transportation(&self, base: f32, span: f32) -> Result<(), String> {
        let root = self.root;
        let plan = detail::Status::load(root.display_path());
        if plan.source.is_some() {
            return Ok(());
        }
        require_space(root, detail::free_space_for_import(plan.bytes))?;
        let mut db = detail::import::open(root)?;
        let manifest = detail::manifest();
        let mut processed = 0;
        for (index, object) in manifest.sources.iter().enumerate() {
            if detail::import::imported(&db, index)? {
                processed += object.bytes;
                continue;
            }
            let already = detail::database_bytes(root);
            let available = require_space(root, detail::free_space_for_import(already))?;
            let index_limit = already.saturating_add(available - detail::STAGING_BYTES);
            let url = format!("{}{}", manifest.base_url, object.key);
            let start = processed as f64 / detail::DOWNLOAD_BYTES as f64;
            let weight = object.bytes as f64 / detail::DOWNLOAD_BYTES as f64;
            let part = format!("{} of {}", index + 1, manifest.sources.len());
            let file = download_with_request(
                &root.path().join("overture-part.parquet"),
                &url,
                object.bytes,
                self.cancelled,
                |bytes| {
                    let fraction = bytes as f64 / object.bytes as f64;
                    self.report(
                        base + span * (start + weight * 0.8 * fraction) as f32,
                        &format!(
                            "Downloading file {part} · {:.0} / {:.0} MB",
                            bytes as f64 / 1e6,
                            object.bytes as f64 / 1e6,
                        ),
                    )
                },
                CommandRequest::new("curl"),
                true,
            )?;
            if file.file.metadata().map_err(|e| e.to_string())?.len() != object.bytes {
                return Err(format!(
                    "Overture object {} has an unexpected byte count",
                    object.key
                ));
            }
            self.report(
                base + span * (start + weight * 0.8) as f32,
                &format!("Indexing file {part}…"),
            );
            detail::import::file(
                &mut db,
                file.file.try_clone().map_err(|e| e.to_string())?,
                object.kind,
                index,
                index_limit,
                self.cancelled,
            )
            .map_err(|error| format!("Overture file {}: {error}", index + 1))?;
            processed += object.bytes;
        }
        self.report(base + span, "Finalizing Overture index…");
        detail::import::finish(&db)
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
                self.remaining()?,
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
            import_netcdf(self.root, &source, self.remaining()?, self.cancelled)
        } else {
            let limit = self
                .remaining()?
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
    pub all_limit_bytes: u64,
    pub disk_usage_bytes: u64,
    pub transport: detail::Status,
    pub status: AssetStatus,
    pub active: Option<DownloadPack>,
    pub last_error: Option<String>,
    pub solar_errors: Option<Vec<String>>,
    progress: Arc<Mutex<DownloadProgress>>,
    cancelled: Arc<AtomicBool>,
    job: Option<Task<Result<DownloadResult, String>>>,
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
            all_limit_bytes: config.geography.max_assets_bytes,
            disk_usage_bytes,
            transport: detail::Status::default(),
            status,
            active: None,
            last_error: None,
            solar_errors: None,
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
        self.start_planned(pack, config, None);
    }

    pub fn start_viewfinder(&mut self, config: &AppConfig, plan: viewfinder::Plan) {
        self.start_planned(DownloadPack::Viewfinder, config, Some(plan));
    }

    fn start_planned(
        &mut self,
        pack: DownloadPack,
        config: &AppConfig,
        plan: Option<viewfinder::Plan>,
    ) {
        if self.active.is_some() {
            return;
        }
        self.last_error = None;
        if pack == DownloadPack::SolarSystem {
            self.solar_errors = None;
        }
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
        // Asset readers can show already installed globes during a long download.
        static DOWNLOAD_WORKERS: LazyLock<BackgroundJobs> =
            LazyLock::new(|| BackgroundJobs::new(1, 2));
        let job = Task::start_unbounded(&DOWNLOAD_WORKERS, move |cancelled| {
            let root = AssetDirectory::create(&assets_dir).map_err(|e| e.to_string())?;
            let _owner = download_owner(&root)?;
            let all_ceiling = (pack == DownloadPack::All)
                .then(|| all_ceiling(&root, config.geography.max_assets_bytes))
                .transpose()?;
            let session = DownloadSession {
                root: &root,
                config: &config,
                cancelled,
                progress: &progress,
                viewfinder: plan.as_ref(),
                all_ceiling,
            };
            if pack == DownloadPack::SolarSystem {
                return session.solar();
            }
            if matches!(pack, DownloadPack::World(_) | DownloadPack::MoonRelief) {
                session.run(pack, 0.0, 0.98)?;
                return Ok(DownloadResult::World {
                    disk_usage_bytes: dir_size(&root.path()),
                    solar_errors: None,
                });
            }
            let queue: Vec<_> = if pack == DownloadPack::All {
                BASE_PACKS
                    .into_iter()
                    .filter(|p| !p.installed(&previous))
                    .collect()
            } else {
                vec![pack]
            };
            session
                .download(&queue, override_dir.as_deref())
                .map(DownloadResult::Earth)
        });
        self.cancelled = job.token.clone();
        self.job = Some(job);
    }

    pub fn poll(&mut self, ctx: &egui::Context) -> Option<Arc<BasemapAssets>> {
        if self.active.is_some() {
            ctx.request_repaint_after(Duration::from_millis(80));
        }
        let result = self.job.as_ref()?.poll()?;
        let solar = self.active.take() == Some(DownloadPack::SolarSystem);
        self.job = None;
        match result {
            Ok(Ok(DownloadResult::World {
                disk_usage_bytes,
                solar_errors,
            })) => {
                self.disk_usage_bytes = disk_usage_bytes;
                if solar {
                    self.solar_errors = solar_errors;
                }
                None
            }
            Ok(Ok(DownloadResult::Earth(outcome))) => {
                self.disk_usage_bytes = outcome.disk_usage_bytes;
                self.transport = outcome.transport;
                self.status = outcome.assets.status();
                self.last_error = (!outcome.errors.is_empty())
                    .then(|| format!("Unavailable map layers:\n{}", outcome.errors.join("\n")));
                Some(outcome.assets)
            }
            error => {
                let error = match error {
                    Ok(Err(error)) => error,
                    Err(error) => format!("Asset worker failed: {error:?}"),
                    Ok(Ok(_)) => unreachable!("handled completed assets"),
                };
                if solar {
                    self.solar_errors = Some(vec![error]);
                } else {
                    self.last_error = Some(error);
                }
                None
            }
        }
    }
}

// GUI packs and bulk CLI runs share one exclusive writer for the entire asset
// tree. The caller retains this guard until all accepted workers have drained.
fn download_owner(root: &AssetDirectory) -> Result<fs::File, String> {
    let owner = fs::File::open(root.path()).map_err(|e| e.to_string())?;
    owner
        .try_lock()
        .map_err(|_| "Another geography download owns this asset directory".to_string())?;
    Ok(owner)
}

fn cached_json<T: serde::de::DeserializeOwned>(
    directory: &AssetDirectory,
    name: &str,
    limit: usize,
) -> Option<T> {
    let file = directory.open_file(std::ffi::OsStr::new(name)).ok()?;
    if file.metadata().ok()?.len() > limit as u64 {
        return None;
    }
    serde_json::from_reader(file.take(limit as u64)).ok()
}

fn read_url(
    url: &str,
    args: &[String],
    limit: usize,
    cancel: &AtomicBool,
) -> Result<Vec<u8>, String> {
    let output = CommandRequest::new("curl")
        .args([
            "--disable",
            "--location",
            "--fail",
            "--silent",
            "--show-error",
            "--proto",
            "=https",
            "--connect-timeout",
            "15",
            "--max-time",
            "45",
        ])
        .args(args)
        .args(["--", url])
        .max_output_bytes(limit)
        .timeout(Duration::from_secs(50))
        .own_process_group()
        .die_with_parent()
        .run_cancellable(cancel)
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(format!(
            "Map metadata: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    if output.stdout.len() >= limit {
        return Err("Map metadata exceeds its limit".into());
    }
    Ok(output.stdout)
}

fn download_to_file(
    dir: &Path,
    filename: &str,
    url: &str,
    max_bytes: u64,
    cancelled: &AtomicBool,
    on_progress: impl Fn(f32, &str),
) -> Result<Downloaded, String> {
    download_with_request(
        &dir.join(filename),
        url,
        max_bytes,
        cancelled,
        |bytes| {
            if bytes == 0 {
                on_progress(0.02, &format!("Downloading {filename}…"));
            } else {
                let mb = bytes as f32 / (1024.0 * 1024.0);
                let approx = (mb / (mb + 80.0)).clamp(0.03, 0.95);
                on_progress(approx, &format!("Downloading {filename} ({mb:.1} MB)…"));
            }
        },
        CommandRequest::new("curl"),
        false,
    )
}

fn all_ceiling(root: &AssetDirectory, limit: u64) -> Result<u64, String> {
    let installed = dir_size(&root.path());
    if installed == u64::MAX {
        return Err("Could not measure installed assets for the Get all budget".into());
    }
    Ok(installed.saturating_add(limit))
}

fn free_space(root: &AssetDirectory) -> Result<u64, String> {
    let directory = fs::File::open(root.path()).map_err(|error| error.to_string())?;
    let stats = rustix::fs::fstatvfs(&directory).map_err(|error| error.to_string())?;
    Ok(stats.f_bavail.saturating_mul(stats.f_frsize))
}

fn require_space(root: &AssetDirectory, required: u64) -> Result<u64, String> {
    let available = free_space(root)?;
    if available < required {
        return Err(format!(
            "The geography filesystem needs {:.2} GiB more free space",
            (required - available) as f64 / (1u64 << 30) as f64,
        ));
    }
    Ok(available)
}

fn download_with_request(
    destination: &Path,
    url: &str,
    max_bytes: u64,
    cancelled: &AtomicBool,
    on_progress: impl Fn(u64),
    request: CommandRequest,
    transient: bool,
) -> Result<Downloaded, String> {
    let dir = destination.parent().ok_or("Missing asset directory")?;
    let filename = destination
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or("Invalid asset filename")?;

    if cancelled.load(Ordering::Relaxed) {
        return Err("Download cancelled".into());
    }
    let directory = AssetDirectory::create(dir).map_err(|e| e.to_string())?;
    let (stage, max_bytes) = DownloadStage::prepare(&directory, transient, max_bytes)?;
    let output_file = stage.file();
    let (request, fd) = request
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
        .inherit_handle(output_file)
        .map_err(|e| e.to_string())?;

    on_progress(0);
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
            let bytes = output_file.metadata().map_or(0, |m| m.len());
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
            on_progress(bytes);
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
    let downloaded_bytes = output_file.metadata().map_err(|e| e.to_string())?.len();
    if downloaded_bytes > max_bytes {
        return Err("Download exceeded the geography disk budget".into());
    }
    let (file, staging) = stage
        .finish(&directory, filename)
        .map_err(|e| e.to_string())?;
    on_progress(downloaded_bytes);
    let path = PathBuf::from(format!("/proc/self/fd/{}", file.as_raw_fd()));
    Ok(Downloaded {
        expansion_budget: max_bytes - downloaded_bytes,
        _staging: staging,
        file,
        directory,
        path,
    })
}

enum DownloadStage {
    Named(tempfile::NamedTempFile),
    Anonymous(fs::File),
}

impl DownloadStage {
    fn prepare(
        directory: &AssetDirectory,
        transient: bool,
        limit: u64,
    ) -> Result<(Self, u64), String> {
        let limit = limit.min(free_space(directory)?);
        let stage = if transient {
            // A force-quit must not leave orphaned raw Parquet downloads behind.
            tempfile::tempfile_in(directory.path()).map(Self::Anonymous)
        } else {
            directory.stage().map(Self::Named)
        }
        .map_err(|error| error.to_string())?;
        Ok((stage, limit))
    }

    fn file(&self) -> &fs::File {
        match self {
            Self::Named(stage) => stage.as_file(),
            Self::Anonymous(file) => file,
        }
    }

    fn finish(
        self,
        directory: &AssetDirectory,
        filename: &str,
    ) -> std::io::Result<(fs::File, Option<tempfile::NamedTempFile>)> {
        match self {
            Self::Anonymous(file) => Ok((file, None)),
            Self::Named(staged) if filename.to_ascii_lowercase().ends_with(".zip") => {
                Ok((staged.as_file().try_clone()?, Some(staged)))
            }
            Self::Named(staged) => directory
                .install(staged, std::ffi::OsStr::new(filename))
                .map(|file| (file, None)),
        }
    }
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

// Codec dispatch and bounded copying are independent of archive enumeration.
fn write_archive_payload(
    entry: &mut impl Read,
    staged: &mut impl Write,
    name: &str,
    size: u64,
    remaining: &mut u64,
    cancelled: &AtomicBool,
) -> Result<String, String> {
    if name.to_ascii_lowercase().ends_with(".hgt") {
        little_clip_core::geography::basemap::hgt::encode(entry, staged, size, cancelled)
            .map_err(|e| e.to_string())?;
        *remaining -= size;
        return Ok(format!("{name}.zst"));
    }
    let mut buffer = [0; 64 * 1024];
    loop {
        if cancelled.load(Ordering::Relaxed) {
            return Err("Extraction cancelled".into());
        }
        let count = entry.read(&mut buffer).map_err(|e| e.to_string())?;
        if count == 0 {
            break;
        }
        *remaining = remaining
            .checked_sub(count as u64)
            .ok_or("Expanded archive exceeds the geography disk budget")?;
        staged
            .write_all(&buffer[..count])
            .map_err(|e| e.to_string())?;
    }
    Ok(name.to_owned())
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
    let mut extracted = 0;
    for i in 0..archive.len() {
        if cancelled.load(Ordering::Relaxed) {
            return Err("Extraction cancelled".into());
        }
        let mut entry = archive.by_index(i).map_err(|error| error.to_string())?;
        if entry.is_dir() {
            continue;
        }
        let member_name = entry.name().map_err(|error| error.to_string())?;
        let Some(name) = Path::new(member_name.as_ref())
            .file_name()
            .and_then(|n| n.to_str())
            .filter(|name| {
                let lower = name.to_ascii_lowercase();
                suffixes.iter().any(|suffix| lower.ends_with(suffix))
            })
            .map(str::to_owned)
        else {
            continue;
        };
        let entry_size = entry.size();
        if entry_size > remaining {
            return Err("Expanded archive exceeds the geography disk budget".into());
        }
        let mut staged = zip_path.directory.stage().map_err(|e| e.to_string())?;
        let mut payload_budget = remaining;
        let target_name = write_archive_payload(
            &mut entry,
            &mut staged,
            &name,
            entry_size,
            &mut payload_budget,
            cancelled,
        )?;
        remaining = remaining
            .checked_sub(
                staged
                    .as_file()
                    .metadata()
                    .map_err(|e| e.to_string())?
                    .len(),
            )
            .ok_or("Expanded archive exceeds the geography disk budget")?;
        if cancelled.load(Ordering::Relaxed) {
            return Err("Extraction cancelled".into());
        }
        extracted += 1;
        zip_path
            .directory
            .install(staged, std::ffi::OsStr::new(&target_name))
            .map_err(|e| e.to_string())?;
    }
    if extracted == 0 {
        return Err("Archive contains no supported map assets".into());
    }
    Ok(())
}

// Conservative accounting: incomplete or protected inventories disable admission.
// Symlinks are never traversed and depth/entry limits prevent cycles or huge scans.
pub(super) fn dir_size(dir: &Path) -> u64 {
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

    #[test]
    fn transient_transfer_reports_exact_bytes_without_publishing_a_raw_file() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("fixture.parquet");
        let destination = temp.path().join("overture-part.parquet");
        let bytes = 73 * 1024;
        fs::write(&source, vec![0_u8; bytes]).unwrap();
        let reported = Mutex::new(Vec::new());
        let downloaded = download_with_request(
            &destination,
            &format!("file://{}", source.display()),
            bytes as u64,
            &AtomicBool::new(false),
            |bytes| reported.lock().unwrap().push(bytes),
            CommandRequest::new("curl"),
            true,
        )
        .unwrap();
        let reported = reported.into_inner().unwrap();
        assert_eq!(reported.first(), Some(&0));
        assert_eq!(reported.last(), Some(&(bytes as u64)));
        assert_eq!(downloaded.file.metadata().unwrap().len(), bytes as u64);
        assert!(!destination.exists());
    }

    #[test]
    fn get_all_caps_additional_space_and_individual_packs_use_available_space() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("line.geojson");
        let payload = br#"{"type":"FeatureCollection","features":[]}"#;
        fs::write(&source, payload).unwrap();

        let mut config = AppConfig::default();
        config.geography.max_assets_bytes = payload.len() as u64;
        let url = format!("file://{}", source.display());
        config.geography.countries_url = Some(url.clone());
        config.geography.states_url = Some(url);
        let root = AssetDirectory::create(&temp.path().join("assets")).unwrap();
        root.write(OsStr::new("existing-optional-pack"), &[0; 1024])
            .unwrap();

        let ceiling = all_ceiling(&root, config.geography.max_assets_bytes).unwrap();
        let cancelled = AtomicBool::new(false);
        let progress = Mutex::new(DownloadProgress {
            fraction: 0.0,
            message: String::new(),
        });
        let mut session = DownloadSession {
            root: &root,
            config: &config,
            cancelled: &cancelled,
            progress: &progress,
            viewfinder: None,
            all_ceiling: Some(ceiling),
        };
        let countries = OsStr::new("ne_10m_admin_0_boundary_lines_land.geojson");
        let states = OsStr::new("ne_10m_admin_1_states_provinces_lines.geojson");

        assert_eq!(session.remaining().unwrap(), payload.len() as u64);
        assert!(session.run(DownloadPack::Borders, 0.0, 1.0).is_err());
        assert_eq!(
            root.open_file(countries).unwrap().metadata().unwrap().len(),
            payload.len() as u64
        );
        assert!(!root.path().join(states).exists());

        session.all_ceiling = None;
        session.run(DownloadPack::Borders, 0.0, 1.0).unwrap();
        assert_eq!(
            root.open_file(states).unwrap().metadata().unwrap().len(),
            payload.len() as u64
        );
    }

    #[test]
    fn solar_plan_covers_every_available_world_and_shared_sky_pack() {
        use std::collections::HashSet;
        let planned: HashSet<_> = solar_packs()
            .iter()
            .filter_map(|(pack, _)| match pack {
                DownloadPack::World(id) => Some(*id),
                _ => None,
            })
            .collect();
        let expected: HashSet<_> = solar::assets::bulk_worlds().map(|(id, _)| id).collect();
        assert_eq!(planned, expected);
        assert_eq!(planned.len(), solar_packs().len() - 3);
        assert!(planned.contains(&399) && planned.contains(&599) && planned.contains(&505));
        assert!(
            !planned.contains(&301),
            "the sky pack already owns the lunar surface map"
        );
        assert!(
            solar_download_bytes() > lunar_terrain::BYTES && solar_download_bytes() < 4_000_000_000
        );
    }

    #[test]
    fn checked_bulk_assets_continue_after_a_missing_source_and_retry_only_missing_files() {
        let temp = tempfile::tempdir().unwrap();
        let root = AssetDirectory::create(&temp.path().join("assets")).unwrap();
        let config = AppConfig::default();
        let cancelled = AtomicBool::new(false);
        let progress = Mutex::new(DownloadProgress {
            fraction: 0.0,
            message: String::new(),
        });
        let session = DownloadSession {
            root: &root,
            config: &config,
            cancelled: &cancelled,
            progress: &progress,
            viewfinder: None,
            all_ceiling: None,
        };
        let first = temp.path().join("first-source");
        let second = temp.path().join("second-source");
        fs::write(&second, b"map").unwrap();
        let first_url = format!("file://{}", first.display());
        let second_url = format!("file://{}", second.display());
        let run = || {
            session.download_checked_files(
                Path::new("worlds-v1"),
                [
                    ("first", first_url.as_str(), 3..=3),
                    ("second", second_url.as_str(), 3..=3),
                ],
                |_, _| true,
                0.0,
                1.0,
            )
        };
        assert!(run().unwrap_err().contains("first"));
        let installed = root.path().join("worlds-v1");
        assert_eq!(fs::read(installed.join("second")).unwrap(), b"map");
        fs::remove_file(second).unwrap();
        fs::write(first, b"dem").unwrap();
        run().unwrap();
        assert_eq!(fs::read(installed.join("first")).unwrap(), b"dem");
    }

    #[test]
    fn sky_pack_installs_black_marble_and_only_transfers_it_for_an_existing_pack() {
        let temp = tempfile::tempdir().unwrap();
        let root = AssetDirectory::create(&temp.path().join("assets")).unwrap();
        let config = AppConfig::default();
        let cancelled = AtomicBool::new(false);
        let progress = Mutex::new(DownloadProgress {
            fraction: 0.0,
            message: String::new(),
        });
        let session = DownloadSession {
            root: &root,
            config: &config,
            cancelled: &cancelled,
            progress: &progress,
            viewfinder: None,
            all_ceiling: None,
        };
        let mut files = DownloadPack::Sky.files(&config);
        let urls: Vec<_> = files
            .iter()
            .map(|file| {
                let path = temp.path().join(file.name);
                let size = *sky::asset_size(file.name).unwrap().start();
                fs::File::create(&path).unwrap().set_len(size).unwrap();
                format!("file://{}", path.display())
            })
            .collect();
        for (file, url) in files.iter_mut().zip(&urls) {
            file.url = url;
        }
        let installed = root.path().join(sky::DIRECTORY);
        session.download_sky(files, false, 0.0, 1.0).unwrap();
        for name in [sky::KERNEL, sky::EOP, sky::STARS, sky::MOON, sky::NIGHT] {
            let size = fs::metadata(installed.join(name)).unwrap().len();
            assert!(sky::asset_size(name).unwrap().contains(&size));
        }

        // An earlier install has the original four assets. If any is
        // transferred again this run will fail because its source is gone.
        fs::remove_file(installed.join(sky::NIGHT)).unwrap();
        let mut files = DownloadPack::Sky.files(&config);
        for (file, url) in files.iter_mut().zip(&urls) {
            file.url = url;
            if file.name != sky::NIGHT {
                fs::remove_file(temp.path().join(file.name)).unwrap();
            }
        }
        session.download_sky(files, false, 0.0, 1.0).unwrap();
        assert_eq!(
            fs::metadata(installed.join(sky::NIGHT)).unwrap().len(),
            sky::NIGHT_BYTES as u64
        );
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
            viewfinder: None,
            all_ceiling: None,
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
        downloader.job = Some(Task::start_unbounded(&pool, move |_| {
            Ok(DownloadResult::Earth(outcome))
        }));
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
    fn terrain_extraction_stores_a_lossless_compressed_frame() {
        use little_clip_core::geography::basemap::{hgt, storage};
        let root = tempfile::tempdir().unwrap();
        let directory = AssetDirectory::open(root.path()).unwrap();
        let raw = [10_i16, -100, i16::MIN, 9000]
            .into_iter()
            .flat_map(i16::to_be_bytes)
            .collect::<Vec<_>>();
        let mut staged = directory.stage().unwrap();
        {
            let mut zip = zip::ZipWriter::new(staged.as_file_mut());
            zip.start_file("N00E000.hgt", zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(&raw).unwrap();
            zip.finish().unwrap();
        }
        let file = staged.as_file().try_clone().unwrap();
        let downloaded = Downloaded {
            path: PathBuf::from(format!("/proc/self/fd/{}", file.as_raw_fd())),
            file,
            expansion_budget: 1024,
            _staging: Some(staged),
            directory,
        };
        extract_matching_from_zip(&downloaded, &AtomicBool::new(false), &[".hgt"]).unwrap();
        assert!(!root.path().join("N00E000.hgt").exists());
        let encoded = storage::read(root.path().join("N00E000.hgt.zst")).unwrap();
        assert_eq!(hgt::decode(encoded, true).unwrap(), raw);
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
