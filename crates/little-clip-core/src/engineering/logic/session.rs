//! Sigrok and DSView session layouts.
//!
//! https://sigrok.org/wiki/File_format:Sigrok/v2
//! https://github.com/DreamSourceLab/DSView/blob/2e9e2c8e726df4ef5687d39b83d4f797cc44b574/libsigrok4DSL/session_driver.c

use super::{
    Lane, MAX_LANES,
    archive::Archive,
    packed::{Chunk, Packing, Stream},
};
use std::{collections::BTreeMap, sync::Arc};

const MAX_METADATA: usize = 256 * 1024;
const MAX_WORD_BYTES: usize = MAX_LANES / 8;
const MAX_LINE: usize = 4096;

pub(super) struct Session {
    pub(super) lanes: Vec<Lane>,
    pub(super) streams: Vec<Stream>,
    pub(super) note: Option<String>,
}

pub(super) fn sigrok(archive: &mut Archive) -> Result<Session, String> {
    let version = archive.metadata("version", 16)?;
    let version = utf8(&version)?.trim();
    if !matches!(version, "1" | "2") {
        return Err("Unsupported Sigrok capture version.".into());
    }
    let metadata = archive.metadata("metadata", MAX_METADATA)?;
    let parser = Metadata::parse(&metadata)?;
    let device = parser
        .get("device 1")
        .ok_or("Sigrok device metadata is absent.")?;
    let (lanes, streams) = SigrokLayout::parse(device)?.open(archive, device)?;
    let analog = device
        .get("total analog")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    let additional = parser
        .keys()
        .any(|name| name.starts_with("device ") && *name != "device 1");
    let note = (analog != 0 || additional)
        .then(|| "Showing the first device’s digital probes. Analog probes and additional devices are unavailable.".into());
    Ok(Session {
        lanes,
        streams,
        note,
    })
}

struct SigrokLayout {
    width: usize,
    probes: usize,
    rate: f64,
}

impl SigrokLayout {
    fn parse(device: &Section<'_>) -> Result<Self, String> {
        let width = bounded_count(device, "unitsize", MAX_WORD_BYTES)?;
        let probes = bounded_count(device, "total probes", MAX_LANES)?;
        let rate = sample_rate(get(device, "samplerate")?)?;
        if get(device, "capturefile")? != "logic-1" {
            return Err("Unsupported Sigrok capture member layout.".into());
        }
        Ok(Self {
            width,
            probes,
            rate,
        })
    }

    fn open(
        &self,
        archive: &mut Archive,
        device: &Section<'_>,
    ) -> Result<(Vec<Lane>, Vec<Stream>), String> {
        let members = sigrok_members(archive, "logic-1")?;
        let width = self.width;
        let (chunks, samples) = chunks(archive, members, |size| {
            size.is_multiple_of(width as u64)
                .then_some(size / width as u64)
        })?;
        let named = probes_named(device, self.probes, 1)?;
        if named.is_empty() {
            return Err("Sigrok capture has no named logic probes.".into());
        }
        let rate = self.rate;
        named
            .into_iter()
            .map(|(bit, name)| {
                if bit >= width * 8 {
                    return Err("A Sigrok probe falls outside the stored sample word.".into());
                }
                let lane = make_lane(name, samples, rate, 0)?;
                let stream = Stream {
                    chunks: Arc::clone(&chunks),
                    packing: Packing::Words { width, bit },
                    samples,
                    rate,
                };
                Ok((lane, stream))
            })
            .collect::<Result<Vec<_>, String>>()
            .map(|entries| entries.into_iter().unzip())
    }
}

fn sigrok_members(archive: &Archive, basename: &str) -> Result<Vec<String>, String> {
    let prefix = format!("{basename}-");
    let chunks = number_suffix(
        archive
            .names()
            .filter_map(|name| name.strip_prefix(&prefix).map(|suffix| (suffix, name))),
        1,
    )?;
    let monolithic = archive.names().any(|name| name == basename);
    match (monolithic, chunks.is_empty()) {
        (true, true) => Ok(vec![basename.to_owned()]),
        (false, false) => Ok(chunks),
        _ => Err("Sigrok capture logic members are absent or ambiguous.".into()),
    }
}

