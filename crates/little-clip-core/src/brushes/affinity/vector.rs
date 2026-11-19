use super::archive::{Archive, le_u32};
use crate::brushes::{
    BrushDynamics, BrushFormat, BrushLibrary, BrushPreset, BrushStrokeParams, mask,
};
use std::sync::atomic::{AtomicBool, Ordering};

const BITMAP: &[u8] = b"mByD";
const BITMAP_FORMAT_GRAY: u32 = 6;
const MAX_TEXTURE_SIDE: u32 = 8192;
const MAX_TEXTURE_PIXELS: u32 = 16 * 1024 * 1024;
const MAX_DECODED_PIXELS: usize = 128 * 1024 * 1024;
const MAX_METADATA_SIZE: usize = 4 * 1024 * 1024;
const MAX_BRUSHES: usize = 256;
const TILE_SIDE: usize = 256;
const TILE_BYTES: usize = TILE_SIDE * TILE_SIDE;

pub(super) fn count(bytes: &[u8], cancelled: &AtomicBool) -> Result<usize, &'static str> {
    let (_, graph) = load_metadata(bytes, cancelled)?;
    Ok(locate_brushes(&graph)?.len())
}

pub(super) fn load(
    bytes: &[u8],
    fallback_name: &str,
    max_brushes: usize,
    cancelled: &AtomicBool,
) -> Result<BrushLibrary, &'static str> {
    let (mut archive, graph) = load_metadata(bytes, cancelled)?;
    let bitmaps = locate_brushes(&graph)?;
    let total_count = bitmaps.len();
    let mut remaining = MAX_DECODED_PIXELS;
    let mut brushes = Vec::new();
    for (index, bitmap) in bitmaps.into_iter().enumerate() {
        if brushes.len() >= max_brushes {
            break;
        }
        let Some(texture) = Texture::read(bitmap) else {
            continue;
        };
        let pixels = texture.pixel_count();
        if pixels > remaining {
            break;
        }
        remaining -= pixels;
        let alpha = texture.decode(&mut archive, cancelled)?;
        let (width, height) = (texture.width, texture.height);
        let tip = mask::raster_at_edge(width, height, alpha, 512)
            .ok_or("Invalid Affinity brush texture")?;
        brushes.push(BrushPreset {
            name: format!("{fallback_name} #{}", index + 1),
            tip,
            params: BrushStrokeParams {
                size_px: height as f32,
                dynamics: BrushDynamics {
                    vector_texture: true,
                    ..Default::default()
                },
                ..Default::default()
            },
            detail: format!("{width}×{height} vector texture"),
        });
    }
    if brushes.is_empty() {
        return Err("No readable Affinity vector brush textures");
    }
    Ok(BrushLibrary {
        format: BrushFormat::AffinityBrushes,
        brushes,
        total_count,
    })
}

fn load_metadata<'a>(
    bytes: &'a [u8],
    cancelled: &AtomicBool,
) -> Result<(Archive<'a>, Vec<u8>), &'static str> {
    let mut archive = Archive::read(bytes, cancelled)?;
    let mut graph = Vec::new();
    archive.unpack(b"Brushes.dat", MAX_METADATA_SIZE, &mut graph)?;
    Ok((archive, graph))
}

fn locate_brushes(graph: &[u8]) -> Result<Vec<&[u8]>, &'static str> {
    if !graph.starts_with(b"\x00\xffKS\x02\x00VurB") {
        return Err("Unsupported Affinity vector brush metadata");
    }
    let count = number_after(graph, b"\xb1VurB").ok_or("Missing Affinity brush list")? as usize;
    if !(1..=MAX_BRUSHES).contains(&count) {
        return Err("Affinity vector brush count exceeds its size limit");
    }
    let mut offsets: Vec<_> = graph
        .windows(BITMAP.len())
        .enumerate()
        .filter_map(|(offset, marker)| {
            if marker != BITMAP {
                return None;
            }
            let fields = &graph[offset + BITMAP.len()..];
            // The first bitmap carries a class-version header. The same
            // class in later brush objects reuses that definition.
            let fields = fields.strip_prefix(b"\x01\0\0\x02").unwrap_or(fields);
            fields.starts_with(b"\x2atmrF").then_some(offset)
        })
        .take(count + 1)
        .collect();
    // This library layout has exactly one bitmap object per vector brush.
    if offsets.len() != count {
        return Err("Unsupported Affinity vector brush bitmap layout");
    }
    offsets.push(graph.len());
    Ok(offsets
        .windows(2)
        .map(|ranges| &graph[ranges[0]..ranges[1]])
        .collect())
}

struct Texture<'a> {
    width: u32,
    height: u32,
    states: &'a [u8],
    refs: Vec<&'a [u8]>,
}

