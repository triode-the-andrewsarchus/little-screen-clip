//! Bounded SoundFont 2 RIFF tables. Embedded samples remain borrowed byte ranges.
use super::{Document, EmbeddedSample, Instrument, Pcm16Sample, Region};
use std::{cell::Cell, collections::BTreeMap, sync::Arc};
type Generators = BTreeMap<u16, i32>;
/// Reads SoundFont presets and key/velocity zones without synthesizing their modulators.
///
/// # Errors
/// Rejects malformed RIFF/table bounds, invalid references, ROM samples and exceeded budgets.
pub fn parse(bytes: Arc<[u8]>) -> Result<Document, String> {
    let bank = Bank::open(&bytes)?;
    let instruments = bank.presets()?;
    let document=Document {instruments,notes:vec!["SoundFont key/velocity mappings with raw 16-bit sample audition. Preset/instrument ranges and sample offsets are applied; envelopes, modulators, filters and tuning are shown where available, not synthesized. Linked stereo channels are inspected/auditioned individually; ROM and compressed SF3 samples are unsupported.".into()],pcm:Some(bytes)};
    document.validate()?;
    Ok(document)
}
struct Bank<'a> {
    bytes: &'a [u8],
    tables: BTreeMap<[u8; 4], std::ops::Range<usize>>,
    pcm: std::ops::Range<usize>,
    work: Cell<usize>,
}
impl<'a> Bank<'a> {
    fn open(bytes: &'a [u8]) -> Result<Self, String> {
        let end = riff_end(bytes)?;
        let mut tables = BTreeMap::new();
        let mut samples = None;
        for (tag, range) in chunks(bytes, 12..end)? {
            if tag != *b"LIST" {
                continue;
            }
            index_list(bytes, range, &mut tables, &mut samples)?;
        }
        let pcm =
            samples.ok_or("SoundFont has no 16-bit PCM table (compressed SF3 is unsupported)")?;
        let bank = Self {
            bytes,
            tables,
            pcm,
            work: Cell::new(1_000_000),
        };
        bank.validate()?;
        Ok(bank)
    }
    fn presets(&self) -> Result<Vec<Instrument>, String> {
        let presets = self.table(b"phdr", 38)?;
        let mut instruments = Vec::new();
        let mut total = 0;
        if presets.len() / 38 > 257 {
            return Err("More than 256 SoundFont presets".into());
        }
        for (index, header) in presets
            .chunks_exact(38)
            .enumerate()
            .take(presets.len() / 38 - 1)
        {
            let next = &presets[(index + 1) * 38..];
            let mut regions =
                self.preset(u16_at(header, 24)? as usize..u16_at(next, 24)? as usize)?;
            total += regions.len();
            if total > 8192 {
                return Err("More than 8,192 SoundFont regions".into());
            }
            for region in &mut regions {
                region.properties.push((
                    "Preset".into(),
                    format!(
                        "Bank {} · program {}",
                        u16_at(header, 22)?,
                        u16_at(header, 20)?
                    ),
                ));
            }
            instruments.push(Instrument {
                name: name(&header[..20]),
                regions,
            });
        }
        Ok(instruments)
    }
    fn table(&self, tag: &[u8; 4], size: usize) -> Result<&'a [u8], String> {
        let range = self
            .tables
            .get(tag)
            .ok_or_else(|| format!("Missing SoundFont table {}", String::from_utf8_lossy(tag)))?;
        let data = &self.bytes[range.clone()];
        if data.len() < size || data.len() % size != 0 || data.len() / size > 65536 {
            return Err("SoundFont table has invalid size or exceeds 65,536 records".into());
        }
        Ok(data)
    }
    fn validate(&self) -> Result<(), String> {
        self.table(b"inst", 22)?;
        self.table(b"shdr", 46)?;
        self.table(b"phdr", 38)?;
        for (bag, generator) in [(b"pbag", b"pgen"), (b"ibag", b"igen")] {
            let bags = self.table(bag, 4)?;
            let generators = self.table(generator, 4)?;
            let mut previous = 0;
            for bag in bags.chunks_exact(4) {
                let offset = u16_at(bag, 0)? as usize;
                if offset < previous || offset > generators.len() / 4 {
                    return Err("Invalid SoundFont generator index".into());
                }
                previous = offset;
            }
        }
        Ok(())
    }
    fn zones(
        &self,
        range: std::ops::Range<usize>,
        bag: &[u8; 4],
        generator: &[u8; 4],
    ) -> Result<Vec<Generators>, String> {
        let bags = self.table(bag, 4)?;
        let generators = self.table(generator, 4)?;
        if range.start > range.end || range.end >= bags.len() / 4 {
            return Err("Invalid SoundFont bag range".into());
        }
        let mut zones = Vec::new();
        for index in range {
            let start = u16_at(bags, index * 4)? as usize * 4;
            let end = u16_at(bags, (index + 1) * 4)? as usize * 4;
            let zone = self.generators(&generators[start..end])?;
            zones.push(zone);
        }
        Ok(zones)
    }
    fn generators(&self, data: &[u8]) -> Result<Generators, String> {
        let cost = 1 + data.len() / 4;
        let remaining = self
            .work
            .get()
            .checked_sub(cost)
            .ok_or("SoundFont mapping exceeds the interpretation budget")?;
        self.work.set(remaining);
        let mut zone = Generators::new();
        for item in data.chunks_exact(4) {
            let opcode = u16_at(item, 0)?;
            let raw = u16_at(item, 2)?;
            // Unknown extensions are not interpreted; retaining them could amplify inherited maps.
            if opcode > 60 {
                continue;
            }
            if matches!(opcode, 43 | 44) && (raw as u8 > (raw >> 8) as u8 || (raw >> 8) > 127) {
                return Err("Invalid SoundFont MIDI range".into());
            }
            zone.insert(
                opcode,
                if matches!(opcode, 41 | 43 | 44 | 53 | 54 | 58) {
                    i32::from(raw)
                } else {
                    i32::from(raw as i16)
                },
            );
        }
        Ok(zone)
    }
    fn preset(&self, range: std::ops::Range<usize>) -> Result<Vec<Region>, String> {
        let zones = self.zones(range, b"pbag", b"pgen")?;
        let mut global = Generators::new();
        let mut regions = Vec::new();
        for zone in zones {
            let Some(&instrument) = zone.get(&41) else {
                global = zone;
                continue;
            };
            let mut preset = global.clone();
            preset.extend(zone);
            regions.extend(self.instrument(instrument as usize, &preset)?);
            if regions.len() > 8192 {
                return Err("More than 8,192 regions in a SoundFont preset".into());
            }
        }
        Ok(regions)
    }
    fn instrument(&self, index: usize, preset: &Generators) -> Result<Vec<Region>, String> {
        let instruments = self.table(b"inst", 22)?;
        if index + 1 >= instruments.len() / 22 {
            return Err("SoundFont instrument index is invalid".into());
        }
        let header = &instruments[index * 22..];
        let next = &instruments[(index + 1) * 22..];
        let zones = self.zones(
            u16_at(header, 20)? as usize..u16_at(next, 20)? as usize,
            b"ibag",
            b"igen",
        )?;
        let mut global = Generators::new();
        let mut regions = Vec::new();
        for zone in zones {
            if !zone.contains_key(&53) {
                global = zone;
                continue;
            }
            let mut fields = global.clone();
            fields.extend(zone);
            let Some(key) = intersection(preset, &fields, 43) else {
                continue;
            };
            let Some(velocity) = intersection(preset, &fields, 44) else {
                continue;
            };
            let mut region = self.sample(&fields, preset)?;
            region.key = key;
            region.velocity = velocity;
            region
                .properties
                .push(("Instrument".into(), name(&header[..20])));
            regions.push(region);
            if regions.len() > 8192 {
                return Err("More than 8,192 regions in a SoundFont instrument".into());
            }
        }
        Ok(regions)
    }
    fn sample(&self, fields: &Generators, preset: &Generators) -> Result<Region, String> {
        let samples = self.table(b"shdr", 46)?;
        let index = fields[&53] as usize;
        if index + 1 >= samples.len() / 46 {
            return Err("SoundFont sample index is invalid".into());
        }
        let header = &samples[index * 46..(index + 1) * 46];
        let kind = u16_at(header, 44)?;
        if kind & 0x8000 != 0 {
            return Err("SoundFont ROM samples are unsupported".into());
        }
        let (sample, frames, loops) = self.sample_data(header, fields, preset)?;
        let rate = sample.rate;
        let supplied_root = *fields
            .get(&58)
            .filter(|value| **value != 65535)
            .unwrap_or(&i32::from(header[40]));
        let root = if supplied_root == 255 {
            60
        } else {
            u8::try_from(supplied_root).map_err(|_| "Invalid root key")?
        };
        Ok(Region {
            name: name(&header[..20]),
            sample: name(&header[..20]),
            key: [0, 127],
            velocity: [0, 127],
            root,
            start: 0,
            end: Some(frames),
            loops,
            properties: vec![
                ("Sample rate".into(), format!("{rate} Hz")),
                (
                    "Sample type".into(),
                    format!("{kind} · linked sample {}", u16_at(header, 42)?),
                ),
                (
                    "Tuning".into(),
                    format!(
                        "{} semitones · {} cents · sample correction {} cents",
                        sum(fields, preset, 51),
                        sum(fields, preset, 52),
                        header[41] as i8
                    ),
                ),
                (
                    "Sample mode".into(),
                    fields.get(&54).unwrap_or(&0).to_string(),
                ),
            ],
            embedded: Some(EmbeddedSample::Pcm16(sample)),
        })
    }
    fn sample_data(
        &self,
        header: &[u8],
        fields: &Generators,
        preset: &Generators,
    ) -> Result<(Pcm16Sample, u64, Vec<[u64; 2]>), String> {
        let start = i64::from(u32_at(header, 20)?) + offset(fields, preset, 0, 4);
        let end = i64::from(u32_at(header, 24)?) + offset(fields, preset, 1, 12);
        let loop_start = i64::from(u32_at(header, 28)?) + offset(fields, preset, 2, 45);
        let loop_end = i64::from(u32_at(header, 32)?) + offset(fields, preset, 3, 50);
        if start < 0 || end <= start || end as u64 > (self.pcm.len() / 2) as u64 {
            return Err("SoundFont sample addresses exceed PCM bounds".into());
        }
        let rate = u32_at(header, 36)?;
        if !(400..=384000).contains(&rate) {
            return Err("Invalid SoundFont sample rate".into());
        }
        let loops = if loop_start >= start && loop_end > loop_start && loop_end <= end {
            vec![[(loop_start - start) as u64, (loop_end - start) as u64]]
        } else {
            Vec::new()
        };
        Ok((
            Pcm16Sample {
                bytes: self.pcm.start + start as usize * 2..self.pcm.start + end as usize * 2,
                rate,
            },
            (end - start) as u64,
            loops,
        ))
    }
}
fn riff_end(bytes: &[u8]) -> Result<usize, String> {
    if bytes.len() > 256 * 1024 * 1024
        || bytes.get(..4) != Some(b"RIFF")
        || bytes.get(8..12) != Some(b"sfbk")
    {
        return Err("Expected SoundFont 2 RIFF, at most 256 MiB".into());
    }
    let end = 12_usize
        .checked_add(u32_at(&bytes, 4)? as usize)
        .and_then(|n| n.checked_sub(4))
        .ok_or("RIFF size overflow")?;
    if end != bytes.len() {
        return Err("SoundFont RIFF length disagrees with input".into());
    }
    Ok(end)
}
fn index_list(
    bytes: &[u8],
    range: std::ops::Range<usize>,
    tables: &mut BTreeMap<[u8; 4], std::ops::Range<usize>>,
    samples: &mut Option<std::ops::Range<usize>>,
) -> Result<(), String> {
    let list = bytes
        .get(range.start..range.start + 4)
        .ok_or("Missing LIST type")?;
    for (tag, child) in chunks(bytes, range.start + 4..range.end)? {
        if list == b"pdta" {
            if tables.insert(tag, child).is_some() {
                return Err("Duplicate SoundFont table".into());
            }
        } else if list == b"sdta" && tag == *b"smpl" {
            if samples.replace(child).is_some() {
                return Err("Duplicate PCM table".into());
            }
        }
    }
    Ok(())
}
fn sum(a: &Generators, b: &Generators, opcode: u16) -> i64 {
    i64::from(*a.get(&opcode).unwrap_or(&0)) + i64::from(*b.get(&opcode).unwrap_or(&0))
}
fn offset(a: &Generators, b: &Generators, fine: u16, coarse: u16) -> i64 {
    sum(a, b, fine) + sum(a, b, coarse) * 32768
}
fn intersection(a: &Generators, b: &Generators, opcode: u16) -> Option<[u8; 2]> {
    let decode = |g: &Generators| {
        g.get(&opcode)
            .map_or([0, 127], |n| [*n as u8, (*n >> 8) as u8])
    };
    let a = decode(a);
    let b = decode(b);
    let result = [a[0].max(b[0]), a[1].min(b[1])];
    (result[0] <= result[1] && result[1] <= 127).then_some(result)
}
fn name(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes.split(|b| *b == 0).next().unwrap_or_default())
        .trim()
        .to_owned()
}
fn u16_at(bytes: &[u8], at: usize) -> Result<u16, String> {
    let b = bytes.get(at..at + 2).ok_or("Truncated SoundFont field")?;
    Ok(u16::from_le_bytes(b.try_into().unwrap()))
}
fn u32_at(bytes: &[u8], at: usize) -> Result<u32, String> {
    let b = bytes.get(at..at + 4).ok_or("Truncated SoundFont field")?;
    Ok(u32::from_le_bytes(b.try_into().unwrap()))
}
fn chunks(
    bytes: &[u8],
    range: std::ops::Range<usize>,
) -> Result<Vec<([u8; 4], std::ops::Range<usize>)>, String> {
    let mut at = range.start;
    let mut chunks = Vec::new();
    while at < range.end {
        let tag = bytes
            .get(at..at + 4)
            .ok_or("Truncated RIFF tag")?
            .try_into()
            .unwrap();
        let length = u32_at(bytes, at + 4)? as usize;
        let start = at + 8;
        let end = start
            .checked_add(length)
            .filter(|n| *n <= range.end)
            .ok_or("RIFF chunk exceeds its parent")?;
        chunks.push((tag, start..end));
        if chunks.len() > 128 {
            return Err("Too many SoundFont chunks".into());
        }
        at = end
            .checked_add(length % 2)
            .filter(|n| *n <= range.end)
            .ok_or("Truncated RIFF padding")?;
    }
    Ok(chunks)
}

