//! GPU map resources owned by the desktop's current OpenGL context.
//! Workers deliver source tiles; callbacks upload each tile once and draw it with
//! camera uniforms. No framebuffer readback or per-camera CPU raster is involved.
mod sky;
mod terrain;
use super::geometry::{Batch, Layer, Layers, RiverKey, Scene};
use eframe::{
    egui_glow,
    glow::{self, HasContext},
};
use little_clip_core::geography::basemap::{
    BasemapStyle, GlobeCamera,
    perspective::{EARTH_RADIUS_M, MOON_RADIUS_M, Perspective},
    tiles::{PADDED_EDGE, SourceTile, TileId},
};
pub(super) use sky::{SkyObject, sky_object_at};
use std::{
    borrow::Cow,
    collections::{BTreeSet, HashMap},
    marker::PhantomData,
    rc::Rc,
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicBool, Ordering},
    },
};
pub(super) use terrain::Surface;

const GPU_BUDGET: usize = 128 * 1024 * 1024;
fn runtime_key() -> egui::Id {
    egui::Id::unique("geography_gpu_runtime")
}

#[derive(Clone)]
struct Handle {
    // Identity only, never dereferenced; the non-Send owner retains the Weak context.
    context_id: usize,
    alive: Arc<AtomicBool>,
    renderer: Arc<Mutex<Renderer>>,
}

pub(in crate::file_manager) struct Runtime {
    ctx: egui::Context,
    handle: Handle,
    context: Weak<glow::Context>,
    _thread: PhantomData<Rc<()>>,
}
impl Runtime {
    pub(in crate::file_manager) fn install(ctx: &egui::Context, gl: &Arc<glow::Context>) -> Self {
        let handle = Handle {
            context_id: Arc::as_ptr(gl) as usize,
            alive: Arc::new(AtomicBool::new(true)),
            renderer: Arc::new(Mutex::new(Renderer::default())),
        };
        ctx.data_mut(|d| d.insert_temp(runtime_key(), handle.clone()));
        Self {
            ctx: ctx.clone(),
            handle,
            context: Arc::downgrade(gl),
            _thread: PhantomData,
        }
    }
    /// The host must retain its owning current context through this call.
    pub(in crate::file_manager) unsafe fn destroy(&mut self, gl: &glow::Context) {
        if self
            .context
            .upgrade()
            .is_some_and(|owner| std::ptr::eq(owner.as_ref(), gl))
        {
            if let Ok(mut renderer) = self.handle.renderer.lock() {
                // SAFETY: the host guarantees the matching context is current.
                unsafe { renderer.destroy(gl) };
            }
            self.handle.alive.store(false, Ordering::Release);
            self.ctx.data_mut(|d| d.remove::<Handle>(runtime_key()));
        }
    }
}
impl Drop for Runtime {
    fn drop(&mut self) {
        self.handle.alive.store(false, Ordering::Release);
        self.ctx.data_mut(|d| d.remove::<Handle>(runtime_key()));
    }
}

#[derive(Clone)]
pub(super) struct Frame {
    pub source: u64,
    pub size: [f32; 2],
    pub camera: GlobeCamera,
    pub style: BasemapStyle,
    pub contours: Option<f32>,
    pub tiles: Vec<Arc<SourceTile>>,
    pub terrain: Option<Arc<Surface>>,
    pub scene: Option<Arc<Scene>>,
    pub rivers: Vec<(RiverKey, Arc<Scene>)>,
    pub layers: Layers,
    pub details: Vec<(TileId, Arc<super::detail::Tile>)>,
    pub sky: Option<super::sky::Scene>,
    pub star_trail: f32,
    pub show_sky: bool,
    pub moon_view: bool,
    pub lunar: Option<(u64, Vec<Arc<SourceTile>>)>,
    pub readable_sky: bool,
    pub show_constellations: bool,
    pub earth_lighting: bool,
}

impl Frame {
    fn surface_source(&self) -> (u64, &[Arc<SourceTile>]) {
        self.lunar
            .as_ref()
            .map_or((self.source, &self.tiles), |(source, tiles)| {
                (*source, tiles.as_slice())
            })
    }

    fn raster_sources(&self) -> impl Iterator<Item = (u64, &Arc<SourceTile>)> {
        self.tiles.iter().map(|tile| (self.source, tile)).chain(
            self.lunar
                .iter()
                .flat_map(|(source, tiles)| tiles.iter().map(|tile| (*source, tile))),
        )
    }
}

