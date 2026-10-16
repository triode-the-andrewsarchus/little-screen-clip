//! Bundled SOG import using Spark's established native Rust decoder.

use serde_json::Value;
use spark_lib::decoder::ChunkReceiver;
use spark_lib::gsplat::GsplatArray;
use spark_lib::sogs::SogsDecoder;
use std::collections::HashMap;
use std::io::{Cursor, Read};
use std::path::Path;
use wgpu_3dgs_viewer::core::Gaussian;
use zip::ZipArchive;

use crate::SceneError;

const MAX_SPLATS: u64 = 2_000_000;
const MAX_ARCHIVE_BYTES: u64 = 512 * 1024 * 1024;
const MAX_IMAGE_PIXELS: u64 = 16 * 1024 * 1024;

fn invalid(message: impl Into<String>) -> SceneError {
    SceneError::Invalid(message.into())
}

pub(crate) fn load(path: &Path) -> Result<Vec<Gaussian>, SceneError> {
    let file = little_clip_core::kernel_interface::open_content(path, true)?;
    if file.metadata()?.len() > MAX_ARCHIVE_BYTES {
        return Err(invalid("SOG archive exceeds the 512 MiB import budget"));
    }
    let bytes = crate::read_bounded(file, MAX_ARCHIVE_BYTES, "SOG archive")?;
    validate_archive(&bytes)?;
    let mut decoder = SogsDecoder::new(GsplatArray::new(), None);
    decoder
        .push(&bytes)
        .and_then(|()| decoder.finish())
        .map_err(|error| invalid(format!("SOG decoding: {error:#}")))?;
    Ok(crate::spark::gaussians(&decoder.into_splats()))
}

// Spark's decoder assumes consistent atlas sizes and valid dictionary indices.
// Check those contracts before handing it untrusted file data. This validates
// the container and arrays; the decoder retains all SOG math and unpacking.
fn validate_archive(bytes: &[u8]) -> Result<(), SceneError> {
    let mut zip =
        ZipArchive::new(Cursor::new(bytes)).map_err(|error| invalid(error.to_string()))?;
    let (meta, metadata_name) = read_metadata(&mut zip)?;
    let version = meta.get("version").and_then(Value::as_u64).unwrap_or(1);
    let count = match version {
        1 => meta.pointer("/means/shape/0"),
        2 => meta.get("count"),
        _ => return Err(invalid(format!("Unsupported SOG version: {version}"))),
    }
    .and_then(Value::as_u64)
    .ok_or_else(|| invalid("SOG has no valid splat count"))?;
    if count == 0 || count > MAX_SPLATS {
        return Err(invalid(
            "SOG splat count exceeds the 2 million point import budget",
        ));
    }
    let prefix = metadata_name.strip_suffix("meta.json").unwrap_or("");
    let mut images = HashMap::new();
    for group in ["means", "scales", "quats", "sh0", "shN"] {
        let section = meta.get(group).filter(|value| !value.is_null());
        let Some(section) = section else {
            if group == "shN" {
                continue;
            }
            return Err(invalid(format!("SOG has no {group} attributes")));
        };
        images.extend(validate_group(
            &mut zip, section, prefix, group, count, version,
        )?);
    }
    let quats = images
        .iter()
        .find(|((group, _), _)| group == "quats")
        .map(|(_, image)| image)
        .ok_or_else(|| invalid("SOG has no quaternion atlas"))?;
    if quats
        .pixels()
        .take(count as usize)
        .any(|pixel| pixel[3] < 252)
    {
        return Err(invalid("Invalid SOG quaternion packing mode"));
    }
    validate_sh(&meta, &images, count as usize, version)
}

fn validate_sh(
    meta: &Value,
    images: &HashMap<(String, String), image::RgbaImage>,
    count: usize,
    version: u64,
) -> Result<(), SceneError> {
    let Some(sh) = meta.get("shN").filter(|value| !value.is_null()) else {
        return Ok(());
    };
    let files = sh["files"]
        .as_array()
        .ok_or_else(|| invalid("Invalid SOG SH files"))?;
    let image = |index: usize| {
        images
            .get(&("shN".into(), files[index].as_str().unwrap_or("").into()))
            .ok_or_else(|| invalid("Missing SOG SH atlas"))
    };
    let centroids = image(0)?;
    let labels = image(1)?;
    let coefficients = if version == 2 {
        match sh["bands"].as_u64() {
            Some(1) => 3,
            Some(2) => 8,
            Some(3) => 15,
            _ => return Err(invalid("Unsupported SOG SH degree")),
        }
    } else {
        match sh.pointer("/shape/1").and_then(Value::as_u64) {
            Some(9) => 3,
            Some(24) => 8,
            Some(45) => 15,
            _ => return Err(invalid("Unsupported SOG SH shape")),
        }
    };
    if labels.width() as usize * (labels.height() as usize) < count {
        return Err(invalid("SOG SH label atlas is too small"));
    }
    for pixel in labels.pixels().take(count) {
        let label = u32::from(pixel[0]) | (u32::from(pixel[1]) << 8);
        if (label % 64 + 1) * coefficients > centroids.width() || label / 64 >= centroids.height() {
            return Err(invalid("SOG SH label points outside the centroid atlas"));
        }
    }
    Ok(())
}

