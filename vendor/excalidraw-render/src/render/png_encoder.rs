//! The owned PNG encoder (ADR-0004): filters each RGBA scanline, deflates the
//! scanlines into one zlib `IDAT` stream with `miniz_oxide` at a fixed level,
//! and assembles `IHDR`, `IDAT`, the optional `tEXt` scene and `iTXt` manifest
//! chunks and `IEND` with CRC-32 trailers. Everything here is pure integer code
//! with fixed parameters, so the bytes are stable across every supported target
//! and the package-consumer expectation pins them; nothing here knows about
//! the plan or the painter. PNG output and PDF's isolated raster fallback
//! both encode through it, so it is compiled with either format.

use miniz_oxide::deflate::core::{compress_to_output, CompressorOxide, TDEFLFlush, TDEFLStatus};
use miniz_oxide::DataFormat;

use super::metadata::{operation, ArtifactMetadata, Stage};
use super::FormatEncodeError;

/// Bytes per RGBA sample; the filter distance to the left neighbour.
const PIXEL_BYTES: usize = 4;

/// The zlib default. Higher levels cost two to three times the encode time for
/// under two percent on measured Excalidraw boards, and the reference renderer's
/// own PNG export runs at this level.
const IDAT_DEFLATE_LEVEL: u8 = 6;

/// One raster's `IDAT` stream, filtered and deflated once, from which every
/// artifact candidate is assembled.
///
/// The stream is a pure function of the raster and independent of the
/// metadata chunks, so metadata selection assembles each candidate and the
/// emission pass assembles the accepted one from this single stream rather
/// than rasterizing and compressing again per call (#117). It is a transient
/// the ledger does not price separately; ADR-0004 documents its lifetime
/// against the spec §13.2 PNG encoder workspace row.
pub(crate) struct PngIdatStream {
    width: u32,
    height: u32,
    idat: Vec<u8>,
}

/// Validates the raster against its dimensions and deflates its scanlines.
pub(in crate::render) fn compress_rgba(
    width: u32,
    height: u32,
    rgba: &[u8],
) -> Result<PngIdatStream, FormatEncodeError> {
    if width == 0 || height == 0 {
        return Err(FormatEncodeError::Encoding(
            "PNG dimensions must be positive".to_owned(),
        ));
    }
    let pixel_row_bytes = usize::try_from(width)
        .ok()
        .and_then(|width| width.checked_mul(PIXEL_BYTES))
        .ok_or_else(png_size_error)?;
    let expected_rgba_bytes = pixel_row_bytes
        .checked_mul(usize::try_from(height).map_err(|_| png_size_error())?)
        .ok_or_else(png_size_error)?;
    if rgba.len() != expected_rgba_bytes {
        return Err(FormatEncodeError::Encoding(
            "PNG raster length does not match its dimensions".to_owned(),
        ));
    }

    // The IDAT stream is produced before any candidate is assembled so the
    // exact PNG size is known before each fallible output allocation. Deflate
    // keeps the stream a small fraction of the raster on ordinary boards
    // (#113); even an incompressible raster holds it only until the accepted
    // candidate is emitted.
    let idat = compress_scanlines(pixel_row_bytes, rgba)?;
    Ok(PngIdatStream {
        width,
        height,
        idat,
    })
}