pub(super) fn prepare_surface(ctx: &egui::Context, frame: &mut Frame) {
    if frame.camera.orbit.active() || frame.moon_view {
        let runtime = ctx.data(|data| data.get_temp::<Handle>(runtime_key()));
        let renderer = runtime
            .as_ref()
            .and_then(|runtime| runtime.renderer.lock().ok());
        let (source, tiles) = frame.surface_source();
        let resident = tiles.iter().filter(|tile| {
            renderer
                .as_ref()
                .is_some_and(|renderer| renderer.tiles.contains_key(&(source, tile.id)))
        });
        if frame.moon_view
            && (frame.lunar.is_none()
                || resident.clone().filter(|tile| tile.id.level == 0).count() != 2)
        {
            return;
        }
        // Keep one snapshot for GPU geometry and immediate UI hit tests. New
        // uploads become visible together on the following repaint.
        let surface = if frame.moon_view {
            Surface::for_radius(resident.cloned(), frame.camera.zoom, MOON_RADIUS_M)
        } else {
            Surface::new(resident.cloned(), frame.camera.zoom)
        };
        frame.terrain = Some(Arc::new(surface));
    }
}

pub(super) fn paint(ui: &egui::Ui, rect: egui::Rect, frame: Frame) -> Result<(), String> {
    paint_at(ui, rect, frame, None)
}

pub(super) fn paint_at(
    ui: &egui::Ui,
    rect: egui::Rect,
    frame: Frame,
    slot: Option<egui::layers::ShapeIdx>,
) -> Result<(), String> {
    let handle = ui
        .ctx()
        .data(|d| d.get_temp::<Handle>(runtime_key()))
        .ok_or("OpenGL maps are unavailable in this host")?;
    if let Some(error) = handle
        .renderer
        .lock()
        .map_err(|_| "Map renderer failed")?
        .error
        .clone()
    {
        return Err(error);
    }
    let ctx = ui.ctx().clone();
    let callback = egui::PaintCallback {
        rect,
        callback: Arc::new(egui_glow::CallbackFn::new(move |info, painter| {
            if !handle.alive.load(Ordering::Acquire)
                || handle.context_id != Arc::as_ptr(painter.gl()) as usize
            {
                return;
            }
            if let Ok(mut renderer) = handle.renderer.lock() {
                // SAFETY: egui invokes callbacks with the verified owning context
                // current. Resource deletion runs only in this callback or on_exit.
                let result = unsafe { renderer.paint(painter.gl(), &info, &frame) };
                match result {
                    Ok(true) => ctx.request_repaint(),
                    Ok(false) => {}
                    Err(error) => {
                        renderer.error = Some(error);
                        ctx.request_repaint();
                    }
                }
            }
        })),
    };
    let painter = ui.painter_at(rect);
    if let Some(slot) = slot {
        painter.set(slot, callback);
    } else {
        painter.add(callback);
    }
    Ok(())
}

#[derive(Default)]
struct Renderer {
    resources: Option<Resources>,
    sky: sky::SkyRenderer,
    heights: terrain::HeightAtlas,
    tiles: HashMap<(u64, TileId), TextureTile>,
    bytes: usize,
    frame: u64,
    vectors: HashMap<(u64, VectorKey), VectorBatch>,
    vector_bytes: usize,
    error: Option<String>,
}
struct Resources {
    program: glow::Program,
    vector_program: glow::Program,
    terrain_vector_program: glow::Program,
    vao: glow::VertexArray,
    grid: glow::Buffer,
    vertices: i32,
    relief_grid: [std::ops::Range<i32>; 2],
}
struct TextureTile {
    textures: [glow::Texture; 3],
    bytes: usize,
    used: u64,
}