impl<'a> Texture<'a> {
    fn read(bitmap: &'a [u8]) -> Option<Self> {
        let (width, height) = texture_dimensions(bitmap)?;
        let tiles = width.div_ceil(TILE_SIDE as u32) * height.div_ceil(TILE_SIDE as u32);
        let (states, refs) = read_tile_index(bitmap, tiles)?;
        Some(Self {
            width,
            height,
            states,
            refs,
        })
    }

    fn pixel_count(&self) -> usize {
        self.width as usize * self.height as usize
    }

    fn decode(
        &self,
        archive: &mut Archive<'_>,
        cancelled: &AtomicBool,
    ) -> Result<Vec<u8>, &'static str> {
        let mut pixels = vec![0; self.pixel_count()];
        let mut tile = Vec::new();
        let mut refs = self.refs.iter();
        for (index, state) in self.states.iter().copied().enumerate() {
            if cancelled.load(Ordering::Relaxed) {
                return Err("Cancelled");
            }
            match state {
                0 | 1 | 2 => {
                    self.write_tile(&mut pixels, index, None, if state == 2 { 0 } else { 255 })
                }
                4 => {
                    archive.unpack(
                        refs.next().ok_or("Missing Affinity texture tile")?,
                        TILE_BYTES,
                        &mut tile,
                    )?;
                    if tile.len() != TILE_BYTES {
                        return Err("Invalid Affinity texture tile size");
                    }
                    self.write_tile(&mut pixels, index, Some(&tile), 0);
                }
                _ => return Err("Unsupported Affinity texture tile type"),
            }
        }
        Ok(pixels)
    }

    fn write_tile(&self, pixels: &mut [u8], index: usize, tile: Option<&[u8]>, fill: u8) {
        let width = self.width as usize;
        let columns = width.div_ceil(TILE_SIDE);
        let (x, y) = ((index % columns) * TILE_SIDE, (index / columns) * TILE_SIDE);
        let rows = TILE_SIDE.min(self.height as usize - y);
        let columns = TILE_SIDE.min(width - x);
        for row in 0..rows {
            let start = (y + row) * width + x;
            let dst = &mut pixels[start..start + columns];
            if let Some(tile) = tile {
                let start = row * TILE_SIDE;
                for (dst, &gray) in dst.iter_mut().zip(&tile[start..start + columns]) {
                    *dst = 255 - gray;
                }
            } else {
                dst.fill(fill);
            }
        }
    }
}

fn texture_dimensions(bitmap: &[u8]) -> Option<(u32, u32)> {
    let width = number_after(bitmap, b"\x07WpmB")?;
    let height = number_after(bitmap, b"\x07HpmB")?;
    if number_after(bitmap, b"\x2atmrF")? != BITMAP_FORMAT_GRAY
        || !(1..=MAX_TEXTURE_SIDE).contains(&width)
        || !(1..=MAX_TEXTURE_SIDE).contains(&height)
        || width * height > MAX_TEXTURE_PIXELS
    {
        return None;
    }
    Some((width, height))
}

fn read_tile_index(bitmap: &[u8], tiles: u32) -> Option<(&[u8], Vec<&[u8]>)> {
    let index = find(bitmap, b"\xb11xdI")? + 5;
    let status_tag = find(bitmap, b"\x811atS")?;
    let status = status_tag + 5;
    let states = bitmap.get(status + 4..)?.get(..tiles as usize)?;
    if le_u32(bitmap, status)? != tiles {
        return None;
    }
    let stored = states.iter().filter(|&&state| state == 4).count();
    if le_u32(bitmap, index)? as usize != stored {
        return None;
    }
    let refs = tile_names(bitmap.get(index + 4..status_tag)?, stored)?;
    Some((states, refs))
}

fn tile_names(mut data: &[u8], count: usize) -> Option<Vec<&[u8]>> {
    const REFERENCE: &[u8] = b"\x33ataDItaD";
    let mut refs = Vec::with_capacity(count);
    for _ in 0..count {
        let start = find(data, REFERENCE)? + REFERENCE.len();
        let len = le_u32(data, start)? as usize;
        if !(3..=32).contains(&len) {
            return None;
        }
        let (name, rest) = data.get(start + 4..)?.split_at_checked(len)?;
        if !name.starts_with(b"d/") || !name[2..].iter().all(u8::is_ascii_alphanumeric) {
            return None;
        }
        refs.push(name);
        data = rest;
    }
    Some(refs)
}

fn number_after(data: &[u8], tag: &[u8]) -> Option<u32> {
    le_u32(data, find(data, tag)? + tag.len())
}

fn find(data: &[u8], tag: &[u8]) -> Option<usize> {
    data.windows(tag.len()).position(|window| window == tag)
}