impl PngIdatStream {
    /// Assembles `IHDR`, the `IDAT` stream, the metadata chunks, and `IEND`
    /// into one artifact candidate.
    pub(crate) fn assemble(
        &self,
        metadata: Option<&ArtifactMetadata>,
    ) -> Result<Vec<u8>, FormatEncodeError> {
        let idat = &self.idat;
        let metadata_bytes = metadata.map_or(0, |metadata| {
            metadata
                .scene_wrapper()
                .map_or(0, |wrapper| {
                    12_usize
                        .saturating_add("application/vnd.excalidraw+json".len())
                        .saturating_add(1)
                        .saturating_add(wrapper.len())
                })
                .saturating_add(metadata.manifest().map_or(0, |manifest| {
                    12_usize
                        .saturating_add("excalidraw-render:artifact-manifest".len())
                        .saturating_add(5)
                        .saturating_add(manifest.len())
                }))
        });
        let output_bytes = 8_usize
            .checked_add(12 + 13)
            .and_then(|bytes| bytes.checked_add(12))
            .and_then(|bytes| bytes.checked_add(idat.len()))
            .and_then(|bytes| bytes.checked_add(metadata_bytes))
            .and_then(|bytes| bytes.checked_add(12))
            .ok_or_else(png_size_error)?;

        let mut png = Vec::new();
        png.try_reserve_exact(output_bytes)
            .map_err(|_| FormatEncodeError::MemoryUnavailable {
                subject_kind: "artifact",
                subject: "png_output",
            })?;
        png.extend_from_slice(b"\x89PNG\r\n\x1a\n");

        let mut ihdr = [0_u8; 13];
        ihdr[..4].copy_from_slice(&self.width.to_be_bytes());
        ihdr[4..8].copy_from_slice(&self.height.to_be_bytes());
        ihdr[8] = 8;
        ihdr[9] = 6;
        append_chunk(&mut png, *b"IHDR", &ihdr)?;
        append_chunk(&mut png, *b"IDAT", idat)?;
        if let Some(metadata) = metadata {
            if let Some(wrapper) = metadata.scene_wrapper() {
                let mut text = Vec::new();
                text.try_reserve_exact(
                    "application/vnd.excalidraw+json"
                        .len()
                        .saturating_add(1)
                        .saturating_add(wrapper.len()),
                )
                .map_err(|_| FormatEncodeError::MemoryUnavailable {
                    subject_kind: "metadata",
                    subject: "png_scene_chunk",
                })?;
                text.extend_from_slice(b"application/vnd.excalidraw+json\0");
                text.extend_from_slice(wrapper);
                operation(Stage::SceneEmbedding, || {
                    append_chunk(&mut png, *b"tEXt", &text)
                })?;
            }
            if let Some(manifest) = metadata.manifest() {
                let mut text = Vec::new();
                text.try_reserve_exact(
                    "excalidraw-render:artifact-manifest"
                        .len()
                        .saturating_add(5)
                        .saturating_add(manifest.len()),
                )
                .map_err(|_| FormatEncodeError::MemoryUnavailable {
                    subject_kind: "metadata",
                    subject: "png_manifest_chunk",
                })?;
                text.extend_from_slice(b"excalidraw-render:artifact-manifest\0\0\0\0\0");
                text.extend_from_slice(manifest.as_bytes());
                operation(Stage::ManifestEmbedding, || {
                    append_chunk(&mut png, *b"iTXt", &text)
                })?;
            }
        }
        append_chunk(&mut png, *b"IEND", &[])?;

        if png.len() != output_bytes {
            return Err(FormatEncodeError::Encoding(
                "PNG encoder length invariant failed".to_owned(),
            ));
        }
        Ok(png)
    }
}

/// Filters and deflates every scanline into one RFC 1950 zlib stream.
///
/// The compressor is pure integer code with fixed parameters and no host,
/// time, or random input, so the stream bytes are stable across every
/// supported target; the `IDAT` bytes of the package-consumer expectation
/// pin that on each of them.
fn compress_scanlines(pixel_row_bytes: usize, rgba: &[u8]) -> Result<Vec<u8>, FormatEncodeError> {
    if pixel_row_bytes == 0 {
        return Err(png_size_error());
    }
    let scanline_bytes = pixel_row_bytes.checked_add(1).ok_or_else(png_size_error)?;
    let mut scanline = zeroed_scratch(scanline_bytes, "png_scanline")?;
    // RFC 2083 filters treat the row above the first scanline as zeros.
    let zero_row = zeroed_scratch(pixel_row_bytes, "png_scanline_above")?;

    let mut compressor = CompressorOxide::default();
    compressor.set_format_and_level(DataFormat::Zlib, IDAT_DEFLATE_LEVEL);
    let mut zlib = Vec::new();
    let mut previous: &[u8] = &zero_row;
    for row in rgba.chunks_exact(pixel_row_bytes) {
        let Some((type_byte, residuals)) = scanline.split_first_mut() else {
            return Err(FormatEncodeError::Encoding(
                "PNG scanline layout invariant failed".to_owned(),
            ));
        };
        let filter = select_scanline_filter(row, previous);
        *type_byte = filter.type_byte();
        apply_scanline_filter(filter, row, previous, residuals);
        deflate_append(&mut compressor, &scanline, TDEFLFlush::None, &mut zlib)?;
        previous = row;
    }
    deflate_append(&mut compressor, &[], TDEFLFlush::Finish, &mut zlib)?;
    Ok(zlib)
}

