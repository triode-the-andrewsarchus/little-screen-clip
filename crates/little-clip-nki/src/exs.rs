// SPDX-License-Identifier: LGPL-3.0-only
// Read-only EXS subset adapted from ConvertWithMoss EXS24Block/Zone/Sample,
// Jürgen Moßgraber, copyright 2019–2026. No sample files are opened here.
use crate::{Result, reader::Reader};
use serde_json::{Value, json};
use std::collections::BTreeMap;

struct Block<'a> {
    kind: u8,
    index: u32,
    name: String,
    data: &'a [u8],
    big: bool,
}
impl<'a> Block<'a> {
    fn read(input: &mut Reader<'a>) -> Result<Self> {
        let header = input.take(84)?;
        if header[0] > 1 || header[1..3] != [1, 0] {
            return Err("Unsupported EXS record version".into());
        }
        let big = header[0] == 0;
        let magic = &header[16..20];
        let valid_magic = if big {
            matches!(magic, b"SOBT" | b"SOBJ")
        } else {
            matches!(magic, b"TBOS" | b"JBOS")
        };
        if !valid_magic {
            return Err("Invalid EXS object signature".into());
        }
        let mut fields = Reader {
            data: &header[4..],
            big,
        };
        let mut size = fields.u32()? as usize;
        if header[3] & 0x80 != 0 {
            size &= 0x7fff;
        }
        let index = fields.u32()?;
        Ok(Self {
            kind: header[3] & 15,
            index,
            name: text(&header[20..84]),
            data: input.take(size)?,
            big,
        })
    }
    fn u32(&self, at: usize) -> Result<u32> {
        Reader {
            data: self.data.get(at..).ok_or("Truncated EXS record")?,
            big: self.big,
        }
        .u32()
    }
}
pub(crate) fn recognizes(bytes: &[u8]) -> bool {
    if bytes.len() < 84 || bytes[0] > 1 || bytes[1..3] != [1, 0] || bytes[3] & 15 != 0 {
        return false;
    }
    matches!(
        (bytes[0], &bytes[16..20]),
        (0, b"SOBT" | b"SOBJ") | (1, b"TBOS" | b"JBOS")
    )
}
#[derive(Default)]
struct Inventory<'a> {
    instrument: Option<String>,
    declared: Option<[usize; 3]>,
    samples: Vec<Sample>,
    zones: Vec<Block<'a>>,
    groups: BTreeMap<u32, String>,
}
impl<'a> Inventory<'a> {
    fn push(&mut self, block: Block<'a>) -> Result<()> {
        match block.kind {
            0 => {
                self.declared = Some([
                    block.u32(4)? as usize,
                    block.u32(8)? as usize,
                    block.u32(12)? as usize,
                ]);
                if self.instrument.replace(block.name).is_some() {
                    return Err("Duplicate EXS instrument header".into());
                }
            }
            1 => {
                if self.zones.len() >= 8192 {
                    return Err("EXS has more than 8,192 zones".into());
                }
                self.zones.push(block);
            }
            2 => {
                if self.groups.insert(block.index, block.name).is_some() {
                    return Err("Duplicate EXS group index".into());
                }
            }
            3 => {
                if self.samples.len() >= 8192 {
                    return Err("EXS has too many samples".into());
                }
                self.samples.push(sample(&block)?);
            }
            _ => {}
        }
        Ok(())
    }
    fn finish(self) -> Result<Vec<Value>> {
        if self.declared != Some([self.zones.len(), self.groups.len(), self.samples.len()]) {
            return Err("EXS declared record counts do not match its contents".into());
        }
        let fallback = self.zones.iter().all(|z| z.u32(92).ok() == Some(u32::MAX));
        let regions: Vec<Value> = self
            .zones
            .iter()
            .map(|z| region(z, &self.samples, &self.groups, fallback))
            .collect::<Result<_>>()?;
        Ok(vec![
            json!({"name":self.instrument.ok_or("EXS has no instrument header")?,"regions":regions}),
        ])
    }
}
pub(crate) fn read(bytes: &[u8]) -> Result<Vec<Value>> {
    if !recognizes(bytes) {
        return Err("Not an EXS sampler instrument".into());
    }
    let mut input = Reader::new(bytes);
    let mut inventory = Inventory::default();
    let mut count = 0;
    while !input.data.is_empty() {
        count += 1;
        if count > 32768 {
            return Err("EXS has too many records".into());
        }
        inventory.push(Block::read(&mut input)?)?;
    }
    inventory.finish()
}

