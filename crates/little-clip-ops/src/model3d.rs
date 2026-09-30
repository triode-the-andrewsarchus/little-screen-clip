//! 3D model parsing and vector geometry using the Scenix asset loaders.
//!
//! Provides 3D vector operations (`Vec3`), triangle representations with optional
//! diffuse color tinting, and multi-format model loading for STL, Wavefront OBJ,
//! and glTF / GLB assets.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

#[derive(Debug, Clone, PartialEq, Eq)]
struct FileRevision {
    modified: SystemTime,
    len: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ModelCacheIdentity {
    source: FileRevision,
    dependencies: Vec<(PathBuf, FileRevision)>,
}

/// Keep the parsed card-preview cache below 128 MiB of model-owned memory.
const MODEL_CACHE_MAX_BYTES: usize = 128 * 1024 * 1024;
/// Bound the number of cached paths even when their meshes are tiny.
const MODEL_CACHE_MAX_ENTRIES: usize = 16;

#[derive(Debug)]
struct ModelCacheEntry {
    identity: ModelCacheIdentity,
    model: Arc<Model3d>,
    retained_bytes: usize,
    last_access: u64,
}

#[derive(Debug, Default)]
struct ModelCache {
    entries: HashMap<PathBuf, ModelCacheEntry>,
    retained_bytes: usize,
    access_clock: u64,
}

impl ModelCache {
    fn remove(&mut self, path: &Path) {
        if let Some(entry) = self.entries.remove(path) {
            self.retained_bytes = self.retained_bytes.saturating_sub(entry.retained_bytes);
        }
    }

    fn get(&mut self, path: &Path, identity: &ModelCacheIdentity) -> Option<Arc<Model3d>> {
        if !self
            .entries
            .get(path)
            .is_some_and(|entry| &entry.identity == identity)
        {
            self.remove(path);
            return None;
        }
        self.access_clock = self.access_clock.saturating_add(1);
        let entry = self.entries.get_mut(path)?;
        entry.last_access = self.access_clock;
        Some(Arc::clone(&entry.model))
    }

    fn insert(
        &mut self,
        path: PathBuf,
        identity: ModelCacheIdentity,
        model: Arc<Model3d>,
        max_bytes: usize,
        max_entries: usize,
    ) {
        self.remove(&path);
        let retained_bytes = model_retained_bytes(&model);
        if retained_bytes > max_bytes || max_entries == 0 {
            return;
        }
        while self.entries.len() >= max_entries
            || self.retained_bytes.saturating_add(retained_bytes) > max_bytes
        {
            let Some(lru_path) = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.last_access)
                .map(|(path, _)| path.clone())
            else {
                break;
            };
            self.remove(&lru_path);
        }
        self.access_clock = self.access_clock.saturating_add(1);
        self.retained_bytes = self.retained_bytes.saturating_add(retained_bytes);
        self.entries.insert(
            path,
            ModelCacheEntry {
                identity,
                model,
                retained_bytes,
                last_access: self.access_clock,
            },
        );
    }
}

static MODEL_CACHE: Mutex<Option<ModelCache>> = Mutex::new(None);

/// Counts the cache-owned allocation footprint; callers' allocations and Arc
/// reference counts are deliberately outside this cache budget.
fn model_retained_bytes(model: &Model3d) -> usize {
    std::mem::size_of::<Model3d>()
        .saturating_add(model.name.capacity())
        .saturating_add(
            model
                .triangles
                .capacity()
                .saturating_mul(std::mem::size_of::<Triangle>()),
        )
        .saturating_add(
            model
                .thumbnail_triangles
                .capacity()
                .saturating_mul(std::mem::size_of::<Triangle>()),
        )
}

fn file_revision(path: &Path) -> Option<FileRevision> {
    let metadata = std::fs::metadata(path).ok()?;
    Some(FileRevision {
        modified: metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH),
        len: metadata.len(),
    })
}

