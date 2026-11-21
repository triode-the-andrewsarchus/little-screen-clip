//! Size-limited, independently downloadable assets for a selected solar-system body.
use super::{Body, Id, Orbit};
use crate::file_manager::geography::sky;
use little_clip_core::file_source::read_file_snapshot;
use serde::Deserialize;
use std::{
    collections::HashMap,
    fs::File,
    io::{Cursor, Read},
    ops::RangeInclusive,
    path::{Path, PathBuf},
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

pub(in crate::file_manager) const HORIZONS: &str = "https://ssd.jpl.nasa.gov/horizons/";
const EPHEMERIS_LIMIT: u64 = 128 * 1024;
// Horizons accepts at most 80 epochs in one URL-based TLIST request.
const EPHEMERIS_SAMPLES: usize = 80;

#[derive(Deserialize)]
pub(super) struct ImageAsset {
    id: Id,
    pub url: &'static str,
    pub source: &'static str,
    pub credit: &'static str,
    pub bytes: u64,
    size: [u32; 2],
    file: Option<&'static str>,
    legacy: Option<LegacyImage>,
}

#[derive(Deserialize)]
struct LegacyImage {
    size: [u32; 2],
    bytes: u64,
}

#[derive(Default)]
pub(super) struct Assets {
    pub texture: Option<sky::Texture>,
    pub ephemeris: Option<Vec<[f64; 3]>>,
    pub preferred: bool,
}

pub(in crate::file_manager) struct Download {
    pub folder: PathBuf,
    pub name: &'static str,
    pub url: String,
    pub size: RangeInclusive<u64>,
}

impl Download {
    pub fn for_body(id: Id) -> Option<Self> {
        let folder = PathBuf::from("worlds-v1").join(id.to_string());
        if let Some(image) = image(id) {
            let (folder, name) = if id == super::MOON {
                (PathBuf::from(sky::DIRECTORY), sky::MOON)
            } else {
                (folder, image.file.unwrap_or("image"))
            };
            return Some(Self {
                folder,
                name,
                url: image.url.to_owned(),
                size: image.bytes..=image.bytes,
            });
        }
        let (orbit, center) = orbital_pack(super::body(id)?)?;
        let start = 2_451_545.0 + orbit.7;
        let epochs = (0..EPHEMERIS_SAMPLES)
            .map(|i| {
                format!(
                    "{:.8}",
                    start + orbit.6 * i as f64 / (EPHEMERIS_SAMPLES - 1) as f64
                )
            })
            .collect::<Vec<_>>()
            .join("%20");
        // Horizons interprets an unqualified number as a major-body index.
        // Designation syntax selects a small body by its cataloged SPK ID.
        let target = if id > 2_000_000 {
            format!("DES%3D{id}%3B")
        } else {
            id.to_string()
        };
        let url = format!(
            "https://ssd.jpl.nasa.gov/api/horizons.api?format=json&COMMAND=%27{target}%27&CENTER=%27500%40{center}%27&EPHEM_TYPE=VECTORS&MAKE_EPHEM=YES&VEC_TABLE=1&CSV_FORMAT=YES&REF_PLANE=ECLIPTIC&OUT_UNITS=KM-S&TLIST_TYPE=JD&TLIST={epochs}"
        );
        Some(Self {
            folder,
            name: "horizons.json",
            url,
            size: 1..=EPHEMERIS_LIMIT,
        })
    }

    pub fn present(&self, root: &Path) -> bool {
        little_clip_core::kernel_interface::open_content(
            &root.join(&self.folder).join(self.name),
            false,
        )
        .ok()
        .is_some_and(|f| {
            f.metadata().is_ok_and(|m| self.size.contains(&m.len())) && verify(self.name, &f)
        })
    }
}

pub(in crate::file_manager) fn verify(name: &str, file: &File) -> bool {
    if name != "horizons.json" {
        return true;
    }
    let mut bytes = Vec::new();
    file.take(EPHEMERIS_LIMIT + 1)
        .read_to_end(&mut bytes)
        .is_ok()
        && ephemeris(&bytes).is_some()
}

pub(in crate::file_manager::geography) fn information(
    body: &Body,
) -> Option<(&'static str, &'static str, &'static str, u64)> {
    if let Some(image) = image(body.id) {
        return Some(("Surface map", image.credit, image.source, image.bytes));
    }
    orbital_pack(body)?;
    Some(("Orbital data", "JPL Horizons", HORIZONS, EPHEMERIS_LIMIT))
}

pub(in crate::file_manager) fn bulk_worlds() -> impl Iterator<Item = (Id, u64)> {
    images()
        .iter()
        .filter(|image| image.id != super::MOON)
        .map(|image| (image.id, image.bytes))
        .chain(
            super::bodies()
                .iter()
                .filter(|body| image(body.id).is_none() && orbital_pack(body).is_some())
                .map(|body| (body.id, EPHEMERIS_LIMIT)),
        )
}

pub(in crate::file_manager::geography) fn map_sources()
-> impl Iterator<Item = (&'static str, &'static str, &'static str)> {
    images()
        .iter()
        .filter_map(|image| Some((name(image.id)?, image.credit, image.source)))
}

pub(in crate::file_manager) fn name(id: Id) -> Option<&'static str> {
    super::body(id).map(|body| body.name)
}

