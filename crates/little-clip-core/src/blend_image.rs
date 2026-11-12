//! Bounded extraction of saved scene previews, without loading Blender scene data.
use crate::{ImageThumbnail, file_source::FileSource};
use std::{
    fs::File,
    io::{self, BufRead, BufReader, Read},
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

const SCAN_LIMIT: usize = 8 * 1024 * 1024;
const EDGE_LIMIT: u32 = 1024;

struct PreviewRead<'a, R> {
    inner: R,
    cancelled: &'a AtomicBool,
    deadline: Instant,
    remaining: usize,
}
impl<'a, R> PreviewRead<'a, R> {
    fn new(inner: R, cancelled: &'a AtomicBool, deadline: Instant) -> Self {
        Self {
            inner,
            cancelled,
            deadline,
            remaining: SCAN_LIMIT,
        }
    }
}
impl<R: Read> Read for PreviewRead<'_, R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        if self.cancelled.load(Ordering::Relaxed) {
            return Err(io::Error::other("Blend preview cancelled"));
        }
        if Instant::now() >= self.deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "Blend preview timed out",
            ));
        }
        if self.remaining == 0 {
            return Err(io::Error::other(
                "Blend preview exceeds the prefix scan limit",
            ));
        }
        let count = buffer.len().min(self.remaining).min(32 * 1024);
        let read = self.inner.read(&mut buffer[..count])?;
        self.remaining -= read;
        Ok(read)
    }
}

/// Extracts the saved scene thumbnail through an ordinary, identity-checked file.
///
/// Supports legacy 32/64-bit and Blender 5 headers, gzip and Zstandard streams.
/// Only the preview prefix is read: scene structures, scripts and external
/// resources are never loaded. Compressed and decoded reads each have an 8 MiB
/// budget. Cancellation and the shared eight-second deadline are cooperative
/// between reads and cannot interrupt a filesystem read already in flight.
///
/// # Errors
/// Returns guarded-source, cancellation, budget, compression, malformed-header
/// or absent-thumbnail errors. A scene without a saved preview is not rendered.
pub fn thumbnail(path: &Path, cancelled: &AtomicBool) -> Result<ImageThumbnail, String> {
    if cancelled.load(Ordering::Relaxed) {
        return Err("Blend preview cancelled".into());
    }
    let input = crate::kernel_interface::open_content(path, true).map_err(|e| e.to_string())?;
    let source = FileSource::capture_file(&input)?;
    let result = load(&input, cancelled).map_err(|e| e.to_string())?;
    let image = image::DynamicImage::ImageRgba8(result)
        .thumbnail(512, 512)
        .to_rgba8();
    let thumbnail = ImageThumbnail::from_rgba(image);
    if cancelled.load(Ordering::Relaxed) {
        return Err("Blend preview cancelled".into());
    }
    source.check_file(&input)?;
    Ok(thumbnail)
}

fn load(input: &File, cancelled: &AtomicBool) -> io::Result<image::RgbaImage> {
    let deadline = Instant::now() + Duration::from_secs(8);
    let mut input = BufReader::new(PreviewRead::new(input, cancelled, deadline));
    let compression = {
        let signature = input.fill_buf()?;
        if signature.starts_with(&[0x1f, 0x8b]) {
            1
        } else if signature.starts_with(&[0x28, 0xb5, 0x2f, 0xfd]) {
            2
        } else {
            0
        }
    };
    let reader: Box<dyn Read + '_> = match compression {
        1 => Box::new(flate2::read::MultiGzDecoder::new(input)),
        2 => {
            let mut decoder = zstd::stream::read::Decoder::with_buffer(input)?;
            decoder.window_log_max(25)?;
            Box::new(decoder)
        }
        _ => Box::new(input),
    };
    preview(&mut PreviewRead::new(reader, cancelled, deadline))
}

fn array<const N: usize>(reader: &mut impl Read) -> io::Result<[u8; N]> {
    let mut bytes = [0; N];
    reader.read_exact(&mut bytes)?;
    Ok(bytes)
}
fn number(bytes: [u8; 4], little: bool) -> u32 {
    if little {
        u32::from_le_bytes(bytes)
    } else {
        u32::from_be_bytes(bytes)
    }
}
fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

