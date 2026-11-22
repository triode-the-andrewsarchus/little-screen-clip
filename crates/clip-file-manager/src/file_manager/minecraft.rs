//! Clickable Anvil terrain strips and a tiled Minecraft map studio.
use super::{filesystem::Entry, media::PreviewData, shelf::preview::Ready, ui};
use crate::{icons::Icon, ui_appearance::WorkbenchColors};
use egui::{
    Align2, Color32, ColorImage, FontId, Pos2, Rect, Sense, TextureHandle, TextureOptions, Ui,
    Vec2, pos2, vec2,
};
use little_clip_core::{
    BackgroundJobs, ImageThumbnail, JobError,
    file_source::ReadCheck,
    jobs::task::Task,
    minecraft::{Dimension, Position, REGION_SIZE, World},
    palette::AppPalette,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    sync::{Arc, LazyLock, atomic::AtomicBool},
    time::Duration,
};

pub(super) static PREVIEW_JOBS: LazyLock<BackgroundJobs> =
    LazyLock::new(|| BackgroundJobs::new(2, 4));
static TILE_JOBS: LazyLock<BackgroundJobs> = LazyLock::new(|| BackgroundJobs::new(2, 4));

const MAX_VISIBLE_TILES: usize = 36;
const MAX_CACHED_TILES: usize = 72;
const MIN_DETAIL_SIZE: f64 = 16.0;
const TILE_JOBS_LIMIT: usize = 2;
const MAP_UV: Rect = Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0));

pub(super) fn preview(path: &Path, revision: u64, cancel: &AtomicBool) -> PreviewData {
    let mut data = PreviewData::new(super::display_path(path), "Minecraft map".into(), Icon::Map);
    match (|| {
        super::filesystem::check_revision(path, revision)?;
        let terrain = World::open(path, cancel)?.preview_terrain(cancel)?;
        super::filesystem::check_revision(path, revision)?;
        Ok::<_, String>(terrain)
    })() {
        Ok(terrain) => data.minecraft_terrain = Some(ImageThumbnail::from_rgba(terrain)),
        Err(error) => data.detail = error,
    }
    data
}

pub(super) fn show_strip(
    ui: &mut Ui,
    row: Rect,
    entry: &Entry,
    columns: ui::ListColumns,
    ready: Option<&Ready>,
    colors: WorkbenchColors,
) -> bool {
    let detail = ready.map_or("Minecraft map", |ready| &ready.data.detail);
    let body = ui::information_strip(ui, row, entry, detail, Some(columns), colors);
    let rect = body.intersect(ui.clip_rect());
    if !rect.is_positive() {
        return false;
    }
    let response = ui.interact(
        rect,
        ui.scope_id().with(("minecraft-strip", &entry.path)),
        Sense::click(),
    );
    response.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::accesskit::Role::Button,
            ui.is_enabled(),
            "Open Minecraft map",
        )
    });
    let painter = ui.painter_at(rect);
    painter.rect_filled(body, 3.0, colors.recessed);
    if let Some(texture) = ready.and_then(|ready| ready.texture.as_ref()) {
        let size = texture.size_vec2();
        let scale = (body.width() / size.x).min(body.height() / size.y);
        let image = Rect::from_center_size(body.center(), size * scale);
        painter.image(texture.id(), image, MAP_UV, Color32::WHITE);
        show_map_hover(ui, response.id.with("terrain-hover"), image, texture);
    } else {
        let label = ready.map_or("Loading terrain…", |_| "Terrain preview unavailable");
        painter.text(
            body.center(),
            Align2::CENTER_CENTER,
            label,
            FontId::proportional(12.0),
            colors.text_secondary,
        );
    }
    let plain_click = ui.input(|input| input.modifiers.is_none());
    response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .clicked()
        && plain_click
}

fn show_map_hover(ui: &Ui, id: egui::Id, image: Rect, texture: &TextureHandle) {
    let Some(pointer) = ui
        .ctx()
        .pointer_hover_pos()
        .filter(|_| ui::preview_hovered(ui, image))
    else {
        return;
    };
    let size = vec2(400.0, 260.0).min(ui.ctx().content_rect().size() * 0.7);
    egui::Tooltip::always_open(ui.ctx().clone(), ui.layer_id(), id, image)
        .width(size.x)
        .gap(6.0)
        .show(|popup| {
            let (viewport, _) = popup.allocate_exact_size(size, Sense::hover());
            let painter = popup.painter_at(viewport);
            painter.rect_filled(viewport, 0, popup.visuals().extreme_bg_color);
            painter.image(
                texture.id(),
                magnified_map_rect(image, pointer, viewport, texture.size_vec2()),
                MAP_UV,
                Color32::WHITE,
            );
        });
}

