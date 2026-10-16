use std::io::{BufReader, Read, Seek};
use std::path::Path;
use wgpu_3dgs_viewer::core::glam::{Quat, U8Vec4, Vec3};
use wgpu_3dgs_viewer::core::{Gaussian, PlyGaussians, PlyHeader};

use crate::SceneError;

/// Decoded Gaussians and their bounds, shared independently of GPU resources.
pub struct SplatScene {
    pub(crate) gaussians: Vec<Gaussian>,
    pub(crate) linear_color: bool,
    pub(crate) center: Vec3,
    pub(crate) radius: f32,
}

impl SplatScene {
    /// Loads Gaussian PLY/GLB, SPZ v1–v4, SPLAT or bundled SOG v1/v2 data.
    ///
    /// # Errors
    /// Returns an error for unreadable, empty, invalid or unsupported data.
    pub fn load(path: &Path) -> Result<Self, SceneError> {
        let extension = path
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if extension == "glb" {
            return crate::glb::load(path);
        }
        let gaussians = match extension.as_str() {
            "ply" => load_ply(path)?,
            "spz" => crate::spz::load(path)?,
            "splat" => decode_splat(&crate::read_bounded(
                little_clip_core::kernel_interface::open_content(path, true)?,
                2_000_000 * 32,
                "SPLAT",
            )?)?,
            "sog" => crate::sog::load(path)?,
            _ => {
                return Err(SceneError::Invalid(format!(
                    "Unsupported splat format: .{extension}"
                )));
            }
        };
        Self::from_gaussians(gaussians)
    }

    /// Returns the number of Gaussians in the decoded scene.
    pub fn len(&self) -> usize {
        self.gaussians.len()
    }

    /// Returns the framing center and bounding sphere radius in scene units.
    pub fn framing(&self) -> ([f32; 3], f32) {
        (self.center.to_array(), self.radius)
    }

    /// Returns whether the scene's colors require linear-to-sRGB output encoding.
    pub fn has_linear_color(&self) -> bool {
        self.linear_color
    }

    /// Returns whether the scene contains no Gaussians.
    pub fn is_empty(&self) -> bool {
        self.gaussians.is_empty()
    }

    pub(crate) fn from_gaussians(gaussians: Vec<Gaussian>) -> Result<Self, SceneError> {
        if gaussians.is_empty() {
            return Err(SceneError::Invalid("Scene contains no Gaussians".into()));
        }
        let mut min = Vec3::splat(f32::INFINITY);
        let mut max = Vec3::splat(f32::NEG_INFINITY);
        for gaussian in &gaussians {
            if !gaussian.pos.is_finite()
                || !gaussian.scale.is_finite()
                || gaussian.scale.min_element() <= 0.0
                || !gaussian.rot.is_finite()
                || gaussian.rot.length_squared() < 1e-12
                || gaussian.sh.iter().any(|sh| !sh.is_finite())
            {
                return Err(SceneError::Invalid(
                    "Non-finite or degenerate Gaussian attributes".into(),
                ));
            }
            let extent = Vec3::splat(gaussian.scale.max_element() * 3.0);
            min = min.min(gaussian.pos - extent);
            max = max.max(gaussian.pos + extent);
        }
        let center = min * 0.5 + max * 0.5;
        let radius = (max - center).length().max(1e-4);
        if !center.is_finite() || !radius.is_finite() {
            return Err(SceneError::Invalid("Scene bounds overflow".into()));
        }
        Ok(Self {
            gaussians,
            linear_color: false,
            center,
            radius,
        })
    }
}

