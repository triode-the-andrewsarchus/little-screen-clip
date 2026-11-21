//! Dated heliocentric positions from JPL Horizons vector tables.
use super::AU_KM;
use crate::file_source::ReadCheck;
use chrono::{DateTime, Utc};
use hifitime::{Epoch, TimeScale};
use std::{
    io::Read,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

const HEADER_BYTES: usize = 16 * 1024;
const MAX_BYTES: usize = 16 * 1024 * 1024;
const CANCELLED: &str = "Trajectory opening cancelled";

pub struct Trajectory {
    pub name: String,
    pub radius_au: f64,
    samples: Vec<Sample>,
}

struct Sample {
    time: DateTime<Utc>,
    au: [f64; 3],
}

impl Trajectory {
    pub fn is_candidate(path: &Path) -> bool {
        path.extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| {
                ["txt", "csv", "dat", "eph", "horizons"]
                    .iter()
                    .any(|suffix| ext.eq_ignore_ascii_case(suffix))
            })
    }

    pub fn read_if_identified(path: &Path, cancel: &AtomicBool) -> Result<Option<Self>, String> {
        if !Self::is_candidate(path) {
            return Ok(None);
        }
        let check_cancel = || {
            if cancel.load(Ordering::Acquire) {
                Err(CANCELLED.to_owned())
            } else {
                Ok(())
            }
        };
        check_cancel()?;
        let (mut file, source) = ReadCheck::default().open(path, None, cancel)?;
        let mut bytes = read_header(&mut file)?;
        if !Self::has_header(&String::from_utf8_lossy(&bytes)) {
            return Ok(None);
        }

        check_cancel()?;
        file.by_ref()
            .take((MAX_BYTES + 1 - bytes.len()) as u64)
            .read_to_end(&mut bytes)
            .map_err(|error| error.to_string())?;
        source.check_file(&file)?;
        if bytes.len() > MAX_BYTES {
            return Err("Horizons trajectory exceeds the 16 MiB opening limit".into());
        }
        let text = String::from_utf8_lossy(&bytes);
        Self::parse(&text, cancel).map(Some)
    }

    pub fn has_file_header(path: &Path, cancel: &AtomicBool) -> Result<bool, String> {
        if !Self::is_candidate(path) {
            return Ok(false);
        }
        let (mut file, source) = ReadCheck::default().open(path, None, cancel)?;
        let header = read_header(&mut file)?;
        source.check_file(&file)?;
        Ok(Self::has_header(&String::from_utf8_lossy(&header)))
    }

    fn has_header(header: &str) -> bool {
        header.contains("/ Horizons")
            && header.contains("Target body name:")
            && header.contains("cartesian states")
    }

    fn parse(text: &str, cancel: &AtomicBool) -> Result<Self, String> {
        let (header, data) = text
            .split_once("$$SOE")
            .and_then(|(header, data)| data.split_once("$$EOE").map(|(data, _)| (header, data)))
            .ok_or("Horizons vector table is missing its start or end marker")?;
        let (metadata, name) = Metadata::parse(header)?;
        let samples = metadata.samples(data, cancel)?;
        let radius_au = samples
            .iter()
            .map(|sample| sample.au.into_iter().map(|axis| axis * axis).sum::<f64>())
            .fold(0.0, f64::max)
            .sqrt();
        Ok(Self {
            name,
            radius_au,
            samples,
        })
    }

    pub fn range(&self) -> [DateTime<Utc>; 2] {
        [
            self.samples[0].time,
            self.samples[self.samples.len() - 1].time,
        ]
    }

    pub fn positions(&self) -> impl Iterator<Item = [f64; 3]> + '_ {
        self.samples.iter().map(|sample| sample.au)
    }

    pub fn position_at(&self, time: DateTime<Utc>) -> Option<[f64; 3]> {
        match self
            .samples
            .binary_search_by_key(&time, |sample| sample.time)
        {
            Ok(index) => Some(self.samples[index].au),
            Err(index) => {
                let next = self.samples.get(index)?;
                let previous = self.samples.get(index.checked_sub(1)?)?;
                let mix = (time - previous.time).as_seconds_f64()
                    / (next.time - previous.time).as_seconds_f64();
                Some(std::array::from_fn(|axis| {
                    previous.au[axis] + mix * (next.au[axis] - previous.au[axis])
                }))
            }
        }
    }
}