fn check_metadata_entry(
    name: &str,
    size: u64,
    metadata_name: &mut Option<String>,
) -> Result<(), SceneError> {
    if !name.ends_with("meta.json") {
        return Ok(());
    }
    if name.rsplit('/').next() != Some("meta.json") {
        return Err(invalid("SOG metadata must be named meta.json"));
    }
    if metadata_name.is_some() || size > 1024 * 1024 {
        return Err(invalid("SOG must contain one bounded meta.json"));
    }
    *metadata_name = Some(name.to_owned());
    Ok(())
}

fn read_metadata(zip: &mut ZipArchive<Cursor<&[u8]>>) -> Result<(Value, String), SceneError> {
    let mut total = 0u64;
    let mut metadata_name = None;
    for index in 0..zip.len() {
        let entry = zip
            .by_index(index)
            .map_err(|error| invalid(error.to_string()))?;
        total = total
            .checked_add(entry.size())
            .ok_or_else(|| invalid("SOG size overflow"))?;
        if total > MAX_ARCHIVE_BYTES || entry.size() > 64 * 1024 * 1024 {
            return Err(invalid("SOG expanded archive exceeds the import budget"));
        }
        check_metadata_entry(entry.name(), entry.size(), &mut metadata_name)?;
    }
    let metadata_name = metadata_name.ok_or_else(|| invalid("SOG has no meta.json"))?;
    let mut metadata = Vec::new();
    zip.by_name(&metadata_name)
        .map_err(|error| invalid(error.to_string()))?
        .read_to_end(&mut metadata)?;
    let meta: Value =
        serde_json::from_slice(&metadata).map_err(|error| invalid(error.to_string()))?;
    Ok((meta, metadata_name))
}

fn validate_group(
    zip: &mut ZipArchive<Cursor<&[u8]>>,
    section: &Value,
    prefix: &str,
    group: &str,
    count: u64,
    version: u64,
) -> Result<HashMap<(String, String), image::RgbaImage>, SceneError> {
    let files = section
        .get("files")
        .and_then(Value::as_array)
        .ok_or_else(|| invalid(format!("SOG {group} has no file list")))?;
    let expected = if group == "means" || group == "shN" {
        2
    } else {
        1
    };
    if files.len() != expected {
        return Err(invalid(format!("Invalid SOG {group} file count")));
    }
    if version == 2 && ["scales", "sh0", "shN"].contains(&group) {
        validate_codebook(section)?;
    }
    let mut images = HashMap::new();
    for name in files {
        let name = name
            .as_str()
            .ok_or_else(|| invalid("Invalid SOG image name"))?;
        if let Some(image) = read_atlas(zip, prefix, name, group, count)? {
            images.insert((group.to_owned(), name.to_owned()), image);
        }
    }
    Ok(images)
}

fn validate_codebook(section: &Value) -> Result<(), SceneError> {
    let codebook = section
        .get("codebook")
        .and_then(Value::as_array)
        .ok_or_else(|| invalid("Missing SOG codebook"))?;
    if codebook.len() != 256
        || codebook
            .iter()
            .any(|value| !value.as_f64().is_some_and(|value| value.is_finite()))
    {
        return Err(invalid("Invalid SOG codebook"));
    }
    Ok(())
}

fn read_atlas(
    zip: &mut ZipArchive<Cursor<&[u8]>>,
    prefix: &str,
    name: &str,
    group: &str,
    count: u64,
) -> Result<Option<image::RgbaImage>, SceneError> {
    let prefixed = format!("{prefix}{name}");
    let resolved = if zip.index_for_name(&prefixed).is_some() {
        prefixed.as_str()
    } else {
        name
    };
    let mut encoded = Vec::new();
    zip.by_name(resolved)
        .map_err(|error| invalid(error.to_string()))?
        .read_to_end(&mut encoded)?;
    let reader = image::ImageReader::with_format(Cursor::new(&encoded), image::ImageFormat::WebP);
    let (width, height) = reader
        .into_dimensions()
        .map_err(|error| invalid(error.to_string()))?;
    let pixels = u64::from(width) * u64::from(height);
    if pixels > MAX_IMAGE_PIXELS || (group != "shN" && pixels < count) {
        return Err(invalid("SOG atlas size is inconsistent with its count"));
    }
    if group == "quats" || group == "shN" {
        let image = image::load_from_memory_with_format(&encoded, image::ImageFormat::WebP)
            .map_err(|error| invalid(error.to_string()))?
            .to_rgba8();
        return Ok(Some(image));
    }
    Ok(None)
}

#[cfg(test)]
mod tests;