impl Renderer {
    unsafe fn paint(
        &mut self,
        gl: &glow::Context,
        info: &egui::PaintCallbackInfo,
        frame: &Frame,
    ) -> Result<bool, String> {
        // SAFETY: all calls use the owning current GL context; buffers contain
        // initialized floats and draw counts are bounded by the static grid.
        let missing_vectors;
        unsafe {
            if self.resources.is_none() {
                self.resources = Some(Resources::new(gl)?);
                tracing::info!(
                    renderer = gl.get_parameter_string(glow::RENDERER),
                    "GPU map renderer initialized"
                );
            }
            self.frame = self.frame.wrapping_add(1);
            self.upload_tiles(gl, frame)?;
            let resources = self.resources.as_ref().ok_or("Map resources unavailable")?;
            let viewport = info.viewport_in_pixels();
            gl.viewport(
                viewport.left_px,
                viewport.from_bottom_px,
                viewport.width_px,
                viewport.height_px,
            );
            gl.disable(glow::DEPTH_TEST);
            gl.disable(glow::CULL_FACE);
            gl.disable(glow::STENCIL_TEST);
            gl.enable(glow::BLEND);
            gl.blend_func(glow::ONE, glow::ONE_MINUS_SRC_ALPHA);
            gl.bind_vertex_array(Some(resources.vao));
            let earth = [0, 1].map(|x| {
                if !frame.moon_view {
                    return None;
                }
                let id = TileId { level: 0, x, y: 0 };
                self.tiles
                    .get(&(frame.source, id))
                    .map(|tile| tile.textures[0])
            });
            self.sky.paint_frame(gl, frame, earth);
            if frame.moon_view && frame.terrain.is_none() {
                gl.bind_vertex_array(None);
                gl.active_texture(glow::TEXTURE0);
                return Ok(self.needs_raster_upload(frame)
                    || frame.lunar.as_ref().is_some_and(|(_, tiles)| {
                        tiles.iter().filter(|tile| tile.id.level == 0).count() == 2
                    }));
            }
            gl.bind_vertex_array(Some(resources.vao));
            gl.use_program(Some(resources.program));
            let p = resources.program;
            if frame.moon_view {
                self.sky.lunar_uniforms(gl, p, frame);
            } else {
                self.sky.earth_uniforms(gl, p, frame);
            }
            let camera = frame.camera;
            let width = info.viewport.width();
            let height = info.viewport.height();
            let radius = if frame.moon_view {
                MOON_RADIUS_M
            } else {
                EARTH_RADIUS_M
            };
            let perspective = (camera.orbit.active() || frame.moon_view)
                .then(|| Perspective::new(&camera, [width, height], radius));
            terrain::uniforms(gl, p, perspective);
            for uniform in ["u_signed_elevation", "u_lunar_relief"] {
                gl.uniform_1_i32(
                    gl.get_uniform_location(p, uniform).as_ref(),
                    i32::from(frame.moon_view),
                );
            }
            gl.uniform_4_f32(
                gl.get_uniform_location(p, "u_camera").as_ref(),
                camera.center_lat as f32,
                camera.center_lon as f32,
                camera.scale_px(width, height) as f32,
                camera.globe_blend() as f32,
            );
            gl.uniform_2_f32(gl.get_uniform_location(p, "u_view").as_ref(), width, height);
            gl.uniform_1_f32(
                gl.get_uniform_location(p, "u_blend").as_ref(),
                camera.globe_blend() as f32,
            );
            gl.uniform_1_f32(
                gl.get_uniform_location(p, "u_zoom").as_ref(),
                camera.zoom as f32,
            );
            gl.uniform_1_i32(
                gl.get_uniform_location(p, "u_style").as_ref(),
                match frame.style {
                    BasemapStyle::Natural => 0,
                    BasemapStyle::Hypsometric => 1,
                    BasemapStyle::Oilslick => 2,
                    BasemapStyle::Hillshade => 3,
                },
            );
            gl.uniform_1_i32(
                gl.get_uniform_location(p, "u_show_water").as_ref(),
                i32::from(frame.layers.rivers),
            );
            gl.uniform_1_f32(
                gl.get_uniform_location(p, "u_contour").as_ref(),
                frame.contours.unwrap_or(0.0),
            );
            for (unit, name) in ["u_color", "u_height", "u_water"].into_iter().enumerate() {
                gl.uniform_1_i32(gl.get_uniform_location(p, name).as_ref(), unit as i32);
            }
            self.paint_surface(gl, resources, frame, [width, height], perspective);
            missing_vectors =
                !frame.moon_view && self.paint_vectors(gl, frame, width, height, perspective)?;
            gl.disable(glow::DEPTH_TEST);
            gl.bind_vertex_array(None);
            gl.active_texture(glow::TEXTURE0);
        }
        let missing_tiles = self.needs_raster_upload(frame);
        let terrain_pending = frame
            .terrain
            .as_ref()
            .is_some_and(|surface| surface.sources.len() < frame.surface_source().1.len());
        Ok(missing_tiles || missing_vectors || terrain_pending)
    }

    fn needs_raster_upload(&self, frame: &Frame) -> bool {
        frame
            .raster_sources()
            .any(|(source, tile)| !self.tiles.contains_key(&(source, tile.id)))
    }
    unsafe fn upload_tiles(&mut self, gl: &glow::Context, frame: &Frame) -> Result<(), String> {
        // Mark current terrain textures before evicting anything. The overlays
        // and surface draw share the UI's snapshot of resident tiles.
        // SAFETY: all textures belong to the callback's current GL context.
        unsafe {
            for (source, tile) in frame.raster_sources() {
                if let Some(cached) = self.tiles.get_mut(&(source, tile.id)) {
                    cached.used = self.frame;
                }
            }
            let mut uploads = 0;
            for (source, tile) in frame.raster_sources() {
                let key = (source, tile.id);
                if !self.tiles.contains_key(&key) && uploads < 4 {
                    self.evict(gl, tile.bytes());
                    let cached = TextureTile::upload(gl, tile, self.frame)?;
                    self.bytes += cached.bytes;
                    self.tiles.insert(key, cached);
                    uploads += 1;
                }
            }
        }
        Ok(())
    }