fn orbital_pack(body: &Body) -> Option<(Orbit, Id)> {
    (body.id > 0).then_some(())?;
    body.orbit.zip(body.parent)
}

fn image(id: Id) -> Option<&'static ImageAsset> {
    images().iter().find(|entry| entry.id == id)
}

fn images() -> &'static [ImageAsset] {
    static SOURCES: OnceLock<Vec<ImageAsset>> = OnceLock::new();
    SOURCES.get_or_init(|| {
        serde_json::from_str(include_str!("images.json"))
            .expect("valid bundled world imagery manifest")
    })
}

/// Texture ownership follows the map session across its individual views.
/// Small maps stay resident and large maps share a byte-bounded LRU.
#[derive(Default)]
pub(in crate::file_manager::geography) struct MapCache {
    maps: Mutex<Maps>,
    replacement_epoch: AtomicU64,
}

#[derive(Default)]
struct Maps {
    previews: HashMap<Id, sky::Texture>,
    detailed: HashMap<Id, (sky::Texture, u64)>,
    sequence: u64,
    detailed_bytes: usize,
}

const DETAILED_BUDGET: usize = 128 * 1024 * 1024;

impl MapCache {
    pub fn refresh_installed(&self, changed: Option<Id>) {
        // Only a versioned replacement may supersede a currently usable map.
        // Existing textures for unchanged files keep their GPU identity.
        if let Ok(mut maps) = self.maps.lock() {
            let replaced = |image: &&ImageAsset| {
                image.legacy.is_some() && changed.is_none_or(|id| id == image.id)
            };
            for image in images().iter().filter(replaced) {
                self.replacement_epoch.fetch_add(1, Ordering::Relaxed);
                maps.previews.remove(&image.id);
                if let Some((map, _)) = maps.detailed.remove(&image.id) {
                    maps.detailed_bytes -= map.pixels.len();
                }
            }
        }
    }

    pub fn preview(&self, id: Id) -> Option<sky::Texture> {
        self.maps.lock().ok()?.previews.get(&id).cloned()
    }

    pub fn map(&self, id: Id, small: bool) -> Option<sky::Texture> {
        let mut maps = self.maps.lock().ok()?;
        if small {
            return maps.previews.get(&id).cloned();
        }
        maps.sequence += 1;
        let use_at = maps.sequence;
        let (map, used) = maps.detailed.get_mut(&id)?;
        *used = use_at;
        Some(map.clone())
    }

    fn store(&self, id: Id, small: bool, map: sky::Texture, epoch: Option<u64>) -> sky::Texture {
        let Ok(mut maps) = self.maps.lock() else {
            return map;
        };
        if epoch.is_some_and(|epoch| epoch != self.replacement_epoch.load(Ordering::Relaxed)) {
            return map;
        }
        if small {
            return maps.previews.entry(id).or_insert(map).clone();
        }
        maps.sequence += 1;
        let use_at = maps.sequence;
        if let Some((current, used)) = maps.detailed.get_mut(&id) {
            *used = use_at;
            return current.clone();
        }
        while maps.detailed_bytes + map.pixels.len() > DETAILED_BUDGET {
            let Some(oldest) = maps
                .detailed
                .iter()
                .min_by_key(|(_, (_, used))| *used)
                .map(|(id, _)| *id)
            else {
                return map;
            };
            if let Some((removed, _)) = maps.detailed.remove(&oldest) {
                maps.detailed_bytes -= removed.pixels.len();
            }
        }
        maps.detailed_bytes += map.pixels.len();
        maps.detailed.insert(id, (map.clone(), use_at));
        map
    }

