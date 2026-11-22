//! Dated heliocentric positions for the Solar System view.
mod oem;

use super::AU_KM;
use crate::file_source::ReadCheck;
use chrono::{DateTime, Utc};
use hifitime::{Epoch, TimeScale};
use serde::Deserialize;
use std::{
    io::Read,
    ops::RangeInclusive,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

const HEADER_BYTES: usize = 16 * 1024;
const MAX_BYTES: usize = 16 * 1024 * 1024;
const CANCELLED: &str = "Trajectory opening cancelled";
const CSV_MARKER: &str = "# Sun-centered J2000 ecliptic";

/// Sampled positions in AU along the J2000 ecliptic axes, relative to the Sun.
pub struct Trajectory {
    pub name: String,
    pub radius_au: f64,
    samples: Vec<Sample>,
    format: Format,
}

struct Sample {
    time: DateTime<Utc>,
    au: [f64; 3],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Format {
    Horizons,
    HorizonsJson,
    OemKvn,
    OemXml,
    Csv,
}

impl Format {
    fn detect(header: &str) -> Option<Self> {
        let header = header.trim_start_matches('\u{feff}').trim_start();
        if header.starts_with('<') {
            return oem::has_xml_header(header).then_some(Self::OemXml);
        }
        if oem::version_line(header.lines().next()?).is_some() {
            return Some(Self::OemKvn);
        }
        if header.lines().next()?.trim_end() == CSV_MARKER {
            return Some(Self::Csv);
        }
        let vector_report = header.contains("/ Horizons")
            && header.contains("Target body name:")
            && header.contains("cartesian states");
        if header.starts_with('{') {
            (vector_report && header.contains("\"NASA/JPL Horizons API\""))
                .then_some(Self::HorizonsJson)
        } else {
            vector_report.then_some(Self::Horizons)
        }
    }
}

impl Trajectory {
    /// Returns whether a file can be probed for a trajectory signature.
    pub fn is_candidate(path: &Path) -> bool {
        path.extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| {
                [
                    "txt", "csv", "dat", "eph", "horizons", "json", "oem", "kvn", "xml",
                ]
                .iter()
                .any(|suffix| ext.eq_ignore_ascii_case(suffix))
            })
    }

    /// Opens a recognized trajectory through a guarded file handle.
    ///
    /// # Errors
    ///
    /// Returns an error on failed reads, cancellation, or invalid or unsupported
    /// data in an identified trajectory.
    pub fn read_if_identified(path: &Path, cancel: &AtomicBool) -> Result<Option<Self>, String> {
        if !Self::is_candidate(path) {
            return Ok(None);
        }
        check_cancel(cancel)?;
        let (mut file, source) = ReadCheck::default().open(path, None, cancel)?;
        let mut bytes = read_header(&mut file)?;
        let Some(format) = Format::detect(&String::from_utf8_lossy(&bytes)) else {
            source.check_file(&file)?;
            return Ok(None);
        };

        check_cancel(cancel)?;
        file.by_ref()
            .take((MAX_BYTES + 1 - bytes.len()) as u64)
            .read_to_end(&mut bytes)
            .map_err(|error| error.to_string())?;
        source.check_file(&file)?;
        if bytes.len() > MAX_BYTES {
            return Err("Trajectory exceeds the 16 MiB opening limit".into());
        }
        check_cancel(cancel)?;
        let text = String::from_utf8_lossy(&bytes);
        let text = text.trim_start_matches('\u{feff}');
        let trajectory = match format {
            Format::Horizons => Self::parse_horizons(text, format, cancel),
            Format::HorizonsJson => {
                let reply: HorizonsReply = serde_json::from_str(text)
                    .map_err(|error| format!("Invalid Horizons API response: {error}"))?;
                if reply.signature.source != "NASA/JPL Horizons API" {
                    return Ok(None);
                }
                Self::parse_horizons(&reply.result, format, cancel)
            }
            Format::OemKvn => oem::read_kvn(text, cancel),
            Format::OemXml => oem::read_xml(text, cancel),
            Format::Csv => Self::parse_csv(text, path, cancel),
        }?;
        Ok(Some(trajectory))
    }

    /// Probes a bounded prefix for an orbital trajectory signature.
    pub fn has_file_header(path: &Path, cancel: &AtomicBool) -> Result<bool, String> {
        if !Self::is_candidate(path) {
            return Ok(false);
        }
        let (mut file, source) = ReadCheck::default().open(path, None, cancel)?;
        let header = read_header(&mut file)?;
        source.check_file(&file)?;
        Ok(Format::detect(&String::from_utf8_lossy(&header)).is_some())
    }

    /// Returns the file format used to load this path.
    pub fn source_label(&self) -> &'static str {
        match self.format {
            Format::Horizons => "JPL Horizons",
            Format::HorizonsJson => "JPL Horizons JSON",
            Format::OemKvn => "CCSDS OEM (KVN)",
            Format::OemXml => "CCSDS OEM (XML)",
            Format::Csv => "Heliocentric CSV",
        }
    }

    fn parse_horizons(text: &str, format: Format, cancel: &AtomicBool) -> Result<Self, String> {
        let (header, data) = text
            .split_once("$$SOE")
            .and_then(|(header, data)| data.split_once("$$EOE").map(|(data, _)| (header, data)))
            .ok_or("Horizons vector table is missing its start or end marker")?;
        let (metadata, name) = Metadata::parse(header)?;
        Self::from_samples(name, format, metadata.samples(data), None, cancel)
    }

    fn parse_csv(text: &str, path: &Path, cancel: &AtomicBool) -> Result<Self, String> {
        let mut lines = text.trim_start().lines();
        lines.next(); // Coordinate frame marker, already verified by detection.
        let units_per_au = match lines.next() {
            Some(line) if csv_fields(line).eq(["utc", "x_au", "y_au", "z_au"]) => 1.0,
            Some(line) if csv_fields(line).eq(["utc", "x_km", "y_km", "z_km"]) => AU_KM,
            _ => return Err("Use CSV columns utc,x_au,y_au,z_au or utc,x_km,y_km,z_km".into()),
        };
        let rows = lines
            .enumerate()
            .filter(|(_, line)| !line.trim().is_empty())
            .map(|(index, line)| {
                let mut columns = csv_fields(line);
                let time = columns
                    .next()
                    .and_then(|stamp| stamp.parse::<DateTime<Utc>>().ok());
                let position = xyz(&mut columns);
                match (time, position, columns.next()) {
                    (Some(time), Some(position), None) => Ok(Sample {
                        time,
                        au: position.map(|axis| axis / units_per_au),
                    }),
                    _ => Err(format!("Invalid trajectory CSV row {}", index + 3)),
                }
            });
        let name = path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        Self::from_samples(name, Format::Csv, rows, None, cancel)
    }

    fn from_samples(
        name: String,
        format: Format,
        rows: impl IntoIterator<Item = Result<Sample, String>>,
        usable: Option<RangeInclusive<DateTime<Utc>>>,
        cancel: &AtomicBool,
    ) -> Result<Self, String> {
        let mut samples: Vec<Sample> = Vec::new();
        let mut radius_squared: f64 = 0.0;
        let mut previous_time = None;
        let mut rows = rows.into_iter();
        loop {
            check_cancel(cancel)?;
            let Some(row) = rows.next() else { break };
            let sample = row?;
            if previous_time.is_some_and(|previous| previous >= sample.time) {
                return Err("Trajectory timestamps must increase strictly".into());
            }
            previous_time = Some(sample.time);
            if !sample
                .au
                .iter()
                .all(|value| value.is_finite() && value.abs() <= 1e6)
            {
                return Err("Trajectory position is outside the supported range".into());
            }
            if usable
                .as_ref()
                .is_some_and(|range| !range.contains(&sample.time))
            {
                continue;
            }
            radius_squared = radius_squared.max(sample.au.iter().map(|axis| axis * axis).sum());
            samples.push(sample);
        }
        if samples.len() < 2 {
            return Err("A trajectory needs at least two dated positions".into());
        }
        Ok(Self {
            name,
            radius_au: radius_squared.sqrt(),
            samples,
            format,
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

#[derive(Deserialize)]
struct HorizonsReply {
    signature: HorizonsSignature,
    result: String,
}

#[derive(Deserialize)]
struct HorizonsSignature {
    source: String,
}

fn check_cancel(cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(Ordering::Acquire) {
        Err(CANCELLED.into())
    } else {
        Ok(())
    }
}

fn icrf_to_ecliptic([x, y, z]: [f64; 3]) -> [f64; 3] {
    // Horizons uses the IAU 1976 fixed obliquity to define its J2000 ecliptic.
    let (sine, cosine) = (84_381.448_f64 / 3600.0).to_radians().sin_cos();
    [x, cosine * y + sine * z, -sine * y + cosine * z]
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
    icrf: bool,
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
        let icrf = match field("Reference frame") {
            Some("ICRF") => true,
            Some("Ecliptic of J2000.0") => false,
            _ => return Err("Export the Horizons vector table in ICRF or J2000 ecliptic".into()),
        };
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
                icrf,
            },
            name.to_owned(),
        ))
    }

    fn samples<'a>(&'a self, data: &'a str) -> impl Iterator<Item = Result<Sample, String>> + 'a {
        let mut lines = data.lines().filter(|line| !line.trim().is_empty());
        std::iter::from_fn(move || {
            let line = lines.next()?;
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
            Some(read)
        })
        .enumerate()
        .map(|(index, row)| {
            let (jd, position) =
                row.ok_or_else(|| format!("Invalid Horizons position at sample {}", index + 1))?;
            let time = self.timestamp(jd)?;
            let au = position.map(|axis| axis / self.units_per_au);
            let au = if self.icrf { icrf_to_ecliptic(au) } else { au };
            Ok(Sample { time, au })
        })
    }

    fn timestamp(&self, jd: f64) -> Result<DateTime<Utc>, String> {
        // Bound the Julian date before converting through hifitime.
        if !(1_000_000.0..=4_000_000.0).contains(&jd) {
            return Err("Horizons Julian date is outside the supported range".into());
        }
        epoch_utc(Epoch::from_jde_in_time_scale(jd, self.time_scale))
            .ok_or_else(|| "Invalid Horizons timestamp".into())
    }
}