    unsafe fn paint_surface(
        &self,
        gl: &glow::Context,
        resources: &Resources,
        frame: &Frame,
        [width, height]: [f32; 2],
        perspective: Option<Perspective>,
    ) {
        let camera = frame.camera;
        let p = resources.program;
        // SAFETY: caller has bound the owning GL context, raster program and grid.
        unsafe {
            if let Some(pose) = perspective {
                gl.depth_mask(true);
                gl.clear_depth_f64(1.0);
                gl.clear(glow::DEPTH_BUFFER_BIT);
                gl.enable(glow::DEPTH_TEST);
                gl.depth_func(glow::LEQUAL);
                for patch in frame.terrain.iter().flat_map(|surface| {
                    surface
                        .patches
                        .values()
                        .filter(move |patch| surface.patch_visible(pose, patch))
                }) {
                    let cached = &self.tiles[&(frame.surface_source().0, patch.source)];
                    let [west, north, side] = patch.source.bounds();
                    let west = little_clip_core::geography::basemap::wrap_lon_delta(
                        west + side * 0.5 - camera.center_lon,
                    ) - side * 0.5;
                    cached.bind(gl);
                    gl.uniform_1_f32(gl.get_uniform_location(p, "u_side").as_ref(), side as f32);
                    gl.uniform_4_f32(
                        gl.get_uniform_location(p, "u_tile").as_ref(),
                        west as f32,
                        north as f32,
                        side as f32,
                        0.0,
                    );
                    gl.uniform_3_f32_slice(
                        gl.get_uniform_location(p, "u_patch").as_ref(),
                        &patch.uv(),
                    );
                    let fine = patch.steps(camera.zoom) == 128;
                    let grid = &resources.relief_grid[usize::from(fine)];
                    gl.draw_arrays(glow::TRIANGLES, grid.start, grid.end - grid.start);
                }
            } else {
                for tile in &frame.tiles {
                    let Some(cached) = self.tiles.get(&(frame.source, tile.id)) else {
                        continue;
                    };
                    cached.bind(gl);
                    let [west, north, side] = tile.id.bounds();
                    let west = little_clip_core::geography::basemap::wrap_lon_delta(
                        west + side * 0.5 - camera.center_lon,
                    ) - side * 0.5;
                    gl.uniform_1_f32(gl.get_uniform_location(p, "u_side").as_ref(), side as f32);
                    for copy in camera.world_shifts(width, height) {
                        let start = west + copy as f64 * 360.0;
                        if camera.globe_blend() > 0.0 && (start > 180.0 || start + side < -180.0) {
                            continue;
                        }
                        gl.uniform_4_f32(
                            gl.get_uniform_location(p, "u_tile").as_ref(),
                            start as f32,
                            north as f32,
                            side as f32,
                            0.0,
                        );
                        gl.draw_arrays(glow::TRIANGLES, 0, resources.vertices);
                    }
                }
            }
        }
    }

    unsafe fn evict(&mut self, gl: &glow::Context, incoming: usize) {
        while self.bytes + incoming > GPU_BUDGET {
            let Some(key) = self
                .tiles
                .iter()
                .min_by_key(|(_, t)| t.used)
                .map(|(key, _)| *key)
            else {
                break;
            };
            if let Some(tile) = self.tiles.remove(&key) {
                self.bytes -= tile.bytes;
                // SAFETY: caller retains the owning current context.
                unsafe { tile.destroy(gl) };
            }
        }
    }
    unsafe fn destroy(&mut self, gl: &glow::Context) {
        // SAFETY: caller retains the owning current context throughout teardown.
        unsafe {
            self.sky.destroy(gl);
            self.heights.destroy(gl);
            for (_, batch) in self.vectors.drain() {
                batch.destroy(gl);
            }
            for (_, tile) in self.tiles.drain() {
                tile.destroy(gl);
            }
            if let Some(resources) = self.resources.take() {
                gl.delete_program(resources.program);
                gl.delete_program(resources.vector_program);
                gl.delete_program(resources.terrain_vector_program);
                gl.delete_vertex_array(resources.vao);
                gl.delete_buffer(resources.grid);
            }
        }
        self.bytes = 0;
        self.error = Some("Map renderer retired".into());
    }
}