pub(super) fn dsview(archive: &mut Archive) -> Result<Session, String> {
    let metadata = archive.metadata("header", MAX_METADATA)?;
    let parser = Metadata::parse(&metadata)?;
    let version = parser
        .get("version")
        .ok_or("DSView version metadata is absent.")?;
    if !matches!(number::<u8>(version, "version")?, 2 | 3) {
        return Err("Unsupported DSView capture version.".into());
    }
    let device = parser
        .get("header")
        .ok_or("DSView capture metadata is absent.")?;
    let layout = DsViewLayout::parse(device)?;
    let named = probes_named(device, MAX_LANES, 0)?;
    if named.is_empty() || named.len() > layout.probes {
        return Err("DSView capture has inconsistent named probes.".into());
    }

    let mut lanes = Vec::with_capacity(named.len());
    let mut streams = Vec::with_capacity(named.len());
    for (index, name) in named {
        let (stream, lane) = layout.stream(archive, index, name)?;
        lanes.push(lane);
        streams.push(stream);
    }

    Ok(Session {
        lanes,
        streams,
        note: None,
    })
}

struct DsViewLayout {
    blocks: usize,
    probes: usize,
    samples: u64,
    trigger: u64,
    rate: f64,
}

impl DsViewLayout {
    fn parse(device: &Section<'_>) -> Result<Self, String> {
        if get(device, "capturefile")? != "data" || number::<u8>(device, "device mode")? != 0 {
            return Err("DSView capture is not in logic-analyzer mode.".into());
        }
        let blocks = bounded_count(device, "total blocks", 32_768)?;
        let probes = bounded_count(device, "total probes", MAX_LANES)?;
        let samples = number::<u64>(device, "total samples")?;
        let trigger = device
            .get("trigger pos")
            .and_then(|value| value.parse().ok())
            .unwrap_or(0);
        if samples == 0 || trigger > samples {
            return Err("DSView capture has an invalid sample count or trigger position.".into());
        }
        let rate = sample_rate(get(device, "samplerate")?)?;
        Ok(Self {
            blocks,
            probes,
            samples,
            trigger,
            rate,
        })
    }

    fn stream(
        &self,
        archive: &mut Archive,
        index: usize,
        name: String,
    ) -> Result<(Stream, Lane), String> {
        let prefix = format!("L-{index}/");
        let members = number_suffix(
            archive
                .names()
                .filter_map(|name| name.strip_prefix(&prefix).map(|suffix| (suffix, name))),
            0,
        )?;
        if members.len() != self.blocks {
            return Err("DSView capture is missing digital blocks.".into());
        }
        let (chunks, capacity) = chunks(archive, members, |size| size.checked_mul(8))?;
        if capacity < self.samples || capacity - self.samples >= 8 {
            return Err("DSView capture sample count disagrees with its channel blocks.".into());
        }
        let lane = make_lane(name, self.samples, self.rate, self.trigger)?;
        Ok((
            Stream {
                chunks,
                packing: Packing::BitPlane,
                samples: self.samples,
                rate: self.rate,
            },
            lane,
        ))
    }
}

fn chunks(
    archive: &mut Archive,
    names: Vec<String>,
    samples: impl Fn(u64) -> Option<u64>,
) -> Result<(Arc<[Chunk]>, u64), String> {
    let mut position = 0_u64;
    let mut chunks = Vec::with_capacity(names.len());
    for name in names {
        let member = archive.member(&name)?;
        let end = samples(member.size)
            .filter(|size| *size > 0)
            .and_then(|size| position.checked_add(size))
            .filter(|end| *end <= 1 << 53)
            .ok_or("Invalid number of samples in logic archive.")?;
        chunks.push(Chunk {
            first: position,
            end,
            member,
        });
        position = end;
    }
    Ok((chunks.into(), position))
}

fn number_suffix<'a>(
    names: impl Iterator<Item = (&'a str, &'a str)>,
    first: usize,
) -> Result<Vec<String>, String> {
    let mut ordered = Vec::new();
    for (index, name) in names {
        let index = index
            .parse::<usize>()
            .map_err(|_| "Invalid logic archive member number.")?;
        ordered.push((index, name));
    }
    ordered.sort_unstable_by_key(|&(index, _)| index);
    if ordered
        .iter()
        .enumerate()
        .any(|(index, (n, _))| index + first != *n)
    {
        return Err("Logic archive member numbers are not consecutive.".into());
    }
    Ok(ordered
        .into_iter()
        .map(|(_, name)| name.to_owned())
        .collect())
}