/// Returns the metadata revisions which affect a file-backed model. Unknown or
/// non-local glTF buffer and image URIs disable caching so they cannot produce
/// stale hits.
fn model_cache_identity(path: &Path) -> Option<ModelCacheIdentity> {
    let source = file_revision(path)?;
    let mut dependencies = Vec::new();

    let extension = path.extension().and_then(|ext| ext.to_str());
    if extension.is_some_and(|ext| ext.eq_ignore_ascii_case("glb")) {
        // GLB dependency URIs live in its JSON chunk; until that chunk is
        // inspected, only caching by the container file can return stale data.
        return None;
    }

    if extension.is_some_and(|ext| ext.eq_ignore_ascii_case("gltf")) {
        let source_bytes = std::fs::read(path).ok()?;
        let document: serde_json::Value = serde_json::from_slice(&source_bytes).ok()?;
        let base_dir = path.parent().unwrap_or_else(|| Path::new("./"));
        for dependency_group in ["buffers", "images"] {
            let Some(entries) = document
                .get(dependency_group)
                .and_then(serde_json::Value::as_array)
            else {
                continue;
            };
            for entry in entries {
                let Some(uri) = entry.get("uri").and_then(serde_json::Value::as_str) else {
                    // Embedded buffers and bufferView-backed images have no
                    // separate external revision; their data belongs to a
                    // captured buffer or to the source GLB.
                    continue;
                };
                if uri.starts_with("data:") {
                    continue;
                }
                // The glTF importer percent-decodes relative URIs before
                // joining them to the source directory. URL joining would
                // instead treat '?' and '#' as query/fragment delimiters, and
                // file: handling has separate semantics. Skip these unusual
                // forms rather than fingerprinting a possibly different file.
                if uri
                    .chars()
                    .any(|character| matches!(character, ':' | '?' | '#' | '%'))
                {
                    return None;
                }
                let dependency_path = base_dir.join(uri);
                let revision = file_revision(&dependency_path)?;
                dependencies.push((dependency_path, revision));
            }
        }
    }

    Some(ModelCacheIdentity {
        source,
        dependencies,
    })
}

/// 3-dimensional single-precision floating point vector for 3D coordinates.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Vec3 {
    /// X component.
    pub x: f32,
    /// Y component.
    pub y: f32,
    /// Z component.
    pub z: f32,
}

impl Vec3 {
    /// Zero vector constant.
    pub const ZERO: Self = Self {
        x: 0.0,
        y: 0.0,
        z: 0.0,
    };

    /// Constructs a new 3D vector.
    pub const fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }

    /// Subtracts vector `other` from `self`.
    pub fn sub(self, other: Vec3) -> Vec3 {
        Vec3::new(self.x - other.x, self.y - other.y, self.z - other.z)
    }

    /// Adds vector `other` to `self`.
    pub fn add(self, other: Vec3) -> Vec3 {
        Vec3::new(self.x + other.x, self.y + other.y, self.z + other.z)
    }

    /// Multiplies all components by scalar `s`.
    pub fn scale(self, s: f32) -> Vec3 {
        Vec3::new(self.x * s, self.y * s, self.z * s)
    }

    /// Computes dot product with `other`.
    pub fn dot(self, other: Vec3) -> f32 {
        self.x * other.x + self.y * other.y + self.z * other.z
    }

    /// Computes 3D cross product with `other`.
    pub fn cross(self, other: Vec3) -> Vec3 {
        Vec3::new(
            self.y * other.z - self.z * other.y,
            self.z * other.x - self.x * other.z,
            self.x * other.y - self.y * other.x,
        )
    }

    /// Computes Euclidean norm (length) of the vector.
    pub fn length(self) -> f32 {
        (self.x * self.x + self.y * self.y + self.z * self.z).sqrt()
    }

    /// Returns unit length vector in same direction, or Z-up fallback if zero length.
    pub fn normalize(self) -> Vec3 {
        let l = self.length();
        if l > 1e-6 {
            Vec3::new(self.x / l, self.y / l, self.z / l)
        } else {
            Vec3::new(0.0, 0.0, 1.0)
        }
    }
}

/// A 3D triangle defined by facet normal, three vertices, and an optional diffuse color tint.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Triangle {
    /// Surface normal vector.
    pub normal: Vec3,
    /// First vertex coordinate.
    pub v1: Vec3,
    /// Second vertex coordinate.
    pub v2: Vec3,
    /// Third vertex coordinate.
    pub v3: Vec3,
    /// Optional diffuse RGBA color in 0.0..1.0 range.
    pub color: Option<[f32; 4]>,
}

impl Triangle {
    /// Constructs a triangle without color tint.
    pub fn new(normal: Vec3, v1: Vec3, v2: Vec3, v3: Vec3) -> Self {
        Self {
            normal,
            v1,
            v2,
            v3,
            color: None,
        }
    }

    /// Attaches an RGBA diffuse tint.
    pub fn with_color(mut self, color: [f32; 4]) -> Self {
        self.color = Some(color);
        self
    }
}

/// Supported 3D model formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum ModelFormat {
    /// StereoLithography (.stl)
    Stl,
    /// Wavefront (.obj)
    Obj,
    /// glTF 2.0 / GLB (.gltf, .glb)
    Gltf,
}

impl ModelFormat {
    /// Returns a short human-readable format badge.
    pub fn badge_label(&self) -> &'static str {
        match self {
            Self::Stl => "STL",
            Self::Obj => "OBJ",
            Self::Gltf => "GLTF",
        }
    }

    /// Identifies model format from file extension.
    pub fn from_extension(ext: &str) -> Option<Self> {
        let ext = ext.trim().to_ascii_lowercase();
        match ext.as_str() {
            "stl" => Some(Self::Stl),
            "obj" => Some(Self::Obj),
            "gltf" | "glb" => Some(Self::Gltf),
            _ => None,
        }
    }
}

