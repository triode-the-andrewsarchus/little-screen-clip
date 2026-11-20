//! The globe's local ephemeris and inertial sky assets.
mod orientation;
pub(super) use orientation::RotationSource;

use anise::{
    almanac::Almanac, astro::Aberration, constants::frames::*, frames::Frame, time::Epoch,
};
use chrono::{DateTime, Utc};
use image::{ImageFormat, ImageReader};
use little_clip_core::file_source::read_file_snapshot;
use orientation::Orientation;
use std::{io::Cursor, path::Path, sync::Arc};

pub(crate) const DIRECTORY: &str = "sky-v1";
pub(crate) const KERNEL: &str = "de440.bsp";
pub(crate) const EOP: &str = "finals2000A.all";
pub(crate) const STARS: &str = "starmap_2020_4k.exr";
pub(crate) const MOON: &str = "lroc_color_poles_2k.tif";
pub(crate) const NIGHT: &str = "BlackMarble_2016_01deg.jpg";
pub(crate) const KERNEL_BYTES: usize = 119_799_808;
const STARS_BYTES: usize = 35_997_085;
const MOON_BYTES: usize = 3_339_438;
pub(crate) const NIGHT_BYTES: usize = 779_638;
pub(crate) const PACK_BYTES: u64 = (KERNEL_BYTES + STARS_BYTES + MOON_BYTES + 5_000_000) as u64;

const PLANETS: [Frame; 7] = [
    MERCURY_J2000,
    VENUS_J2000,
    MARS_BARYCENTER_J2000,
    JUPITER_BARYCENTER_J2000,
    SATURN_BARYCENTER_J2000,
    URANUS_BARYCENTER_J2000,
    NEPTUNE_BARYCENTER_J2000,
];

#[derive(Clone)]
pub(super) struct Texture {
    pub size: [i32; 2],
    pub pixels: Arc<[u8]>,
}

pub(super) struct Assets {
    almanac: Almanac,
    domain: (Epoch, Epoch),
    orientation: Orientation,
    pub stars: Texture,
    pub _moon: Option<Texture>,
    pub night: Option<Texture>,
}

#[derive(Clone)]
pub(super) struct Scene {
    // Rows of ICRF → ITRF, also columns of the inverse used by the sky shader.
    pub inertial_rows: [[f32; 3]; 3],
    pub sun: [f32; 3],
    pub moon: [f32; 3],
    pub planets: [[f32; 3]; 7],
    pub rotation_source: RotationSource,
    pub assets: Arc<Assets>,
}

impl Assets {
    pub fn load(root: &Path) -> Result<Self, String> {
        let dir = root.join(DIRECTORY);
        let kernel = read_file_snapshot(&dir.join(KERNEL), KERNEL_BYTES)?;
        if kernel.len() != KERNEL_BYTES || !kernel.starts_with(b"DAF/SPK") {
            return Err("Expected the stock JPL DE440 kernel".into());
        }
        let almanac = Almanac::default()
            .load_from_bytes(
                bytes::Bytes::from(kernel)
                    .try_into_mut()
                    .map_err(|_| "Could not own DE440 kernel bytes")?,
            )
            .map_err(|error| error.to_string())?;
        let domain = almanac.spk_domain(301).map_err(|error| error.to_string())?;
        let orientation = read_file_snapshot(&dir.join(EOP), 5_000_000)
            .ok()
            .and_then(|bytes| String::from_utf8(bytes).ok())
            .map_or_else(Orientation::default, |text| Orientation::parse(&text));
        Ok(Self {
            almanac,
            domain,
            orientation,
            stars: read_texture(&dir, STARS, STARS_BYTES, [4096, 2048], ImageFormat::OpenExr)?,
            _moon: read_texture(&dir, MOON, MOON_BYTES, [2048, 1024], ImageFormat::Tiff).ok(),
            night: read_texture(&dir, NIGHT, NIGHT_BYTES, [3600, 1800], ImageFormat::Jpeg).ok(),
        })
    }

    pub fn at(self: &Arc<Self>, time: DateTime<Utc>) -> Option<Scene> {
        let (epoch, matrix, rotation_source) = self.orientation.at(time);
        if epoch < self.domain.0 || epoch > self.domain.1 {
            return None;
        }
        let body = |target| {
            let state = self
                .almanac
                .translate(target, EARTH_J2000, epoch, Aberration::CN_S)
                .ok()?;
            let inertial = state.radius_km.into();
            Some(erfa::vectors_and_matrices::mat_mul_pvec(matrix, inertial).map(|v| v as f32))
        };
        Some(Scene {
            inertial_rows: matrix.map(|row| row.map(|v| v as f32)),
            sun: body(SUN_J2000)?,
            moon: body(MOON_J2000)?,
            planets: PLANETS.map(|planet| body(planet).unwrap_or([0.0; 3])),
            rotation_source,
            assets: self.clone(),
        })
    }
}

fn read_texture(
    dir: &Path,
    name: &str,
    max_bytes: usize,
    size: [i32; 2],
    format: ImageFormat,
) -> Result<Texture, String> {
    let bytes = read_file_snapshot(&dir.join(name), max_bytes)?;
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(size[0] as u32);
    limits.max_image_height = Some(size[1] as u32);
    limits.max_alloc = Some(160 * 1024 * 1024);
    reader.limits(limits);
    let image = reader.decode().map_err(|error| error.to_string())?;
    if [image.width(), image.height()] != size.map(|v| v as u32) {
        return Err(format!("Unexpected dimensions for {name}"));
    }
    // The EXR is linear HDR. Tone-map once on the worker; preserve faint stars.
    let pixels = if format == ImageFormat::OpenExr {
        image
            .into_rgb32f()
            .into_raw()
            .into_iter()
            .map(|v| {
                let linear = (v.max(0.0) * 1.4).min(1.0);
                (255.0 * linear.powf(1.0 / 2.2)).round() as u8
            })
            .collect()
    } else {
        image.into_rgb8().into_raw()
    };
    Ok(Texture {
        size,
        pixels: pixels.into(),
    })
}