impl Resources {
    unsafe fn new(gl: &glow::Context) -> Result<Self, String> {
        // SAFETY: initialization runs only from the current-context paint callback.
        unsafe {
            let program = program(
                gl,
                &with_projection(include_str!("map.vert")),
                &with_shadow(include_str!("map.frag")),
            )?;
            let vector_program = match self::program(
                gl,
                &with_projection(include_str!("vector.vert")),
                include_str!("vector.frag"),
            ) {
                Ok(p) => p,
                Err(error) => {
                    gl.delete_program(program);
                    return Err(error);
                }
            };
            let terrain_vector_program = match link_program(
                gl,
                [
                    (glow::VERTEX_SHADER, include_str!("terrain_vector.vert")),
                    (
                        glow::GEOMETRY_SHADER,
                        &with_projection(include_str!("terrain_vector.geom")),
                    ),
                    (glow::FRAGMENT_SHADER, include_str!("vector.frag")),
                ],
            ) {
                Ok(p) => p,
                Err(error) => {
                    gl.delete_program(program);
                    gl.delete_program(vector_program);
                    return Err(error);
                }
            };
            let vao = match gl.create_vertex_array() {
                Ok(vao) => vao,
                Err(error) => {
                    gl.delete_program(program);
                    gl.delete_program(vector_program);
                    gl.delete_program(terrain_vector_program);
                    return Err(error);
                }
            };
            let grid = match gl.create_buffer() {
                Ok(buffer) => buffer,
                Err(error) => {
                    gl.delete_program(program);
                    gl.delete_program(vector_program);
                    gl.delete_program(terrain_vector_program);
                    gl.delete_vertex_array(vao);
                    return Err(error);
                }
            };
            let mut vertices = Vec::new();
            let legacy = terrain::grid(32, false, &mut vertices);
            let relief_grid = [
                terrain::grid(32, true, &mut vertices),
                terrain::grid(128, true, &mut vertices),
            ];
            gl.bind_vertex_array(Some(vao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(grid));
            gl.buffer_data_u8_slice(
                glow::ARRAY_BUFFER,
                float_bytes(&vertices),
                glow::STATIC_DRAW,
            );
            gl.enable_vertex_attrib_array(0);
            gl.vertex_attrib_pointer_f32(0, 2, glow::FLOAT, false, 12, 0);
            gl.enable_vertex_attrib_array(1);
            gl.vertex_attrib_pointer_f32(1, 1, glow::FLOAT, false, 12, 8);
            gl.bind_vertex_array(None);
            Ok(Self {
                program,
                vector_program,
                terrain_vector_program,
                vao,
                grid,
                vertices: legacy.end,
                relief_grid,
            })
        }
    }
}

impl TextureTile {
    unsafe fn bind(&self, gl: &glow::Context) {
        // SAFETY: the renderer owns these textures and the current context.
        unsafe {
            for (unit, texture) in self.textures.iter().enumerate() {
                gl.active_texture(glow::TEXTURE0 + unit as u32);
                gl.bind_texture(glow::TEXTURE_2D, Some(*texture));
            }
        }
    }
    unsafe fn upload(gl: &glow::Context, tile: &SourceTile, used: u64) -> Result<Self, String> {
        // SAFETY: fixed-size initialized source buffers match their texture formats.
        unsafe {
            let mut allocated = Vec::new();
            for _ in 0..3 {
                match gl.create_texture() {
                    Ok(texture) => allocated.push(texture),
                    Err(error) => {
                        for texture in allocated {
                            gl.delete_texture(texture);
                        }
                        return Err(error);
                    }
                }
            }
            let textures = [allocated[0], allocated[1], allocated[2]];
            gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
            for (index, (format, external, ty, data)) in [
                (
                    glow::RGBA8,
                    glow::RGBA,
                    glow::UNSIGNED_BYTE,
                    tile.color.as_slice(),
                ),
                (
                    glow::RG32F,
                    glow::RG,
                    glow::FLOAT,
                    float_bytes(&tile.elevation),
                ),
                (
                    glow::R8,
                    glow::RED,
                    glow::UNSIGNED_BYTE,
                    tile.water.as_slice(),
                ),
            ]
            .into_iter()
            .enumerate()
            {
                let (edge, data) = if data.is_empty() {
                    // Surface-only tiles share the same shader and a single invalid DEM texel.
                    (1, &[0; 8][..])
                } else {
                    (PADDED_EDGE as i32, data)
                };
                gl.bind_texture(glow::TEXTURE_2D, Some(textures[index]));
                gl.tex_parameter_i32(
                    glow::TEXTURE_2D,
                    glow::TEXTURE_MIN_FILTER,
                    glow::LINEAR as i32,
                );
                gl.tex_parameter_i32(
                    glow::TEXTURE_2D,
                    glow::TEXTURE_MAG_FILTER,
                    glow::LINEAR as i32,
                );
                gl.tex_parameter_i32(
                    glow::TEXTURE_2D,
                    glow::TEXTURE_WRAP_S,
                    glow::CLAMP_TO_EDGE as i32,
                );
                gl.tex_parameter_i32(
                    glow::TEXTURE_2D,
                    glow::TEXTURE_WRAP_T,
                    glow::CLAMP_TO_EDGE as i32,
                );
                gl.tex_image_2d(
                    glow::TEXTURE_2D,
                    0,
                    format as i32,
                    edge,
                    edge,
                    0,
                    external,
                    ty,
                    glow::PixelUnpackData::Slice(Some(data)),
                );
            }
            gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 4);
            let error = gl.get_error();
            if error != glow::NO_ERROR {
                for texture in textures {
                    gl.delete_texture(texture);
                }
                return Err(format!("Map tile upload failed: OpenGL {error:#x}"));
            }
            Ok(Self {
                textures,
                bytes: tile.bytes(),
                used,
            })
        }
    }
    unsafe fn destroy(self, gl: &glow::Context) {
        // SAFETY: caller retains the owning current context.
        for texture in self.textures {
            unsafe { gl.delete_texture(texture) };
        }
    }
}

pub(super) fn float_bytes(values: &[f32]) -> &[u8] {
    // SAFETY: all bit patterns of initialized f32s are valid bytes; the borrowed
    // slice retains the original storage's lifetime and covers exactly its size.
    unsafe { std::slice::from_raw_parts(values.as_ptr().cast(), std::mem::size_of_val(values)) }
}

