//! Offline orbital geometry and the solar system view.
pub(in crate::file_manager) mod assets;
mod coverage;
mod view;
pub(super) mod world;

pub(super) use view::{Selection, System, paint_stars};

use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::{f64::consts::TAU, sync::OnceLock};

pub(in crate::file_manager) type Id = i32;
pub(super) const AU_KM: f64 = 149_597_870.7;
pub(super) const SUN: Id = 10;
pub(super) const EARTH: Id = 399;
pub(super) const MOON: Id = 301;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Kind {
    Star,
    Planet,
    DwarfPlanet,
    Moon,
}

#[derive(Deserialize)]
pub(super) struct Body {
    pub id: Id,
    pub name: &'static str,
    pub parent: Option<Id>,
    /// Mean spherical radius in kilometers where one is available.
    pub radius: Option<f64>,
    pub orbit: Option<Orbit>,
    frame_pole: Option<[f64; 2]>,
}

#[derive(Clone, Copy, Deserialize)]
pub(super) struct Orbit(
    f64, // Semi-major axis (AU around the Sun, km around a parent).
    f64, // Eccentricity.
    f64, // Inclination.
    f64, // Longitude of the ascending node.
    f64, // Argument of periapsis.
    f64, // Mean anomaly at epoch.
    f64, // Period in days.
    f64, // Epoch, days since J2000.
);

impl Body {
    pub fn kind(&self) -> Kind {
        match self.parent {
            None => Kind::Star,
            Some(SUN) if matches!(self.id, 199 | 299 | 399 | 499 | 599 | 699 | 799 | 899) => {
                Kind::Planet
            }
            Some(SUN) => Kind::DwarfPlanet,
            Some(_) => Kind::Moon,
        }
    }

    pub fn color(&self) -> eframe::egui::Color32 {
        use eframe::egui::Color32;
        let [r, g, b] = match self.id {
            SUN => [255, 195, 103],
            199 => [186, 178, 171],
            299 => [222, 184, 123],
            EARTH => [118, 173, 248],
            499 => [222, 132, 98],
            599 => [212, 166, 137],
            699 => [225, 200, 151],
            799 => [139, 210, 218],
            899 => [115, 147, 230],
            999 => [206, 172, 154],
            _ if self.kind() == Kind::DwarfPlanet => [171, 185, 207],
            _ => [166, 179, 201],
        };
        Color32::from_rgb(r, g, b)
    }

    fn semimajor(&self) -> f64 {
        self.orbit.map_or(0.0, |orbit| orbit.0)
    }

    fn orbit_point(&self, anomaly: f64) -> [f64; 3] {
        let Some(Orbit(a, e, inclination, node, apsis, ..)) = self.orbit else {
            return [0.0; 3];
        };
        let (sin, cos) = anomaly.sin_cos();
        let (x, y) = (a * (cos - e), a * (1.0 - e * e).sqrt() * sin);
        let (sw, cw) = apsis.to_radians().sin_cos();
        let (si, ci) = inclination.to_radians().sin_cos();
        let (sn, cn) = node.to_radians().sin_cos();
        let (u, v) = (cw * x - sw * y, sw * x + cw * y);
        let (x, y, z) = (cn * u - sn * ci * v, sn * u + cn * ci * v, si * v);
        let Some([ra, dec]) = self.frame_pole else {
            return [x, y, z];
        };
        // The catalog gives pole coordinates in ICRF for Laplace-frame orbits.
        // Rotate from that plane and then from equatorial to ecliptic coordinates.
        let (sr, cr) = ra.to_radians().sin_cos();
        let (sd, cd) = dec.to_radians().sin_cos();
        let [x, y, z] = [
            -sr * x - cr * sd * y + cr * cd * z,
            cr * x - sr * sd * y + sr * cd * z,
            cd * y + sd * z,
        ];
        let (se, ce) = 23.43928_f64.to_radians().sin_cos();
        [x, ce * y + se * z, -se * y + ce * z]
    }

    fn at(&self, days: f64) -> [f64; 3] {
        let Some(Orbit(_, e, _, _, _, mean, period, epoch)) = self.orbit else {
            return [0.0; 3];
        };
        let mean =
            (mean.to_radians() + (days - epoch).rem_euclid(period) * TAU / period).rem_euclid(TAU);
        let mut anomaly = mean;
        for _ in 0..8 {
            anomaly -= (anomaly - e * anomaly.sin() - mean) / (1.0 - e * anomaly.cos());
        }
        self.orbit_point(anomaly)
    }
}

