//! Depth-tested terrain patches and their shared projection uniforms.
use eframe::glow::{self, HasContext};
use little_clip_core::geography::basemap::{
    perspective::{EARTH_RADIUS_M, Perspective, coordinates, sphere_distance, unit},
    tiles::{SourceTile, TileId},
};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    sync::Arc,
};

#[derive(Clone, Copy)]
pub(super) struct Patch {
    pub source: TileId,
    pub region: TileId,
}

pub(in super::super) struct Surface {
    pub(super) sources: BTreeMap<TileId, Arc<SourceTile>>,
    pub(super) patches: BTreeMap<TileId, Patch>,
    pub(super) zoom: f64,
    pub(in super::super) mesh_step_deg: f64,
    max_radius: f64,
    max_level: u8,
}

impl Surface {
    pub(in super::super) fn new(tiles: impl Iterator<Item = Arc<SourceTile>>, zoom: f64) -> Self {
        let sources: BTreeMap<_, _> = tiles.map(|tile| (tile.id, tile)).collect();
        let mut divisions = BTreeSet::new();
        for &tile in sources.keys() {
            let mut parent = tile.parent();
            while let Some(id) = parent {
                if !divisions.insert(id) {
                    break;
                }
                parent = id.parent();
            }
        }
        let mut patches = BTreeMap::new();
        for x in 0..2 {
            cover(
                TileId { level: 0, x, y: 0 },
                None,
                &sources,
                &divisions,
                &mut patches,
            );
        }
        let mesh_step_deg = patches
            .values()
            .map(|p| p.mesh_step(zoom))
            .fold(180.0, f64::min);
        let max_radius = 1.0
            + f64::from(
                sources
                    .values()
                    .filter_map(|tile| tile.elevation_range.map(|range| range[1]))
                    .fold(0.0, f32::max),
            ) / EARTH_RADIUS_M;
        let max_level = patches.keys().map(|p| p.level).max().unwrap_or(0);
        Self {
            sources,
            patches,
            zoom,
            mesh_step_deg,
            max_radius,
            max_level,
        }
    }

    fn sample(&self, lat: f64, lon: f64) -> Option<([f64; 3], f64)> {
        let mut id = TileId::at(self.max_level, lat, lon);
        let patch = loop {
            if let Some(patch) = self.patches.get(&id) {
                break patch;
            }
            id = id.parent()?;
        };
        let tile = &self.sources[&patch.source];
        let [west, north, _] = patch.region.bounds();
        let steps = patch.steps(self.zoom) as f64;
        let step = patch.mesh_step(self.zoom);
        let grid = [(lon - west).rem_euclid(360.0) / step, (north - lat) / step]
            .map(|v| v.clamp(0.0, steps));
        let [x, y] = grid.map(|v| v.floor().min(steps - 1.0));
        let (u, v) = (grid[0] - x, grid[1] - y);
        let vertices = if u + v <= 1.0 {
            [(x, y, 1.0 - u - v), (x + 1.0, y, u), (x, y + 1.0, v)]
        } else {
            [
                (x, y + 1.0, 1.0 - u),
                (x + 1.0, y, 1.0 - v),
                (x + 1.0, y + 1.0, u + v - 1.0),
            ]
        };
        let mut point = [0.0; 3];
        let [source_west, source_north, source_side] = tile.id.bounds();
        for (x, y, weight) in vertices {
            let (lat, lon) = (north - step * y, west + step * x);
            let uv = [
                (lon - source_west) / source_side,
                (source_north - lat) / source_side,
            ];
            let meters = tile.height_uv(uv).unwrap_or(0.0).max(0.0);
            let normal = unit(lat, lon);
            for axis in 0..3 {
                point[axis] += normal[axis] * (1.0 + meters / EARTH_RADIUS_M) * weight;
            }
        }
        Some((point, step))
    }

