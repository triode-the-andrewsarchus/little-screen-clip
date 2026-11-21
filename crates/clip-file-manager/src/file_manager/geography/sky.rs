//! The globe's local ephemeris and inertial sky assets.
pub(crate) mod catalog;
mod orientation;
pub(super) use orientation::RotationSource;

use anise::{
    almanac::Almanac,
    astro::Aberration,
    constants::frames::*,
    frames::Frame,
    time::{Epoch, Unit},
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
const EOP_LIMIT: usize = 5_000_000;
pub(crate) const PACK_BYTES: u64 =
    (KERNEL_BYTES + STARS_BYTES + MOON_BYTES + EOP_LIMIT + NIGHT_BYTES) as u64;

pub(crate) fn asset_size(name: &str) -> Option<std::ops::RangeInclusive<u64>> {
    let exact = match name {
        KERNEL => KERNEL_BYTES,
        STARS => STARS_BYTES,
        MOON => MOON_BYTES,
        NIGHT => NIGHT_BYTES,
        EOP => return Some(1_000_000..=EOP_LIMIT as u64),
        _ => {
            catalog::SOURCES
                .iter()
                .find(|(file, _, _)| *file == name)?
                .2
        }
    } as u64;
    Some(exact..=exact)
}

const PLANETS: [(&str, Frame); 7] = [
    ("Mercury", MERCURY_J2000),
    ("Venus", VENUS_J2000),
    ("Mars", MARS_BARYCENTER_J2000),
    ("Jupiter", JUPITER_BARYCENTER_J2000),
    ("Saturn", SATURN_BARYCENTER_J2000),
    ("Uranus", URANUS_BARYCENTER_J2000),
    ("Neptune", NEPTUNE_BARYCENTER_J2000),
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
    pub moon: Option<Texture>,
    pub night: Option<Texture>,
    pub catalog: Result<Option<catalog::Catalog>, String>,
}

#[derive(Clone)]
pub(super) struct Scene {
    // Rows of ICRF → ITRF, also columns of the inverse used by the sky shader.
    pub inertial_rows: [[f32; 3]; 3],
    pub sun: [f32; 3],
    pub time: DateTime<Utc>,
    pub moon: [f32; 3],
    pub lunar_sun: [f32; 3],
    pub lunar_earth: [f32; 3],
    pub solar_possible: bool,
    pub planets: [[f32; 3]; 7],
    pub rotation_source: RotationSource,
    pub assets: Arc<Assets>,
}

impl Scene {
    pub(super) fn bodies(&self) -> impl Iterator<Item = (&'static str, [f32; 3])> {
        [("Sun", self.sun), ("Moon", self.moon)]
            .into_iter()
            .chain(PLANETS.into_iter().map(|(name, _)| name).zip(self.planets))
    }
}

impl Assets {
    pub(super) fn star_catalog(&self) -> Option<&catalog::Catalog> {
        self.catalog.as_ref().ok()?.as_ref()
    }

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
        let orientation = read_file_snapshot(&dir.join(EOP), EOP_LIMIT)
            .ok()
            .and_then(|bytes| String::from_utf8(bytes).ok())
            .map_or_else(Orientation::default, |text| Orientation::parse(&text));
        Ok(Self {
            almanac,
            domain,
            orientation,
            stars: read_texture(&dir, STARS, STARS_BYTES, [4096, 2048], ImageFormat::OpenExr)?,
            moon: read_texture(&dir, MOON, MOON_BYTES, [2048, 1024], ImageFormat::Tiff).ok(),
            night: read_texture(&dir, NIGHT, NIGHT_BYTES, [3600, 1800], ImageFormat::Jpeg).ok(),
            catalog: catalog::Catalog::load(&dir),
        })
    }

    pub fn at(self: &Arc<Self>, time: DateTime<Utc>) -> Option<Scene> {
        let (epoch, matrix, rotation_source) = self.orientation.at(time);
        if epoch < self.domain.0 || epoch > self.domain.1 {
            return None;
        }
        let body = |target, observer, time, corrections| {
            let state = self
                .almanac
                .translate(target, observer, time, corrections)
                .ok()?;
            let inertial = state.radius_km.into();
            Some(erfa::vectors_and_matrices::mat_mul_pvec(matrix, inertial).map(|v| v as f32))
        };
        let from_earth = |target| body(target, EARTH_J2000, epoch, Aberration::CN_S);
        let sun = from_earth(SUN_J2000)?;
        let moon = from_earth(MOON_J2000)?;
        let lunar_epoch = epoch - Unit::Second * (norm(moon) as f64 / 299_792.458);
        let from_moon = |target| body(target, MOON_J2000, lunar_epoch, Aberration::CN);
        let lunar_sun = from_moon(SUN_J2000)?;
        let lunar_earth = from_moon(EARTH_J2000)?;
        let dot: f32 = sun.iter().zip(moon).map(|(s, m)| s * m).sum();
        Some(Scene {
            time,
            inertial_rows: matrix.map(|row| row.map(|v| v as f32)),
            sun,
            moon,
            lunar_sun,
            lunar_earth,
            solar_possible: dot / (norm(sun) * norm(moon)) > 0.9993,
            planets: PLANETS.map(|(_, planet)| from_earth(planet).unwrap_or([0.0; 3])),
            rotation_source,
            assets: self.clone(),
        })
    }
}