pub(super) fn scene_day(time: DateTime<Utc>) -> f64 {
    (time.timestamp_millis() as f64 * 0.001 - 946_728_000.0) / 86_400.0
}

pub(super) fn clock(time: DateTime<Utc>, animation_seconds: f64) -> [f32; 2] {
    [
        scene_day(time) as f32,
        animation_seconds.rem_euclid(4096.0) as f32,
    ]
}

fn position_au(body: &Body, day: f64) -> [f64; 3] {
    let primary = body.parent.and_then(self::body).filter(|p| p.id != SUN);
    primary.map_or_else(
        || body.at(day),
        |p| {
            let center = p.at(day);
            let relative = body.at(day);
            std::array::from_fn(|i| center[i] + relative[i] / AU_KM)
        },
    )
}

pub(super) fn sunlight(body: &Body, day: f64) -> ([f32; 3], f32) {
    let position = position_au(body, day);
    let distance = position.iter().map(|v| v * v).sum::<f64>().sqrt();
    if distance <= 0.0 {
        return ([0.811, 0.324, 0.487], 0.0);
    }
    let direction = position.map(|value| (-value / distance) as f32);
    // A compressed angular scale keeps the Sun recognizable beyond Jupiter.
    // It still shrinks monotonically with heliocentric distance.
    let radius = (0.068 / distance.powf(0.62)).min(0.22) as f32;
    (direction, radius)
}

pub(super) fn bodies() -> &'static [Body] {
    // Published JPL mean elements and physical parameters, captured 2026-11-20.
    // https://ssd.jpl.nasa.gov/planets/approx_pos.html
    // https://ssd.jpl.nasa.gov/sats/elem/
    // https://ssd.jpl.nasa.gov/sats/phys_par/
    // Dwarf planet orbits from https://ssd.jpl.nasa.gov/tools/sbdb_lookup.html
    // Additional dwarf satellites use the orbital estimates in arXiv:0903.4213,
    // arXiv:2009.13733 and doi:10.3847/2041-8205/825/1/L9.
    static CATALOG: OnceLock<Vec<Body>> = OnceLock::new();
    CATALOG.get_or_init(|| {
        serde_json::from_str(include_str!("solar/catalog.json"))
            .expect("the bundled orbital catalog passed validation")
    })
}

pub(super) fn body(id: Id) -> Option<&'static Body> {
    bodies().iter().find(|body| body.id == id)
}

pub(super) fn satellites(id: Id) -> impl Iterator<Item = &'static Body> + Clone {
    bodies().iter().filter(move |body| body.parent == Some(id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn apparent_sun_shrinks_from_mercury_to_neptune() {
        let sizes = [199, 399, 599, 899].map(|id| sunlight(body(id).unwrap(), 9_600.0).1);
        assert!(sizes.windows(2).all(|pair| pair[0] > pair[1]));
        assert!(sizes[3] > 0.005);
    }

    #[test]
    fn bundled_catalog_has_valid_orbits_and_unique_targets() {
        let mut seen = HashSet::new();
        assert_eq!(satellites(SUN).count(), 13);
        assert!(satellites(599).count() >= 100);
        assert!(satellites(699).count() >= 290);
        for body_entry in bodies() {
            assert!(seen.insert(body_entry.id), "{}", body_entry.name);
            if let Some(parent) = body_entry.parent {
                assert!(body(parent).is_some());
                let orbit = body_entry.orbit.unwrap();
                assert!(orbit.0 > 0.0 && (0.0..1.0).contains(&orbit.1) && orbit.6 > 0.0);
                let position = body_entry.at(10_000.0);
                let distance = position.iter().map(|x| x * x).sum::<f64>().sqrt();
                assert!((distance / orbit.0 - 1.0).abs() <= orbit.1 + 1e-9);
                let repeated = body_entry.at(10_000.0 + orbit.6);
                assert!(
                    position
                        .iter()
                        .zip(repeated)
                        .all(|(a, b)| (a - b).abs() <= 1e-7 * orbit.0)
                );
            }
        }
    }
}
