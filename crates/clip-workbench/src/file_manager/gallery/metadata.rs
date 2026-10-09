//! Header-only probes and a persistent cache, independent of thumbnail decoding.
use crate::file_source::FileSource;
use serde::{Deserialize, Serialize};
use std::{
    hash::{Hash, Hasher},
    io::{BufReader, Seek},
    path::{Path, PathBuf},
    time::Duration,
};

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub(super) struct Metadata {
    pub width: u32,
    pub height: u32,
    pub orientation: u8,
}
impl Metadata {
    pub fn aspect(self) -> f32 {
        let ratio = self.width as f32 / self.height.max(1) as f32;
        if ratio <= 0.0 {
            return 1.0;
        }
        if (5..=8).contains(&self.orientation) {
            ratio.recip()
        } else {
            ratio
        }
    }
}

pub(super) fn probe(path: &Path, revision: u64) -> Result<Metadata, String> {
    let source = FileSource::capture(path)?;
    super::super::filesystem::check_revision(path, revision)?;
    let cache = cache_path(path, revision);
    if let Some(value) = cache
        .as_ref()
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|bytes| serde_json::from_slice::<Metadata>(&bytes).ok())
        .filter(|value| value.width > 0 && value.height > 0)
    {
        source.check()?;
        return Ok(value);
    }
    let value = if little_clip_core::raw_image::is_raw_image_path(path) {
        raw_metadata(path)?
    } else {
        raster_metadata(path)?
    };
    source.check()?;
    if FileSource::capture(path)? != source {
        return Err("Photo changed while probing".into());
    }
    if let Some(cache) = cache {
        save_cache(&cache, value);
    }
    Ok(value)
}

fn raster_metadata(path: &Path) -> Result<Metadata, String> {
    let file = crate::file_source::open_regular_file(path, true).map_err(|e| e.to_string())?;
    let mime = little_clip_core::item::guess_image_mime(path);
    if matches!(mime.as_str(), "image/heic" | "image/heif" | "image/avif") {
        let (width, height) = little_clip_core::gallery_image::heif_dimensions(file)
            .ok_or("HEIF metadata unavailable")?;
        return Ok(Metadata {
            width,
            height,
            orientation: 1,
        });
    }
    let mut reader = BufReader::new(file);
    let dimensions = imagesize::reader_size(&mut reader).map_err(|e| e.to_string());
    reader.rewind().map_err(|e| e.to_string())?;
    let orientation = header_orientation(&mut reader, &mime);
    let dimensions = dimensions.or_else(|_| {
        // SVG and hinted formats unsupported by imagesize still use core's
        // bounded header reader, never its raster decoding path.
        let bytes = crate::file_source::read_file_bytes(path, 256 * 1024, false, true)
            .map_err(|e| e.to_string())?;
        little_clip_core::item::read_image_dimensions_with_mime(
            &bytes,
            Some(&little_clip_core::item::guess_image_mime(path)),
        )
        .map(|(width, height)| imagesize::ImageSize {
            width: width as usize,
            height: height as usize,
        })
        .ok_or_else(|| "Photo dimensions unavailable".to_string())
    })?;
    Ok(Metadata {
        width: dimensions.width as u32,
        height: dimensions.height as u32,
        orientation,
    })
}

fn header_orientation(reader: &mut BufReader<std::fs::File>, mime: &str) -> u8 {
    use image::ImageDecoder;
    if matches!(mime, "image/jpeg" | "image/jpg") {
        // JPEG's EXIF container parser stops at SOS; unlike image's JPEG
        // decoder constructor, it does not read the compressed pixel stream.
        return exif::Reader::new()
            .read_from_container(reader)
            .ok()
            .and_then(|exif| {
                exif.get_field(exif::Tag::Orientation, exif::In::PRIMARY)
                    .and_then(|field| field.value.get_uint(0))
            })
            .unwrap_or(1) as u8;
    }
    if !matches!(mime, "image/png" | "image/webp" | "image/tiff") {
        return 1;
    }
    // These codec constructors inspect metadata/IFDs and seek past encoded
    // payloads. Never use read_from_container for TIFF (it reads to EOF),
    // or for PNG/WebP (it consumes compressed chunks while searching).
    let mut reader = image::ImageReader::new(reader);
    reader.limits(super::super::media::raster_preview_limits());
    reader
        .with_guessed_format()
        .ok()
        .and_then(|reader| reader.into_decoder().ok())
        .and_then(|mut decoder| decoder.orientation().ok())
        .map_or(1, |orientation| orientation.to_exif())
}

fn raw_metadata(path: &Path) -> Result<Metadata, String> {
    let output = little_clip_core::process::CommandRequest::new("exiftool")
        .args([
            "-j",
            "-n",
            "-ImageWidth",
            "-ImageHeight",
            "-Orientation",
            "-PreviewImageWidth",
            "-PreviewImageHeight",
        ])
        .arg(path)
        .timeout(Duration::from_secs(5))
        .max_output_bytes(16 * 1024)
        .run()
        .map_err(|e| e.to_string())?;
    let values: Vec<serde_json::Value> =
        serde_json::from_slice(&output.stdout).map_err(|e| e.to_string())?;
    let value = values.first().ok_or("RAW metadata unavailable")?;
    let dimension = |key, fallback| {
        value[key]
            .as_u64()
            .or_else(|| value[fallback].as_u64())
            .unwrap_or(0) as u32
    };
    let (width, height) = (
        dimension("ImageWidth", "PreviewImageWidth"),
        dimension("ImageHeight", "PreviewImageHeight"),
    );
    if width == 0 || height == 0 {
        return Err("RAW dimensions unavailable".into());
    }
    Ok(Metadata {
        width,
        height,
        orientation: value["Orientation"].as_u64().unwrap_or(1) as u8,
    })
}

fn cache_path(path: &Path, revision: u64) -> Option<PathBuf> {
    let root = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))?;
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut hash);
    revision.hash(&mut hash);
    Some(
        root.join("little-screen-clip/gallery-metadata-v1")
            .join(format!("{:016x}.json", hash.finish())),
    )
}
fn save_cache(path: &Path, metadata: Metadata) {
    let Some(parent) = path.parent() else {
        return;
    };
    let _ = std::fs::create_dir_all(parent);
    if let Ok(mut file) = tempfile::NamedTempFile::new_in(parent) {
        if serde_json::to_writer(file.as_file_mut(), &metadata).is_ok() {
            let _ = file.persist(path);
        }
    }
}
