// SPDX-License-Identifier: LGPL-3.0-only
// Mapping read subset adapted from ConvertWithMoss KontaktPresetChunk, Program,
// Zone and FileList, Jürgen Moßgraber, copyright 2019–2026. See ../README.md.
use crate::{Result, reader::Reader};
use serde_json::{Value, json};
struct Chunk<'a> {
    id: u16,
    version: u16,
    data: &'a [u8],
    children: Vec<Chunk<'a>>,
}
struct Budget {
    nodes: usize,
}
impl Budget {
    fn admit(&mut self, depth: usize) -> Result<()> {
        self.nodes += 1;
        if depth > 64 || self.nodes > 100_000 {
            return Err("NKI preset nesting/count exceeds limits".into());
        }
        Ok(())
    }
    fn chunks<'a>(&mut self, mut r: Reader<'a>, depth: usize) -> Result<Vec<Chunk<'a>>> {
        let mut result = Vec::new();
        while !r.data.is_empty() {
            self.admit(depth)?;
            let id = r.u16()?;
            let data = r.sized()?;
            let chunk = match id {
                0x03 | 0x28 | 0x29 => self.structure(id, &mut Reader::new(data), depth + 1)?,
                0x33 | 0x34 => self.array(id, data, depth + 1)?,
                _ => Chunk {
                    id,
                    version: 0,
                    data,
                    children: Vec::new(),
                },
            };
            result.push(chunk);
        }
        Ok(result)
    }
    fn structure<'a>(&mut self, id: u16, r: &mut Reader<'a>, depth: usize) -> Result<Chunk<'a>> {
        self.admit(depth)?;
        if r.u8()? == 0 {
            // Unstructured array entries have no independent length. Reject them
            // rather than consuming the following entries as one public section.
            return Err("Unstructured NKI program/zone is unsupported".into());
        }
        let version = r.u16()?;
        r.sized()?;
        let data = r.sized()?;
        let children = self.chunks(Reader::new(r.sized()?), depth + 1)?;
        Ok(Chunk {
            id,
            version,
            data,
            children,
        })
    }
    fn array<'a>(&mut self, id: u16, data: &'a [u8], depth: usize) -> Result<Chunk<'a>> {
        let mut r = Reader::new(data);
        let n = r.u32()?;
        if n > 8192 {
            return Err("Too many NKI zones/groups".into());
        }
        let mut children = Vec::new();
        for _ in 0..n {
            if id == 0x34 {
                r.u32()?;
            }
            children.push(self.structure(if id == 0x34 { 0x2c } else { 4 }, &mut r, depth + 1)?);
        }
        Ok(Chunk {
            id,
            version: 0,
            data: &[],
            children,
        })
    }
}
fn find<'a, 'b>(chunks: &'b [Chunk<'a>], id: u16) -> Option<&'b Chunk<'a>> {
    chunks
        .iter()
        .find(|c| c.id == id)
        .or_else(|| chunks.iter().find_map(|c| find(&c.children, id)))
}
pub(crate) fn read(data: &[u8]) -> Result<Vec<Value>> {
    if data.len() > crate::PAYLOAD {
        return Err("NKI preset exceeds 16 MiB".into());
    }
    let chunks = Budget { nodes: 0 }.chunks(Reader::new(data), 0)?;
    let paths = find(&chunks, 0x3d)
        .or_else(|| find(&chunks, 0x4b))
        .map(file_list)
        .transpose()?
        .unwrap_or_default();
    let mut result = Vec::new();
    programs(&chunks, &paths, &mut result, &mut 0)?;
    if result.is_empty() {
        return Err("No readable Kontakt program".into());
    }
    Ok(result)
}
fn programs(
    chunks: &[Chunk<'_>],
    paths: &[String],
    result: &mut Vec<Value>,
    region_count: &mut usize,
) -> Result<()> {
    for chunk in chunks {
        if chunk.id == 0x28 {
            if result.len() >= 256 {
                return Err("Too many NKI programs".into());
            }
            let name = Reader::new(chunk.data).text()?;
            let mut regions = Vec::new();
            if let Some(zones) = chunk.children.iter().find(|c| c.id == 0x34) {
                for zone in &zones.children {
                    *region_count += 1;
                    if *region_count > 8192 {
                        return Err("More than 8,192 NKI regions".into());
                    }
                    regions.push(region(zone, paths)?);
                }
            }
            result.push(json!({"name":name,"regions":regions}));
        } else {
            programs(&chunk.children, paths, result, region_count)?;
        }
    }
    Ok(())
}
fn region(chunk: &Chunk<'_>, paths: &[String]) -> Result<Value> {
    if !(0x80..=0x9c).contains(&chunk.version) {
        return Err("Unsupported NKI zone version".into());
    }
    let mut r = Reader::new(chunk.data);
    let start = r.u32()?;
    let end = r.u32()?;
    r.u32()?;
    let Mapping {
        key,
        velocity,
        root,
    } = Mapping::read(&mut r)?;
    let volume = r.f32()?;
    let pan = r.f32()?;
    let tune = r.f32()?;
    let SampleInfo { id, rate, channels } = SampleInfo::read(&mut r, chunk.version)?;
    let sample = paths
        .get(id)
        .cloned()
        .unwrap_or_else(|| format!("[Embedded or unresolved sample {id}]"));
    let loops = chunk
        .children
        .iter()
        .find(|c| c.id == 0x39)
        .map(|c| loops(c.data))
        .transpose()?
        .unwrap_or_default();
    Ok(
        json!({"name":sample,"sample":sample,"key":key,"velocity":velocity,"root":root,
        "start":start,"end":if end>start {Some(end)} else {None},"loops":loops,
        "properties":[["Tuning",tune.to_string()],["Volume",volume.to_string()],
        ["Pan",pan.to_string()],["Sample rate",rate.to_string()],["Channels",channels.to_string()]]}),
    )
}
fn loops(data: &[u8]) -> Result<Vec<[u64; 2]>> {
    if data.len() < 2 {
        return Ok(Vec::new());
    }
    let mut r = Reader::new(data);
    let flags = r.u16()?;
    let mut result = Vec::new();
    for i in 0..8 {
        if flags & (1 << i) == 0 {
            continue;
        }
        if r.u16()? != 0x60 {
            return Err("Unsupported NKI loop record".into());
        }
        r.u32()?;
        let start = r.u32()? as u64;
        let length = r.u32()? as u64;
        result.push([start, start + length]);
        r.take(13)?;
        if !r.data.is_empty() {
            r.u8()?;
        }
    }
    Ok(result)
}
fn file_list(chunk: &Chunk<'_>) -> Result<Vec<String>> {
    let mut r = Reader::new(chunk.data);
    if chunk.id == 0x4b {
        match r.u16()? {
            2 => {}
            3 => return files_v3(&mut r),
            _ => return Err("Unsupported NKI filename table version".into()),
        }
    }
    files(&mut r)?;
    files(&mut r)
}
fn files(r: &mut Reader<'_>) -> Result<Vec<String>> {
    if r.data.is_empty() {
        return Ok(Vec::new());
    }
    let count = r.u32()?;
    if count > 8192 {
        return Err("Too many NKI filenames".into());
    }
    (0..count).map(|_| filename(r)).collect()
}
fn files_v3(r: &mut Reader<'_>) -> Result<Vec<String>> {
    let count = r.u32()?;
    if count > 8193 {
        return Err("Too many NKI filenames".into());
    }
    r.take(8)?;
    let mut result = Vec::new();
    for _ in 0..count.saturating_sub(1) {
        result.push(filename(r)?);
        r.take(28)?;
    }
    r.take(24)?;
    Ok(result)
}
fn filename(r: &mut Reader<'_>) -> Result<String> {
    let count = r.u32()?;
    if count > 256 {
        return Err("Too many NKI path segments".into());
    }
    let mut path = String::new();
    for _ in 0..count {
        match r.u8()? {
            0 => {
                let drive = String::from_utf8_lossy(r.take(2)?).trim().to_owned();
                path.push_str(&drive);
                path.push_str(if drive.is_empty() { "/" } else { ":/" });
            }
            1 => {
                let drive = r.text()?;
                path.push_str(&drive);
                path.push_str(if drive.is_empty() { "/" } else { ":/" });
            }
            2 => {
                path.push_str(&r.text()?);
                path.push('/');
            }
            3 => path.push_str("../"),
            4 | 8 | 9 => path.push_str(&r.text()?),
            6 => {}
            _ => return Err("Unsupported NKI path segment".into()),
        }
        if path.len() > 4096 {
            return Err("NKI sample path exceeds 4 KiB".into());
        }
    }
    Ok(path.replace('\\', "/"))
}

struct Mapping {
    key: [u16; 2],
    velocity: [u16; 2],
    root: u16,
}
impl Mapping {
    fn read(r: &mut Reader<'_>) -> Result<Self> {
        let velocity = [r.u16()?, r.u16()?];
        let key = [r.u16()?, r.u16()?];
        r.take(8)?;
        let root = r.u16()?;
        if key[0] > key[1]
            || key[1] > 127
            || velocity[0] > velocity[1]
            || velocity[1] > 127
            || root > 127
        {
            return Err("Invalid NKI key/velocity range".into());
        }
        Ok(Self {
            key,
            velocity,
            root,
        })
    }
}
struct SampleInfo {
    id: usize,
    rate: u32,
    channels: u8,
}
impl SampleInfo {
    fn read(r: &mut Reader<'_>, version: u16) -> Result<Self> {
        if version >= 0x9a {
            r.take(6)?;
        }
        if r.data.is_empty() {
            return Err("Script-only zone has no sample mapping".into());
        }
        let id = r.u32()? as usize;
        r.u32()?;
        let rate = r.u32()?;
        let channels = r.u8()?;
        Ok(Self { id, rate, channels })
    }
}