#[cfg(test)]
mod tests {
    use super::super::archive::crc;
    use super::*;
    use crate::brushes::{BrushFormat, BrushTipKind, load_brush_bytes};

    #[test]
    fn reads_predicted_native_vector_tiles_and_checks_their_crc() {
        let token = AtomicBool::new(false);
        let mut bytes = vector_fixture();
        let library =
            load_brush_bytes(BrushFormat::AffinityBrushes, &bytes, "Pen", 1, &token).unwrap();
        assert_eq!(library.total_count, 1);
        let preset = &library.brushes[0];
        assert!(preset.params.dynamics.vector_texture);
        let BrushTipKind::AlphaMask {
            width,
            height,
            alpha,
        } = &preset.tip
        else {
            panic!("expected a recovered Affinity tile")
        };
        assert_eq!((*width, *height), (4, 2));
        assert_eq!(&**alpha, &[0, 127, 255, 0, 0, 127, 255, 0]);

        let payload = bytes
            .windows(4)
            .rposition(|bytes| bytes == b"#Fil")
            .unwrap()
            + 4;
        bytes[payload] ^= 0xff;
        assert_eq!(count(&bytes, &token), Ok(1), "counting only reads metadata");
        assert!(load_brush_bytes(BrushFormat::AffinityBrushes, &bytes, "Pen", 1, &token).is_err());
    }

    fn vector_fixture() -> Vec<u8> {
        let mut metadata = b"\x00\xffKS\x02\x00VurB\xb1VurB".to_vec();
        metadata.extend_from_slice(&1_u32.to_le_bytes());
        metadata.extend_from_slice(b"\0mByD\x01\0\0\x02\x2atmrF");
        metadata.extend_from_slice(&6_u32.to_le_bytes());
        metadata.extend_from_slice(b"\x07WpmB");
        metadata.extend_from_slice(&4_u32.to_le_bytes());
        metadata.extend_from_slice(b"\x07HpmB");
        metadata.extend_from_slice(&2_u32.to_le_bytes());
        metadata.extend_from_slice(b"\xb11xdI");
        metadata.extend_from_slice(&1_u32.to_le_bytes());
        metadata.extend_from_slice(b"\x33ataDItaD");
        metadata.extend_from_slice(&3_u32.to_le_bytes());
        metadata.extend_from_slice(b"d/1\x811atS");
        metadata.extend_from_slice(&1_u32.to_le_bytes());
        metadata.push(4);

        let mut tile = vec![255_u8; TILE_BYTES];
        for first in [0, TILE_SIDE] {
            tile[first..first + 4].copy_from_slice(&[255, 128, 0, 255]);
        }
        let mut last = 0_u8;
        let deltas: Vec<_> = tile
            .iter()
            .copied()
            .map(|byte| {
                let delta = byte.wrapping_sub(last);
                last = byte;
                delta
            })
            .collect();
        let packed_tile = zstd::bulk::compress(&deltas, 1).unwrap();
        let mut archive = vec![0; 72];
        archive[..16].copy_from_slice(b"\0\xffKA\x0b\0\0\0rArB#Inf");
        let mut index = Vec::new();
        let entries = [
            (
                b"Brushes.dat".as_slice(),
                metadata.as_slice(),
                metadata.as_slice(),
                0_u8,
            ),
            (
                b"d/1".as_slice(),
                tile.as_slice(),
                packed_tile.as_slice(),
                0x42,
            ),
        ];
        for (id, (name, decoded, packed, codec)) in entries.into_iter().enumerate() {
            index.extend_from_slice(&(id as u32 + 1).to_le_bytes());
            index.push(0);
            index.extend_from_slice(&(archive.len() as u64).to_le_bytes());
            index.extend_from_slice(&(decoded.len() as u64).to_le_bytes());
            index.extend_from_slice(&(packed.len() as u64).to_le_bytes());
            index.extend_from_slice(&crc(decoded).to_le_bytes());
            index.push(codec);
            index.extend_from_slice(&7_u32.to_le_bytes());
            index.extend_from_slice(&crc(packed).to_le_bytes());
            index.extend_from_slice(&(name.len() as u16).to_le_bytes());
            index.extend_from_slice(name);
            archive.extend_from_slice(b"#Fil");
            archive.extend_from_slice(packed);
        }
        let fat = archive.len();
        archive[16..24].copy_from_slice(&(fat as u64).to_le_bytes());
        archive.resize(fat + 59, 0);
        archive[fat..fat + 4].copy_from_slice(b"#FT4");
        archive[fat + 44..fat + 48].copy_from_slice(&2_u32.to_le_bytes());
        archive[fat + 52..fat + 56].copy_from_slice(&(index.len() as u32).to_le_bytes());
        archive.extend_from_slice(&index);
        archive
    }
}
