//! Pinned Stellarium Modern figures and HYG J2000 stars, indexed off the UI thread.
use little_clip_core::file_source::read_file_snapshot;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    io::Read,
    path::Path,
};

pub(crate) const FIGURES: &str = "stellarium-modern-v25.4.json";
pub(crate) const HYG: &str = "hygdata_v41.csv";
pub(crate) const SOURCES: [(&str, &str, usize); 2] = [
    (
        FIGURES,
        "https://raw.githubusercontent.com/Stellarium/stellarium/e12516c74886c7c7c46260eebba4ef34bf4cb05f/skycultures/modern/index.json",
        205_767,
    ),
    (
        HYG,
        "https://raw.githubusercontent.com/astronexus/HYG-Database/c7f7f883fe678cc7680169a50ccd7dcc49b060ce/hyg/CURRENT/hygdata_v41.csv",
        33_932_548,
    ),
];
pub(crate) const DOWNLOAD_BYTES: usize = SOURCES[0].2 + SOURCES[1].2;
pub(in crate::file_manager::geography) const STELLARIUM_CREDIT: &str = "https://github.com/Stellarium/stellarium/blob/e12516c74886c7c7c46260eebba4ef34bf4cb05f/skycultures/modern/description.md#license";
pub(in crate::file_manager::geography) const HYG_CREDIT: &str = "https://github.com/astronexus/HYG-Database/tree/c7f7f883fe678cc7680169a50ccd7dcc49b060ce/hyg/CURRENT";

type Direction = [f32; 3];
type Cell = [i8; 3];

pub(crate) fn verified(name: &str, data: impl Read) -> bool {
    let expected = match name {
        FIGURES => "9ed2143eb38767e9e6f19ed06a330ee2e164dfe5d350b2a5bd917b7d68b78374",
        HYG => "d9f69fd86bbf90a4e4d52b4c5c53eacfa6dfc0bfdef85bfd94f095e0bebe4ebd",
        _ => return false,
    };
    let Some(&(_, _, len)) = SOURCES.iter().find(|(file, _, _)| *file == name) else {
        return false;
    };
    let mut digest = Sha256::new();
    std::io::copy(&mut data.take((len + 1) as u64), &mut digest).ok() == Some(len as u64)
        && format!("{:x}", digest.finalize()) == expected
}

#[derive(Debug)]
pub(in crate::file_manager::geography) struct Star {
    pub direction: Direction,
    pub name: Box<str>,
    pub hip: Option<u32>,
    pub magnitude: f32,
}

// Cartesian cells on the unit sphere avoid singularities at the poles and at RA 0h.
fn cell(direction: Direction) -> Cell {
    direction.map(|v| (v * 32.0).floor() as i8)
}

pub(in crate::file_manager::geography) struct Catalog {
    pub figures: Vec<Direction>,
    stars: HashMap<Cell, Vec<Star>>,
}

#[derive(Deserialize)]
struct Culture {
    constellations: Vec<Figure>,
}

#[derive(Deserialize)]
struct Figure {
    lines: Vec<Vec<u32>>,
}

#[derive(Deserialize)]
struct Record<'a> {
    id: u32,
    hip: Option<u32>,
    hd: Option<u32>,
    proper: &'a str,
    bayer: &'a str,
    con: &'a str,
    rarad: f64,
    decrad: f64,
    mag: f32,
}

impl Record<'_> {
    fn direction(&self) -> Result<Direction, String> {
        if !self.rarad.is_finite() || !self.decrad.is_finite() || !self.mag.is_finite() {
            return Err(format!(
                "Non-finite coordinates or magnitude for HYG {}",
                self.id
            ));
        }
        let (sin_ra, cos_ra) = self.rarad.sin_cos();
        let (sin_dec, cos_dec) = self.decrad.sin_cos();
        Ok([cos_ra * cos_dec, sin_ra * cos_dec, sin_dec].map(|v| v as f32))
    }

    fn name(&self) -> Box<str> {
        if !self.proper.is_empty() {
            return self.proper.into();
        }
        if !self.bayer.is_empty() && !self.con.is_empty() {
            return format!("{} {}", self.bayer, self.con).into();
        }
        match (self.hip, self.hd) {
            (Some(hip), _) => format!("HIP {hip}").into(),
            (_, Some(hd)) => format!("HD {hd}").into(),
            _ => format!("HYG {}", self.id).into(),
        }
    }
}

impl Catalog {
    pub(super) fn load(dir: &Path) -> Result<Option<Self>, String> {
        if SOURCES.iter().any(|(name, _, _)| !dir.join(name).is_file()) {
            return Ok(None);
        }
        let bytes = |(name, _, size)| {
            let data = read_file_snapshot(&dir.join(name), size)?;
            if !verified(name, data.as_slice()) {
                return Err(format!("Unexpected source contents for {name}"));
            }
            Ok(data)
        };
        Self::parse(&bytes(SOURCES[0])?, &bytes(SOURCES[1])?).map(Some)
    }

