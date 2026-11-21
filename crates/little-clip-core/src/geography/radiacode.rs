//! RadiaCode tab-separated and JSON track exports.
use super::{
    Document, Kind, MAX_BYTES, MAX_PATHS, MAX_POINTS, Point, RadiationReading, RawFeature, clip,
    prepare,
};
use chrono::{DateTime, SecondsFormat};
use serde::{
    Deserialize, Deserializer,
    de::{self, SeqAccess, Visitor},
};
use std::{fmt, marker::PhantomData};

// RadiaCode uses 100 µR/h per µSv/h in its exports and display.
// Text tracks and JSON tracks with an explicit false unit flag use µR/h.
const MICRO_ROENTGEN_PER_MICRO_SIEVERT: f64 = 100.0;

pub(super) fn parse(bytes: &[u8]) -> Result<Document, String> {
    if bytes.len() > MAX_BYTES {
        return Err("RadiaCode track exceeds the 16 MiB opening limit".into());
    }
    let text = std::str::from_utf8(bytes)
        .map_err(|_| "Expected a UTF-8 RadiaCode track")?
        .trim_start_matches('\u{feff}')
        .trim_start();
    if text.starts_with('{') {
        let track: JsonTrack = serde_json::from_str(text)
            .map_err(|error| format!("Invalid RadiaCode track: {error}"))?;
        track.prepare()
    } else {
        parse_tab_separated(text)
    }
}

#[derive(Deserialize)]
struct JsonTrack {
    #[serde(default)]
    title: String,
    #[serde(default, deserialize_with = "bounded_vec::<_, _, 256>")]
    devices: Vec<String>,
    #[serde(default, deserialize_with = "bounded_vec::<_, _, MAX_PATHS>")]
    periods: Vec<Period>,
    sv: Option<bool>,
    #[serde(rename = "isSievert")]
    is_sievert: Option<bool>,
    #[serde(deserialize_with = "bounded_vec::<_, _, MAX_POINTS>")]
    markers: Vec<Marker>,
}