    pub(in super::super) fn project(
        &self,
        camera: Perspective,
        lat: f64,
        lon: f64,
    ) -> Option<([f32; 2], [f64; 3])> {
        let (point, _) = self.sample(lat, lon)?;
        Some((camera.project_position(point)?, point))
    }

    pub(in super::super) fn occluded(&self, eye: [f64; 3], target: [f64; 3]) -> bool {
        let delta: [f64; 3] = std::array::from_fn(|i| target[i] - eye[i]);
        let distance = length(delta);
        let ray = delta.map(|v| v / distance);
        let mut traveled = if length(eye) <= self.max_radius {
            0.0
        } else if let Some(entry) = sphere_distance(eye, ray, self.max_radius) {
            entry
        } else {
            return false;
        };
        let clearance = 1.5 / EARTH_RADIUS_M;
        let minimum_step = (distance - traveled) / 768.0;
        while traveled < distance - clearance {
            let position = std::array::from_fn(|axis| eye[axis] + ray[axis] * traveled);
            let (lat, lon) = coordinates(position);
            let step = if let Some((point, step)) = self.sample(lat, lon) {
                if length(position) + clearance < length(point) {
                    return true;
                }
                step
            } else {
                self.mesh_step_deg
            };
            // Sample within the local mesh cells, with a cap on work along
            // very shallow rays. Longitude cells narrow toward the poles.
            let step = step.to_radians() * 0.4 * lat.to_radians().cos().max(0.025);
            traveled += step.max(minimum_step);
        }
        false
    }
}

fn length(v: [f64; 3]) -> f64 {
    v.iter().map(|x| x * x).sum::<f64>().sqrt()
}

fn cover(
    region: TileId,
    fallback: Option<TileId>,
    loaded: &BTreeMap<TileId, Arc<SourceTile>>,
    divisions: &BTreeSet<TileId>,
    result: &mut BTreeMap<TileId, Patch>,
) {
    let source = loaded.contains_key(&region).then_some(region).or(fallback);
    if divisions.contains(&region) {
        for y in 0..2 {
            for x in 0..2 {
                cover(
                    TileId {
                        level: region.level + 1,
                        x: region.x * 2 + x,
                        y: region.y * 2 + y,
                    },
                    source,
                    loaded,
                    divisions,
                    result,
                );
            }
        }
    } else if let Some(source) = source {
        result.insert(region, Patch { source, region });
    }
}

impl Patch {
    pub fn steps(self, zoom: f64) -> u32 {
        if zoom >= 64.0 && self.source.level >= 8 {
            128
        } else {
            32
        }
    }

    fn mesh_step(self, zoom: f64) -> f64 {
        self.region.bounds()[2] / f64::from(self.steps(zoom))
    }

    pub fn uv(&self) -> [f32; 3] {
        let scale = 1u32 << (self.region.level - self.source.level);
        let span = 1.0 / scale as f32;
        [
            (self.region.x % scale) as f32 * span,
            (self.region.y % scale) as f32 * span,
            span,
        ]
    }
}

pub(super) unsafe fn uniforms(
    gl: &glow::Context,
    program: glow::Program,
    pose: Option<Perspective>,
) {
    // SAFETY: the renderer owns this active program and current context.
    unsafe {
        let loc = |name| gl.get_uniform_location(program, name);
        gl.uniform_1_i32(loc("u_perspective").as_ref(), i32::from(pose.is_some()));
        if let Some(camera) = pose {
            gl.uniform_3_f32_slice(loc("u_eye").as_ref(), &camera.eye.map(|v| v as f32));
            let basis = camera.axes.map(|axis| axis.map(|v| v as f32));
            gl.uniform_matrix_3_f32_slice(loc("u_view_axes").as_ref(), false, basis.as_flattened());
            gl.uniform_2_f32_slice(loc("u_half_fov").as_ref(), &camera.half_fov());
            let near = (0.015 * camera.altitude()).clamp(1e-7, 0.03);
            gl.uniform_1_f32(loc("u_near").as_ref(), near as f32);
        }
    }
}