    fn parse(figures: &[u8], hyg: &[u8]) -> Result<Self, String> {
        let culture: Culture = serde_json::from_slice(figures).map_err(|e| e.to_string())?;
        let polylines: Vec<_> = culture
            .constellations
            .into_iter()
            .flat_map(|figure| figure.lines)
            .collect();
        let endpoints: HashSet<_> = polylines.iter().flatten().copied().collect();
        let mut hip_directions = HashMap::with_capacity(endpoints.len());
        let mut stars: HashMap<Cell, Vec<Star>> = HashMap::new();
        let mut reader = csv::Reader::from_reader(hyg);
        let headers = reader.headers().map_err(|e| e.to_string())?.clone();
        let mut values = csv::StringRecord::new();
        while reader.read_record(&mut values).map_err(|e| e.to_string())? {
            let row: Record<'_> = values
                .deserialize(Some(&headers))
                .map_err(|e| e.to_string())?;
            let endpoint = row.hip.filter(|hip| endpoints.contains(hip));
            if row.id == 0 || (row.mag > 6.5 && endpoint.is_none()) {
                continue;
            }
            let direction = row.direction()?;
            if let Some(hip) = endpoint {
                hip_directions.insert(hip, direction);
            }
            stars.entry(cell(direction)).or_default().push(Star {
                direction,
                name: row.name(),
                hip: row.hip,
                magnitude: row.mag,
            });
        }

        let figures = polylines
            .iter()
            .flat_map(|line| line.windows(2))
            .flatten()
            .map(|hip| {
                hip_directions
                    .get(hip)
                    .copied()
                    .ok_or_else(|| format!("Missing constellation endpoint HIP {hip}"))
            })
            .collect::<Result<_, _>>()?;
        Ok(Self { figures, stars })
    }

    pub(in crate::file_manager::geography) fn nearby(
        &self,
        ray: Direction,
        radius: f32,
    ) -> impl Iterator<Item = &Star> {
        let [x0, y0, z0] = cell(ray.map(|v| (v - radius).max(-1.0)));
        let [x1, y1, z1] = cell(ray.map(|v| (v + radius).min(1.0)));
        (x0..=x1)
            .flat_map(move |x| {
                (y0..=y1)
                    .flat_map(move |y| (z0..=z1).filter_map(move |z| self.stars.get(&[x, y, z])))
            })
            .flatten()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_faint_stars_but_keeps_every_line_endpoint_and_falls_back_to_ids() {
        let figures = br#"{"constellations":[{"lines":[[1,2,3]]}]}"#;
        let hyg = b"id,hip,hd,proper,bayer,con,rarad,decrad,mag\n0,,,Sol,,,0,0,-26.7\n1,1,,Named star,,,0,0,0.1\n2,2,,,Bet,Ori,1,0,6.8\n3,3,,,,,0,1,2.5\n4,4,,,,,0,0,7.1\n5,,88,,,,0,0,6.2\n";
        let catalog = Catalog::parse(figures, hyg).unwrap();
        assert_eq!(catalog.figures.len(), 4);
        assert_eq!(catalog.figures[0], [1.0, 0.0, 0.0]);
        assert_eq!(catalog.figures[1], catalog.figures[2]);
        let names = catalog
            .stars
            .values()
            .flatten()
            .map(|star| star.name.as_ref())
            .collect::<HashSet<_>>();
        assert_eq!(
            names,
            HashSet::from(["Named star", "Bet Ori", "HIP 3", "HD 88"])
        );
        assert_eq!(catalog.nearby([1.0, 0.0, 0.0], 0.0).count(), 2);
    }

    #[test]
    #[ignore = "requires the pinned Stellarium and HYG files in LITTLE_CLIP_STAR_FIXTURE/sky-v1"]
    fn pinned_sources_contain_all_figures_and_visible_stars() {
        let root = std::env::var_os("LITTLE_CLIP_STAR_FIXTURE").expect("Set the fixture root");
        let dir = Path::new(&root).join(super::super::DIRECTORY);
        let catalog = Catalog::load(&dir).unwrap().unwrap();
        assert_eq!(catalog.figures.len(), 695 * 2);
        assert_eq!(catalog.stars.values().map(Vec::len).sum::<usize>(), 8_922);
        assert!(
            catalog
                .stars
                .values()
                .flatten()
                .any(|star| star.name.as_ref() == "Sirius" && star.hip == Some(32349))
        );
    }
}