#[cfg(test)]
mod tests {
    use super::*;
    const FIXTURE: &[u8] = include_bytes!("../../tests/fixtures/multisample/harbor.sf2");
    #[test]
    fn presets_ranges_velocity_layers_and_pcm_offsets_are_preserved() {
        let doc = parse(Arc::from(FIXTURE)).unwrap();
        assert_eq!(doc.instruments.len(), 2);
        let keys = &doc.instruments[0];
        assert_eq!(keys.name, "Harbor Keys");
        assert_eq!(keys.regions.len(), 2);
        assert_eq!(keys.regions[0].key, [48, 71]);
        assert_eq!(keys.regions[0].velocity, [0, 63]);
        assert_eq!(keys.regions[1].velocity, [64, 127]);
        assert_eq!(keys.regions[1].root, 60);
        assert_eq!(keys.regions[0].loops, vec![[10, 22050]]);
        let EmbeddedSample::Pcm16(sample) = keys.regions[1].embedded.as_ref().unwrap() else {
            panic!()
        };
        assert_eq!(sample.rate, 22050);
        assert_eq!(sample.bytes.len(), 88200);
        let pcm = doc.pcm.as_ref().unwrap();
        let raw = &pcm[sample.bytes.clone()];
        assert!(
            raw.chunks_exact(2)
                .any(|b| i16::from_le_bytes([b[0], b[1]]).abs() > 20000)
        );
        assert_eq!(doc.instruments[1].regions[0].key, [36, 47]);
        assert_eq!(doc.instruments[1].regions[0].root, 48);
    }
    #[test]
    fn default_root_override_uses_the_sample_header_pitch() {
        let mut bytes = FIXTURE.to_vec();
        let at = bytes.windows(4).position(|w| w == b"igen").unwrap() + 8;
        let marker = bytes[at..]
            .windows(4)
            .position(|w| w == [58, 0, 48, 0])
            .unwrap()
            + at;
        bytes[marker + 2..marker + 4].copy_from_slice(&65535_u16.to_le_bytes());
        let doc = parse(bytes.into()).unwrap();
        assert_eq!(doc.instruments[1].regions[0].root, 60);
    }
    #[test]
    fn malformed_tables_references_and_sample_addresses_fail_without_panics() {
        for length in [0, 4, 11, 100, FIXTURE.len() - 1] {
            assert!(parse(Arc::from(&FIXTURE[..length])).is_err());
        }
        for (tag, relative, value) in [
            (b"shdr", 8 + 20, u32::MAX),
            (b"phdr", 8 + 24, 65535),
            (b"pbag", 8, 65535),
            (b"igen", 8 + 14, 65535),
        ] {
            let mut bytes = FIXTURE.to_vec();
            let at = bytes.windows(4).position(|w| w == tag).unwrap() + relative;
            bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
            assert!(parse(bytes.into()).is_err());
        }
        let budget = Bank::open(FIXTURE).unwrap();
        budget.work.set(0);
        assert!(
            budget
                .presets()
                .err()
                .unwrap()
                .contains("interpretation budget")
        );
        let doc = parse(Arc::from(FIXTURE)).unwrap();
        let bank = Bank {
            bytes: FIXTURE,
            tables: BTreeMap::new(),
            pcm: 0..0,
            work: Cell::new(0),
        };
        assert!(bank.table(b"inst", 22).is_err());
        assert_eq!(doc.instruments[0].regions.len(), 2);
    }
}