fn read_header(file: &mut std::fs::File) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::with_capacity(HEADER_BYTES);
    file.take(HEADER_BYTES as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    Ok(bytes)
}

struct Metadata {
    units_per_au: f64,
    time_scale: TimeScale,
}

impl Metadata {
    fn parse(header: &str) -> Result<(Self, String), String> {
        let field = |key| {
            header.lines().find_map(|line| {
                let (name, value) = line.split_once(':')?;
                (name.trim() == key).then_some(value.trim())
            })
        };
        let name = field("Target body name")
            .and_then(|value| value.split('{').next())
            .map(str::trim)
            .filter(|name| !name.is_empty() && name.len() <= 256)
            .ok_or("Horizons vector table needs a target name")?;
        if !field("Center body name").is_some_and(|center| center.starts_with("Sun (10)"))
            || field("Center-site name") != Some("BODY CENTER")
        {
            return Err("Export the Horizons vector table with Sun center (500@10)".into());
        }
        if field("Reference frame") != Some("Ecliptic of J2000.0") {
            return Err("Export the Horizons vector table in the J2000 ecliptic frame".into());
        }
        if field("Output type") != Some("GEOMETRIC cartesian states") {
            return Err(
                "Export the Horizons vector table with geometric positions (no corrections)".into(),
            );
        }
        let units_per_au = match field("Output units") {
            Some("AU-D") => 1.0,
            Some("KM-D" | "KM-S") => AU_KM,
            _ => return Err("Horizons vector table needs AU or kilometer distance units".into()),
        };
        let time_scale = match field("Start time").and_then(|value| value.split_whitespace().last())
        {
            Some("TDB") => TimeScale::TDB,
            Some("TT") => TimeScale::TT,
            Some("UT" | "UTC") => TimeScale::UTC,
            _ => return Err("Horizons vector table needs TDB, TT or UTC timestamps".into()),
        };
        let format = field("Output format").and_then(|value| value.split_whitespace().next());
        if !matches!(format, Some("1" | "2" | "3" | "4")) {
            return Err("Export a Horizons vector table with X, Y and Z positions".into());
        }
        Ok((
            Self {
                units_per_au,
                time_scale,
            },
            name.to_owned(),
        ))
    }

    fn samples(&self, data: &str, cancel: &AtomicBool) -> Result<Vec<Sample>, String> {
        let mut samples = Vec::<Sample>::new();
        let mut lines = data.lines().filter(|line| !line.trim().is_empty());
        while let Some(line) = lines.next() {
            if cancel.load(Ordering::Acquire) {
                return Err(CANCELLED.into());
            }
            let read = if line.contains(',') {
                csv_row(line)
            } else {
                let vector = lines.next().unwrap_or_default();
                let result = text_row(line, vector);
                // Other quantities (such as velocity) follow X/Y/Z and share its epoch.
                while lines.clone().next().is_some_and(|line| !epoch_line(line)) {
                    lines.next();
                }
                result
            };
            let (jd, position) = read.ok_or_else(|| {
                format!("Invalid Horizons position at sample {}", samples.len() + 1)
            })?;
            let time = self.timestamp(jd)?;
            if samples.last().is_some_and(|previous| previous.time >= time) {
                return Err("Horizons timestamps must increase strictly".into());
            }
            let au = position.map(|axis| axis / self.units_per_au);
            if !au
                .iter()
                .all(|value| value.is_finite() && value.abs() <= 1e6)
            {
                return Err("Horizons position is outside the supported range".into());
            }
            samples.push(Sample { time, au });
        }
        if samples.len() < 2 {
            return Err("A Horizons trajectory needs at least two dated positions".into());
        }
        Ok(samples)
    }

    fn timestamp(&self, jd: f64) -> Result<DateTime<Utc>, String> {
        // Bound the Julian date before converting through hifitime.
        if !(1_000_000.0..=4_000_000.0).contains(&jd) {
            return Err("Horizons Julian date is outside the supported range".into());
        }
        let seconds = Epoch::from_jde_in_time_scale(jd, self.time_scale).to_unix_seconds();
        DateTime::from_timestamp_micros((seconds * 1e6).round() as i64)
            .ok_or_else(|| "Invalid Horizons timestamp".into())
    }
}

