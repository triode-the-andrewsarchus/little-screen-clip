//! The Sun-centered ICRF subset of CCSDS Orbit Ephemeris Messages.

use super::{AU_KM, Format, Sample, Trajectory, check_cancel, epoch_utc, icrf_to_ecliptic, xyz};
use chrono::{DateTime, Datelike, NaiveDateTime, Timelike, Utc};
use hifitime::{Epoch, TimeScale};
use roxmltree::Node;
use std::{collections::HashMap, ops::RangeInclusive, sync::atomic::AtomicBool};

const VERSION_KEY: &str = "CCSDS_OEM_VERS";
const SINGLE_SEGMENT: &str = "The Solar System view needs a single CCSDS OEM segment";

pub(super) fn version_line(line: &str) -> Option<&str> {
    let (key, value) = line.split_once('=')?;
    (key.trim() == VERSION_KEY).then_some(value.trim())
}

pub(super) fn has_xml_header(text: &str) -> bool {
    crate::xml_content::root(text.as_bytes()).is_some_and(|root| {
        root.local_name().as_ref() == b"oem"
            && root
                .try_get_attribute("id")
                .ok()
                .flatten()
                .is_some_and(|id| id.value.as_ref() == VERSION_KEY.as_bytes())
    })
}

pub(super) fn read_kvn(text: &str, cancel: &AtomicBool) -> Result<Trajectory, String> {
    let mut lines = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && line.split_whitespace().next() != Some("COMMENT"));
    check_version(lines.next().and_then(version_line))?;
    lines
        .by_ref()
        .find(|line| *line == "META_START")
        .ok_or("CCSDS OEM is missing META_START")?;

    let mut fields = HashMap::new();
    for line in lines.by_ref().take_while(|line| *line != "META_STOP") {
        check_cancel(cancel)?;
        let (key, value) = line.split_once('=').ok_or("Invalid CCSDS OEM metadata")?;
        if fields.insert(key.trim(), value.trim()).is_some() {
            return Err("Duplicate CCSDS OEM metadata field".into());
        }
    }
    if lines.clone().any(|line| line == "META_START") {
        return Err(SINGLE_SEGMENT.into());
    }
    let metadata = Metadata::parse(|key| fields.get(key).copied())?;
    let rows = lines
        .take_while(|line| *line != "COVARIANCE_START")
        .enumerate()
        .map(|(index, line)| {
            let (stamp, km) =
                kvn_position(line).ok_or_else(|| format!("Invalid CCSDS OEM row {}", index + 1))?;
            metadata.sample(stamp, km)
        });
    metadata.trajectory(Format::OemKvn, rows, cancel)
}

fn kvn_position(line: &str) -> Option<(&str, [f64; 3])> {
    let mut fields = line.split_whitespace();
    let stamp = fields.next()?;
    let km = xyz(&mut fields)?;
    // Position and velocity are required. Acceleration is optional.
    let velocity = xyz(&mut fields)?;
    if !velocity.iter().all(|value| value.is_finite()) {
        return None;
    }
    if let Some(first) = fields.next() {
        let acceleration = xyz(&mut std::iter::once(first).chain(&mut fields))?;
        if !acceleration.iter().all(|value| value.is_finite()) || fields.next().is_some() {
            return None;
        }
    }
    Some((stamp, km))
}

pub(super) fn read_xml(text: &str, cancel: &AtomicBool) -> Result<Trajectory, String> {
    crate::xml_content::validate(text.as_bytes(), 1_000_000, 32)?;
    check_cancel(cancel)?;
    let doc =
        roxmltree::Document::parse(text).map_err(|err| format!("Invalid CCSDS OEM: {err}"))?;
    let root = doc.root_element();
    check_version(root.attribute("version"))?;
    let mut segments = child(root, "body")
        .ok_or("CCSDS OEM is missing its body")?
        .children()
        .filter(|node| node.has_tag_name("segment"));
    let segment = segments.next().ok_or(SINGLE_SEGMENT)?;
    if segments.next().is_some() {
        return Err(SINGLE_SEGMENT.into());
    }
    let fields = child(segment, "metadata").ok_or("CCSDS OEM is missing its metadata")?;
    let data = child(segment, "data").ok_or("CCSDS OEM is missing its data")?;
    let metadata = Metadata::parse(|key| child(fields, key).and_then(|node| node.text()))?;
    let rows = data
        .children()
        .filter(|node| node.has_tag_name("stateVector"))
        .map(|row| {
            let stamp = child(row, "EPOCH")
                .and_then(|node| node.text())
                .ok_or("CCSDS OEM state vector needs an epoch")?;
            let km = [
                xml_component(row, "X")?,
                xml_component(row, "Y")?,
                xml_component(row, "Z")?,
            ];
            metadata.sample(stamp, km)
        });
    metadata.trajectory(Format::OemXml, rows, cancel)
}

fn xml_component(row: Node<'_, '_>, key: &str) -> Result<f64, String> {
    let node = child(row, key).ok_or_else(|| format!("CCSDS OEM state vector needs {key}"))?;
    if node.attribute("units").is_some_and(|units| units != "km") {
        return Err("CCSDS OEM positions must use kilometers".into());
    }
    node.text()
        .and_then(super::number)
        .ok_or_else(|| format!("Invalid CCSDS OEM position {key}"))
}