/// Returns `true` if the path has a supported 3D model extension (.stl, .obj, .gltf, .glb).
pub fn is_3d_model_file<P: AsRef<Path>>(path: P) -> bool {
    let p = path.as_ref();
    if let Some(ext) = p.extension().and_then(|s| s.to_str()) {
        ModelFormat::from_extension(ext).is_some()
    } else {
        false
    }
}

/// Returns `true` if the path has an `.stl` extension.
pub fn is_stl_file<P: AsRef<Path>>(path: P) -> bool {
    let p = path.as_ref();
    if let Some(ext) = p.extension().and_then(|s| s.to_str()) {
        ext.eq_ignore_ascii_case("stl")
    } else {
        false
    }
}

/// Parsed 3D model with triangle facets, bounding box, and centering geometry.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Model3d {
    /// Model name.
    pub name: String,
    /// Underlying file format.
    pub format: ModelFormat,
    /// Triangle facets making up the 3D surface.
    pub triangles: Vec<Triangle>,
    /// Pre-downsampled triangles (~2,000 facets) for microsecond card preview rasterization.
    pub thumbnail_triangles: Vec<Triangle>,
    /// Minimum axis-aligned bounding box coordinates.
    pub min_bounds: Vec3,
    /// Maximum axis-aligned bounding box coordinates.
    pub max_bounds: Vec3,
    /// Geometric center point of the model.
    pub center: Vec3,
    /// Dimensional span along X, Y, and Z axes.
    pub size: Vec3,
    /// Maximum dimension among width, height, and depth.
    pub max_dimension: f32,
}

impl Model3d {
    /// Alias for backwards compatibility with STL loaders.
    pub fn parse_file<P: AsRef<Path>>(path: P) -> Result<Self, String> {
        Self::load_file(path)
    }

    /// Alias for backwards compatibility with STL memory loaders.
    pub fn parse_bytes(buffer: &[u8], default_name: String) -> Result<Self, String> {
        Self::load_bytes(buffer, "stl", default_name)
    }

    /// Loads a 3D model file from disk, auto-detecting format from extension.
    pub fn load_file<P: AsRef<Path>>(path: P) -> Result<Self, String> {
        let path = path.as_ref();
        let ext = path
            .extension()
            .and_then(|s| s.to_str())
            .ok_or_else(|| "File has no extension".to_string())?;
        let format = ModelFormat::from_extension(ext)
            .ok_or_else(|| format!("Unsupported 3D file format: .{}", ext))?;
        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("3D Model")
            .to_string();

        match format {
            ModelFormat::Stl => Self::load_stl_file(path, name),
            ModelFormat::Obj => Self::load_obj_file(path, name),
            ModelFormat::Gltf => Self::load_gltf_file(path, name),
        }
    }

    /// Loads a 3D model from disk, caching the parsed mesh in memory.
    ///
    /// The cached mesh is returned as an `Arc<Model3d>` if the file's modification time
    /// and length have not changed, eliminating file I/O and parsing overhead during
    /// continuous animation / turntable rendering.
    pub fn load_cached<P: AsRef<Path>>(path: P) -> Result<Arc<Self>, String> {
        let path = path.as_ref();
        let path_buf = path.to_path_buf();
        let identity = model_cache_identity(path);
        {
            let mut guard = MODEL_CACHE.lock().unwrap_or_else(|p| p.into_inner());
            let cache = guard.get_or_insert_with(ModelCache::default);
            if let Some(identity) = identity.as_ref() {
                if let Some(model) = cache.get(&path_buf, identity) {
                    return Ok(model);
                }
            } else {
                cache.remove(&path_buf);
            }
        }

        let model = Arc::new(Self::load_file(path)?);
        {
            let mut guard = MODEL_CACHE.lock().unwrap_or_else(|p| p.into_inner());
            let cache = guard.get_or_insert_with(ModelCache::default);
            if let Some(identity) = identity {
                cache.insert(
                    path_buf,
                    identity,
                    Arc::clone(&model),
                    MODEL_CACHE_MAX_BYTES,
                    MODEL_CACHE_MAX_ENTRIES,
                );
            } else {
                cache.remove(&path_buf);
            }
        }
        Ok(model)
    }

    /// Loads raw bytes for the given format extension.
    pub fn load_bytes(buffer: &[u8], ext: &str, default_name: String) -> Result<Self, String> {
        let format = ModelFormat::from_extension(ext)
            .ok_or_else(|| format!("Unsupported 3D file format: .{}", ext))?;
        match format {
            ModelFormat::Stl => {
                let geom = scenix_loader::stl::load_bytes(buffer)
                    .map_err(|e| format!("Failed to parse STL bytes: {:?}", e))?;
                let triangles = geometry_to_triangles(&geom, None);
                Self::build_mesh(default_name, ModelFormat::Stl, triangles)
            }
            ModelFormat::Obj | ModelFormat::Gltf => {
                let mut temp = tempfile::Builder::new()
                    .suffix(&format!(".{}", ext))
                    .tempfile()
                    .map_err(|e| format!("Failed to create temporary 3D file: {}", e))?;
                temp.write_all(buffer)
                    .map_err(|e| format!("Failed to write temporary 3D data: {}", e))?;
                Self::load_file(temp.path())
            }
        }
    }