fn with_shadow(shader: &str) -> String {
    let (version, body) = shader.split_once('\n').unwrap_or(("", shader));
    let body = body.replace("#include <lunar>", include_str!("lunar.glsl"));
    format!(
        "{version}\n{}\n{}\n{body}",
        include_str!("eclipse.glsl"),
        include_str!("atmosphere.glsl"),
    )
}

fn with_projection(shader: &str) -> String {
    let (version, body) = shader.split_once('\n').unwrap_or(("", shader));
    format!("{version}\n{}\n{body}", include_str!("perspective.glsl"))
}

pub(super) unsafe fn program(
    gl: &glow::Context,
    vertex: &str,
    fragment: &str,
) -> Result<glow::Program, String> {
    // SAFETY: caller retains the owning current GL context.
    unsafe {
        link_program(
            gl,
            [
                (glow::VERTEX_SHADER, vertex),
                (glow::FRAGMENT_SHADER, fragment),
            ],
        )
    }
}

unsafe fn link_program<'a>(
    gl: &glow::Context,
    shaders: impl IntoIterator<Item = (u32, &'a str)>,
) -> Result<glow::Program, String> {
    // SAFETY: compilation runs on the caller's current context. Every shader is
    // deleted after linking, including compile/link failure paths.
    unsafe {
        let program = gl.create_program()?;
        for (kind, source) in shaders {
            let shader = match gl.create_shader(kind) {
                Ok(shader) => shader,
                Err(error) => {
                    gl.delete_program(program);
                    return Err(error);
                }
            };
            gl.shader_source(shader, source);
            gl.compile_shader(shader);
            if !gl.get_shader_compile_status(shader) {
                let error = gl.get_shader_info_log(shader);
                gl.delete_shader(shader);
                gl.delete_program(program);
                return Err(format!("Map shader: {error}"));
            }
            gl.attach_shader(program, shader);
            gl.delete_shader(shader);
        }
        gl.link_program(program);
        if !gl.get_program_link_status(program) {
            let error = gl.get_program_info_log(program);
            gl.delete_program(program);
            return Err(format!("Map shader link: {error}"));
        }
        Ok(program)
    }
}

struct VectorRange {
    start: usize,
    count: i32,
    span: f32,
}

fn vector_ranges(batch: &Batch) -> (Cow<'_, [f32]>, Vec<VectorRange>) {
    const BINS: usize = 20;
    const BASE: f32 = 1.0 / 2048.0;
    let data = batch.data.as_slice();
    let span = |line: &[f32]| {
        let dx = (line[2] - line[0] + 180.0).rem_euclid(360.0) - 180.0;
        dx.abs().max((line[3] - line[1]).abs())
    };
    let bucket = |span: f32| ((span / BASE).log2().ceil().max(0.0) as usize).min(BINS - 1);
    let records = data.chunks_exact(6);
    if batch.layer == Layer::Lake {
        return (
            Cow::Borrowed(data),
            vec![VectorRange {
                start: 0,
                count: records.len() as i32,
                span: 0.0,
            }],
        );
    }
    let mut bins = [(0usize, 0.0_f32); BINS];
    for line in records.clone() {
        let length = span(line);
        let (count, longest) = &mut bins[bucket(length)];
        *count += 1;
        *longest = longest.max(length);
    }
    let mut offsets = [0usize; BINS];
    let mut start = 0;
    let ranges: Vec<_> = bins
        .into_iter()
        .enumerate()
        .filter_map(|(bin, (count, span))| {
            offsets[bin] = start;
            let range = VectorRange {
                start,
                count: count as i32,
                span,
            };
            start += count * 6;
            (count != 0).then_some(range)
        })
        .collect();
    if ranges.len() <= 1 {
        return (Cow::Borrowed(data), ranges);
    }

    // A long border or route must not multiply every short road in its batch.
    let mut sorted = vec![0.0; data.len()];
    for line in records {
        let offset = &mut offsets[bucket(span(line))];
        sorted[*offset..*offset + 6].copy_from_slice(line);
        *offset += 6;
    }
    (Cow::Owned(sorted), ranges)
}