#[derive(Deserialize)]
struct Period {
    start: i64,
    end: i64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Marker {
    date: Option<i64>,
    lat: Option<f64>,
    lon: Option<f64>,
    dose_rate: Option<f64>,
    count_rate: Option<f64>,
    acc: Option<f64>,
}

impl JsonTrack {
    fn prepare(mut self) -> Result<Document, String> {
        self.markers.sort_by_key(|marker| marker.date);
        self.periods.retain(|period| period.start <= period.end);
        self.periods.sort_by_key(|period| period.start);
        let in_sieverts = self.sv.or(self.is_sievert).unwrap_or(true);
        let mut previous = None;
        let positions = self.markers.into_iter().map(|marker| {
            let Some(date) = marker.date else {
                return (None, false);
            };
            let next = self.periods.partition_point(|period| period.start <= date);
            let inside = next > 0 && date <= self.periods[next - 1].end;
            let period = (next, inside);
            let split = previous.replace(period).is_some_and(|p| p != period);
            let dose = marker.dose_rate.map(|value| {
                if in_sieverts {
                    value
                } else {
                    value / MICRO_ROENTGEN_PER_MICRO_SIEVERT
                }
            });
            let reading = reading(dose, marker.count_rate, marker.acc);
            let time = DateTime::from_timestamp(date, 0)
                .map(|time| time.to_rfc3339_opts(SecondsFormat::Secs, true));
            let point = time.and_then(|time| position(marker.lat, marker.lon, reading, time));
            (point, split)
        });
        let devices = self
            .devices
            .iter()
            .map(|device| clip(device, 128))
            .collect::<Vec<_>>()
            .join(", ");
        prepare_track(&self.title, [("Devices", devices.as_str())], positions)
    }
}

fn parse_tab_separated(text: &str) -> Result<Document, String> {
    let mut lines = text.lines();
    let mut header = lines
        .next()
        .and_then(|line| line.strip_prefix("Track:"))
        .ok_or("Expected a RadiaCode Track header or JSON export")?
        .split('\t');
    let title = header.next().unwrap_or_default();
    let properties = [
        ("Device", header.next().unwrap_or_default()),
        ("Comment", header.next().unwrap_or_default()),
    ];
    let fields: Vec<_> = lines
        .next()
        .ok_or("RadiaCode track is missing column labels")?
        .split('\t')
        .take(32)
        .collect();
    let column = |label: &str| {
        fields
            .iter()
            .position(|field| field.trim().eq_ignore_ascii_case(label))
    };
    let lat = column("Latitude").ok_or("RadiaCode track is missing Latitude")?;
    let lon = column("Longitude").ok_or("RadiaCode track is missing Longitude")?;
    let timestamp = column("Timestamp");
    let local_time = column("Time");
    let dose = column("DoseRate");
    let count = column("CountRate");
    let accuracy = column("Accuracy");
    let positions = lines.filter(|line| !line.trim().is_empty()).map(|line| {
        let row: Vec<_> = line.split('\t').take(fields.len()).collect();
        let field = |index: Option<usize>| index.and_then(|i| row.get(i)).map(|s| s.trim());
        let number = |index| field(index).and_then(|s| s.parse().ok());
        let time = field(timestamp)
            .and_then(filetime)
            .unwrap_or_else(|| clip(field(local_time).unwrap_or_default(), 128));
        let dose = number(dose).map(|rate| rate / MICRO_ROENTGEN_PER_MICRO_SIEVERT);
        let reading = reading(dose, number(count), number(accuracy));
        (
            position(number(Some(lat)), number(Some(lon)), reading, time),
            false,
        )
    });
    prepare_track(title, properties, positions)
}

fn filetime(value: &str) -> Option<String> {
    const TICKS_PER_SECOND: i64 = 10_000_000;
    const UNIX_EPOCH_SECONDS: i64 = 11_644_473_600;
    let ticks: i64 = value.parse().ok()?;
    if ticks <= 0 {
        return None;
    }
    let seconds = ticks / TICKS_PER_SECOND - UNIX_EPOCH_SECONDS;
    let nanos = (ticks % TICKS_PER_SECOND) as u32 * 100;
    DateTime::from_timestamp(seconds, nanos)
        .map(|time| time.to_rfc3339_opts(SecondsFormat::AutoSi, true))
}

fn reading(dose: Option<f64>, count: Option<f64>, accuracy: Option<f64>) -> RadiationReading {
    let valid = |value: &f64| value.is_finite() && *value >= 0.0;
    RadiationReading {
        dose_rate_usv_h: dose.filter(valid),
        count_rate_cps: count.filter(valid),
        accuracy_m: accuracy.filter(valid),
    }
}

fn position(
    lat: Option<f64>,
    lon: Option<f64>,
    reading: RadiationReading,
    time: String,
) -> Option<Point> {
    let lat = lat.filter(|value| (-90.0..=90.0).contains(value))?;
    let lon = lon.filter(|value| (-180.0..=180.0).contains(value))?;
    Some(Point {
        lat,
        lon,
        time,
        elevation: None,
        radiation: Some(reading),
    })
}

fn prepare_track<'a>(
    title: &str,
    properties: impl IntoIterator<Item = (&'a str, &'a str)>,
    positions: impl Iterator<Item = (Option<Point>, bool)>,
) -> Result<Document, String> {
    let mut paths: Vec<Vec<Point>> = Vec::new();
    let mut split = true;
    let mut skipped = 0;
    for (index, (point, period_changed)) in positions.enumerate() {
        if index >= MAX_POINTS {
            return Err("RadiaCode track exceeds the 200,000-position limit".into());
        }
        let Some(point) = point else {
            split = true;
            skipped += 1;
            continue;
        };
        if split || period_changed {
            if paths.len() == MAX_PATHS {
                return Err("RadiaCode track exceeds the 4,096-segment limit".into());
            }
            paths.push(Vec::new());
        }
        paths.last_mut().unwrap().push(point);
        split = false;
    }
    if paths.is_empty() {
        return Err("No readable RadiaCode track positions".into());
    }
    let title = clip(title.trim(), 256);
    let name = if title.is_empty() {
        "RadiaCode track".into()
    } else {
        title.clone()
    };
    let properties = properties
        .into_iter()
        .filter(|(_, value)| !value.trim().is_empty())
        .map(|(name, value)| (name.into(), clip(value.trim(), 512)))
        .collect();
    let notes = (skipped > 0)
        .then(|| format!("Skipped {skipped} unreadable track positions."))
        .into_iter()
        .collect();
    prepare(
        title,
        vec![RawFeature {
            name,
            kind: Kind::Line,
            paths,
            properties,
            time_range: None,
        }],
        notes,
    )
}