fn csv_row(line: &str) -> Option<(f64, [f64; 3])> {
    let mut columns = line.split(',');
    let jd = columns.next()?.trim().parse().ok()?;
    columns.next()?; // Horizons repeats the same epoch as a human-readable calendar date.
    Some((jd, xyz(&mut columns)?))
}

fn csv_fields(line: &str) -> impl Iterator<Item = &str> {
    line.split(',').map(|value| {
        let value = value.trim();
        value
            .strip_prefix('"')
            .and_then(|quoted| quoted.strip_suffix('"'))
            .unwrap_or(value)
    })
}

fn xyz<'a>(columns: &mut impl Iterator<Item = &'a str>) -> Option<[f64; 3]> {
    Some([
        number(columns.next()?)?,
        number(columns.next()?)?,
        number(columns.next()?)?,
    ])
}

fn epoch_utc(epoch: Epoch) -> Option<DateTime<Utc>> {
    let nanos = epoch.to_unix_duration().total_nanoseconds();
    DateTime::from_timestamp(
        nanos.div_euclid(1_000_000_000).try_into().ok()?,
        nanos.rem_euclid(1_000_000_000) as u32,
    )
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
        Trajectory::parse_horizons(
            &format!("{HEADER}$$SOE\n{rows}\n$$EOE"),
            Format::Horizons,
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
    fn horizons_json_and_oem_match_jpl_ecliptic_vectors() {
        let examples = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/examples");
        let cancel = AtomicBool::new(false);
        let expected_km = [
            -4.388_577_457_378_983e7,
            -2.170_849_264_747_524e8,
            -3.473_007_284_583_181e6,
        ];
        let mut start = None;
        for (file, format) in [
            ("horizons/Mars-ICRF.json", Format::HorizonsJson),
            ("solar-system/Mars-ICRF.oem", Format::OemKvn),
            ("solar-system/Mars-ICRF.xml", Format::OemXml),
        ] {
            let path = examples.join(file);
            assert!(Trajectory::has_file_header(&path, &cancel).unwrap());
            let trajectory = Trajectory::read_if_identified(&path, &cancel)
                .unwrap()
                .unwrap();
            assert_eq!(trajectory.format, format);
            assert_eq!(trajectory.positions().count(), 3);
            for (au, km) in trajectory
                .positions()
                .next()
                .unwrap()
                .into_iter()
                .zip(expected_km)
            {
                assert!((au * AU_KM - km).abs() < 1e-5);
            }
            let epoch = trajectory.range()[0];
            let first = start.get_or_insert(epoch);
            assert!((epoch - *first).abs() < chrono::Duration::microseconds(5));
        }
    }

    #[test]
    fn explicit_csv_supports_kilometers_and_leaves_ordinary_tables_alone() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("Notebook output.csv");
        let rows = "utc,x_km,y_km,z_km\n2024-01-01T00:00:00Z,149597870.7,0,0\n2024-01-02T00:00:00Z,0,149597870.7,0";
        let cancel = AtomicBool::new(false);
        std::fs::write(&file, rows).unwrap();
        assert!(!Trajectory::has_file_header(&file, &cancel).unwrap());
        assert!(
            Trajectory::read_if_identified(&file, &cancel)
                .unwrap()
                .is_none()
        );

        std::fs::write(&file, format!("\u{feff}{CSV_MARKER}\r\n{rows}")).unwrap();
        let trajectory = Trajectory::read_if_identified(&file, &cancel)
            .unwrap()
            .unwrap();
        assert_eq!(trajectory.name, "Notebook output");
        assert_eq!(
            trajectory.positions().collect::<Vec<_>>(),
            [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]
        );
        assert_eq!(
            trajectory.range()[0].to_rfc3339(),
            "2024-01-01T00:00:00+00:00"
        );

        std::fs::write(
            &file,
            format!("{CSV_MARKER}\n{rows}\n2024-01-02T00:00:00Z,1,1,1"),
        )
        .unwrap();
        let err = Trajectory::read_if_identified(&file, &cancel)
            .err()
            .unwrap();
        assert!(err.contains("increase strictly"));
    }

    #[test]
    fn non_trajectory_xml_and_json_keep_their_default_route() {
        for header in [
            "<instrument><oem id=\"CCSDS_OEM_VERS\" version=\"3.0\"/></instrument>",
            r#"{"result": "cartesian states / Horizons Target body name:"}"#,
        ] {
            assert_eq!(Format::detect(header), None);
        }
    }

    #[test]
    fn oem_rejects_other_origins_and_segment_boundaries_and_honors_usable_time() {
        let cancel = AtomicBool::new(false);
        let data = include_str!("../../../../docs/examples/solar-system/Mars-ICRF.oem");
        let earth = data.replace("CENTER_NAME = SUN", "CENTER_NAME = EARTH");
        assert!(
            oem::read_kvn(&earth, &cancel)
                .err()
                .unwrap()
                .contains("SUN center")
        );
        let second_segment = data.find("META_START").unwrap();
        let multiple = format!("{data}\n{}", &data[second_segment..]);
        assert!(
            oem::read_kvn(&multiple, &cancel)
                .err()
                .unwrap()
                .contains("single CCSDS OEM segment")
        );
        let with_usable_range = data.replace(
            "START_TIME = 2024-01-01T00:00:00",
            "START_TIME = 2024-001T00:00:00\nUSEABLE_START_TIME = 2024-001T22:00:00",
        );
        let trajectory = oem::read_kvn(&with_usable_range, &cancel).unwrap();
        assert_eq!(trajectory.positions().count(), 2);
        assert_eq!(
            trajectory.range()[0].format("%F %H:%M").to_string(),
            "2024-01-01 23:58"
        );
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
        assert_eq!(Format::detect("JD,X,Y,Z\n1,2,3,4"), None);
        let header = HEADER.replace("Sun (10)", "Earth (399)");
        assert_eq!(Format::detect(&header), Some(Format::Horizons));
        assert!(
            Metadata::parse(&header)
                .err()
                .unwrap()
                .contains("Sun center")
        );
    }
}