pub(super) fn grid(steps: usize, skirts: bool, vertices: &mut Vec<f32>) -> std::ops::Range<i32> {
    let start = vertices.len() as i32 / 3;
    let mut triangle_pair = |point: &dyn Fn(usize, usize) -> [f32; 3]| {
        for (x, y) in [(0, 0), (1, 0), (0, 1), (0, 1), (1, 0), (1, 1)] {
            vertices.extend(point(x, y));
        }
    };
    let uv = |coordinate| coordinate as f32 / steps as f32;
    for y in 0..steps {
        for x in 0..steps {
            triangle_pair(&|dx, dy| [uv(x + dx), uv(y + dy), 0.0]);
        }
    }
    if skirts {
        for i in 0..steps {
            for side in [0.0, 1.0] {
                triangle_pair(&|x, y| [uv(i + x), side, y as f32]);
                triangle_pair(&|x, y| [side, uv(i + x), y as f32]);
            }
        }
    }
    start..vertices.len() as i32 / 3
}

const PAGE_COUNT: usize = 96;
const TABLE_SIZE: usize = 512;

/// A bounded copy of visible DEMs, shared by vector vertices across tile boundaries.
#[derive(Default)]
pub(super) struct HeightAtlas {
    textures: Option<[glow::Texture; 2]>,
    source: u64,
    pages: HashMap<TileId, usize>,
}

impl HeightAtlas {
    pub unsafe fn bind(
        &mut self,
        gl: &glow::Context,
        program: glow::Program,
        source: u64,
        surface: &Surface,
    ) -> Result<bool, String> {
        use little_clip_core::geography::basemap::tiles::PADDED_EDGE;
        // SAFETY: the renderer owns the active program, context and atlas.
        unsafe {
            let loc = |name| gl.get_uniform_location(program, name);
            gl.uniform_1_i32(loc("u_terrain_dem").as_ref(), 6);
            gl.uniform_1_i32(loc("u_terrain_pages").as_ref(), 7);
            let dimension = PADDED_EDGE as i32;
            let [dem, table] = self.initialize(gl, dimension)?;
            if self.source != source {
                self.pages.clear();
                self.source = source;
            }
            let visible: BTreeSet<_> = surface
                .sources
                .values()
                .filter(|tile| !tile.elevation.is_empty())
                .map(|tile| tile.id)
                .collect();
            self.pages.retain(|id, _| visible.contains(id));
            let mut used = [false; PAGE_COUNT];
            for page in self.pages.values() {
                used[*page] = true;
            }
            gl.active_texture(glow::TEXTURE6);
            gl.bind_texture(glow::TEXTURE_2D_ARRAY, Some(dem));
            let mut updates = 0;
            for tile in surface
                .sources
                .values()
                .filter(|tile| visible.contains(&tile.id))
            {
                if updates == 8 {
                    break;
                }
                if self.pages.contains_key(&tile.id) {
                    continue;
                }
                let Some(page) = used.iter().position(|&taken| !taken) else {
                    break;
                };
                let bytes = super::float_bytes(&tile.elevation);
                gl.tex_sub_image_3d(
                    glow::TEXTURE_2D_ARRAY,
                    0,
                    0,
                    0,
                    page as i32,
                    dimension,
                    dimension,
                    1,
                    glow::RG,
                    glow::FLOAT,
                    glow::PixelUnpackData::Slice(Some(bytes)),
                );
                used[page] = true;
                self.pages.insert(tile.id, page);
                updates += 1;
            }
            let mut entries = [[0u32; 4]; TABLE_SIZE];
            for patch in surface.patches.values() {
                let tile = patch.region;
                let key = ((u32::from(tile.level) << 24) | (tile.y << 12) | tile.x) + 1;
                let mut slot = (key.wrapping_mul(2_654_435_761) >> 23) as usize;
                while entries[slot][0] != 0 {
                    slot = (slot + 1) % TABLE_SIZE;
                }
                entries[slot] = [
                    key,
                    self.pages
                        .get(&patch.source)
                        .map_or(0, |page| *page as u32 + 1),
                    u32::from(patch.source.level),
                    patch.steps(surface.zoom),
                ];
            }
            let bytes = std::slice::from_raw_parts(
                entries.as_ptr().cast(),
                std::mem::size_of_val(&entries),
            );
            gl.active_texture(glow::TEXTURE7);
            gl.bind_texture(glow::TEXTURE_2D, Some(table));
            gl.tex_sub_image_2d(
                glow::TEXTURE_2D,
                0,
                0,
                0,
                TABLE_SIZE as i32,
                1,
                glow::RGBA_INTEGER,
                glow::UNSIGNED_INT,
                glow::PixelUnpackData::Slice(Some(bytes)),
            );
            gl.uniform_1_i32(loc("u_dem_level").as_ref(), i32::from(surface.max_level));
            gl.uniform_1_f32(loc("u_drape_step").as_ref(), surface.mesh_step_deg as f32);
            gl.active_texture(glow::TEXTURE0);
            Ok(self.pages.len() < visible.len())
        }
    }