struct VectorBatch {
    vao: glow::VertexArray,
    buffer: glow::Buffer,
    ranges: Vec<VectorRange>,
    count: i32,
    bytes: usize,
    used: u64,
}
impl VectorBatch {
    unsafe fn upload(gl: &glow::Context, batch: &Batch, used: u64) -> Result<Self, String> {
        let (data, ranges) = vector_ranges(batch);
        // SAFETY: the instance buffer contains six initialized floats per record.
        unsafe {
            let vao = gl.create_vertex_array()?;
            let buffer = match gl.create_buffer() {
                Ok(buffer) => buffer,
                Err(error) => {
                    gl.delete_vertex_array(vao);
                    return Err(error);
                }
            };
            gl.bind_vertex_array(Some(vao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(buffer));
            gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, float_bytes(&data), glow::STATIC_DRAW);
            Self::instance_attributes(gl, 0);
            Ok(Self {
                vao,
                buffer,
                ranges,
                count: (data.len() / 6) as i32,
                bytes: data.len() * 4,
                used,
            })
        }
    }
    unsafe fn instance_attributes(gl: &glow::Context, start: usize) {
        // SAFETY: the caller binds this batch's instance buffer and VAO.
        unsafe {
            for (index, size, offset) in [(0, 4, 0), (1, 2, 16)] {
                gl.enable_vertex_attrib_array(index);
                gl.vertex_attrib_pointer_f32(
                    index,
                    size,
                    glow::FLOAT,
                    false,
                    24,
                    (start * 4) as i32 + offset,
                );
                gl.vertex_attrib_divisor(index, 1);
            }
        }
    }