fn magnified_map_rect(image: Rect, pointer: Pos2, viewport: Rect, blocks: Vec2) -> Rect {
    let fit = (viewport.width() / blocks.x).max(viewport.height() / blocks.y);
    // Even a wide overview should show individual blocks in the pop-out.
    let pixels_per_block = (fit * 1.6).ceil().max(2.0);
    let size = blocks * pixels_per_block;
    let focus = (pointer - image.min) / image.size();
    Rect::from_min_size(viewport.center() - focus * size, size)
}

#[cfg(test)]
mod hover_tests {
    use super::*;

    #[test]
    fn magnification_centers_the_hovered_terrain_even_at_map_edges() {
        let image = Rect::from_min_size(pos2(100.0, 20.0), vec2(165.0, 55.0));
        let viewport = Rect::from_min_size(pos2(250.0, 120.0), vec2(400.0, 260.0));
        let blocks = vec2(1536.0, 512.0);
        for pointer in [image.min, image.center(), image.max] {
            let zoomed = magnified_map_rect(image, pointer, viewport, blocks);
            let scale = zoomed.width() / image.width();
            let projected = zoomed.min + (pointer - image.min) * scale;
            assert!(projected.distance(viewport.center()) < 0.001);
            assert_eq!(zoomed.size(), blocks * 2.0);
            assert!((zoomed.height() / image.height() - scale).abs() < 0.001);
        }
    }
}

type TileImage = Result<ColorImage, String>;

enum TileState {
    Loading(Task<TileImage>),
    Ready(TextureHandle),
    Failed,
}

struct Tile {
    state: TileState,
    used_in: u64,
}

struct Overview {
    bounds: [Position; 2],
    raster: Option<ColorImage>,
    texture: Option<TextureHandle>,
}

impl Overview {
    fn new(dimension: &Dimension) -> Self {
        const EDGE: usize = 256;
        let bounds = dimension.bounds();
        let mut raster = ColorImage::filled([EDGE; 2], Color32::TRANSPARENT);
        let span = extent(bounds);
        for position in dimension.positions() {
            let bin = std::array::from_fn::<_, 2, _>(|axis| {
                (((f64::from(position[axis]) - f64::from(bounds[0][axis])) / span[axis])
                    * EDGE as f64) as usize
            });
            raster[(bin[0], bin[1])] = Color32::from_rgb(93, 125, 109);
        }
        Self {
            bounds,
            raster: Some(raster),
            texture: None,
        }
    }
}

pub(super) struct Viewer {
    world: Arc<World>,
    overviews: Vec<Overview>,
    check: ReadCheck,
    dimension: usize,
    camera: Option<Camera>,
    tiles: BTreeMap<Position, Tile>,
    frame: u64,
}

impl Viewer {
    pub(super) fn load(path: &Path, cancel: &AtomicBool) -> Result<Self, String> {
        let world = Arc::new(World::open(path, cancel)?);
        let overviews = world.dimensions.iter().map(Overview::new).collect();
        Ok(Self {
            world,
            overviews,
            check: ReadCheck::default(),
            dimension: 0,
            camera: None,
            tiles: BTreeMap::new(),
            frame: 0,
        })
    }

    pub(super) fn set_check(&mut self, check: ReadCheck) {
        self.check = check;
    }

    pub(super) fn ui(&mut self, ui: &mut Ui, palette: &AppPalette) {
        let colors = WorkbenchColors::from_palette(palette);
        let previous = self.dimension;
        let mut fit = false;
        ui.horizontal(|ui| {
            ui.heading(&self.world.title);
            if self.world.dimensions.len() > 1 {
                egui::ComboBox::from_id_salt("minecraft-dimension")
                    .selected_text(self.world.dimensions[self.dimension].name)
                    .show_ui(ui, |ui| {
                        for (index, dimension) in self.world.dimensions.iter().enumerate() {
                            ui.selectable_value(&mut self.dimension, index, dimension.name);
                        }
                    });
            }
            fit = ui.button("Fit map").clicked();
            ui.label("Drag to pan · Scroll to zoom");
        });
        if previous != self.dimension {
            self.camera = None;
            self.tiles.clear();
        }

        let (rect, response) =
            ui.allocate_exact_size(ui.available_size().max(Vec2::ZERO), Sense::click_and_drag());
        if !rect.is_positive() {
            return;
        }
        let overview = &self.overviews[self.dimension];
        let minimum = Camera::fit(rect, overview.bounds);
        let camera = self.camera.get_or_insert_with(|| {
            let focus = self.world.dimensions[self.dimension].focus;
            Camera {
                center: focus.map(|n| f64::from(n) + 0.5),
                scale: f64::from(rect.size().min_elem()) * 0.78,
            }
        });
        if fit {
            *camera = minimum;
        }
        camera.interact(ui, rect, &response, minimum.scale);
        let camera = *camera;
        response.on_hover_cursor(egui::CursorIcon::Grab);
        self.frame += 1;
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, colors.recessed);