fn bounded_vec<'de, D, T, const LIMIT: usize>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    struct Bounded<T, const LIMIT: usize>(PhantomData<T>);
    impl<'de, T: Deserialize<'de>, const LIMIT: usize> Visitor<'de> for Bounded<T, LIMIT> {
        type Value = Vec<T>;

        fn expecting(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(fmt, "an array of at most {LIMIT} items")
        }

        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
            let mut items = Vec::new();
            while let Some(item) = seq.next_element()? {
                if items.len() == LIMIT {
                    return Err(de::Error::custom(format!(
                        "Track array exceeds the {LIMIT}-item limit"
                    )));
                }
                items.push(item);
            }
            Ok(items)
        }
    }
    deserializer.deserialize_seq(Bounded::<T, LIMIT>(PhantomData))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_track_converts_dose_and_filetime_and_keeps_source_readings() {
        let doc = parse(include_bytes!(
            "../../tests/fixtures/geography/survey.rctrk"
        ))
        .unwrap();
        let feature = &doc.features[0];
        assert_eq!(doc.title, "Harbor survey");
        assert_eq!(feature.dose_rate, Some([0.06, 0.17]));
        assert_eq!(feature.elevation, None);
        assert_eq!(feature.points, 4);
        let point = &feature.paths[0][0].point;
        assert_eq!(point.time, "2024-01-01T20:00:00Z");
        let reading = point.radiation.unwrap();
        assert_eq!(reading.dose_rate_usv_h, Some(0.06));
        assert_eq!(reading.count_rate_cps, Some(4.8));
        assert_eq!(reading.accuracy_m, Some(3.5));
    }

    #[test]
    fn text_track_accepts_older_columns_and_breaks_at_invalid_fixes() {
        let doc = parse(b"Track: Older export\r\nTimestamp\tTime\tLatitude\tLongitude\tAccuracy\tDoseRate\tComment\r\n133486128000000000\t\t0\t0\t3\t12\t\r\n133486128010000000\t\tNaN\t3\t3\t12\t\r\n133486128020000000\t\t4\t3\t-1\t-1\t\r\n").unwrap();
        let feature = &doc.features[0];
        assert_eq!(feature.paths.len(), 2);
        assert_eq!(feature.distance_m, 0.0);
        let reading = feature.paths[1][0].point.radiation.unwrap();
        assert_eq!(reading.dose_rate_usv_h, None);
        assert_eq!(reading.accuracy_m, None);
        assert_eq!(reading.count_rate_cps, None);
        assert!(doc.notes[0].contains("Skipped 1"));
    }

    #[test]
    fn json_track_restores_chronology_and_keeps_pauses_disconnected() {
        let doc = parse(
            br#"{
            "title":"Harbor survey","devices":["RC-102-000001"],"sv":false,
            "periods":[{"start":40,"end":60},{"start":10,"end":20}],
            "markers":[
                {"date":41,"lat":2,"lon":2,"doseRate":30},
                {"date":19,"lat":1,"lon":0.01,"doseRate":20},
                {"date":12,"lat":1,"lon":0,"doseRate":10,"countRate":5,"acc":3},
                {"date":50,"lat":2,"lon":2.01,"doseRate":40}
            ]
        }"#,
        )
        .unwrap();
        let feature = &doc.features[0];
        assert_eq!(feature.dose_rate, Some([0.1, 0.4]));
        assert_eq!(feature.paths.len(), 2);
        assert!(feature.paths.iter().all(|path| path.len() == 2));
        assert_eq!(feature.paths[0][0].point.time, "1970-01-01T00:00:12Z");
        assert_eq!(feature.paths[1][0].point.time, "1970-01-01T00:00:41Z");
        assert!(feature.distance_m < 2300.0);
    }

    #[test]
    fn json_units_follow_the_export_flags_and_default_to_sieverts() {
        for (flags, expected) in [
            ("", 0.4),
            (r#""sv":true,"#, 0.4),
            (r#""isSievert":false,"#, 0.004),
        ] {
            let text =
                format!(r#"{{{flags}"markers":[{{"date":10,"lat":1,"lon":0,"doseRate":0.4}}]}}"#);
            let doc = parse(text.as_bytes()).unwrap();
            assert_eq!(doc.features[0].dose_rate, Some([expected; 2]));
        }
    }
}