    fn load_stl_file(path: &Path, name: String) -> Result<Self, String> {
        let geom = scenix_loader::stl::load(path)
            .map_err(|e| format!("Failed to parse STL file: {:?}", e))?;
        let triangles = geometry_to_triangles(&geom, None);
        Self::build_mesh(name, ModelFormat::Stl, triangles)
    }

    fn load_obj_file(path: &Path, name: String) -> Result<Self, String> {
        let obj_asset = scenix_loader::obj::load_with_materials(path)
            .map_err(|e| format!("Failed to parse OBJ file: {:?}", e))?;
        let mut all_triangles = Vec::new();
        for geom in &obj_asset.geometries {
            let tris = geometry_to_triangles(geom, None);
            all_triangles.extend(tris);
        }
        Self::build_mesh(name, ModelFormat::Obj, all_triangles)
    }

    fn load_gltf_file(path: &Path, name: String) -> Result<Self, String> {
        let loader = scenix_loader::GltfLoader::new();
        let gltf_asset = loader
            .load(path)
            .map_err(|e| format!("Failed to parse glTF file: {:?}", e))?;
        let mut all_triangles = Vec::new();
        for geom in gltf_asset.meshes.values() {
            let tris = geometry_to_triangles(geom, None);
            all_triangles.extend(tris);
        }
        Self::build_mesh(name, ModelFormat::Gltf, all_triangles)
    }

    /// Computes bounding box and geometric center for a set of triangles,
    /// subsampling thumbnail triangles according to the configured preview triangle budget.
    pub fn build_mesh(
        name: String,
        format: ModelFormat,
        triangles: Vec<Triangle>,
    ) -> Result<Self, String> {
        let budget = little_clip_core::config::AppConfig::load()
            .model3d
            .preview_triangle_limit
            .max(100);
        Self::build_mesh_with_budget(name, format, triangles, budget)
    }

    /// Computes bounding box and geometric center for a set of triangles with an explicit preview triangle budget.
    pub fn build_mesh_with_budget(
        name: String,
        format: ModelFormat,
        triangles: Vec<Triangle>,
        preview_budget: usize,
    ) -> Result<Self, String> {
        if triangles.is_empty() {
            return Err("3D model contains no triangles".to_string());
        }

        let mut min_x = f32::INFINITY;
        let mut min_y = f32::INFINITY;
        let mut min_z = f32::INFINITY;
        let mut max_x = f32::NEG_INFINITY;
        let mut max_y = f32::NEG_INFINITY;
        let mut max_z = f32::NEG_INFINITY;

        for t in &triangles {
            for v in &[t.v1, t.v2, t.v3] {
                if v.x < min_x {
                    min_x = v.x;
                }
                if v.x > max_x {
                    max_x = v.x;
                }
                if v.y < min_y {
                    min_y = v.y;
                }
                if v.y > max_y {
                    max_y = v.y;
                }
                if v.z < min_z {
                    min_z = v.z;
                }
                if v.z > max_z {
                    max_z = v.z;
                }
            }
        }

        let min_bounds = Vec3::new(min_x, min_y, min_z);
        let max_bounds = Vec3::new(max_x, max_y, max_z);
        let center = Vec3::new(
            (min_x + max_x) * 0.5,
            (min_y + max_y) * 0.5,
            (min_z + max_z) * 0.5,
        );
        let size = Vec3::new(max_x - min_x, max_y - min_y, max_z - min_z);
        let max_dimension = size.x.max(size.y).max(size.z).max(0.001);

        let step = triangles.len().div_ceil(preview_budget.max(100)).max(1);
        let thumbnail_triangles = if step > 1 {
            triangles.iter().step_by(step).copied().collect()
        } else {
            triangles.clone()
        };

        Ok(Self {
            name,
            format,
            triangles,
            thumbnail_triangles,
            min_bounds,
            max_bounds,
            center,
            size,
            max_dimension,
        })
    }
}