fn norm(vector: [f32; 3]) -> f32 {
    vector.iter().map(|value| value * value).sum::<f32>().sqrt()
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

#[cfg(test)]
mod tests {
    use super::*;

    fn angular(sun: [f64; 3], blocker: [f64; 3], radius: f64) -> [f64; 3] {
        [
            erfa::separation::sep_vectors(sun, blocker),
            (695_700.0 / erfa::vectors_and_matrices::modulus(sun)).asin(),
            (radius / erfa::vectors_and_matrices::modulus(blocker)).asin(),
        ]
    }

    fn at_surface(scene: &Scene, lat: f64, lon: f64) -> [f64; 3] {
        let p = erfa::transform::geodetic_to_geocentric(
            erfa::Ellipsoid::WGS84,
            lon.to_radians(),
            lat.to_radians(),
            0.0,
        );
        let p = p.unwrap();
        let offset =
            |position: [f32; 3]| std::array::from_fn(|i| f64::from(position[i]) - p[i] / 1000.0);
        angular(offset(scene.sun), offset(scene.moon), 1737.4)
    }

    #[test]
    #[ignore = "requires the stock NASA sky files in LITTLE_CLIP_SKY_FIXTURE/sky-v1"]
    fn de440_matches_published_eclipses_and_near_far_full_moons() {
        let root = std::env::var_os("LITTLE_CLIP_SKY_FIXTURE").expect("Set the fixture root");
        let assets = Arc::new(Assets::load(Path::new(&root)).unwrap());
        assert!(assets.moon.is_some(), "NASA albedo decodes");
        assert!(assets.night.is_some(), "Black Marble decodes");
        let at = |text: &str| assets.at(text.parse().unwrap()).unwrap();

        // NASA GSFC greatest-eclipse geographic points and UT times.
        let total = at("2024-04-08T18:17:16Z");
        assert_eq!(total.rotation_source, RotationSource::Observed);
        assert!(total.solar_possible);
        let [offset, sun, moon] = at_surface(&total, 25.29, -104.13);
        assert!(offset + sun < moon, "total: {offset} + {sun} < {moon}");
        let [offset, sun, moon] = at_surface(&total, 40.0, -104.13);
        assert!(
            (sun - moon).abs() < offset && offset < sun + moon,
            "penumbra"
        );

        let annular = at("2023-10-14T17:59:30Z");
        assert!(annular.solar_possible);
        let [offset, sun, moon] = at_surface(&annular, 11.37, -83.10);
        assert!(offset + moon < sun, "annular: {offset} + {moon} < {sun}");

        let lunar = at("2022-11-08T10:59:11Z");
        assert!(!lunar.solar_possible);
        let [offset, sun, earth] = angular(
            lunar.lunar_sun.map(f64::from),
            lunar.lunar_earth.map(f64::from),
            6371.0,
        );
        let lunar_radius = (1737.4 / f64::from(norm(lunar.moon))).asin();
        assert!(
            offset + sun + lunar_radius < earth,
            "Moon entirely inside the umbra"
        );

        // NASA SVS Dial-a-Moon hourly geocentric distances and diameters.
        for (time, distance, diameter) in [
            ("2024-02-24T13:00:00Z", 405_931.0, 1765.6),
            ("2024-10-17T11:00:00Z", 357_353.0, 2005.6),
        ] {
            let km = f64::from(norm(at(time).moon));
            let arcseconds = (2.0 * (1737.4 / km).asin()).to_degrees() * 3600.0;
            assert!((km - distance).abs() < 15.0, "{time}: {km} km");
            assert!(
                (arcseconds - diameter).abs() < 0.4,
                "{time}: {arcseconds} arcsec"
            );
        }
        assert_eq!(
            at("1600-01-01T00:00:00Z").rotation_source,
            RotationSource::Modeled
        );
        assert!(assets.at("1549-01-01T00:00:00Z".parse().unwrap()).is_none());
        assert!(assets.at("2651-01-01T00:00:00Z".parse().unwrap()).is_none());
    }
}