fn load_ply(path: &Path) -> Result<Vec<Gaussian>, SceneError> {
    let mut reader = BufReader::new(little_clip_core::kernel_interface::open_content(
        path, true,
    )?);
    if reader.get_ref().metadata()?.len() > 512 * 1024 * 1024 {
        return Err(SceneError::Invalid(
            "PLY exceeds the 512 MiB import budget".into(),
        ));
    }
    let parser = ply_rs::parser::Parser::<ply_rs::ply::DefaultElement>::new();
    let header = parser.read_header(&mut reader)?;
    let vertex = header
        .elements
        .get("vertex")
        .ok_or_else(|| SceneError::Invalid("PLY has no vertices".into()))?;
    if vertex.count > 2_000_000
        || header
            .elements
            .values()
            .any(|element| element.count > 2_000_000)
    {
        return Err(SceneError::Invalid(
            "PLY exceeds the two-million-element import budget".into(),
        ));
    }
    if header.elements.contains_key("chunk") {
        return load_compressed_ply(reader);
    }
    let coefficients = validate_gaussian_ply_vertex(vertex)?;
    // The upstream 0.6 reader's fast-path detection compares a zipped prefix
    // without checking property count. A degree-zero binary PLY must use the
    // general reader or it is read as a full degree-three record.
    let pods = PlyGaussians::read_gaussians(&mut reader, PlyHeader::Custom(header))?
        .collect::<Result<Vec<_>, _>>()?;
    let per_channel = coefficients / 3;
    Ok(pods
        .into_iter()
        .map(|mut pod| {
            // PLY channel arrays have degree-dependent lengths; upstream's
            // intermediate representation always uses 15 coefficients per channel.
            let source = pod.sh;
            pod.sh = [0.0; 45];
            for channel in 0..3 {
                for coefficient in 0..per_channel {
                    pod.sh[channel * 15 + coefficient] =
                        source[channel * per_channel + coefficient];
                }
            }
            Gaussian::from_ply(&pod)
        })
        .collect())
}

fn validate_gaussian_ply_vertex(vertex: &ply_rs::ply::ElementDef) -> Result<usize, SceneError> {
    if !vertex.properties.contains_key("f_dc_0") {
        return Err(SceneError::Invalid(
            "PLY contains mesh or point-cloud data; only Gaussian PLY is supported".into(),
        ));
    }
    for name in [
        "x", "y", "z", "f_dc_0", "f_dc_1", "f_dc_2", "opacity", "scale_0", "scale_1", "scale_2",
        "rot_0", "rot_1", "rot_2", "rot_3",
    ] {
        let expected = ply_rs::ply::PropertyType::Scalar(ply_rs::ply::ScalarType::Float);
        if !vertex
            .properties
            .get(name)
            .is_some_and(|property| property.data_type == expected)
        {
            return Err(SceneError::Invalid(format!(
                "PLY is missing a float Gaussian property: {name}"
            )));
        }
    }
    let coefficients = vertex
        .properties
        .keys()
        .filter(|name| name.starts_with("f_rest_"))
        .count();
    if ![0, 9, 24, 45].contains(&coefficients)
        || (0..coefficients)
            .any(|index| !vertex.properties.contains_key(&format!("f_rest_{index}")))
    {
        return Err(SceneError::Invalid(
            "Unsupported PLY spherical harmonics layout".into(),
        ));
    }
    Ok(coefficients)
}

fn load_compressed_ply(mut reader: BufReader<std::fs::File>) -> Result<Vec<Gaussian>, SceneError> {
    use spark_lib::{decoder::ChunkReceiver, gsplat::GsplatArray, ply::PlyDecoder};
    const MAX_BYTES: u64 = 512 * 1024 * 1024;
    if reader.get_ref().metadata()?.len() > MAX_BYTES {
        return Err(SceneError::Invalid(
            "Compressed PLY exceeds the 512 MiB import budget".into(),
        ));
    }
    reader.rewind()?;
    let mut decoder = PlyDecoder::new(GsplatArray::new());
    let mut bytes = [0u8; 64 * 1024];
    let mut total = 0u64;
    loop {
        let count = reader.read(&mut bytes)?;
        if count == 0 {
            break;
        }
        total += count as u64;
        if total > MAX_BYTES {
            return Err(SceneError::Invalid(
                "Compressed PLY changed beyond its import budget".into(),
            ));
        }
        decoder
            .push(&bytes[..count])
            .map_err(|error| SceneError::Invalid(format!("Compressed PLY decoding: {error:#}")))?;
    }
    decoder
        .finish()
        .map_err(|error| SceneError::Invalid(format!("Compressed PLY decoding: {error:#}")))?;
    Ok(crate::spark::gaussians(&decoder.into_splats()))
}

