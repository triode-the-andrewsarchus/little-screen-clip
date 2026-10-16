//! Bounded SPZ containers around the upstream packed-attribute decoder.
//!
//! The v4 table/stream layout follows Niantic's format specification and
//! loadPackedGaussiansFromNgsp at affd0ecea7fbb4c265ee119475af7ee5b2997482.
//! https://github.com/nianticlabs/spz

use crate::SceneError;
use std::io::{BufRead, BufReader, Cursor, Read};
use std::path::Path;
use wgpu_3dgs_viewer::core::{Gaussian, IterGaussian, SpzGaussians, SpzGaussiansHeader};

const MAX_BYTES: u64 = 512 * 1024 * 1024;

fn invalid(message: &str) -> SceneError {
    SceneError::Invalid(message.into())
}

pub(crate) fn load(path: &Path) -> Result<Vec<Gaussian>, SceneError> {
    let file = little_clip_core::kernel_interface::open_content(path, true)?;
    if file.metadata()?.len() > MAX_BYTES {
        return Err(invalid("SPZ exceeds the 512 MiB import budget"));
    }
    let mut reader = BufReader::new(file.take(MAX_BYTES + 1));
    let splats = if reader.fill_buf()?.starts_with(b"NGSP") {
        let bytes = crate::read_bounded(reader, MAX_BYTES, "SPZ")?;
        decode_v4(&bytes)?
    } else {
        let mut decoder = flate2::read::GzDecoder::new(reader);
        let mut bytes = [0; 16];
        decoder.read_exact(&mut bytes)?;
        let header = legacy_header(&bytes)?;
        let splats = SpzGaussians::read_gaussians(&mut decoder, header)?;
        // Reaching EOF checks the gzip checksum without inflating unbounded
        // trailing content absent from the declared attribute arrays.
        if decoder.read(&mut [0u8; 1])? != 0 {
            return Err(invalid("SPZ contains trailing point data"));
        }
        splats
    };
    Ok(splats.iter_gaussian().collect())
}

fn legacy_header(bytes: &[u8; 16]) -> Result<SpzGaussiansHeader, SceneError> {
    // Upstream validates magic/version, but relies on valid degree/shift values
    // in its attribute reader. Reject them before handing it any untrusted data.
    if bytes[12] > 3 || bytes[13] > 30 || bytes[14] & !1 != 0 || bytes[15] != 0 {
        return Err(invalid(
            "Unsupported SPZ degree, coordinate precision or flags",
        ));
    }
    let header = SpzGaussians::read_header(&mut bytes.as_slice())?;
    if header.num_points() == 0 || header.num_points() > 2_000_000 {
        return Err(invalid("SPZ exceeds the two-million-point import budget"));
    }
    Ok(header)
}

fn decode_v4(bytes: &[u8]) -> Result<SpzGaussians, SceneError> {
    let header = bytes
        .get(..32)
        .ok_or_else(|| invalid("Truncated SPZ v4 header"))?;
    if header[4..8] != 4u32.to_le_bytes() || header[20..].iter().any(|byte| *byte != 0) {
        return Err(invalid("Unsupported SPZ version or reserved header fields"));
    }
    // Extended coordinate systems can change SH orientation. Reject them
    // explicitly rather than display silently incorrect positions or colors.
    if header[14] & !1 != 0 || header[16..20] != 32u32.to_le_bytes() {
        return Err(invalid("SPZ v4 vendor extensions are unsupported"));
    }
    let mut legacy = [0; 16];
    legacy[..15].copy_from_slice(&header[..15]);
    legacy[4..8].copy_from_slice(&3u32.to_le_bytes());
    let decoded_header = legacy_header(&legacy)?;
    let count = decoded_header.num_points();
    let sizes = [
        9,
        1,
        3,
        3,
        4,
        decoded_header.sh_degree().num_coefficients() * 3,
    ]
    .map(|size| size * count);
    let sizes: Vec<_> = sizes.into_iter().filter(|size| *size != 0).collect();
    if usize::from(header[15]) != sizes.len() {
        return Err(invalid("SPZ v4 attribute stream count mismatch"));
    }
    let packed = unpack_streams(bytes, &sizes)?;
    // v3 and v4 use identical attribute packing, including smallest-three
    // quaternions. Only the compression envelope differs.
    Ok(SpzGaussians::read_gaussians(
        &mut Cursor::new(packed),
        decoded_header,
    )?)
}