    pub fn load_map(
        &self,
        root: &Path,
        id: Id,
        small: bool,
    ) -> Result<Option<sky::Texture>, String> {
        if let Some(map) = self.map(id, small) {
            return Ok(Some(map));
        }
        let Some(asset) = image(id) else {
            return Ok(None);
        };
        let Some(download) = Download::for_body(id) else {
            return Ok(None);
        };
        let epoch = asset
            .legacy
            .as_ref()
            .map(|_| self.replacement_epoch.load(Ordering::Relaxed));
        let Some(rgb) = decode_installed(root, asset, download)? else {
            return Ok(None);
        };
        let preview = self.store(id, true, thumbnail(&rgb), epoch);
        if small {
            return Ok(Some(preview));
        }
        Ok(Some(self.store(id, false, texture(rgb), epoch)))
    }
}

fn decode_installed(
    root: &Path,
    asset: &ImageAsset,
    download: Download,
) -> Result<Option<image::RgbImage>, String> {
    let preferred = download.present(root);
    let size = if preferred {
        (download.name, asset.bytes, asset.size)
    } else {
        let Some(legacy) = &asset.legacy else {
            return Ok(None);
        };
        ("image", legacy.bytes, legacy.size)
    };
    let path = root.join(download.folder).join(size.0);
    let bytes = match read_file_snapshot(&path, size.1 as usize) {
        Ok(bytes) if bytes.len() as u64 == size.1 => bytes,
        _ if !preferred => return Ok(None),
        _ => return Err("World image changed during read".to_owned()),
    };
    decode(bytes, asset.id, size.2).map(Some)
}

// Speculative image reads have their own single worker. They never take the
// foreground slot needed by the selected world or the Earth map.
pub(super) fn preview_workers() -> &'static little_clip_core::BackgroundJobs {
    static POOL: std::sync::LazyLock<little_clip_core::BackgroundJobs> =
        std::sync::LazyLock::new(|| little_clip_core::BackgroundJobs::new(1, 2));
    &POOL
}
pub(super) fn overview_maps(
    cache: &MapCache,
    root: &Path,
    earth: Option<&little_clip_core::geography::basemap::EquirectTexture>,
    cancel: &AtomicBool,
) {
    if let Some(earth) = earth {
        // The existing Natural Earth raster is also the overview's Earth albedo.
        let size = [512, 256];
        let pixels: Vec<_> = (0..size[1])
            .flat_map(|y| {
                (0..size[0]).flat_map(move |x| {
                    earth
                        .sample_rgb(
                            90.0 - 180.0 * f64::from(y) / f64::from(size[1] - 1),
                            -180.0 + 360.0 * f64::from(x) / f64::from(size[0]),
                        )
                        .into_iter()
                })
            })
            .collect();
        cache.store(
            super::EARTH,
            true,
            sky::Texture {
                size,
                pixels: pixels.into(),
            },
            None,
        );
    }
    for image in images()
        .iter()
        .take_while(|_| !cancel.load(Ordering::Relaxed))
    {
        cache.load_map(root, image.id, true).ok();
    }
}

impl Assets {
    pub fn load(cache: &MapCache, root: &Path, id: Id) -> Result<Option<Self>, String> {
        if let Some(asset) = image(id) {
            let preferred = asset.legacy.is_none()
                || Download::for_body(id).is_some_and(|download| download.present(root));
            return Ok(cache.load_map(root, id, false)?.map(|texture| Self {
                texture: Some(texture),
                ephemeris: None,
                preferred,
            }));
        }
        let Some(download) = Download::for_body(id) else {
            return Ok(None);
        };
        let path = root.join(&download.folder).join(download.name);
        if !download.present(root) {
            return Ok(None);
        }
        let bytes = read_file_snapshot(&path, *download.size.end() as usize)?;
        Ok(Some(Self {
            ephemeris: ephemeris(&bytes),
            preferred: true,
            ..Self::default()
        }))
    }
}

fn decode(bytes: Vec<u8>, id: Id, expected: [u32; 2]) -> Result<image::RgbImage, String> {
    let mut reader = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|error| error.to_string())?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(expected[0]);
    limits.max_image_height = Some(expected[1]);
    limits.max_alloc = Some(160 * 1024 * 1024);
    reader.limits(limits);
    let rgb = reader
        .decode()
        .map_err(|error| error.to_string())?
        .into_rgba8();
    if [rgb.width(), rgb.height()] != expected {
        return Err("Unexpected world image dimensions".into());
    }
    super::coverage::albedo(rgb, id).ok_or_else(|| "No observed world imagery".to_owned())
}

fn texture(rgb: image::RgbImage) -> sky::Texture {
    sky::Texture {
        size: [rgb.width() as i32, rgb.height() as i32],
        pixels: rgb.into_raw().into(),
    }
}

fn thumbnail(rgb: &image::RgbImage) -> sky::Texture {
    texture(image::imageops::resize(
        rgb,
        rgb.width().min(640),
        (rgb.height() * rgb.width().min(640) / rgb.width()).max(1),
        image::imageops::FilterType::Triangle,
    ))
}

