//! Bounded Wavefront material libraries shared by mesh and file previews.
use std::{
    io::{BufRead, BufReader, Read},
    path::{Path, PathBuf},
};

#[derive(Clone, Debug)]
pub struct Material {
    pub name: String,
    pub diffuse: [f32; 3],
    pub specular: [f32; 3],
    pub shininess: f32,
    pub opacity: f32,
    pub optical_density: f32,
    pub textures: Vec<(String, PathBuf)>,
}
impl Material {
    /// Returns the diffuse color and opacity used by compact mesh renderers.
    pub fn rgba(&self) -> [f32; 4] {
        [
            self.diffuse[0],
            self.diffuse[1],
            self.diffuse[2],
            self.opacity,
        ]
    }
}

pub fn is_material_file(path: &Path) -> bool {
    path.extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("mtl"))
}

pub fn load(path: &Path) -> Result<Vec<Material>, String> {
    const MAX_BYTES: u64 = 2 * 1024 * 1024;
    let file = little_clip_core::kernel_interface::open_content(path, true)
        .map_err(|error| error.to_string())?;
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err("Material library exceeds the 2 MiB preview limit".into());
    }
    let (materials, _) = tobj::load_mtl_buf(&mut std::io::Cursor::new(bytes))
        .map_err(|error| format!("Cannot parse material library: {error}"))?;
    if materials.len() > 2048 {
        return Err("Material library exceeds the 2,048-material preview limit".into());
    }
    let base = path.parent().unwrap_or_else(|| Path::new("."));
    Ok(materials
        .into_iter()
        .map(|source| convert(source, base))
        .collect())
}

fn finite(value: Option<f32>, fallback: f32, maximum: f32) -> f32 {
    value
        .filter(|value| value.is_finite())
        .unwrap_or(fallback)
        .clamp(0.0, maximum)
}
fn color(value: Option<[f32; 3]>, fallback: [f32; 3]) -> [f32; 3] {
    value
        .unwrap_or(fallback)
        .map(|value| finite(Some(value), 0.5, 1.0))
}
fn convert(source: tobj::Material, base: &Path) -> Material {
    let textures = [
        ("Diffuse", source.diffuse_texture),
        ("Ambient", source.ambient_texture),
        ("Specular", source.specular_texture),
        ("Normal", source.normal_texture),
        ("Shininess", source.shininess_texture),
        ("Opacity", source.dissolve_texture),
    ]
    .into_iter()
    .filter_map(|(kind, value)| {
        let name = value.filter(|name| !name.is_empty())?;
        Some((kind.into(), base.join(name)))
    })
    .collect();
    Material {
        name: source.name,
        diffuse: color(source.diffuse, [0.65; 3]),
        specular: color(source.specular, [0.0; 3]),
        shininess: finite(source.shininess, 32.0, 1000.0),
        opacity: finite(source.dissolve, 1.0, 1.0),
        optical_density: finite(source.optical_density, 1.0, 10.0),
        textures,
    }
}

/// Finds library declarations using the same filename-with-spaces rule as tobj.
pub(super) fn obj_libraries(path: &Path) -> Result<Vec<PathBuf>, String> {
    let base = path.parent().unwrap_or_else(|| Path::new("."));
    let file = little_clip_core::kernel_interface::open_content(path, true)
        .map_err(|error| error.to_string())?;
    let mut paths = Vec::new();
    for line in BufReader::new(file).lines() {
        let line = line.map_err(|error| error.to_string())?;
        let line = line.trim();
        if line.split_whitespace().next() == Some("mtllib") {
            let filename = line[6..].trim();
            if !filename.is_empty() {
                paths.push(base.join(filename));
            }
        }
    }
    Ok(paths)
}