    unsafe fn draw(&self, gl: &glow::Context, program: glow::Program, frame: &Frame, layer: Layer) {
        // SAFETY: the caller has bound this batch's VAO and active program.
        unsafe {
            if let Some(surface) = &frame.terrain {
                // Each input point covers at most seven cells in either axis.
                let step = surface.mesh_step_deg as f32;
                gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.buffer));
                for range in &self.ranges {
                    Self::instance_attributes(gl, range.start);
                    let chunks = (range.span / (7.0 * step)).ceil().max(1.0) as i32;
                    gl.draw_arrays_instanced(glow::POINTS, 0, chunks, range.count);
                }
                Self::instance_attributes(gl, 0);
            } else {
                for copy in frame.camera.world_shifts(frame.size[0], frame.size[1]) {
                    gl.uniform_1_f32(
                        gl.get_uniform_location(program, "u_shift").as_ref(),
                        copy as f32 * 360.0,
                    );
                    gl.draw_arrays_instanced(
                        glow::TRIANGLES,
                        0,
                        if layer == Layer::Lake { 3 } else { 6 },
                        self.count,
                    );
                }
            }
        }
    }

    unsafe fn destroy(self, gl: &glow::Context) {
        // SAFETY: caller retains the owning current context.
        unsafe {
            gl.delete_vertex_array(self.vao);
            gl.delete_buffer(self.buffer);
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum VectorKey {
    Base(usize),
    River(RiverKey, usize),
    Detail(TileId, usize),
}
fn visible_batches(frame: &Frame) -> Vec<(VectorKey, &Batch)> {
    let mut batches = Vec::new();
    if let Some(scene) = &frame.scene {
        for tile in little_clip_core::geography::basemap::tiles::surface_tiles(
            &frame.camera,
            frame.size[0],
            frame.size[1],
            4,
        ) {
            if let Some(indices) = scene.index.get(&tile) {
                batches.extend(
                    indices
                        .iter()
                        .map(|&i| (VectorKey::Base(i), &scene.batches[i])),
                );
            }
        }
    }
    for (key, scene) in &frame.rivers {
        batches.extend(
            scene
                .batches
                .iter()
                .enumerate()
                .map(|(i, b)| (VectorKey::River(*key, i), b)),
        );
    }
    for (key, tile) in &frame.details {
        batches.extend(
            tile.batches
                .iter()
                .enumerate()
                .map(|(i, b)| (VectorKey::Detail(*key, i), b)),
        );
    }
    batches.retain(|(_, b)| b.appearance(frame.camera.zoom, frame.layers).is_some());
    batches.sort_by_key(|(_, b)| b.layer);
    batches
}

impl Renderer {
    fn covered_road_tiles(&self, frame: &Frame) -> BTreeSet<TileId> {
        if !frame.layers.roads {
            return BTreeSet::new();
        }
        frame
            .details
            .iter()
            .filter_map(|(tile, prepared)| {
                let mut roads = prepared
                    .batches
                    .iter()
                    .enumerate()
                    .filter(|(_, b)| b.layer.is_detailed_road());
                let available = |(i, _)| {
                    self.vectors
                        .contains_key(&(frame.source, VectorKey::Detail(*tile, i)))
                };
                (roads.next().is_some_and(available) && roads.all(available)).then_some(*tile)
            })
            .collect()
    }

    unsafe fn upload_vectors(
        &mut self,
        gl: &glow::Context,
        source: u64,
        batches: &[(VectorKey, &Batch)],
    ) -> Result<(), String> {
        let mut uploads = 0;
        // SAFETY: the owning context is current through preparation and drawing.
        unsafe {
            for &(id, batch) in batches {
                let key = (source, id);
                if let Some(batch) = self.vectors.get_mut(&key) {
                    batch.used = self.frame;
                    continue;
                }
                if uploads >= 8 {
                    continue;
                }
                let data = &batch.data;
                while self.vector_bytes + data.len() * 4 > 192 * 1024 * 1024 {
                    let Some(old) = self
                        .vectors
                        .iter()
                        .min_by_key(|(_, b)| b.used)
                        .map(|(k, _)| *k)
                    else {
                        break;
                    };
                    if let Some(batch) = self.vectors.remove(&old) {
                        self.vector_bytes -= batch.bytes;
                        batch.destroy(gl);
                    }
                }
                let batch = VectorBatch::upload(gl, batch, self.frame)?;
                self.vector_bytes += batch.bytes;
                self.vectors.insert(key, batch);
                uploads += 1;
            }
        }
        Ok(())
    }

    unsafe fn paint_vectors(
        &mut self,
        gl: &glow::Context,
        frame: &Frame,
        width: f32,
        height: f32,
        perspective: Option<Perspective>,
    ) -> Result<bool, String> {
        let batches = visible_batches(frame);
        // SAFETY: this method runs inside the owning current-context callback.
        unsafe {
            self.upload_vectors(gl, frame.source, &batches)?;
            let resources = self.resources.as_ref().ok_or("Map resources unavailable")?;
            let p = if frame.terrain.is_some() {
                resources.terrain_vector_program
            } else {
                resources.vector_program
            };
            gl.use_program(Some(p));
            terrain::uniforms(gl, p, perspective);
            if let Some(surface) = &frame.terrain {
                if surface.patches.is_empty() {
                    return Ok(false);
                }
                if self.heights.bind(gl, p, frame.source, surface)? {
                    return Ok(true);
                }
                gl.depth_mask(false);
            }
            gl.uniform_4_f32(
                gl.get_uniform_location(p, "u_camera").as_ref(),
                frame.camera.center_lat as f32,
                frame.camera.center_lon as f32,
                frame.camera.scale_px(width, height) as f32,
                frame.camera.globe_blend() as f32,
            );
            gl.uniform_2_f32(gl.get_uniform_location(p, "u_view").as_ref(), width, height);
            gl.uniform_1_f32(
                gl.get_uniform_location(p, "u_blend").as_ref(),
                frame.camera.globe_blend() as f32,
            );
            let road_tiles = self.covered_road_tiles(frame);
            let covered: Vec<_> = road_tiles
                .iter()
                .flat_map(|tile| [tile.x as i32, tile.y as i32])
                .collect();
            gl.uniform_1_i32(
                gl.get_uniform_location(p, "u_covered_count").as_ref(),
                (covered.len() / 2) as i32,
            );
            if !covered.is_empty() {
                gl.uniform_2_i32_slice(
                    gl.get_uniform_location(p, "u_covered[0]").as_ref(),
                    &covered,
                );
            }
            self.sky.vector_uniforms(gl, p, frame);
            for &(id, source) in &batches {
                let Some(batch) = self.vectors.get(&(frame.source, id)) else {
                    continue;
                };
                let layer = source.layer;
                if layer.is_detailed_road() && !road_tiles.contains(&source.tile) {
                    continue;
                }
                let Some((mut color, line_width, dash)) =
                    source.appearance(frame.camera.zoom, frame.layers)
                else {
                    continue;
                };
                if frame.style == BasemapStyle::Oilslick
                    && matches!(
                        layer,
                        Layer::Lake
                            | Layer::RiverSmall
                            | Layer::RiverMedium
                            | Layer::RiverLarge
                            | Layer::GrwlSmall
                            | Layer::GrwlLarge
                    )
                {
                    color = [0.0, 0.0, 0.0, 1.0];
                }
                vector_style_uniforms(gl, p, layer);
                gl.bind_vertex_array(Some(batch.vao));
                gl.uniform_4_f32_slice(gl.get_uniform_location(p, "u_color").as_ref(), &color);
                gl.uniform_1_f32(gl.get_uniform_location(p, "u_width").as_ref(), line_width);
                gl.uniform_1_i32(
                    gl.get_uniform_location(p, "u_dashed").as_ref(),
                    i32::from(dash),
                );
                batch.draw(gl, p, frame, layer);
            }
            if perspective.is_some() {
                gl.depth_mask(true);
            }
        }
        Ok(batches
            .iter()
            .any(|(id, _)| !self.vectors.contains_key(&(frame.source, *id))))
    }
}

unsafe fn vector_style_uniforms(gl: &glow::Context, program: glow::Program, layer: Layer) {
    // SAFETY: this runs with the vector program active on the renderer's context.
    unsafe {
        let loc = |name| gl.get_uniform_location(program, name);
        gl.uniform_1_f32(
            loc("u_night_dimming").as_ref(),
            match layer {
                Layer::Coast => 0.78,
                Layer::Country | Layer::CountryDisputed => 0.60,
                _ => 0.0,
            },
        );
        gl.uniform_1_i32(
            loc("u_fallback_road").as_ref(),
            i32::from(matches!(layer, Layer::RoadMajor | Layer::RoadMinor)),
        );
        gl.uniform_1_i32(
            loc("u_detailed_road").as_ref(),
            i32::from(
                matches!(layer, Layer::Detailed(stroke) if stroke.fill && stroke.class.is_road()),
            ),
        );
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests;