struct Header {
    block_size: usize,
    little: bool,
    modern: bool,
}
fn header(reader: &mut impl Read) -> io::Result<Header> {
    let header = array::<12>(reader)?;
    if &header[..7] != b"BLENDER" {
        return Err(invalid("Not a Blend file"));
    }
    let modern = &header[7..9] == b"17";
    let (block_size, little) = if modern {
        let tail = array::<5>(reader)?;
        if &header[9..12] != b"-01" || tail[0] != b'v' || !tail[1..].iter().all(u8::is_ascii_digit)
        {
            return Err(invalid("Unsupported Blend file header"));
        }
        (32, true)
    } else {
        let block_size = match header[7] {
            b'_' => 20,
            b'-' => 24,
            _ => return Err(invalid("Invalid Blend pointer-size marker")),
        };
        if !matches!(header[8], b'v' | b'V') || !header[9..].iter().all(u8::is_ascii_digit) {
            return Err(invalid("Invalid Blend endian/version marker"));
        }
        (block_size, header[8] == b'v')
    };
    Ok(Header {
        block_size,
        little,
        modern,
    })
}

fn preview(reader: &mut impl Read) -> io::Result<image::RgbaImage> {
    let Header {
        block_size,
        little,
        modern,
    } = header(reader)?;
    // Scene previews precede GLOB/DNA/scene data. Do not scan those structures.
    for _ in 0..1024 {
        let mut block = [0_u8; 32];
        reader.read_exact(&mut block[..block_size])?;
        let length = if modern {
            let mut bytes = [0; 8];
            bytes.copy_from_slice(&block[16..24]);
            u64::from_le_bytes(bytes)
        } else {
            u64::from(number([block[4], block[5], block[6], block[7]], little))
        };
        if length > SCAN_LIMIT as u64 {
            return Err(invalid("Blend preview block exceeds its size limit"));
        }
        match &block[..4] {
            b"REND" => {
                let mut left = length as usize;
                let mut skipped = [0_u8; 4096];
                while left != 0 {
                    let count = left.min(skipped.len());
                    reader.read_exact(&mut skipped[..count])?;
                    left -= count;
                }
            }
            b"TEST" => return pixels(reader, length, little),
            _ => return Err(invalid("Blend file has no saved scene preview")),
        }
    }
    Err(invalid("Blend preview exceeds its block-count limit"))
}