fn zeroed_scratch(bytes: usize, subject: &'static str) -> Result<Vec<u8>, FormatEncodeError> {
    let mut scratch = Vec::new();
    scratch
        .try_reserve_exact(bytes)
        .map_err(|_| FormatEncodeError::MemoryUnavailable {
            subject_kind: "artifact",
            subject,
        })?;
    scratch.resize(bytes, 0);
    Ok(scratch)
}

/// Feeds `input` through the compressor, appending its output fallibly.
fn deflate_append(
    compressor: &mut CompressorOxide,
    input: &[u8],
    flush: TDEFLFlush,
    output: &mut Vec<u8>,
) -> Result<(), FormatEncodeError> {
    let mut memory_unavailable = false;
    let (status, consumed) = compress_to_output(compressor, input, flush, |bytes| {
        if output.try_reserve(bytes.len()).is_err() {
            memory_unavailable = true;
            return false;
        }
        output.extend_from_slice(bytes);
        true
    });
    if memory_unavailable {
        return Err(FormatEncodeError::MemoryUnavailable {
            subject_kind: "artifact",
            subject: "png_idat",
        });
    }
    let expected = if flush == TDEFLFlush::Finish {
        TDEFLStatus::Done
    } else {
        TDEFLStatus::Okay
    };
    if status != expected || consumed != input.len() {
        return Err(FormatEncodeError::Encoding(
            "PNG deflate stream invariant failed".to_owned(),
        ));
    }
    Ok(())
}

/// RFC 2083 §6 scanline filter types.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ScanlineFilter {
    None,
    Sub,
    Up,
    Average,
    Paeth,
}

impl ScanlineFilter {
    const ALL: [Self; 5] = [Self::None, Self::Sub, Self::Up, Self::Average, Self::Paeth];

    fn type_byte(self) -> u8 {
        match self {
            Self::None => 0,
            Self::Sub => 1,
            Self::Up => 2,
            Self::Average => 3,
            Self::Paeth => 4,
        }
    }

    /// The filtered value of `row[index]`.
    fn residual(self, index: usize, row: &[u8], previous: &[u8]) -> u8 {
        row.get(index)
            .copied()
            .unwrap_or(0)
            .wrapping_sub(self.predict(index, row, previous))
    }

    /// The predicted value of `row[index]` from its left, upper, and
    /// upper-left neighbours, each zero where it falls outside the image.
    fn predict(self, index: usize, row: &[u8], previous: &[u8]) -> u8 {
        let left = index.checked_sub(PIXEL_BYTES);
        let a = left.and_then(|left| row.get(left)).copied().unwrap_or(0);
        let b = previous.get(index).copied().unwrap_or(0);
        let c = left
            .and_then(|left| previous.get(left))
            .copied()
            .unwrap_or(0);
        match self {
            Self::None => 0,
            Self::Sub => a,
            Self::Up => b,
            // floor((a + b) / 2) without leaving u8.
            Self::Average => a / 2 + b / 2 + (a & b & 1),
            Self::Paeth => paeth_predictor(a, b, c),
        }
    }
}

fn paeth_predictor(a: u8, b: u8, c: u8) -> u8 {
    let estimate = i16::from(a) + i16::from(b) - i16::from(c);
    let to_a = (estimate - i16::from(a)).abs();
    let to_b = (estimate - i16::from(b)).abs();
    let to_c = (estimate - i16::from(c)).abs();
    if to_a <= to_b && to_a <= to_c {
        a
    } else if to_b <= to_c {
        b
    } else {
        c
    }
}