fn make_lane(name: String, samples: u64, rate: f64, trigger: u64) -> Result<Lane, String> {
    let start_seconds = -(trigger as f64 / rate);
    let end_seconds = (samples - trigger) as f64 / rate;
    if !start_seconds.is_finite() || !end_seconds.is_finite() || end_seconds <= start_seconds {
        return Err("Logic archive has invalid timing information.".into());
    }
    Ok(Lane {
        name,
        start_seconds,
        end_seconds,
        sample_rate: Some(rate),
    })
}

fn probes_named(
    device: &Section<'_>,
    total: usize,
    base: usize,
) -> Result<BTreeMap<usize, String>, String> {
    let mut probes = BTreeMap::new();
    for (&key, &name) in device {
        let Some(index) = key
            .strip_prefix("probe")
            .and_then(|value| value.parse::<usize>().ok())
        else {
            continue;
        };
        let probe = index
            .checked_sub(base)
            .filter(|index| *index < total)
            .ok_or("Logic probe index falls outside the declared range.")?;
        let name = name.trim();
        let name = if name.is_empty() {
            format!("D{probe}")
        } else {
            name.chars().take(128).collect()
        };
        if probes.insert(probe, name).is_some() {
            return Err("Logic archive has duplicate probe indices.".into());
        }
    }
    Ok(probes)
}

pub(super) fn sample_rate(value: &str) -> Result<f64, String> {
    let split = value
        .find(|ch: char| ch.is_ascii_alphabetic())
        .unwrap_or(value.len());
    let (number, unit) = value.split_at(split);
    let scale = match unit.trim().to_ascii_lowercase().as_str() {
        "" | "hz" => 1.0,
        "khz" => 1_000.0,
        "mhz" => 1_000_000.0,
        "ghz" => 1_000_000_000.0,
        _ => return Err("Unsupported logic sample-rate unit.".into()),
    };
    let rate = number
        .trim()
        .parse::<f64>()
        .map_err(|_| "Invalid logic sample rate.")?
        * scale;
    if !rate.is_finite() || rate <= 0.0 {
        return Err("Invalid logic sample rate.".into());
    }
    Ok(rate)
}

type Section<'a> = BTreeMap<&'a str, &'a str>;
type Metadata<'a> = BTreeMap<&'a str, Section<'a>>;

trait ParseMetadata<'a> {
    fn parse(bytes: &'a [u8]) -> Result<Self, String>
    where
        Self: Sized;
}

impl<'a> ParseMetadata<'a> for Metadata<'a> {
    fn parse(bytes: &'a [u8]) -> Result<Self, String> {
        let mut sections = Self::new();
        let mut section = None;
        for line in utf8(bytes)?.split(['\r', '\n']) {
            let line = line.trim();
            if line.is_empty() || line.starts_with(['#', ';']) {
                continue;
            }
            if line.len() > MAX_LINE {
                return Err("Logic archive metadata line is too long.".into());
            }
            if let Some(name) = line
                .strip_prefix('[')
                .and_then(|line| line.strip_suffix(']'))
            {
                section = Some(name.trim());
                continue;
            }
            let name = section.ok_or("Logic archive field is outside a section.")?;
            let (key, value) = line
                .split_once('=')
                .ok_or("Invalid logic archive metadata field.")?;
            if sections
                .entry(name)
                .or_default()
                .insert(key.trim(), value.trim())
                .is_some()
            {
                return Err("Duplicate logic archive metadata field.".into());
            }
        }
        Ok(sections)
    }
}

fn utf8(bytes: &[u8]) -> Result<&str, String> {
    std::str::from_utf8(bytes).map_err(|_| "Logic capture metadata is not UTF-8.".into())
}

fn get<'a>(section: &Section<'a>, key: &str) -> Result<&'a str, String> {
    section
        .get(key)
        .copied()
        .ok_or_else(|| format!("Missing logic metadata field {key}."))
}

fn number<T: std::str::FromStr>(section: &Section<'_>, key: &str) -> Result<T, String> {
    get(section, key)?
        .parse()
        .map_err(|_| format!("Invalid logic metadata field {key}."))
}

fn bounded_count(section: &Section<'_>, key: &str, max: usize) -> Result<usize, String> {
    let count = number::<usize>(section, key)?;
    if !(1..=max).contains(&count) {
        return Err(format!(
            "Logic metadata field {key} is outside the supported range."
        ));
    }
    Ok(count)
}
