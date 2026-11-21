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
        OnceLock,
        atomic::{AtomicBool, Ordering},
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
}

#[derive(Default)]
pub(super) struct Assets {
    pub texture: Option<sky::Texture>,
    pub ephemeris: Option<Vec<[f64; 3]>>,
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
                (folder, "image")
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

pub(super) fn overview_maps(
    root: &Path,
    earth: Option<&little_clip_core::geography::basemap::EquirectTexture>,
    cancel: &AtomicBool,
) -> HashMap<Id, sky::Texture> {
    let mut maps = HashMap::new();
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
        maps.insert(
            super::EARTH,
            sky::Texture {
                size,
                pixels: pixels.into(),
            },
        );
    }
    for image in images()
        .iter()
        .take_while(|_| !cancel.load(Ordering::Relaxed))
    {
        if maps.contains_key(&image.id) {
            continue;
        }
        if let Some(map) = load_installed_map(root, image, true) {
            maps.insert(image.id, map);
        }
    }
    maps
}

fn load_installed_map(root: &Path, image: &ImageAsset, thumbnail: bool) -> Option<sky::Texture> {
    let asset = Download::for_body(image.id)?;
    let path = root.join(asset.folder).join(asset.name);
    let bytes = read_file_snapshot(&path, image.bytes as usize).ok()?;
    (bytes.len() as u64 == image.bytes).then_some(())?;
    decode(bytes, image.size, thumbnail).ok()
}

pub(super) fn neighboring_maps(
    root: &Path,
    focused: &Body,
    cancel: &AtomicBool,
) -> HashMap<Id, sky::Texture> {
    images()
        .iter()
        .take_while(|_| !cancel.load(Ordering::Relaxed))
        .filter_map(|image| {
            let is_parent = focused.parent == Some(image.id);
            let is_moon = super::body(image.id).is_some_and(|body| body.parent == Some(focused.id));
            if !(is_parent || is_moon && focused.id != super::SUN) {
                return None;
            }
            Some((image.id, load_installed_map(root, image, !is_parent)?))
        })
        .collect()
}

impl Assets {
    pub fn load(root: &Path, id: Id) -> Result<Option<Self>, String> {
        let Some(download) = Download::for_body(id) else {
            return Ok(None);
        };
        let path = root.join(&download.folder).join(download.name);
        if !download.present(root) {
            return Ok(None);
        }
        let bytes = read_file_snapshot(&path, *download.size.end() as usize)?;
        let Some(asset) = image(id) else {
            return Ok(Some(Self {
                ephemeris: ephemeris(&bytes),
                ..Self::default()
            }));
        };
        Ok(Some(Self {
            texture: Some(decode(bytes, asset.size, false)?),
            ephemeris: None,
        }))
    }
}

fn decode(bytes: Vec<u8>, expected: [u32; 2], thumbnail: bool) -> Result<sky::Texture, String> {
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
        .into_rgb8();
    if [rgb.width(), rgb.height()] != expected {
        return Err("Unexpected world image dimensions".into());
    }
    let rgb = if thumbnail && rgb.width() > 640 {
        image::imageops::resize(
            &rgb,
            640,
            (rgb.height() * 640 / rgb.width()).max(1),
            image::imageops::FilterType::Triangle,
        )
    } else {
        rgb
    };
    Ok(sky::Texture {
        size: [rgb.width() as i32, rgb.height() as i32],
        pixels: rgb.into_raw().into(),
    })
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
            let data = Assets::load(root.path(), id).unwrap().unwrap();
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