        if camera.scale < MIN_DETAIL_SIZE {
            self.tiles
                .retain(|_, tile| !matches!(tile.state, TileState::Loading(_)));
            self.paint_overview(ui, rect, camera);
        } else {
            self.paint_detail(ui, rect, camera, colors);
        }
        self.paint_coordinates(ui, rect, camera, colors);
    }

    fn paint_overview(&mut self, ui: &Ui, rect: Rect, camera: Camera) {
        let overview = &mut self.overviews[self.dimension];
        if let Some(image) = overview.raster.take() {
            overview.texture = Some(ui.ctx().load_texture(
                "minecraft-world-overview",
                image,
                TextureOptions::NEAREST,
            ));
        }
        if let Some(texture) = &overview.texture {
            let [min, max] = overview.bounds.map(|point| point.map(f64::from));
            ui.painter_at(rect).image(
                texture.id(),
                Rect::from_min_max(
                    camera.project(rect, min),
                    camera.project(rect, max.map(|n| n + 1.0)),
                ),
                MAP_UV,
                Color32::WHITE,
            );
        }
    }

    fn paint_detail(&mut self, ui: &Ui, rect: Rect, camera: Camera, colors: WorkbenchColors) {
        let dimension = &self.world.dimensions[self.dimension];
        let [min, max] = [rect.min, rect.max]
            .map(|point| camera.unproject(rect, point).map(|n| n.floor() as i32));
        // Enumerate the visible rectangle directly. At the minimum detail scale
        // this is bounded by the viewport pixel area rather than by world size.
        let mut visible: Vec<_> = dimension.positions_in(min, max).collect();
        visible.sort_unstable_by(|a, b| camera.distance(*a).total_cmp(&camera.distance(*b)));
        let wanted: BTreeSet<_> = visible.iter().copied().take(MAX_VISIBLE_TILES).collect();
        self.poll_tiles(ui, &wanted);
        let mut loading = self
            .tiles
            .values()
            .filter(|tile| matches!(tile.state, TileState::Loading(_)))
            .count();
        let painter = ui.painter_at(rect);
        for position in visible {
            let tile_rect = camera.tile_rect(rect, position);
            if let Some(tile) = self.tiles.get_mut(&position) {
                tile.used_in = self.frame;
                if let TileState::Ready(texture) = &tile.state {
                    painter.image(texture.id(), tile_rect, MAP_UV, Color32::WHITE);
                    continue;
                }
                if matches!(tile.state, TileState::Failed) {
                    if tile_rect.width() > 120.0 {
                        painter.text(
                            tile_rect.center(),
                            Align2::CENTER_CENTER,
                            "Region unavailable",
                            FontId::proportional(12.0),
                            colors.text_secondary,
                        );
                    }
                    continue;
                }
            } else if loading < TILE_JOBS_LIMIT && wanted.contains(&position) {
                self.start_tile(ui, position);
                loading += 1;
            }
            painter.rect_filled(
                tile_rect.shrink(0.5),
                0,
                colors.text_secondary.gamma_multiply(0.2),
            );
        }
        while self.tiles.len() > MAX_CACHED_TILES {
            let Some(oldest) = self
                .tiles
                .iter()
                .filter(|(_, tile)| !matches!(tile.state, TileState::Loading(_)))
                .min_by_key(|(_, tile)| tile.used_in)
                .map(|(&position, _)| position)
            else {
                break;
            };
            self.tiles.remove(&oldest);
        }
        if loading > 0 {
            ui.ctx().request_repaint_after(Duration::from_millis(30));
        }
    }

    fn start_tile(&mut self, ui: &Ui, position: Position) {
        let world = self.world.clone();
        let dimension = self.dimension;
        let check = self.check.clone();
        let ctx = ui.ctx().clone();
        let job = Task::start(&TILE_JOBS, Duration::from_secs(30), move |cancel| {
            let result = check
                .read(cancel, || {
                    world.dimensions[dimension].render(position, cancel)
                })
                .map(|image| {
                    ColorImage::from_rgba_unmultiplied([REGION_SIZE as usize; 2], image.as_raw())
                });
            ctx.request_repaint();
            result
        });
        self.tiles.insert(
            position,
            Tile {
                state: TileState::Loading(job),
                used_in: self.frame,
            },
        );
    }

    fn poll_tiles(&mut self, ui: &Ui, wanted: &BTreeSet<Position>) {
        self.tiles.retain(|position, tile| {
            let TileState::Loading(job) = &tile.state else {
                return true;
            };
            if !wanted.contains(position) {
                return false;
            }
            tile.state = match job.poll() {
                None => return true,
                Some(Err(JobError::QueueFull)) => return false,
                Some(Ok(Ok(image))) => TileState::Ready(ui.ctx().load_texture(
                    "minecraft-region",
                    image,
                    TextureOptions::NEAREST,
                )),
                Some(Ok(Err(error))) => {
                    tracing::debug!(?position, %error, "Minecraft region preview unavailable");
                    TileState::Failed
                }
                Some(Err(_)) => TileState::Failed,
            };
            true
        });
    }

    fn paint_coordinates(&self, ui: &Ui, rect: Rect, camera: Camera, colors: WorkbenchColors) {
        let pointer = ui
            .ctx()
            .pointer_hover_pos()
            .filter(|p| rect.contains(*p))
            .unwrap_or(rect.center());
        let [x, z] = camera
            .unproject(rect, pointer)
            .map(|n| (n * f64::from(REGION_SIZE)).floor() as i64);
        let text = format!("X {x}   Z {z}");
        let font = FontId::monospace(12.0);
        let painter = ui.painter_at(rect);
        let label = painter.layout_no_wrap(text, font, colors.text_primary);
        let area = Rect::from_min_size(
            rect.left_bottom() + vec2(8.0, -label.size().y - 15.0),
            label.size() + vec2(12.0, 6.0),
        );
        painter.rect_filled(area, 4, colors.recessed);
        painter.galley(area.min + vec2(6.0, 3.0), label, colors.text_primary);
    }
}

