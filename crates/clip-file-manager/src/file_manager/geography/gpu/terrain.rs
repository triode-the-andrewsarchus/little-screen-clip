//! Depth-tested terrain patches and their shared projection uniforms.
use super::{Frame, TextureTile};
use eframe::glow::{self, HasContext};
use little_clip_core::geography::basemap::{perspective::Perspective, tiles::TileId};
use std::collections::{BTreeSet, HashMap};

pub(super) struct Patch {
    pub source: TileId,
    pub region: TileId,
}

pub(super) fn patches(frame: &Frame, textures: &HashMap<(u64, TileId), TextureTile>) -> Vec<Patch> {
    let visible: BTreeSet<_> = frame
        .tiles
        .iter()
        .map(|tile| tile.id)
        .filter(|id| textures.contains_key(&(frame.source, *id)))
        .collect();
    let mut divisions = BTreeSet::new();
    for &tile in &visible {
        let mut parent = tile.parent();
        while let Some(id) = parent {
            if !divisions.insert(id) {
                break;
            }
            parent = id.parent();
        }
    }
    let mut output = Vec::new();
    for x in 0..2 {
        cover(
            TileId { level: 0, x, y: 0 },
            None,
            &visible,
            &divisions,
            &mut output,
        );
    }
    output
}

fn cover(
    region: TileId,
    fallback: Option<TileId>,
    loaded: &BTreeSet<TileId>,
    divisions: &BTreeSet<TileId>,
    result: &mut Vec<Patch>,
) {
    let source = loaded.get(&region).copied().or(fallback);
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
        result.push(Patch { source, region });
    }
}

impl Patch {
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
        frame: &Frame,
        enabled: bool,
    ) -> Result<bool, String> {
        use little_clip_core::geography::basemap::tiles::PADDED_EDGE;
        // SAFETY: the renderer owns the active program, context and atlas.
        unsafe {
            let loc = |name| gl.get_uniform_location(program, name);
            gl.uniform_1_i32(loc("u_terrain_dem").as_ref(), 6);
            gl.uniform_1_i32(loc("u_terrain_pages").as_ref(), 7);
            if !enabled {
                return Ok(false);
            }
            let dimension = PADDED_EDGE as i32;
            let [dem, table] = self.initialize(gl, dimension)?;
            if self.source != frame.source {
                self.pages.clear();
                self.source = frame.source;
            }
            let visible: BTreeSet<_> = frame
                .tiles
                .iter()
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
            for tile in frame.tiles.iter().filter(|tile| visible.contains(&tile.id)) {
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
            let mut entries = [0u32; TABLE_SIZE * 2];
            for (&tile, &page) in &self.pages {
                let key = ((u32::from(tile.level) << 24) | (tile.y << 12) | tile.x) + 1;
                let mut slot = (key.wrapping_mul(2_654_435_761) >> 23) as usize;
                while entries[slot * 2] != 0 {
                    slot = (slot + 1) % TABLE_SIZE;
                }
                entries[slot * 2] = key;
                entries[slot * 2 + 1] = page as u32;
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
                glow::RG_INTEGER,
                glow::UNSIGNED_INT,
                glow::PixelUnpackData::Slice(Some(bytes)),
            );
            gl.uniform_1_i32(
                loc("u_dem_level").as_ref(),
                self.pages
                    .keys()
                    .map(|tile| i32::from(tile.level))
                    .max()
                    .unwrap_or(-1),
            );
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
                glow::RG32UI as i32,
                TABLE_SIZE as i32,
                1,
                0,
                glow::RG_INTEGER,
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