/// Chooses the filter for one scanline.
///
/// Anti-aliased vector content is mostly flat runs and repeated rows, which
/// deflate matches directly; every predictive filter measured 10-20% larger
/// on real boards. A row whose bytes mostly equal their left or upper
/// neighbour therefore stays unfiltered. Other rows (photographic image
/// content) take the filter with the smallest sum of absolute residuals,
/// which halves their size; ties go to the lowest filter type.
fn select_scanline_filter(row: &[u8], previous: &[u8]) -> ScanlineFilter {
    let neighbour_matches = row
        .iter()
        .zip(previous)
        .enumerate()
        .filter(|(index, (byte, above))| {
            let left = index
                .checked_sub(PIXEL_BYTES)
                .and_then(|left| row.get(left));
            byte == above || left == Some(byte)
        })
        .count();
    if neighbour_matches.saturating_mul(2) >= row.len() {
        return ScanlineFilter::None;
    }

    let mut scores = [0_u64; 5];
    for index in 0..row.len() {
        for (filter, score) in ScanlineFilter::ALL.iter().zip(scores.iter_mut()) {
            let residual = filter.residual(index, row, previous);
            *score = score.saturating_add(u64::from(residual_magnitude(residual)));
        }
    }
    let mut best = ScanlineFilter::None;
    let mut best_score = u64::MAX;
    for (filter, score) in ScanlineFilter::ALL.into_iter().zip(scores) {
        if score < best_score {
            best = filter;
            best_score = score;
        }
    }
    best
}

/// The magnitude of a residual read as a signed byte, so 255 counts as 1.
fn residual_magnitude(residual: u8) -> u8 {
    if residual > 127 {
        0_u8.wrapping_sub(residual)
    } else {
        residual
    }
}

/// Writes the filtered residuals of `row` into `residuals`.
fn apply_scanline_filter(
    filter: ScanlineFilter,
    row: &[u8],
    previous: &[u8],
    residuals: &mut [u8],
) {
    for (index, residual) in residuals.iter_mut().enumerate().take(row.len()) {
        *residual = filter.residual(index, row, previous);
    }
}

pub(in crate::render) fn canonicalize_straight_alpha(
    rgba: &mut [u8],
) -> Result<(), FormatEncodeError> {
    for pixel in rgba.as_chunks_mut::<4>().0 {
        let (channels, alpha) = pixel.split_at_mut(3);
        let alpha = alpha.first().copied().ok_or_else(|| {
            FormatEncodeError::Encoding("PNG alpha layout invariant failed".to_owned())
        })?;
        if alpha == 0 {
            channels.fill(0);
            continue;
        }
        // Match the canvas's float32 load/unpremultiply/store sequence, including
        // its rounded reciprocal of 255. Rearranging this as channel*255/alpha
        // changes half-way results (the exhaustive reference grid pins both
        // 5/6 -> 212 and 3/10 -> 77). Keep the operation order; an approximate
        // reciprocal or fused operation would change this contract.
        let unit = 1.0_f32 / 255.0;
        let inverse_alpha = 1.0_f32 / (f32::from(alpha) * unit);
        for channel in channels {
            let normalized = f32::from(*channel) * unit;
            *channel = ((normalized * inverse_alpha) * 255.0).round_ties_even() as u8;
        }
    }
    Ok(())
}

fn append_chunk(output: &mut Vec<u8>, kind: [u8; 4], data: &[u8]) -> Result<(), FormatEncodeError> {
    let length = u32::try_from(data.len()).map_err(|_| png_size_error())?;
    output.extend_from_slice(&length.to_be_bytes());
    let crc_start = output.len();
    output.extend_from_slice(&kind);
    output.extend_from_slice(data);
    output.extend_from_slice(&crc32fast::hash(&output[crc_start..]).to_be_bytes());
    Ok(())
}