struct Sample {
    path: String,
    rate: u32,
    channels: u32,
    frames: u32,
}
fn sample(block: &Block<'_>) -> Result<Sample> {
    let folder = text(block.data.get(80..336).ok_or("Truncated EXS sample path")?);
    let name = if block.data.len() > 336 {
        text(
            block
                .data
                .get(336..592)
                .ok_or("Truncated EXS sample filename")?,
        )
    } else {
        block.name.clone()
    };
    let path = if folder.is_empty() {
        name
    } else {
        format!("{}/{name}", folder.trim_end_matches(['/', '\\']))
    };
    Ok(Sample {
        path: path.replace('\\', "/"),
        rate: block.u32(8)?,
        channels: block.u32(16)?,
        frames: block.u32(4)?,
    })
}
fn region(
    block: &Block<'_>,
    samples: &[Sample],
    groups: &BTreeMap<u32, String>,
    fallback: bool,
) -> Result<Value> {
    let data = block.data.get(..96).ok_or("Truncated EXS zone")?;
    let index = if fallback {
        block.index
    } else {
        block.u32(92)?
    } as usize;
    let sample = samples.get(index);
    let path = sample
        .map(|s| s.path.clone())
        .unwrap_or_else(|| format!("[Unresolved sample {index}]"));
    let key = [data[6], data[7]];
    let velocity = if data[0] & 8 != 0 {
        [data[9], data[10]]
    } else {
        [0, 127]
    };
    validate_midi(key, velocity, data[1])?;
    let start = block.u32(12)?;
    let end = block.u32(16)?;
    let loops = zone_loops(block)?;
    let properties = properties(block, sample, groups)?;
    Ok(
        json!({"name":block.name,"sample":path,"key":key,"velocity":velocity,"root":data[1],
        "start":start,"end":if end>start {Some(end)} else {None},"loops":loops,"properties":properties}),
    )
}
fn validate_midi(key: [u8; 2], velocity: [u8; 2], root: u8) -> Result<()> {
    if key[0] > key[1]
        || key[1] > 127
        || velocity[0] > velocity[1]
        || velocity[1] > 127
        || root > 127
    {
        return Err("Invalid EXS MIDI mapping".into());
    }
    Ok(())
}
fn zone_loops(block: &Block<'_>) -> Result<Vec<[u64; 2]>> {
    if block.data.get(33).is_none_or(|value| value & 1 == 0) {
        return Ok(Vec::new());
    }
    let start = u64::from(block.u32(20)?);
    let end = u64::from(block.u32(24)?);
    if end <= start {
        return Err("Invalid EXS loop range".into());
    }
    Ok(vec![[start, end]])
}
fn properties(
    block: &Block<'_>,
    sample: Option<&Sample>,
    groups: &BTreeMap<u32, String>,
) -> Result<Vec<(String, String)>> {
    let data = block.data.get(..96).ok_or("Truncated EXS zone")?;
    let mut result = vec![
        ("Fine tuning".into(), (data[2] as i8).to_string()),
        ("Coarse tuning".into(), (data[80] as i8).to_string()),
        ("Pan".into(), (data[3] as i8).to_string()),
        ("Volume adjustment".into(), (data[4] as i8).to_string()),
        ("Zone options".into(), format!("0x{:02x}", data[0])),
    ];
    if let Some(group) = groups.get(&block.u32(88)?) {
        result.push(("Group".into(), group.clone()));
    }
    if let Some(sample) = sample {
        result.extend([
            ("Sample rate".into(), sample.rate.to_string()),
            ("Channels".into(), sample.channels.to_string()),
            ("Sample frames".into(), sample.frames.to_string()),
        ]);
    }
    Ok(result)
}
fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes.split(|b| *b == 0).next().unwrap_or_default()).into_owned()
}