fn unpack_streams(bytes: &[u8], sizes: &[usize]) -> Result<Vec<u8>, SceneError> {
    let table_end = 32 + sizes.len() * 16;
    let table = bytes
        .get(32..table_end)
        .ok_or_else(|| invalid("Truncated SPZ v4 table"))?;
    let mut offset = table_end;
    let mut packed = Vec::with_capacity(sizes.iter().sum());
    for (entry, &expected) in table.as_chunks::<16>().0.iter().zip(sizes) {
        let length = u64::from_le_bytes(entry[..8].try_into().expect("fixed table entry"));
        let unpacked = u64::from_le_bytes(entry[8..].try_into().expect("fixed table entry"));
        if unpacked != expected as u64 || length > bytes.len() as u64 {
            return Err(invalid("SPZ v4 attribute length mismatch"));
        }
        let end = offset
            .checked_add(length as usize)
            .ok_or_else(|| invalid("SPZ v4 stream offset overflow"))?;
        let stream = bytes
            .get(offset..end)
            .ok_or_else(|| invalid("Truncated SPZ v4 stream"))?;
        let decoded = zstd::bulk::decompress(stream, expected)?;
        if decoded.len() != expected {
            return Err(invalid("SPZ v4 decompressed attribute length mismatch"));
        }
        packed.extend(decoded);
        offset = end;
    }
    if offset != bytes.len() {
        return Err(invalid("SPZ v4 contains trailing stream data"));
    }
    Ok(packed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SplatScene;

    #[test]
    fn v4_export_preserves_attributes_and_rejects_invalid_stream_tables() {
        let original = include_bytes!("../tests/fixtures/packed-v4.spz");
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("packed.spz");
        std::fs::write(&path, original).unwrap();
        let scene = SplatScene::load(&path).unwrap();
        assert_eq!(scene.len(), 2);
        for gaussian in &scene.gaussians {
            assert!((gaussian.pos.x.abs() - 0.5).abs() < 0.002);
            assert!(gaussian.color.x > 190 && gaussian.color.w > 230);
            assert!(gaussian.scale.min_element() > 0.09);
            assert!(gaussian.rot.is_finite());
        }
        for (offset, value) in [
            (12, 4),
            (13, 255),
            (14, 2),
            (15, 6),
            (16, 31),
            (20, 1),
            (40, 255),
        ] {
            let mut bytes = original.to_vec();
            bytes[offset] = value;
            assert!(
                decode_v4(&bytes).is_err(),
                "accepted invalid byte at {offset}"
            );
        }
        for length in [16, 32, 100, original.len() - 1] {
            assert!(decode_v4(&original[..length]).is_err());
        }
        let mut trailing = original.to_vec();
        trailing.push(0);
        assert!(decode_v4(&trailing).is_err());
    }

    #[test]
    fn v4_streams_reuse_v3_packing_for_directional_color() {
        let mut scene = crate::splat::tests::scene();
        for gaussian in &mut scene.gaussians {
            for (index, coefficient) in gaussian.sh.iter_mut().enumerate() {
                *coefficient =
                    wgpu_3dgs_viewer::core::glam::Vec3::new(index as f32 / 32.0, -0.2, 0.1);
            }
        }
        let packed = SpzGaussians::from_gaussians(scene.gaussians);
        let mut legacy = Vec::new();
        packed.write_decompressed(&mut legacy).unwrap();
        let count = packed.header.num_points();
        let sizes = [9, 1, 3, 3, 4, 45].map(|size| size * count);
        let mut bytes = vec![0u8; 32 + sizes.len() * 16];
        bytes[..15].copy_from_slice(&legacy[..15]);
        bytes[4..8].copy_from_slice(&4u32.to_le_bytes());
        bytes[15] = 6;
        bytes[16..20].copy_from_slice(&32u32.to_le_bytes());
        let mut offset = 16;
        for (index, size) in sizes.into_iter().enumerate() {
            let compressed = zstd::bulk::compress(&legacy[offset..offset + size], 1).unwrap();
            let entry = 32 + index * 16;
            bytes[entry..entry + 8].copy_from_slice(&(compressed.len() as u64).to_le_bytes());
            bytes[entry + 8..entry + 16].copy_from_slice(&(size as u64).to_le_bytes());
            bytes.extend(compressed);
            offset += size;
        }
        let decoded = decode_v4(&bytes).unwrap();
        for (expected, actual) in packed.iter_gaussian().zip(decoded.iter_gaussian()) {
            assert_eq!(actual.pos, expected.pos);
            assert_eq!(actual.scale, expected.scale);
            assert_eq!(actual.rot, expected.rot);
            assert_eq!(actual.color, expected.color);
            assert_eq!(actual.sh, expected.sh);
        }
    }
}