fn csv_row(line: &str) -> Option<(f64, [f64; 3])> {
    let mut columns = line.split(',');
    let jd = columns.next()?.trim().parse().ok()?;
    columns.next()?; // Horizons repeats the same epoch as a human-readable calendar date.
    Some((
        jd,
        [
            number(columns.next()?)?,
            number(columns.next()?)?,
            number(columns.next()?)?,
        ],
    ))
}

fn epoch_line(line: &str) -> bool {
    line.split_once('=')
        .is_some_and(|(jd, _)| jd.trim().parse::<f64>().is_ok())
}

fn text_row(epoch: &str, vector: &str) -> Option<(f64, [f64; 3])> {
    let (jd, _) = epoch.split_once('=')?;
    let (_, y_and_z) = vector.split_once('X')?;
    let (x, z) = y_and_z.split_once('Y')?;
    let (y, z) = z.split_once('Z')?;
    Some((
        jd.trim().parse().ok()?,
        [
            number(x.trim().strip_prefix('=')?)?,
            number(y.trim().strip_prefix('=')?)?,
            number(z.trim().strip_prefix('=')?)?,
        ],
    ))
}

fn number(value: &str) -> Option<f64> {
    value.trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEADER: &str = "Ephemeris / API_USER / Horizons\n\
Target body name: 4 Vesta {source: JPL#36}\n\
Center body name: Sun (10) {source: DE441}\n\
Center-site name: BODY CENTER\n\
Start time : A.D. 2000-Jan-01 12:00:00.0000 TDB\n\
Output units : AU-D\n\
Output type : GEOMETRIC cartesian states\n\
Output format : 2 (position and velocity)\n\
Reference frame : Ecliptic of J2000.0\n";

    fn parse_rows(rows: &str) -> Trajectory {
        Trajectory::parse(
            &format!("{HEADER}$$SOE\n{rows}\n$$EOE"),
            &AtomicBool::new(false),
        )
        .unwrap()
    }

    #[test]
    fn real_horizons_exports_load_in_both_formats() {
        let examples = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/examples/horizons");
        for (filename, name, minimum_samples) in [
            ("1P-Halley.txt", "1P/Halley", 2800),
            ("99942-Apophis.txt", "Apophis", 450),
            ("3I-ATLAS.txt", "ATLAS", 140),
            ("4-Vesta.txt", "Vesta", 45),
        ] {
            let path = examples.join(filename);
            let path = Trajectory::read_if_identified(&path, &AtomicBool::new(false))
                .unwrap()
                .unwrap();
            assert!(path.name.contains(name));
            assert!(path.positions().count() >= minimum_samples);
            assert!(path.range()[0] < path.range()[1]);
        }
    }

    #[test]
    fn csv_vectors_follow_the_recorded_range() {
        let path = parse_rows("2451545.0, date, 1.0, -2.0, 3.0\n2451546.0, date, 3.0, 2.0, -1.0");
        let [start, end] = path.range();
        assert_eq!(path.name, "4 Vesta");
        assert_eq!(start.format("%F %H:%M").to_string(), "2000-01-01 11:58");
        assert_eq!(path.position_at(start), Some([1.0, -2.0, 3.0]));
        let midpoint = path.position_at(start + (end - start) / 2).unwrap();
        assert!((midpoint[0] - 2.0).abs() < 1e-8);
        assert_eq!(path.position_at(end + chrono::Duration::seconds(1)), None);
    }

    #[test]
    fn plain_text_vectors_ignore_velocity_rows() {
        let path = parse_rows(
            "2451545.0 = first\n X = 1.0 Y=-2.0 Z =3.0\n VX= 0 VY= 0 VZ= 0\n2451546.0 = last\n X = 3.0 Y = 2.0 Z = -1.0",
        );
        assert_eq!(
            path.positions().collect::<Vec<_>>(),
            [[1.0, -2.0, 3.0], [3.0, 2.0, -1.0]]
        );
    }

    #[test]
    fn unrelated_csv_and_non_heliocentric_vectors_are_distinguished() {
        assert!(!Trajectory::has_header("JD,X,Y,Z\n1,2,3,4"));
        let header = HEADER.replace("Sun (10)", "Earth (399)");
        assert!(Trajectory::has_header(&header));
        assert!(
            Metadata::parse(&header)
                .err()
                .unwrap()
                .contains("Sun center")
        );
    }
}
