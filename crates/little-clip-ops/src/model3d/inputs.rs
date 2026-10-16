//! Descriptor-backed views for path-based asset parsers and their resources.
use super::materials;
use std::{
    borrow::Cow,
    collections::HashMap,
    fs::File,
    io::{BufRead, BufReader, Read, Seek, SeekFrom, Write},
    os::fd::AsRawFd,
    path::{Path, PathBuf},
};

const MAX_SOURCE: u64 = 512 * 1024 * 1024;
const MAX_RESOURCES: usize = 4096;

pub(super) struct Prepared {
    source: File,
    resources: HashMap<PathBuf, File>,
    material_order: Vec<PathBuf>,
}
fn descriptor(file: &File) -> PathBuf {
    format!("/proc/self/fd/{}", file.as_raw_fd()).into()
}
fn error(error: impl ToString) -> String {
    error.to_string()
}
fn bounded(path: &Path) -> Result<Vec<u8>, String> {
    let file = little_clip_core::kernel_interface::open_content(path, true).map_err(error)?;
    let mut bytes = Vec::new();
    file.take(MAX_SOURCE + 1)
        .read_to_end(&mut bytes)
        .map_err(error)?;
    if bytes.len() as u64 > MAX_SOURCE {
        return Err("Model source exceeds the 512 MiB import limit".into());
    }
    Ok(bytes)
}
impl Prepared {
    fn new() -> Result<Self, String> {
        Ok(Self {
            source: tempfile::tempfile().map_err(error)?,
            resources: HashMap::new(),
            material_order: Vec::new(),
        })
    }
    pub fn path(&self) -> PathBuf {
        descriptor(&self.source)
    }
    fn resource(&mut self, path: &Path, missing_material: bool) -> Result<PathBuf, String> {
        if !self.resources.contains_key(path) {
            if self.resources.len() >= MAX_RESOURCES {
                return Err("Model exceeds the 4096-resource import limit".into());
            }
            let file = match little_clip_core::kernel_interface::open_content(path, true) {
                Ok(file) => file,
                Err(error) if missing_material && error.kind() == std::io::ErrorKind::NotFound => {
                    tempfile::tempfile().map_err(self::error)?
                }
                Err(error) => return Err(error.to_string()),
            };
            if file.metadata().map_err(error)?.len() > MAX_SOURCE {
                return Err("Model resource exceeds the 512 MiB import limit".into());
            }
            self.resources.insert(path.into(), file);
        }
        Ok(descriptor(&self.resources[path]))
    }
    pub fn obj(path: &Path) -> Result<Self, String> {
        let mut prepared = Self::new()?;
        let bytes = bounded(path)?;
        let base = path.parent().unwrap_or_else(|| Path::new("."));
        for line in BufReader::new(bytes.as_slice()).lines() {
            let line = line.map_err(error)?;
            let trimmed = line.trim();
            if trimmed.split_whitespace().next() == Some("mtllib")
                && !trimmed[6..].trim().is_empty()
            {
                // Match tobj's complete remainder, including spaces in filenames.
                let library = base.join(trimmed[6..].trim());
                let pinned = prepared.resource(&library, true)?;
                if prepared.material_order.len() >= MAX_RESOURCES {
                    return Err("OBJ exceeds the 4096-material-reference limit".into());
                }
                prepared.material_order.push(library);
                writeln!(prepared.source, "mtllib {}", pinned.display()).map_err(error)?;
            } else {
                writeln!(prepared.source, "{line}").map_err(error)?;
            }
        }
        prepared.source.seek(SeekFrom::Start(0)).map_err(error)?;
        Ok(prepared)
    }
    pub fn materials(&self) -> HashMap<String, materials::Material> {
        self.material_order
            .iter()
            .flat_map(|path| {
                materials::load(&descriptor(&self.resources[path])).unwrap_or_default()
            })
            .map(|material| (material.name.clone(), material))
            .collect()
    }
    pub fn gltf(path: &Path) -> Result<Self, String> {
        let mut prepared = Self::new()?;
        let bytes = bounded(path)?;
        let binary = bytes.starts_with(b"glTF");
        if binary {
            let mut glb = gltf::binary::Glb::from_slice(&bytes).map_err(error)?;
            glb.json = Cow::Owned(prepared.rewrite_json(path, &glb.json)?);
            glb.to_writer(&mut prepared.source).map_err(error)?;
        } else {
            let json = prepared.rewrite_json(path, &bytes)?;
            prepared.source.write_all(&json).map_err(error)?;
        }
        prepared.source.seek(SeekFrom::Start(0)).map_err(error)?;
        Ok(prepared)
    }
    fn rewrite_json(&mut self, path: &Path, bytes: &[u8]) -> Result<Vec<u8>, String> {
        let mut document: serde_json::Value = serde_json::from_slice(bytes).map_err(error)?;
        let base = path.parent().unwrap_or_else(|| Path::new("."));
        for group in ["buffers", "images"] {
            if let Some(entries) = document
                .get_mut(group)
                .and_then(serde_json::Value::as_array_mut)
            {
                for entry in entries {
                    let Some(uri) = entry
                        .get("uri")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_owned)
                    else {
                        continue;
                    };
                    if uri.starts_with("data:") {
                        continue;
                    }
                    let resource = resource_path(base, &uri)?;
                    let pinned = self.resource(&resource, false)?;
                    if group == "images"
                        && entry.get("mimeType").is_none_or(serde_json::Value::is_null)
                    {
                        let mime = match resource
                            .extension()
                            .and_then(|value| value.to_str())
                            .map(str::to_ascii_lowercase)
                            .as_deref()
                        {
                            Some("png") => Some("image/png"),
                            Some("jpg" | "jpeg") => Some("image/jpeg"),
                            _ => None,
                        };
                        if let Some(mime) = mime {
                            entry["mimeType"] = mime.into();
                        }
                    }
                    entry["uri"] = format!("file:{}", pinned.display()).into();
                }
            }
        }
        serde_json::to_vec(&document).map_err(error)
    }
}
fn resource_path(base: &Path, uri: &str) -> Result<PathBuf, String> {
    if let Some(path) = uri
        .strip_prefix("file://")
        .or_else(|| uri.strip_prefix("file:"))
    {
        return Ok(PathBuf::from(path));
    }
    if uri.contains(':') {
        return Err("Unsupported model resource URI scheme".into());
    }
    let decoded = percent_encoding::percent_decode_str(uri)
        .decode_utf8()
        .map_err(error)?;
    Ok(base.join(decoded.as_ref()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn obj_resources_stay_pinned_and_missing_materials_remain_optional() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("mesh.obj");
        let library = temp.path().join("colors with spaces.mtl");
        std::fs::write(
            &path,
            "mtllib colors with spaces.mtl\nv 0 0 0\nv 1 0 0\nv 0 1 0\nusemtl Red\nf 1 2 3\n",
        )
        .unwrap();
        std::fs::write(&library, "newmtl Red\nKd 1 0 0\n").unwrap();
        let input = Prepared::obj(&path).unwrap();
        std::fs::rename(&library, temp.path().join("original.mtl")).unwrap();
        std::os::unix::fs::symlink("/dev/null", &library).unwrap();
        let asset = scenix_loader::obj::load_with_materials(input.path()).unwrap();
        assert_eq!(asset.geometries.len(), 1);
        assert_eq!(input.materials()["Red"].diffuse, [1.0, 0.0, 0.0]);
        assert!(Prepared::obj(&path).is_err());
        std::fs::remove_file(&library).unwrap();
        assert!(
            scenix_loader::obj::load_with_materials(Prepared::obj(&path).unwrap().path()).is_ok()
        );
    }
    #[test]
    fn gltf_resource_references_use_guarded_inodes_and_preserve_uri_resolution() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("mesh.gltf");
        let buffer = temp.path().join("buffer space.bin");
        std::fs::write(&buffer, b"original bytes").unwrap();
        std::fs::write(&path,r#"{"asset":{"version":"2.0"},"buffers":[{"uri":"buffer%20space.bin","byteLength":14}]}"#).unwrap();
        let input = Prepared::gltf(&path).unwrap();
        std::fs::rename(&buffer, temp.path().join("original.bin")).unwrap();
        std::os::unix::fs::symlink("/proc/uptime", &buffer).unwrap();
        let document: serde_json::Value =
            serde_json::from_slice(&std::fs::read(input.path()).unwrap()).unwrap();
        let uri = document["buffers"][0]["uri"].as_str().unwrap();
        assert_eq!(
            std::fs::read(uri.strip_prefix("file:").unwrap()).unwrap(),
            b"original bytes"
        );
        assert!(Prepared::gltf(&path).is_err());
        assert_eq!(
            resource_path(temp.path(), "a?b#c.bin").unwrap(),
            temp.path().join("a?b#c.bin")
        );
        assert!(resource_path(temp.path(), "https://example.com/x").is_err());
        assert!(resource_path(temp.path(), "%FF.bin").is_err());
    }
    #[test]
    fn gltf_and_glb_vendor_imports_keep_external_buffers_and_images_pinned() {
        let temp = tempfile::tempdir().unwrap();
        let buffer = temp.path().join("geometry.bin");
        let texture = temp.path().join("texture.png");
        let bytes: Vec<u8> = [0.0f32, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]
            .into_iter()
            .flat_map(f32::to_le_bytes)
            .collect();
        std::fs::write(&buffer, &bytes).unwrap();
        image::RgbaImage::from_pixel(2, 2, image::Rgba([255, 0, 0, 255]))
            .save(&texture)
            .unwrap();
        let mut json = serde_json::json!({"asset":{"version":"2.0"},"buffers":[{"uri":"geometry.bin","byteLength":36}],"bufferViews":[{"buffer":0,"byteLength":36}],"accessors":[{"bufferView":0,"componentType":5126,"count":3,"type":"VEC3","min":[0,0,0],"max":[1,1,0]}],"meshes":[{"primitives":[{"attributes":{"POSITION":0}}]}],"nodes":[{"mesh":0}],"scenes":[{"nodes":[0]}],"scene":0,"images":[{"uri":"texture.png"}]});
        let path = temp.path().join("mesh.gltf");
        std::fs::write(&path, serde_json::to_vec(&json).unwrap()).unwrap();
        let input = Prepared::gltf(&path).unwrap();
        json["buffers"][0].as_object_mut().unwrap().remove("uri");
        let binary = gltf::binary::Glb {
            header: gltf::binary::Header {
                magic: *b"glTF",
                version: 2,
                length: 0,
            },
            json: Cow::Owned(serde_json::to_vec(&json).unwrap()),
            bin: Some(Cow::Borrowed(&bytes)),
        };
        let glb = temp.path().join("mesh.glb");
        binary.to_writer(File::create(&glb).unwrap()).unwrap();
        let glb_input = Prepared::gltf(&glb).unwrap();
        for (path, original) in [(&buffer, "old.bin"), (&texture, "old.png")] {
            std::fs::rename(path, temp.path().join(original)).unwrap();
            std::os::unix::fs::symlink("/dev/null", path).unwrap();
        }
        for prepared in [&input, &glb_input] {
            let asset = scenix_loader::GltfLoader::new()
                .load(prepared.path())
                .unwrap();
            assert_eq!(asset.meshes.len(), 1);
        }
        assert!(Prepared::gltf(&path).is_err());
        assert!(Prepared::gltf(&glb).is_err());
    }
    #[test]
    fn obj_material_name_overrides_follow_original_library_order() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("mesh.obj");
        std::fs::write(temp.path().join("first.mtl"), "newmtl Shared\nKd 1 0 0\n").unwrap();
        std::fs::write(temp.path().join("second.mtl"), "newmtl Shared\nKd 0 1 0\n").unwrap();
        std::fs::write(
            &path,
            "mtllib first.mtl\nmtllib second.mtl\nmtllib first.mtl\n",
        )
        .unwrap();
        assert_eq!(
            Prepared::obj(&path).unwrap().materials()["Shared"].diffuse,
            [1.0, 0.0, 0.0]
        );
    }
}