#[derive(Clone, Copy)]
struct Camera {
    center: [f64; 2],
    scale: f64,
}

impl Camera {
    fn fit(rect: Rect, [min, max]: [Position; 2]) -> Self {
        let size = extent([min, max]);
        let scale =
            (f64::from(rect.width()) / size[0]).min(f64::from(rect.height()) / size[1]) * 0.92;
        let center = std::array::from_fn(|axis| f64::from(min[axis]) + size[axis] / 2.0);
        Self { center, scale }
    }

    fn project(self, rect: Rect, [x, z]: [f64; 2]) -> Pos2 {
        rect.center()
            + vec2(
                ((x - self.center[0]) * self.scale) as f32,
                ((z - self.center[1]) * self.scale) as f32,
            )
    }

    fn unproject(self, rect: Rect, position: Pos2) -> [f64; 2] {
        let delta = position - rect.center();
        [
            self.center[0] + f64::from(delta.x) / self.scale,
            self.center[1] + f64::from(delta.y) / self.scale,
        ]
    }

    fn distance(self, position: Position) -> f64 {
        (f64::from(position[0]) + 0.5 - self.center[0]).abs()
            + (f64::from(position[1]) + 0.5 - self.center[1]).abs()
    }

    fn tile_rect(self, rect: Rect, position: Position) -> Rect {
        let min = position.map(f64::from);
        Rect::from_min_max(
            self.project(rect, min),
            self.project(rect, min.map(|n| n + 1.0)),
        )
    }

    fn interact(&mut self, ui: &Ui, rect: Rect, response: &egui::Response, minimum: f64) {
        if response.dragged() {
            let delta = response.drag_delta();
            self.center[0] -= f64::from(delta.x) / self.scale;
            self.center[1] -= f64::from(delta.y) / self.scale;
        }
        if let Some(pointer) = response.hover_pos() {
            let factor =
                ui.input(|input| input.zoom_delta() * (input.smooth_scroll_delta.y * 0.006).exp());
            if factor.is_finite() && factor != 1.0 {
                let before = self.unproject(rect, pointer);
                let floor = minimum.clamp(1.0e-9, MIN_DETAIL_SIZE);
                self.scale =
                    (self.scale * f64::from(factor)).clamp(floor, f64::from(REGION_SIZE) * 16.0);
                let after = self.unproject(rect, pointer);
                for axis in 0..2 {
                    self.center[axis] += before[axis] - after[axis];
                }
            }
        }
    }
}

fn extent([min, max]: [Position; 2]) -> [f64; 2] {
    std::array::from_fn(|axis| f64::from(max[axis]) - f64::from(min[axis]) + 1.0)
}