fn child<'a, 'input>(parent: Node<'a, 'input>, name: &str) -> Option<Node<'a, 'input>> {
    parent.children().find(|node| node.has_tag_name(name))
}

fn check_version(version: Option<&str>) -> Result<(), String> {
    match version {
        Some("1.0" | "2.0" | "3.0") => Ok(()),
        _ => Err("Expected CCSDS OEM version 1.0, 2.0 or 3.0".into()),
    }
}

struct Metadata<'a> {
    name: &'a str,
    time_scale: TimeScale,
    total: RangeInclusive<DateTime<Utc>>,
    usable: RangeInclusive<DateTime<Utc>>,
}

impl<'a> Metadata<'a> {
    fn parse(field: impl Fn(&str) -> Option<&'a str>) -> Result<Self, String> {
        let get = |key| field(key).map(str::trim);
        let name = get("OBJECT_NAME")
            .filter(|name| !name.is_empty() && name.len() <= 256)
            .ok_or("CCSDS OEM needs an object name")?;
        if get("CENTER_NAME") != Some("SUN") || get("REF_FRAME") != Some("ICRF") {
            return Err(
                "The Solar System view needs a CCSDS OEM with SUN center and ICRF axes".into(),
            );
        }
        let time_scale = match get("TIME_SYSTEM") {
            Some("UTC") => TimeScale::UTC,
            Some("TAI") => TimeScale::TAI,
            Some("TT") => TimeScale::TT,
            Some("GPS") => TimeScale::GPST,
            Some("TDB") => TimeScale::TDB,
            _ => return Err("CCSDS OEM imports use UTC, TAI, TT, GPS or TDB timestamps".into()),
        };
        let time = |key| {
            get(key)
                .map(|stamp| timestamp(stamp, time_scale))
                .transpose()
        };
        let start = time("START_TIME")?.ok_or("CCSDS OEM needs START_TIME")?;
        let stop = time("STOP_TIME")?.ok_or("CCSDS OEM needs STOP_TIME")?;
        let usable_start = time("USEABLE_START_TIME")?.unwrap_or(start);
        let usable_stop = time("USEABLE_STOP_TIME")?.unwrap_or(stop);
        if !(start <= usable_start && usable_start < usable_stop && usable_stop <= stop) {
            return Err("Invalid CCSDS OEM coverage interval".into());
        }
        Ok(Self {
            name,
            time_scale,
            total: start..=stop,
            usable: usable_start..=usable_stop,
        })
    }

    fn sample(&self, stamp: &str, km: [f64; 3]) -> Result<Sample, String> {
        let time = timestamp(stamp, self.time_scale)?;
        if !self.total.contains(&time) {
            return Err("CCSDS OEM sample falls outside declared coverage".into());
        }
        Ok(Sample {
            time,
            au: icrf_to_ecliptic(km.map(|axis| axis / AU_KM)),
        })
    }

    fn trajectory(
        &self,
        format: Format,
        rows: impl IntoIterator<Item = Result<Sample, String>>,
        cancel: &AtomicBool,
    ) -> Result<Trajectory, String> {
        Trajectory::from_samples(
            self.name.to_owned(),
            format,
            rows,
            Some(self.usable.clone()),
            cancel,
        )
    }
}

fn timestamp(stamp: &str, scale: TimeScale) -> Result<DateTime<Utc>, String> {
    let stamp = stamp.trim();
    let stamp = if scale == TimeScale::UTC {
        stamp.strip_suffix('Z').unwrap_or(stamp)
    } else {
        stamp
    };
    let invalid = || format!("Invalid CCSDS OEM timestamp {stamp}");
    if !stamp
        .as_bytes()
        .get(..5)
        .is_some_and(|prefix| prefix[4] == b'-' && prefix[..4].iter().all(u8::is_ascii_digit))
    {
        return Err(invalid());
    }
    let calendar = NaiveDateTime::parse_from_str(stamp, "%Y-%m-%dT%H:%M:%S%.f")
        .or_else(|_| NaiveDateTime::parse_from_str(stamp, "%Y-%jT%H:%M:%S%.f"))
        .map_err(|_| invalid())?;
    if scale == TimeScale::UTC {
        return Ok(calendar.and_utc());
    }
    let epoch = Epoch::maybe_from_gregorian(
        calendar.year(),
        calendar.month() as u8,
        calendar.day() as u8,
        calendar.hour() as u8,
        calendar.minute() as u8,
        calendar.second() as u8,
        calendar.nanosecond(),
        scale,
    )
    .map_err(|_| invalid())?;
    epoch_utc(epoch).ok_or_else(invalid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gps_timestamps_use_their_declared_clock() {
        let stamp = "2024-001T00:00:00.123456";
        let utc = timestamp(stamp, TimeScale::UTC).unwrap();
        let gps = timestamp(stamp, TimeScale::GPST).unwrap();
        assert_eq!(utc - gps, chrono::TimeDelta::seconds(18));
    }
}