fn pixels(reader: &mut impl Read, length: u64, little: bool) -> io::Result<image::RgbaImage> {
    if length < 8 {
        return Err(invalid("Truncated Blend preview dimensions"));
    }
    let size = array::<8>(reader)?;
    let width = number([size[0], size[1], size[2], size[3]], little);
    let height = number([size[4], size[5], size[6], size[7]], little);
    if width == 0 || height == 0 || width > EDGE_LIMIT || height > EDGE_LIMIT {
        return Err(invalid("Blend preview dimensions exceed their limit"));
    }
    let bytes = width as usize * height as usize * 4;
    if length != bytes as u64 + 8 {
        return Err(invalid("Invalid Blend preview payload length"));
    }
    let mut pixels = vec![0; bytes];
    reader.read_exact(&mut pixels)?;
    let mut image = image::RgbaImage::from_raw(width, height, pixels)
        .ok_or_else(|| invalid("Invalid Blend preview pixels"))?;
    // Embedded image rows use a bottom-left origin; desktop previews use top-left.
    image::imageops::flip_vertical_in_place(&mut image);
    Ok(image)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write};

    fn block(
        bytes: &mut Vec<u8>,
        code: &[u8; 4],
        payload: &[u8],
        pointer: usize,
        little: bool,
        modern: bool,
    ) {
        let mut header = vec![0; if modern { 32 } else { 16 + pointer }];
        header[..4].copy_from_slice(code);
        if modern {
            header[16..24].copy_from_slice(&(payload.len() as u64).to_le_bytes());
        } else {
            let length = payload.len() as u32;
            header[4..8].copy_from_slice(&if little {
                length.to_le_bytes()
            } else {
                length.to_be_bytes()
            });
        }
        bytes.extend(header);
        bytes.extend_from_slice(payload);
    }
    fn fixture(pointer: usize, little: bool, modern: bool) -> Vec<u8> {
        let mut bytes = if modern {
            b"BLENDER17-01v0500".to_vec()
        } else {
            let mut header = b"BLENDER_v276".to_vec();
            header[7] = if pointer == 4 { b'_' } else { b'-' };
            header[8] = if little { b'v' } else { b'V' };
            header
        };
        block(&mut bytes, b"REND", &[0; 4], pointer, little, modern);
        let mut data = Vec::new();
        for dimension in [2_u32, 2] {
            data.extend_from_slice(&if little {
                dimension.to_le_bytes()
            } else {
                dimension.to_be_bytes()
            });
        }
        // Bottom row is blue/yellow, top row red/green. Stored bottom to top.
        data.extend_from_slice(&[
            0, 0, 255, 255, 255, 255, 0, 255, 255, 0, 0, 255, 0, 255, 0, 255,
        ]);
        block(&mut bytes, b"TEST", &data, pointer, little, modern);
        bytes
    }
    #[test]
    fn legacy_and_modern_thumbnails_preserve_colors_and_top_left_row_order() {
        for (pointer, little, modern) in [
            (4, true, false),
            (8, true, false),
            (4, false, false),
            (8, false, false),
            (8, true, true),
        ] {
            let image = preview(&mut Cursor::new(fixture(pointer, little, modern))).unwrap();
            assert_eq!(image.dimensions(), (2, 2));
            assert_eq!(
                image.as_raw(),
                &[
                    255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 0, 255
                ]
            );
        }
    }
    #[test]
    fn plain_gzip_and_zstd_prefixes_use_the_same_guarded_reader() {
        let bytes = fixture(8, true, true);
        let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        gzip.write_all(&bytes).unwrap();
        let gzip = gzip.finish().unwrap();
        let zstd = zstd::stream::encode_all(bytes.as_slice(), 1).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("scene.blend");
        for content in [&bytes, &gzip, &zstd] {
            std::fs::write(&path, content).unwrap();
            let result = thumbnail(&path, &AtomicBool::new(false)).unwrap();
            assert_eq!((result.width, result.height), (512, 512));
            assert_eq!(&result.premultiplied_rgba[..4], &[255, 0, 0, 255]);
        }
        std::fs::write(&path, bytes).unwrap();
        File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_len((SCAN_LIMIT * 4) as u64)
            .unwrap();
        assert!(
            thumbnail(&path, &AtomicBool::new(false)).is_ok(),
            "scene size must not force full-file loading"
        );
        assert!(
            thumbnail(&path, &AtomicBool::new(true))
                .unwrap_err()
                .contains("cancelled")
        );
    }
    #[test]
    fn malformed_lengths_dimensions_and_missing_previews_fail_without_allocating_a_scene() {
        let bytes = fixture(4, true, false);
        for size in 0..bytes.len() {
            assert!(
                preview(&mut Cursor::new(&bytes[..size])).is_err(),
                "accepted truncated prefix {size}"
            );
        }
        let test = bytes.windows(4).position(|word| word == b"TEST").unwrap();
        for (offset, value) in [(4, u32::MAX), (20, 0), (20, 1025), (24, 0)] {
            let mut invalid = bytes.clone();
            invalid[test + offset..test + offset + 4].copy_from_slice(&value.to_le_bytes());
            assert!(preview(&mut Cursor::new(invalid)).is_err());
        }
        let mut missing = b"BLENDER_v276".to_vec();
        block(&mut missing, b"GLOB", &[], 4, true, false);
        assert!(
            preview(&mut Cursor::new(missing))
                .unwrap_err()
                .to_string()
                .contains("no saved scene preview")
        );
    }
    #[test]
    fn compressed_prefix_bombs_and_expired_reads_are_bounded() {
        let mut bomb = b"BLENDER_v276".to_vec();
        block(&mut bomb, b"REND", &vec![0; SCAN_LIMIT], 4, true, false);
        let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        gzip.write_all(&bomb).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("bomb.blend");
        std::fs::write(&path, gzip.finish().unwrap()).unwrap();
        assert!(
            thumbnail(&path, &AtomicBool::new(false))
                .unwrap_err()
                .contains("scan limit")
        );
        let cancel = AtomicBool::new(false);
        let mut expired = PreviewRead::new(
            Cursor::new(b"x"),
            &cancel,
            Instant::now() - Duration::from_secs(1),
        );
        assert_eq!(
            expired.read(&mut [0; 1]).unwrap_err().kind(),
            io::ErrorKind::TimedOut
        );
    }
    #[test]
    #[ignore = "requires BLEND_TEST_FILE and installed blender-thumbnailer for a reference comparison"]
    fn real_scene_matches_installed_thumbnailer_pixels() {
        let path = std::env::var_os("BLEND_TEST_FILE").expect("set BLEND_TEST_FILE");
        let actual = thumbnail(Path::new(&path), &AtomicBool::new(false)).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let output = directory.path().join("reference.png");
        assert!(
            std::process::Command::new("blender-thumbnailer")
                .arg(&path)
                .arg(&output)
                .status()
                .unwrap()
                .success()
        );
        let reference =
            ImageThumbnail::generate(&std::fs::read(output).unwrap(), 512, 512).unwrap();
        assert_eq!(
            (actual.width, actual.height),
            (reference.width, reference.height)
        );
        assert_eq!(actual.premultiplied_rgba, reference.premultiplied_rgba);
    }
}