// The established antimatter15 format is 32 bytes per splat: six little-endian
// floats (position, linear scale), RGBA bytes, and a quantized wxyz quaternion.
// Reference: https://github.com/antimatter15/splat/blob/main/main.js
fn decode_splat(bytes: &[u8]) -> Result<Vec<Gaussian>, SceneError> {
    if bytes.is_empty() || !bytes.len().is_multiple_of(32) {
        return Err(SceneError::Invalid(
            "SPLAT data must contain complete 32-byte records".into(),
        ));
    }
    bytes
        .as_chunks::<32>()
        .0
        .iter()
        .map(|record| {
            let float = |offset| {
                f32::from_le_bytes(record[offset..offset + 4].try_into().expect("fixed record"))
            };
            let component = |offset| (f32::from(record[offset]) - 128.0) / 128.0;
            let rot = Quat::from_xyzw(component(29), component(30), component(31), component(28));
            if rot.length_squared() < 1e-12 {
                return Err(SceneError::Invalid(
                    "SPLAT quaternion has zero length".into(),
                ));
            }
            Ok(Gaussian {
                pos: Vec3::new(float(0), float(4), float(8)),
                scale: Vec3::new(float(12), float(16), float(20)),
                color: U8Vec4::new(record[24], record[25], record[26], record[27]),
                rot: rot.normalize(),
                sh: [Vec3::ZERO; 15],
            })
        })
        .collect()
}

#[cfg(test)]
pub(crate) mod tests {
    #[test]
    fn scene_containers_reject_kernel_interfaces_before_reading() {
        let directory = tempfile::tempdir().unwrap();
        for extension in ["ply", "splat", "spz", "sog", "glb"] {
            let path = directory.path().join(format!("interface.{extension}"));
            std::os::unix::fs::symlink("/dev/null", &path).unwrap();
            assert!(
                matches!(super::SplatScene::load(&path), Err(crate::SceneError::Io(error)) if error.kind() == std::io::ErrorKind::PermissionDenied)
            );
        }
    }

    use super::*;
    use wgpu_3dgs_viewer::core::{Gaussians, GaussiansSource};

    #[test]
    fn compressed_ply_preserves_exported_records_and_rejects_truncation() {
        let bytes = include_bytes!("../tests/fixtures/packed.compressed.ply");
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("packed.ply");
        std::fs::write(&path, bytes).unwrap();
        let scene = SplatScene::load(&path).unwrap();
        assert_eq!(scene.len(), 2);
        for gaussian in &scene.gaussians {
            assert!((gaussian.pos.x.abs() - 0.5).abs() < 0.002);
            assert!(gaussian.scale.abs_diff_eq(
                Vec3::new((-2.3f32).exp(), (-1.6f32).exp(), (-1.9f32).exp()),
                0.0002
            ));
            assert!(gaussian.color.x > 190 && gaussian.color.w > 230);
            assert!(gaussian.rot.angle_between(Quat::IDENTITY) < 0.003);
        }
        std::fs::write(&path, &bytes[..bytes.len() - 2]).unwrap();
        assert!(SplatScene::load(&path).is_err());
    }

    pub(crate) fn scene() -> SplatScene {
        SplatScene::from_gaussians(vec![
            Gaussian {
                pos: Vec3::new(-0.5, 0.0, 0.0),
                scale: Vec3::splat(0.15),
                color: U8Vec4::new(255, 30, 20, 240),
                rot: Quat::IDENTITY,
                sh: [Vec3::ZERO; 15],
            },
            Gaussian {
                pos: Vec3::new(0.5, 0.3, 0.2),
                scale: Vec3::new(0.08, 0.25, 0.1),
                color: U8Vec4::new(20, 255, 30, 240),
                rot: Quat::IDENTITY,
                sh: [Vec3::ZERO; 15],
            },
        ])
        .unwrap()
    }

    #[test]
    fn ply_and_spz_roundtrip_preserve_scene_bounds_and_opacity() {
        let original = scene();
        let directory = tempfile::tempdir().unwrap();
        for (extension, source) in [("ply", GaussiansSource::Ply), ("spz", GaussiansSource::Spz)] {
            let path = directory.path().join(format!("scene.{extension}"));
            Gaussians::from_gaussians_iter(original.gaussians.iter().copied(), source)
                .write_to_file(&path)
                .unwrap();
            let decoded = SplatScene::load(&path).unwrap();
            assert_eq!(decoded.len(), original.len());
            assert!((decoded.center - original.center).length() < 0.04);
            assert!((decoded.radius - original.radius).abs() < 0.04);
            assert_eq!(decoded.gaussians[0].color.w, 240);
        }
    }