fn geometry_to_triangles(geom: &scenix_mesh::Geometry, color: Option<[f32; 4]>) -> Vec<Triangle> {
    let mut triangles = Vec::with_capacity(geom.indices.len() / 3);
    if !geom.indices.is_empty() {
        for chunk in geom.indices.chunks_exact(3) {
            let i1 = chunk[0] as usize;
            let i2 = chunk[1] as usize;
            let i3 = chunk[2] as usize;
            if i1 < geom.positions.len() && i2 < geom.positions.len() && i3 < geom.positions.len() {
                let v1 = Vec3::new(
                    geom.positions[i1].x,
                    geom.positions[i1].y,
                    geom.positions[i1].z,
                );
                let v2 = Vec3::new(
                    geom.positions[i2].x,
                    geom.positions[i2].y,
                    geom.positions[i2].z,
                );
                let v3 = Vec3::new(
                    geom.positions[i3].x,
                    geom.positions[i3].y,
                    geom.positions[i3].z,
                );

                let cross = v2.sub(v1).cross(v3.sub(v1));
                let mut normal = if cross.length() > 1e-6 {
                    cross.normalize()
                } else if i1 < geom.normals.len() {
                    Vec3::new(geom.normals[i1].x, geom.normals[i1].y, geom.normals[i1].z)
                        .normalize()
                } else {
                    Vec3::new(0.0, 0.0, 1.0)
                };

                if i1 < geom.normals.len() && i2 < geom.normals.len() && i3 < geom.normals.len() {
                    let n1 = Vec3::new(geom.normals[i1].x, geom.normals[i1].y, geom.normals[i1].z);
                    let n2 = Vec3::new(geom.normals[i2].x, geom.normals[i2].y, geom.normals[i2].z);
                    let n3 = Vec3::new(geom.normals[i3].x, geom.normals[i3].y, geom.normals[i3].z);
                    let avg_n = n1.add(n2).add(n3);
                    if avg_n.length() > 1e-4 && normal.dot(avg_n) < 0.0 {
                        normal = normal.scale(-1.0);
                    }
                }

                triangles.push(Triangle {
                    normal,
                    v1,
                    v2,
                    v3,
                    color,
                });
            }
        }
    } else {
        for chunk in geom.positions.chunks_exact(3) {
            let v1 = Vec3::new(chunk[0].x, chunk[0].y, chunk[0].z);
            let v2 = Vec3::new(chunk[1].x, chunk[1].y, chunk[1].z);
            let v3 = Vec3::new(chunk[2].x, chunk[2].y, chunk[2].z);
            let normal = v2.sub(v1).cross(v3.sub(v1)).normalize();
            triangles.push(Triangle {
                normal,
                v1,
                v2,
                v3,
                color,
            });
        }
    }
    triangles
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cache_identity(revision: u64) -> ModelCacheIdentity {
        ModelCacheIdentity {
            source: FileRevision {
                modified: SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(revision),
                len: revision,
            },
            dependencies: Vec::new(),
        }
    }

    fn cache_model(triangle_count: usize) -> Arc<Model3d> {
        let triangle = Triangle::new(Vec3::ZERO, Vec3::ZERO, Vec3::ZERO, Vec3::ZERO);
        Arc::new(Model3d {
            name: "cache test model".to_owned(),
            format: ModelFormat::Stl,
            triangles: vec![triangle; triangle_count],
            thumbnail_triangles: Vec::new(),
            min_bounds: Vec3::ZERO,
            max_bounds: Vec3::ZERO,
            center: Vec3::ZERO,
            size: Vec3::ZERO,
            max_dimension: 0.0,
        })
    }

    fn cache_has_path(cache: &ModelCache, path: &Path) -> bool {
        cache.entries.contains_key(path)
    }

    #[test]
    fn model_cache_byte_budget_evicts_entries_and_keeps_returned_arc_valid() {
        let mut cache = ModelCache::default();
        let first = cache_model(1);
        let second = cache_model(1);
        let entry_bytes = model_retained_bytes(&first);
        cache.insert(
            PathBuf::from("first.stl"),
            cache_identity(1),
            Arc::clone(&first),
            entry_bytes * 2 - 1,
            16,
        );
        cache.insert(
            PathBuf::from("second.stl"),
            cache_identity(2),
            Arc::clone(&second),
            entry_bytes * 2 - 1,
            16,
        );

        assert!(!cache_has_path(&cache, Path::new("first.stl")));
        assert!(cache_has_path(&cache, Path::new("second.stl")));
        assert_eq!(cache.retained_bytes, model_retained_bytes(&second));
        assert_eq!(first.triangles.len(), 1);
    }

    #[test]
    fn model_cache_evicts_least_recently_used_entry() {
        let mut cache = ModelCache::default();
        let a = PathBuf::from("a.stl");
        let b = PathBuf::from("b.stl");
        let c = PathBuf::from("c.stl");
        cache.insert(a.clone(), cache_identity(1), cache_model(1), usize::MAX, 2);
        cache.insert(b.clone(), cache_identity(2), cache_model(1), usize::MAX, 2);
        assert!(cache.get(&a, &cache_identity(1)).is_some());
        cache.insert(c.clone(), cache_identity(3), cache_model(1), usize::MAX, 2);

        assert!(cache_has_path(&cache, &a));
        assert!(!cache_has_path(&cache, &b));
        assert!(cache_has_path(&cache, &c));
    }

    #[test]
    fn model_cache_replaces_stale_revision_for_same_path() {
        let mut cache = ModelCache::default();
        let path = PathBuf::from("revisioned.stl");
        let old = cache_model(1);
        let new = cache_model(2);
        cache.insert(
            path.clone(),
            cache_identity(1),
            Arc::clone(&old),
            usize::MAX,
            16,
        );
        assert!(cache.get(&path, &cache_identity(2)).is_none());
        assert!(!cache_has_path(&cache, &path));

        cache.insert(
            path.clone(),
            cache_identity(2),
            Arc::clone(&new),
            usize::MAX,
            16,
        );
        let cached = cache.get(&path, &cache_identity(2)).unwrap();
        assert!(Arc::ptr_eq(&cached, &new));
        assert!(!Arc::ptr_eq(&cached, &old));
        assert_eq!(cache.entries.len(), 1);
    }

    #[test]
    fn model_cache_bypasses_entries_larger_than_budget() {
        let mut cache = ModelCache::default();
        let oversized = cache_model(10);
        let bytes = model_retained_bytes(&oversized);
        cache.insert(
            PathBuf::from("large.stl"),
            cache_identity(1),
            Arc::clone(&oversized),
            bytes - 1,
            16,
        );

        assert!(cache.entries.is_empty());
        assert_eq!(cache.retained_bytes, 0);
        assert_eq!(oversized.triangles.len(), 10);
    }

    #[test]
    fn test_ascii_stl_parsing() {
        let ascii_stl = r#"solid simple_triangle
  facet normal 0 0 1
    outer loop
      vertex 0 0 0
      vertex 10 0 0
      vertex 0 10 0
    endloop
  endfacet
endsolid simple_triangle
"#;
        let mesh = Model3d::parse_bytes(ascii_stl.as_bytes(), "simple".to_string()).unwrap();
        assert_eq!(mesh.triangles.len(), 1);
        assert_eq!(mesh.triangles[0].v2, Vec3::new(10.0, 0.0, 0.0));
        assert!((mesh.max_dimension - 10.0).abs() < 1e-4);
    }

    #[test]
    fn test_binary_stl_roundtrip() {
        let mut bytes = vec![0u8; 84 + 50];
        // 1 triangle
        bytes[80] = 1;
        // Normal 0, 0, 1
        let normal_z_bytes = 1.0f32.to_le_bytes();
        bytes[84 + 8..84 + 12].copy_from_slice(&normal_z_bytes);
        // v1 = 0, 0, 0
        // v2 = 20, 0, 0
        let v2_x = 20.0f32.to_le_bytes();
        bytes[84 + 24..84 + 28].copy_from_slice(&v2_x);
        // v3 = 0, 20, 0
        let v3_y = 20.0f32.to_le_bytes();
        bytes[84 + 40..84 + 44].copy_from_slice(&v3_y);

        let mesh = Model3d::parse_bytes(&bytes, "bin_model".to_string()).unwrap();
        assert_eq!(mesh.triangles.len(), 1);
        assert!((mesh.max_dimension - 20.0).abs() < 1e-4);
    }

    #[test]
    fn test_obj_parsing_from_bytes() {
        let obj_data = r#"
o Cube
v 0.0 0.0 0.0
v 1.0 0.0 0.0
v 0.0 1.0 0.0
vn 0.0 0.0 1.0
f 1//1 2//1 3//1
"#;
        let model =
            Model3d::load_bytes(obj_data.as_bytes(), "obj", "TestCube".to_string()).unwrap();
        assert_eq!(model.format, ModelFormat::Obj);
        assert_eq!(model.triangles.len(), 1);
        assert!((model.max_dimension - 1.0).abs() < 1e-4);
    }

    #[test]
    fn test_gltf_parsing_from_bytes() {
        let gltf_json = r#"{
  "asset": { "version": "2.0" },
  "scenes": [{ "nodes": [0] }],
  "nodes": [{ "mesh": 0 }],
  "meshes": [{
    "primitives": [{
      "attributes": { "POSITION": 0 }
    }]
  }],
  "buffers": [{
    "uri": "data:application/octet-stream;base64,AAAAAAAAAAAAAAAAAACAPwAAAAAAAAAAAAAAAAAAgD8AAAAA",
    "byteLength": 36
  }],
  "bufferViews": [{
    "buffer": 0,
    "byteOffset": 0,
    "byteLength": 36
  }],
  "accessors": [{
    "bufferView": 0,
    "byteOffset": 0,
    "componentType": 5126,
    "count": 3,
    "type": "VEC3",
    "max": [1.0, 1.0, 0.0],
    "min": [0.0, 0.0, 0.0]
  }]
}"#;
        let model =
            Model3d::load_bytes(gltf_json.as_bytes(), "gltf", "TestGltf".to_string()).unwrap();
        assert_eq!(model.format, ModelFormat::Gltf);
        assert_eq!(model.triangles.len(), 1);
        assert!((model.max_dimension - 1.0).abs() < 1e-4);
    }

    #[test]
    fn test_model3d_projection_and_geometry() {
        let v0 = Vec3::new(0.0, 0.0, 0.0);
        let v1 = Vec3::new(10.0, 0.0, 0.0);
        let v2 = Vec3::new(5.0, 10.0, 0.0);
        let v3 = Vec3::new(5.0, 5.0, 10.0);

        let triangles = vec![
            Triangle::new(Vec3::new(0.0, 0.0, -1.0), v0, v1, v2),
            Triangle::new(Vec3::new(0.0, -0.894, 0.447), v0, v1, v3),
            Triangle::new(Vec3::new(0.894, 0.447, 0.447), v1, v2, v3),
            Triangle::new(Vec3::new(-0.894, 0.447, 0.447), v2, v0, v3),
        ];

        let mesh =
            Model3d::build_mesh("Tetrahedron".to_string(), ModelFormat::Stl, triangles).unwrap();
        assert_eq!(mesh.triangles.len(), 4);
        assert_eq!(mesh.min_bounds, Vec3::new(0.0, 0.0, 0.0));
        assert_eq!(mesh.max_bounds, Vec3::new(10.0, 10.0, 10.0));
        assert_eq!(mesh.center, Vec3::new(5.0, 5.0, 5.0));
        assert_eq!(mesh.max_dimension, 10.0);
    }

    #[test]
    fn test_is_3d_model_file() {
        assert!(is_3d_model_file("part.stl"));
        assert!(is_3d_model_file("PART.STL"));
        assert!(is_3d_model_file("mesh.obj"));
        assert!(is_3d_model_file("model.gltf"));
        assert!(is_3d_model_file("asset.glb"));
        assert!(!is_3d_model_file("photo.png"));
        assert!(!is_3d_model_file("code.rs"));
    }

    #[test]
    fn test_load_cached_roundtrip() {
        let ascii_stl = r#"solid test_cache
  facet normal 0 0 1
    outer loop
      vertex 0 0 0
      vertex 1 0 0
      vertex 0 1 0
    endloop
  endfacet
endsolid test_cache
"#;
        let tmp = std::env::temp_dir().join("cached_test.stl");
        std::fs::write(&tmp, ascii_stl.as_bytes()).unwrap();

        let m1 = Model3d::load_cached(&tmp).unwrap();
        let m2 = Model3d::load_cached(&tmp).unwrap();
        assert_eq!(m1.name, "cached_test");
        assert!(std::sync::Arc::ptr_eq(&m1, &m2));

        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn cached_gltf_reloads_when_external_buffer_revision_changes() {
        let temp_dir = tempfile::tempdir().unwrap();
        let gltf_path = temp_dir.path().join("mesh.gltf");
        let buffer_path = temp_dir.path().join("mesh.bin");
        let gltf_json = r#"{
  "asset": { "version": "2.0" },
  "scenes": [{ "nodes": [0] }],
  "nodes": [{ "mesh": 0 }],
  "meshes": [{ "primitives": [{ "attributes": { "POSITION": 0 } }] }],
  "buffers": [{ "uri": "mesh.bin", "byteLength": 36 }],
  "bufferViews": [{ "buffer": 0, "byteOffset": 0, "byteLength": 36 }],
  "accessors": [{
    "bufferView": 0,
    "componentType": 5126,
    "count": 3,
    "type": "VEC3",
    "max": [1.0, 1.0, 0.0],
    "min": [0.0, 0.0, 0.0]
  }]
}"#;
        std::fs::write(&gltf_path, gltf_json).unwrap();

        let write_triangle_buffer = |second_vertex_x: f32| {
            let mut bytes = Vec::with_capacity(36);
            for coordinate in [0.0_f32, 0.0, 0.0, second_vertex_x, 0.0, 0.0, 0.0, 1.0, 0.0] {
                bytes.extend_from_slice(&coordinate.to_le_bytes());
            }
            std::fs::write(&buffer_path, bytes).unwrap();
        };
        write_triangle_buffer(1.0);

        let first = Model3d::load_cached(&gltf_path).unwrap();
        assert_eq!(first.triangles.len(), 1);
        assert_eq!(first.triangles[0].v2.x, 1.0);
        let cached = Model3d::load_cached(&gltf_path).unwrap();
        assert!(Arc::ptr_eq(&first, &cached));

        // Ensure the metadata revision changes even on filesystems with coarse
        // modification-time granularity; the buffer size stays constant.
        std::thread::sleep(std::time::Duration::from_millis(1100));
        write_triangle_buffer(2.0);

        let updated = Model3d::load_cached(&gltf_path).unwrap();
        assert!(!Arc::ptr_eq(&first, &updated));
        assert_eq!(updated.triangles[0].v2.x, 2.0);
    }

    #[test]
    fn cached_gltf_tracks_external_image_revision_and_bypasses_missing_images() {
        let temp_dir = tempfile::tempdir().unwrap();
        let gltf_path = temp_dir.path().join("mesh.gltf");
        let image_path = temp_dir.path().join("texture.png");
        let gltf_json = r#"{
  "asset": { "version": "2.0" },
  "scenes": [{ "nodes": [0] }],
  "nodes": [{ "mesh": 0 }],
  "meshes": [{ "primitives": [{ "attributes": { "POSITION": 0 } }] }],
  "buffers": [{ "uri": "mesh.bin", "byteLength": 36 }],
  "bufferViews": [{ "buffer": 0, "byteOffset": 0, "byteLength": 36 }],
  "accessors": [{
    "bufferView": 0,
    "componentType": 5126,
    "count": 3,
    "type": "VEC3",
    "max": [1.0, 1.0, 0.0],
    "min": [0.0, 0.0, 0.0]
  }],
  "images": [{ "uri": "texture.png" }]
}"#;
        std::fs::write(&gltf_path, gltf_json).unwrap();
        let mut buffer = Vec::with_capacity(36);
        for coordinate in [0.0_f32, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0] {
            buffer.extend_from_slice(&coordinate.to_le_bytes());
        }
        std::fs::write(temp_dir.path().join("mesh.bin"), buffer).unwrap();

        let write_pixel = |rgba: [u8; 4]| {
            let image = image::DynamicImage::ImageRgba8(
                image::RgbaImage::from_raw(1, 1, rgba.to_vec()).unwrap(),
            );
            let mut encoded = std::io::Cursor::new(Vec::new());
            image
                .write_to(&mut encoded, image::ImageFormat::Png)
                .unwrap();
            std::fs::write(&image_path, encoded.into_inner()).unwrap();
        };
        write_pixel([255, 0, 0, 255]);
        let initial_image_len = std::fs::metadata(&image_path).unwrap().len();

        let first = Model3d::load_cached(&gltf_path).unwrap();
        let cached = Model3d::load_cached(&gltf_path).unwrap();
        assert!(Arc::ptr_eq(&first, &cached));

        // Keep the image byte length constant while ensuring its metadata
        // revision changes on filesystems with coarse timestamp granularity.
        std::thread::sleep(std::time::Duration::from_millis(1100));
        write_pixel([0, 255, 0, 255]);
        assert_eq!(
            std::fs::metadata(&image_path).unwrap().len(),
            initial_image_len
        );

        let updated = Model3d::load_cached(&gltf_path).unwrap();
        assert!(!Arc::ptr_eq(&first, &updated));
        assert_eq!(updated.triangles, first.triangles);

        std::fs::remove_file(&image_path).unwrap();
        assert!(Model3d::load_cached(&gltf_path).is_err());
    }

    #[test]
    fn gltf_cache_bypasses_uri_forms_with_importer_specific_resolution() {
        let temp_dir = tempfile::tempdir().unwrap();
        let gltf_path = temp_dir.path().join("mesh.gltf");
        std::fs::write(
            &gltf_path,
            r#"{
  "asset": { "version": "2.0" },
  "buffers": [],
  "images": [{ "uri": "texture?.png" }]
}"#,
        )
        .unwrap();

        assert!(model_cache_identity(&gltf_path).is_none());
    }

    #[test]
    fn glb_cache_identity_is_disabled_until_container_dependencies_are_scanned() {
        let temp_dir = tempfile::tempdir().unwrap();
        let glb_path = temp_dir.path().join("mesh.glb");
        std::fs::write(&glb_path, b"glTF").unwrap();

        assert!(model_cache_identity(&glb_path).is_none());
    }

    #[test]
    fn test_build_mesh_with_budget() {
        let dummy_tri = Triangle::new(
            Vec3::new(0.0, 0.0, 1.0),
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
        );
        let triangles = vec![dummy_tri; 10_000];

        // Custom budget of 2,000 should downsample to ~2,000
        let mesh_low = Model3d::build_mesh_with_budget(
            "low".into(),
            ModelFormat::Stl,
            triangles.clone(),
            2_000,
        )
        .unwrap();
        assert_eq!(mesh_low.triangles.len(), 10_000);
        assert_eq!(mesh_low.thumbnail_triangles.len(), 2_000);

        // Custom budget of 20,000 should keep all 10,000
        let mesh_high =
            Model3d::build_mesh_with_budget("high".into(), ModelFormat::Stl, triangles, 20_000)
                .unwrap();
        assert_eq!(mesh_high.triangles.len(), 10_000);
        assert_eq!(mesh_high.thumbnail_triangles.len(), 10_000);
    }
}
