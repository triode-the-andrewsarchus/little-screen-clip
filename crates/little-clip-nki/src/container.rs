// SPDX-License-Identifier: LGPL-3.0-only
// Read subset adapted from ConvertWithMoss NIContainerItem/DataChunk and
// PresetChunkData/SubTreeItemChunkData, Jürgen Moßgraber, copyright 2019–2026.
use crate::{
    Result,
    reader::{Reader, fastlz},
};
#[derive(Default)]
struct Inventory {
    preset: Option<Vec<u8>>,
    kontakt: bool,
    nodes: usize,
    expanded: usize,
}
impl Inventory {
    fn budget(&mut self, depth: usize) -> Result<()> {
        self.nodes += 1;
        if self.nodes > 100_000 || depth > 64 {
            return Err("NKI container nesting/count exceeds limits".into());
        }
        Ok(())
    }
    fn item(&mut self, input: &mut Reader<'_>, depth: usize) -> Result<()> {
        self.budget(depth)?;
        let mut r = Reader::new(input.block()?);
        if r.u32()? != 1 || r.take(4)? != b"hsin" {
            return Err("Invalid NI sound container".into());
        }
        r.take(24)?;
        self.chunk(&mut r, depth + 1)?;
        r.u32()?;
        let n = r.u32()?;
        if n > 8192 {
            return Err("Too many NKI child items".into());
        }
        for _ in 0..n {
            r.take(12)?;
            self.item(&mut r, depth + 1)?;
        }
        Ok(())
    }
    fn chunk(&mut self, input: &mut Reader<'_>, depth: usize) -> Result<()> {
        self.budget(depth)?;
        let mut r = Reader::new(input.block()?);
        r.take(4)?;
        let kind = r.u32()?;
        if r.u32()? != 1 {
            return Err("Unsupported NI chunk version".into());
        }
        if kind != 1 {
            self.chunk(&mut r, depth + 1)?;
        }
        match kind {
            101 => self.kontakt |= author(&mut r)?,
            109 => {
                if self.preset.is_some() {
                    return Err("Duplicate NKI preset".into());
                }
                self.preset = Some(preset_data(&mut r)?);
            }
            115 => self.subtree(&mut r, depth + 1)?,
            _ => {}
        }
        Ok(())
    }
    fn subtree(&mut self, r: &mut Reader<'_>, depth: usize) -> Result<()> {
        if r.u32()? != 1 {
            return Err("Unsupported NI subtree version".into());
        }
        if r.u8()? == 0 {
            return self.item(r, depth);
        }
        let expected = r.u32()? as usize;
        self.expanded = self
            .expanded
            .checked_add(expected)
            .ok_or("NKI expansion overflow")?;
        if self.expanded > crate::PAYLOAD {
            return Err("NKI subtree expansion exceeds 16 MiB".into());
        }
        let compressed = r.sized()?;
        let expanded = fastlz(compressed, expected)?;
        self.item(&mut Reader::new(&expanded), depth)
    }
}
pub(crate) fn preset(data: &[u8]) -> Result<Vec<u8>> {
    let mut inventory = Inventory::default();
    inventory.item(&mut Reader::new(data), 0)?;
    if !inventory.kontakt {
        return Err("NI container is not a Kontakt instrument".into());
    }
    inventory
        .preset
        .ok_or("No readable NKI preset (possibly protected)".into())
}

fn author(r: &mut Reader<'_>) -> Result<bool> {
    if r.u32()? != 1 {
        return Err("Unsupported author metadata".into());
    }
    r.u8()?;
    Ok(r.u32()? == 2)
}
fn preset_data(r: &mut Reader<'_>) -> Result<Vec<u8>> {
    if r.u32()? != 1 {
        return Err("Unsupported preset dictionary".into());
    }
    r.u32()?;
    if r.u32()? != 1 {
        return Err("Unsupported preset dictionary count".into());
    }
    let size = r.u32()? as usize;
    r.u32()?;
    if size > crate::PAYLOAD {
        return Err("Oversized NKI preset".into());
    }
    Ok(r.take(size)?.to_vec())
}