    #[test]
    fn spz_rejects_excessive_counts_unsupported_versions_and_corrupt_gzip() {
        use std::io::Write;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("scene.spz");
        Gaussians::from_gaussians_iter(scene().gaussians.into_iter(), GaussiansSource::Spz)
            .write_to_file(&path)
            .unwrap();
        let compressed = std::fs::read(&path).unwrap();
        let mut bytes = Vec::new();
        flate2::read::GzDecoder::new(compressed.as_slice())
            .read_to_end(&mut bytes)
            .unwrap();
        for (offset, value, message) in [(8, 2_000_001u32, "budget"), (4, 4, "version")] {
            let mut altered = bytes.clone();
            altered[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
            let mut encoder =
                flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
            encoder.write_all(&altered).unwrap();
            std::fs::write(&path, encoder.finish().unwrap()).unwrap();
            assert!(
                SplatScene::load(&path)
                    .err()
                    .unwrap()
                    .to_string()
                    .contains(message)
            );
        }
        let mut corrupt = compressed;
        let checksum = corrupt.len() - 8;
        corrupt[checksum] ^= 1;
        std::fs::write(&path, corrupt).unwrap();
        assert!(SplatScene::load(&path).is_err());
    }

    #[test]
    fn splat_decodes_linear_scales_rgba_and_wxyz_rotation() {
        let mut record = Vec::new();
        for float in [1.0f32, 2.0, 3.0, 0.1, 0.2, 0.3] {
            record.extend(float.to_le_bytes());
        }
        record.extend([10, 20, 30, 200, 255, 128, 128, 128]);
        let decoded = SplatScene::from_gaussians(decode_splat(&record).unwrap()).unwrap();
        assert_eq!(decoded.gaussians[0].pos, Vec3::new(1.0, 2.0, 3.0));
        assert_eq!(decoded.gaussians[0].scale, Vec3::new(0.1, 0.2, 0.3));
        assert_eq!(decoded.gaussians[0].color, U8Vec4::new(10, 20, 30, 200));
        assert_eq!(decoded.gaussians[0].rot, Quat::IDENTITY);
        assert!(decode_splat(&record[..31]).is_err());
        record[0..4].copy_from_slice(&f32::NAN.to_le_bytes());
        assert!(SplatScene::from_gaussians(decode_splat(&record).unwrap()).is_err());
    }

    #[test]
    fn lower_degree_binary_ply_reads_complete_records_and_maps_sh_channels() {
        let directory = tempfile::tempdir().unwrap();
        for per_channel in [0, 3, 8, 15] {
            let mut properties = vec![
                ("x".to_owned(), 1.0),
                ("y".to_owned(), 2.0),
                ("z".to_owned(), 3.0),
            ];
            for name in ["nx", "ny", "nz", "f_dc_0", "f_dc_1", "f_dc_2"] {
                properties.push((name.into(), 0.0));
            }
            for index in 0..per_channel * 3 {
                properties.push((format!("f_rest_{index}"), index as f32 + 1.0));
            }
            for (name, value) in [
                ("opacity", 1.0),
                ("scale_0", -2.0),
                ("scale_1", -2.0),
                ("scale_2", -2.0),
                ("rot_0", 1.0),
                ("rot_1", 0.0),
                ("rot_2", 0.0),
                ("rot_3", 0.0),
            ] {
                properties.push((name.into(), value));
            }
            let mut bytes = b"ply\nformat binary_little_endian 1.0\nelement vertex 1\n".to_vec();
            for (name, _) in &properties {
                bytes.extend(format!("property float {name}\n").as_bytes());
            }
            bytes.extend(b"end_header\n");
            for (_, value) in properties {
                bytes.extend(f32::to_le_bytes(value));
            }
            let path = directory.path().join("low-degree.ply");
            std::fs::write(&path, bytes).unwrap();
            let decoded = SplatScene::load(&path).unwrap();
            assert_eq!(decoded.len(), 1);
            let gaussian = &decoded.gaussians[0];
            assert_eq!(gaussian.pos, Vec3::new(1.0, 2.0, 3.0));
            for (index, coefficient) in gaussian.sh.iter().enumerate() {
                let expected = if index < per_channel {
                    Vec3::new(
                        index as f32 + 1.0,
                        (index + per_channel) as f32 + 1.0,
                        (index + per_channel * 2) as f32 + 1.0,
                    )
                } else {
                    Vec3::ZERO
                };
                assert_eq!(*coefficient, expected);
            }
        }
    }
}