fn ephemeris(bytes: &[u8]) -> Option<Vec<[f64; 3]>> {
    let response: serde_json::Value = serde_json::from_slice(bytes).ok()?;
    let text = response["result"].as_str()?;
    let rows = text
        .split_once("$$SOE")?
        .1
        .split_once("$$EOE")?
        .0
        .trim()
        .lines();
    let positions: Vec<[f64; 3]> = rows
        .map(|row| {
            let mut columns = row.split(',').skip(2);
            let mut xyz = [0.0_f64; 3];
            for axis in &mut xyz {
                *axis = columns.next()?.trim().parse().ok()?;
            }
            xyz.iter()
                .all(|coordinate| coordinate.is_finite())
                .then_some(xyz)
        })
        .collect::<Option<_>>()?;
    (positions.len() == EPHEMERIS_SAMPLES).then_some(positions)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overview_and_closeup_reuse_distinct_textures_after_an_unrelated_download() {
        let cache = MapCache::default();
        let texture = |color, width| sky::Texture {
            size: [width, 1],
            pixels: vec![color; 3 * width as usize].into(),
        };
        let thumbnail = cache.store(599, true, texture(26, 4), None);
        let detail = cache.store(599, false, texture(87, 8), None);
        let legacy = cache.store(801, true, texture(1, 4), Some(0));
        cache.refresh_installed(Some(801));
        assert!(cache.preview(801).is_none());
        cache.store(801, true, legacy, Some(0));
        assert!(cache.preview(801).is_none(), "a retired decode republished");
        assert!(std::sync::Arc::ptr_eq(
            &cache.preview(599).unwrap().pixels,
            &thumbnail.pixels
        ));
        assert!(std::sync::Arc::ptr_eq(
            &cache.map(599, false).unwrap().pixels,
            &detail.pixels
        ));
    }

    #[test]
    fn downloadable_datasets_match_catalog_ids_and_fit_their_budgets() {
        for body in super::super::bodies() {
            if body.id < 0 || body.id == super::super::SUN {
                assert!(Download::for_body(body.id).is_none());
                continue;
            }
            let download = Download::for_body(body.id).unwrap();
            assert!(*download.size.end() < 4_000_000_000);
            if let Some(image) = image(body.id) {
                assert!(image.size[0] <= 8192 && image.size[1] <= 4096);
            }
            assert!(download.url.starts_with("https://"));
        }
    }

    #[test]
    fn observation_only_targets_use_procedural_worlds_without_a_reference_image() {
        for id in [2_136_108, 2_136_199, 2_136_472, -1, -2, -3, -4] {
            assert!(image(id).is_none());
        }
        let haumea = Download::for_body(2_136_108).unwrap();
        assert!(haumea.url.contains("COMMAND=%27DES%3D2136108%3B%27"));
    }

    #[test]
    #[ignore = "uses live NASA/JPL image and Horizons endpoints"]
    fn downloads_a_world_map_and_orbit_then_reuses_both_offline() {
        use crate::file_manager::geography_downloader::{AssetDownloader, DownloadPack};
        use little_clip_core::config::AppConfig;
        use std::{
            os::unix::fs::MetadataExt,
            time::{Duration, Instant},
        };
        let root = tempfile::tempdir().unwrap();
        let mut config = AppConfig::default();
        config.geography.assets_dir = Some(root.path().to_owned());
        let mut downloader = AssetDownloader::new(&config);
        let ctx = egui::Context::default();
        let mut install = |id| {
            downloader.start(DownloadPack::World(id), &config);
            let deadline = Instant::now() + Duration::from_secs(25);
            while downloader.active.is_some() {
                downloader.poll(&ctx);
                assert!(Instant::now() < deadline, "World {id} download timed out");
                std::thread::sleep(Duration::from_millis(10));
            }
            assert_eq!(downloader.last_error, None);
        };
        for id in [699, 505] {
            install(id);
            let asset = Download::for_body(id).unwrap();
            let path = root.path().join(asset.folder).join(asset.name);
            let installed_inode = path.metadata().unwrap().ino();
            let data = Assets::load(&MapCache::default(), root.path(), id)
                .unwrap()
                .unwrap();
            if id == 699 {
                assert_eq!(data.texture.unwrap().size, [720, 360]);
            } else {
                assert_eq!(data.ephemeris.unwrap().len(), EPHEMERIS_SAMPLES);
            }
            install(id);
            assert_eq!(path.metadata().unwrap().ino(), installed_inode);
        }
    }
}