pub(in crate::render) fn png_size_error() -> FormatEncodeError {
    FormatEncodeError::Encoding("PNG dimensions exceed encoder capacity".to_owned())
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use crate::render::metadata::{
        test_fixture, ArtifactMetadata, ManifestTarget, MetadataSelection,
    };

    use super::{
        append_chunk, apply_scanline_filter, compress_rgba, select_scanline_filter, ScanlineFilter,
    };

    /// A raster with a flat region, a smooth gradient, a noisy region, and
    /// varying alpha so every scanline filter has something to work on.
    fn synthetic_rgba(width: u32, height: u32) -> Vec<u8> {
        let mut rgba = Vec::new();
        let mut lcg = 0x2545_f491_u32;
        for y in 0..height {
            for x in 0..width {
                lcg = lcg.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let noise = (lcg >> 24) as u8;
                let third = x * 3 / width;
                let pixel = match third {
                    0 => [255, 255, 255, 255],
                    1 => [
                        (x * 255 / width) as u8,
                        (y * 255 / height) as u8,
                        ((x + y) * 127 / (width + height)) as u8,
                        if y % 7 == 0 { 0 } else { 255 },
                    ],
                    _ => [
                        noise,
                        noise.wrapping_mul(3),
                        noise ^ 0x5a,
                        128 | (noise >> 1),
                    ],
                };
                rgba.extend_from_slice(&pixel);
            }
        }
        rgba
    }

    fn mostly_white_rgba(width: u32, height: u32) -> Vec<u8> {
        let mut rgba = vec![255_u8; (width * height * 4) as usize];
        for y in height / 4..height / 2 {
            for x in width / 4..width / 2 {
                let offset = ((y * width + x) * 4) as usize;
                rgba[offset..offset + 3].copy_from_slice(&[0x1e, 0x1e, 0x1e]);
            }
        }
        rgba
    }

    fn encode(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
        let Ok(png) = compress_rgba(width, height, rgba).and_then(|stream| stream.assemble(None))
        else {
            panic!("the raster encodes");
        };
        png
    }

    /// Artifact metadata carrying both PNG chunks: the `tEXt` scene wrapper
    /// and the `iTXt` manifest with provenance and the diagnostic summary.
    fn full_metadata() -> ArtifactMetadata {
        let (scene, profile, report) = test_fixture();
        ArtifactMetadata::build(
            &scene,
            profile,
            &report,
            None,
            MetadataSelection {
                provenance: true,
                diagnostic_summary: true,
                embedded_scene: true,
            },
            ManifestTarget::Scene,
        )
        .unwrap_or_else(|_| panic!("metadata with every component builds"))
    }

    /// The `tEXt` and `iTXt` chunks an independent decoder reads after the
    /// image data, as `(chunk type, keyword, text)`.
    fn text_chunks(png: &[u8]) -> Vec<(&'static str, String, String)> {
        let decoder = png::Decoder::new(Cursor::new(png));
        let mut reader = decoder.read_info().expect("PNG structure is valid");
        let mut rgba = vec![0; reader.output_buffer_size().expect("PNG buffer fits")];
        reader.next_frame(&mut rgba).expect("PNG pixels decode");
        reader.finish().expect("trailing PNG chunks decode");
        let info = reader.info();
        info.uncompressed_latin1_text
            .iter()
            .map(|chunk| ("tEXt", chunk.keyword.clone(), chunk.text.clone()))
            .chain(info.utf8_text.iter().map(|chunk| {
                (
                    "iTXt",
                    chunk.keyword.clone(),
                    chunk.get_text().expect("iTXt text is UTF-8"),
                )
            }))
            .collect()
    }

    fn decode_rgba(png: &[u8]) -> (u32, u32, Vec<u8>) {
        let mut decoder = png::Decoder::new(Cursor::new(png));
        decoder.set_transformations(png::Transformations::IDENTITY);
        let mut reader = decoder.read_info().expect("PNG structure is valid");
        assert_eq!(reader.info().color_type, png::ColorType::Rgba);
        assert_eq!(reader.info().bit_depth, png::BitDepth::Eight);
        let mut rgba = vec![0; reader.output_buffer_size().expect("PNG buffer fits")];
        let frame = reader.next_frame(&mut rgba).expect("PNG pixels decode");
        rgba.truncate(frame.buffer_size());
        (frame.width, frame.height, rgba)
    }

    fn chunks(png: &[u8]) -> Vec<([u8; 4], &[u8])> {
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        let mut chunks = Vec::new();
        let mut offset = 8;
        while offset < png.len() {
            let length = u32::from_be_bytes(png[offset..offset + 4].try_into().unwrap()) as usize;
            let kind: [u8; 4] = png[offset + 4..offset + 8].try_into().unwrap();
            chunks.push((kind, &png[offset + 8..offset + 8 + length]));
            offset += 12 + length;
        }
        assert_eq!(offset, png.len());
        chunks
    }

    fn idat(png: &[u8]) -> Vec<u8> {
        chunks(png)
            .into_iter()
            .filter(|(kind, _)| kind == b"IDAT")
            .flat_map(|(_, data)| data.to_vec())
            .collect()
    }

    fn inflate_scanlines(png: &[u8]) -> Vec<u8> {
        let zlib = idat(png);
        let cmf = u16::from(zlib[0]);
        let flg = u16::from(zlib[1]);
        assert_eq!(cmf & 0x0f, 8, "IDAT is a zlib stream using deflate");
        assert_eq!((cmf * 256 + flg) % 31, 0, "zlib header check bits hold");
        miniz_oxide::inflate::decompress_to_vec_zlib(&zlib).expect("IDAT inflates")
    }

    #[test]
    fn encoded_png_round_trips_through_an_independent_decoder() {
        let (width, height) = (37_u32, 23_u32);
        let rgba = synthetic_rgba(width, height);

        let first = encode(width, height, &rgba);
        let second = encode(width, height, &rgba);

        assert_eq!(first, second, "encoding is a pure function of the raster");
        assert_eq!(
            chunks(&first)
                .iter()
                .map(|(kind, _)| *kind)
                .collect::<Vec<_>>(),
            [*b"IHDR", *b"IDAT", *b"IEND"]
        );
        let ihdr = chunks(&first)[0].1;
        assert_eq!(&ihdr[..8], &[0, 0, 0, 37, 0, 0, 0, 23]);
        assert_eq!(
            &ihdr[8..],
            &[8, 6, 0, 0, 0],
            "8-bit RGBA, deflate, adaptive, no interlace"
        );
        assert_eq!(decode_rgba(&first), (width, height, rgba));
    }

    /// Metadata selection assembles up to four candidates and emission one
    /// more from the same raster, so the raster is filtered and deflated once
    /// and every candidate is the same `IDAT` stream followed by its own
    /// metadata chunks (#117).
    #[test]
    fn one_compressed_stream_assembles_every_metadata_candidate() {
        let (width, height) = (37_u32, 23_u32);
        let rgba = synthetic_rgba(width, height);
        let metadata = full_metadata();
        let Ok(stream) = compress_rgba(width, height, &rgba) else {
            panic!("the raster compresses");
        };

        let Ok(bare) = stream.assemble(None) else {
            panic!("the baseline candidate assembles");
        };
        let Ok(embedded) = stream.assemble(Some(&metadata)) else {
            panic!("the embedded candidate assembles");
        };
        let Ok(emitted) = stream.assemble(None) else {
            panic!("the stream survives assembly for the emission pass");
        };

        assert_eq!(
            bare, emitted,
            "assembly neither consumes nor alters the stream"
        );
        assert_eq!(
            chunks(&bare)
                .iter()
                .map(|(kind, _)| *kind)
                .collect::<Vec<_>>(),
            [*b"IHDR", *b"IDAT", *b"IEND"]
        );
        assert_eq!(
            chunks(&embedded)
                .iter()
                .map(|(kind, _)| *kind)
                .collect::<Vec<_>>(),
            [*b"IHDR", *b"IDAT", *b"tEXt", *b"iTXt", *b"IEND"]
        );
        assert_eq!(
            idat(&bare),
            idat(&embedded),
            "the IDAT stream does not depend on the metadata chunks"
        );
        assert_eq!(decode_rgba(&bare), (width, height, rgba.clone()));
        assert_eq!(decode_rgba(&embedded), (width, height, rgba));

        let wrapper = metadata
            .scene_wrapper()
            .expect("the fixture embeds a scene")
            .iter()
            .map(|byte| char::from(*byte))
            .collect::<String>();
        let manifest = metadata
            .manifest()
            .expect("the fixture carries a manifest")
            .to_owned();
        assert_eq!(text_chunks(&bare), []);
        assert_eq!(
            text_chunks(&embedded),
            [
                (
                    "tEXt",
                    "application/vnd.excalidraw+json".to_owned(),
                    wrapper
                ),
                (
                    "iTXt",
                    "excalidraw-render:artifact-manifest".to_owned(),
                    manifest
                ),
            ]
        );
    }

    #[test]
    fn idat_is_a_deflate_compressed_stream_of_filtered_scanlines() {
        let (width, height) = (37_u32, 23_u32);
        let rgba = synthetic_rgba(width, height);
        let png = encode(width, height, &rgba);

        let row_bytes = width as usize * 4 + 1;
        let scanlines = inflate_scanlines(&png);
        assert_eq!(scanlines.len(), row_bytes * height as usize);
        let filter_types = scanlines
            .chunks_exact(row_bytes)
            .map(|row| row[0])
            .collect::<Vec<_>>();
        assert!(filter_types.iter().all(|kind| *kind <= 4));
        assert!(
            filter_types.iter().any(|kind| *kind != 0),
            "noisy scanlines pick a predictive filter: {filter_types:?}"
        );
        assert!(
            idat(&png).len() < scanlines.len(),
            "the stream is smaller than the scanlines it carries"
        );
    }

    #[test]
    fn flat_rasters_compress_far_below_their_stored_size() {
        let (width, height) = (512_u32, 256_u32);
        let rgba = mostly_white_rgba(width, height);
        let raw_bytes = rgba.len() + height as usize;

        let png = encode(width, height, &rgba);

        assert!(
            png.len() * 20 < raw_bytes,
            "{} PNG bytes is not far below {raw_bytes} stored bytes (#113)",
            png.len()
        );
        assert_eq!(decode_rgba(&png), (width, height, rgba));
        let row_bytes = width as usize * 4 + 1;
        assert!(
            inflate_scanlines(&png)
                .chunks_exact(row_bytes)
                .all(|row| row[0] == 0),
            "flat vector-style scanlines stay unfiltered, which deflates best"
        );
    }

    #[test]
    fn scanline_filter_selection_keeps_flat_rows_raw_and_predicts_noisy_rows() {
        let zeros = vec![0_u8; 60 * 4];
        let flat = [200_u8, 100, 50, 255].repeat(60);
        let gentle_gradient = (0..60_u8).flat_map(|x| [x, x, x, 255]).collect::<Vec<_>>();
        let steep_gradient = (0..60_u8)
            .flat_map(|x| [x * 3, x * 3, x * 3, 255])
            .collect::<Vec<_>>();
        let steep_gradient_plus_one = (0..60_u8)
            .flat_map(|x| [x * 3 + 1, x * 3 + 1, x * 3 + 1, 255])
            .collect::<Vec<_>>();

        // Rows whose bytes mostly repeat their left or upper neighbour are
        // vector-style content; unfiltered bytes deflate best there.
        assert_eq!(select_scanline_filter(&flat, &zeros), ScanlineFilter::None);
        assert_eq!(
            select_scanline_filter(&steep_gradient, &steep_gradient),
            ScanlineFilter::None
        );
        // Otherwise the smallest sum of absolute residuals wins, with ties
        // resolved toward the lowest filter type.
        assert_eq!(
            select_scanline_filter(&gentle_gradient, &zeros),
            ScanlineFilter::Sub
        );
        assert_eq!(
            select_scanline_filter(&steep_gradient_plus_one, &steep_gradient),
            ScanlineFilter::Up
        );
    }

    #[test]
    fn every_scanline_filter_inverts_under_an_independent_decoder() {
        let (width, height) = (11_u32, 5_u32);
        let rgba = synthetic_rgba(width, height);
        let pixel_row_bytes = width as usize * 4;
        let zeros = vec![0_u8; pixel_row_bytes];

        let filters = [
            ScanlineFilter::None,
            ScanlineFilter::Sub,
            ScanlineFilter::Up,
            ScanlineFilter::Average,
            ScanlineFilter::Paeth,
        ];
        let mut scanlines = Vec::new();
        for (row_index, row) in rgba.chunks_exact(pixel_row_bytes).enumerate() {
            let previous = if row_index == 0 {
                &zeros[..]
            } else {
                &rgba[(row_index - 1) * pixel_row_bytes..row_index * pixel_row_bytes]
            };
            let filter = filters[row_index % filters.len()];
            let mut residuals = vec![0_u8; pixel_row_bytes];
            apply_scanline_filter(filter, row, previous, &mut residuals);
            scanlines.push(filter.type_byte());
            scanlines.extend_from_slice(&residuals);
        }

        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        let mut ihdr = [0_u8; 13];
        ihdr[..4].copy_from_slice(&width.to_be_bytes());
        ihdr[4..8].copy_from_slice(&height.to_be_bytes());
        ihdr[8] = 8;
        ihdr[9] = 6;
        assert!(append_chunk(&mut png, *b"IHDR", &ihdr).is_ok());
        let zlib = miniz_oxide::deflate::compress_to_vec_zlib(&scanlines, 6);
        assert!(append_chunk(&mut png, *b"IDAT", &zlib).is_ok());
        assert!(append_chunk(&mut png, *b"IEND", &[]).is_ok());

        assert_eq!(decode_rgba(&png), (width, height, rgba));
    }
}