    unsafe fn initialize(
        &mut self,
        gl: &glow::Context,
        dimension: i32,
    ) -> Result<[glow::Texture; 2], String> {
        if let Some(textures) = self.textures {
            return Ok(textures);
        }
        // SAFETY: storage sizes match the atlas updates and the owning context is current.
        unsafe {
            let dem = gl.create_texture()?;
            let table = gl
                .create_texture()
                .inspect_err(|_| gl.delete_texture(dem))?;
            gl.active_texture(glow::TEXTURE6);
            gl.bind_texture(glow::TEXTURE_2D_ARRAY, Some(dem));
            gl.tex_image_3d(
                glow::TEXTURE_2D_ARRAY,
                0,
                glow::RG32F as i32,
                dimension,
                dimension,
                PAGE_COUNT as i32,
                0,
                glow::RG,
                glow::FLOAT,
                glow::PixelUnpackData::Slice(None),
            );
            gl.active_texture(glow::TEXTURE7);
            gl.bind_texture(glow::TEXTURE_2D, Some(table));
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::RGBA32UI as i32,
                TABLE_SIZE as i32,
                1,
                0,
                glow::RGBA_INTEGER,
                glow::UNSIGNED_INT,
                glow::PixelUnpackData::Slice(None),
            );
            for (target, filter) in [
                (glow::TEXTURE_2D_ARRAY, glow::LINEAR),
                (glow::TEXTURE_2D, glow::NEAREST),
            ] {
                gl.active_texture(if target == glow::TEXTURE_2D_ARRAY {
                    glow::TEXTURE6
                } else {
                    glow::TEXTURE7
                });
                for parameter in [glow::TEXTURE_MIN_FILTER, glow::TEXTURE_MAG_FILTER] {
                    gl.tex_parameter_i32(target, parameter, filter as i32);
                }
                for parameter in [glow::TEXTURE_WRAP_S, glow::TEXTURE_WRAP_T] {
                    gl.tex_parameter_i32(target, parameter, glow::CLAMP_TO_EDGE as i32);
                }
            }
            let error = gl.get_error();
            if error != glow::NO_ERROR {
                gl.delete_texture(dem);
                gl.delete_texture(table);
                return Err(format!(
                    "Terrain height atlas allocation failed ({error:#x})"
                ));
            }
            self.textures = Some([dem, table]);
            Ok([dem, table])
        }
    }

    pub unsafe fn destroy(&mut self, gl: &glow::Context) {
        if let Some(textures) = self.textures.take() {
            // SAFETY: the renderer owns the textures and their current context.
            unsafe {
                for texture in textures {
                    gl.delete_texture(texture);
                }
            }
        }
    }
}
