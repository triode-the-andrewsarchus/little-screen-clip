//! 3D model inspection and orbital viewport workbench view.
//!
//! Renders 3D triangle meshes (STL, OBJ, glTF/GLB, static FBX) using software projection,
//! painter's algorithm depth sorting, directional shading with material tinting,
//! wireframe mode, and background async asset parsing.

use crate::icons::Icon;
use crate::shell::{EscapeHatch, EscapeHatchLauncher, StudioShell};
use crate::ui_ext::StatusToast;
use eframe::egui::{self, Color32, Pos2, Rounding, Stroke, Vec2};
use little_clip_core::JobTicket;
use little_clip_core::palette::AppPalette;
use little_clip_core::presentation::model3d::{Model3d, ModelFormat, Vec3};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, mpsc::Receiver};
use std::time::Duration;

const INLINE_MODEL3D_TRIANGLE_BUDGET: usize = 2_000;
static NEXT_INLINE_MODEL3D_ID: AtomicU64 = AtomicU64::new(1);

/// Caps the number of facets submitted per frame for extreme high-poly meshes based on configured budget.
/// Preserves full triangle density without dropping facets during interactive dragging.
fn triangle_sample_step(total_triangles: usize, budget: usize) -> usize {
    total_triangles.div_ceil(budget.max(100)).max(1)
}

/// Status of background model parsing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelLoadingStatus {
    Loading,
    Ready,
    Error(String),
}

/// Up-axis orientation convention for 3D model orbit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpAxis {
    /// Screen Y is up (standard 3D computer graphics / glTF / OBJ convention).
    YUp,
    /// Model Z is up (standard 3D printing, CAD, and STL convention).
    ZUp,
}

/// Interactive 3D orbital viewer state for supported triangle meshes.
pub struct Model3dViewerState {
    pub path: Option<PathBuf>,
    pub title: String,
    pub status: ModelLoadingStatus,
    pub mesh: Option<Arc<Model3d>>,
    pub rx: Option<Receiver<Result<Result<Arc<Model3d>, String>, little_clip_core::JobError>>>,
    load_ticket: Option<JobTicket>,
    pub pitch: f32, // X rotation (radians)
    pub yaw: f32,   // Y rotation (radians)
    pub zoom: f32,
    pub pan_offset: Vec2,
    pub auto_rotate: bool,
    pub wireframe: bool,
    pub up_axis: UpAxis,
    pub status_toast: StatusToast,
    pub config: Option<little_clip_core::config::Model3dConfig>,
}

/// Compact 3D mesh preview for embedding in another view.
///
/// Parsed geometry is shared across hosts; orbit, zoom, pan, and wireframe
/// state belong to each preview instance.
pub struct InlineModel3dPreview {
    mesh: Arc<Model3d>,
    host_size: Vec2,
    id: egui::Id,
    pitch: f32,
    yaw: f32,
    zoom: f32,
    pan_offset: Vec2,
    up_axis: UpAxis,
    wireframe: bool,
}

impl InlineModel3dPreview {
    /// Creates a compact preview with a fixed finite host size.
    pub fn new(mesh: Arc<Model3d>, host_size: Vec2) -> Self {
        assert!(
            host_size.x.is_finite()
                && host_size.y.is_finite()
                && host_size.x > 0.0
                && host_size.y > 0.0,
            "inline Model3D preview requires a positive finite host size"
        );
        let up_axis = if mesh.format == ModelFormat::Stl {
            UpAxis::ZUp
        } else {
            UpAxis::YUp
        };
        Self {
            mesh,
            host_size,
            id: egui::Id::new((
                "inline-model3d-preview",
                NEXT_INLINE_MODEL3D_ID.fetch_add(1, Ordering::Relaxed),
            )),
            pitch: 0.4,
            yaw: 0.6,
            zoom: 1.0,
            pan_offset: Vec2::ZERO,
            up_axis,
            wireframe: false,
        }
    }

    /// Draws a bounded mesh preview; drag to orbit/pan and scroll to zoom.
    pub fn show(&mut self, ui: &mut egui::Ui, palette: &AppPalette) -> egui::Response {
        let (rect, response) = ui
            .push_id(self.id, |ui| {
                ui.allocate_exact_size(self.host_size, egui::Sense::click_and_drag())
            })
            .inner;
        if !ui.is_rect_visible(rect) {
            return response;
        }
        if response.dragged_by(egui::PointerButton::Primary) {
            let delta = response.drag_delta();
            self.yaw += delta.x * 0.01;
            self.pitch = (self.pitch + delta.y * 0.01).clamp(-1.5, 1.5);
        }
        if response.dragged_by(egui::PointerButton::Secondary)
            || response.dragged_by(egui::PointerButton::Middle)
        {
            self.pan_offset += response.drag_delta();
        }
        let scroll = ui.input(|input| input.raw_scroll_delta.y);
        if scroll.abs() > 0.1 && response.hovered() {
            self.zoom = (self.zoom * if scroll > 0.0 { 1.15 } else { 0.85 }).clamp(0.1, 20.0);
        }

        let painter = ui.painter_at(rect);
        let background = crate::hex_to_c32(&palette.colors.surface_recessed);
        painter.rect_filled(
            rect,
            Rounding::same(crate::ui_ext::PANEL_CORNER_RADIUS),
            background,
        );
        painter.rect_stroke(
            rect,
            Rounding::same(crate::ui_ext::PANEL_CORNER_RADIUS),
            Stroke::new(1.0_f32, crate::hex_to_c32(&palette.colors.surface_panel)),
        );
        let accent = crate::hex_to_c32(&palette.harmonies.base);
        render_mesh_projection(
            &self.mesh,
            &painter,
            rect.shrink(crate::ui_ext::SPACE_XS),
            self.pan_offset,
            self.zoom,
            self.yaw,
            self.pitch,
            self.wireframe,
            self.up_axis,
            accent,
            INLINE_MODEL3D_TRIANGLE_BUDGET,
        );
        response
    }
}

impl Model3dViewerState {
    /// Begins loading a 3D model asynchronously from disk on a background thread.
    pub fn from_file(path: PathBuf) -> Self {
        let title = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("3D Model")
            .to_string();

        let up_axis = if path
            .extension()
            .and_then(|s| s.to_str())
            .map(|e| e.eq_ignore_ascii_case("stl"))
            .unwrap_or(false)
        {
            UpAxis::ZUp
        } else {
            UpAxis::YUp
        };

        let load_path = path.clone();
        let (ticket, rx) = little_clip_core::BackgroundJobs::global()
            .submit(Duration::from_secs(30), move || {
                Model3d::load_file(&load_path).map(Arc::new)
            });

        Self {
            path: Some(path),
            title,
            status: ModelLoadingStatus::Loading,
            mesh: None,
            rx: Some(rx),
            load_ticket: Some(ticket),
            pitch: 0.4,
            yaw: 0.6,
            zoom: 1.0,
            pan_offset: Vec2::ZERO,
            auto_rotate: true,
            wireframe: false,
            up_axis,
            status_toast: StatusToast::new(),
            config: None,
        }
    }

    /// Attaches 3D configuration settings (e.g. facet budgets).
    pub fn with_config(mut self, config: little_clip_core::config::Model3dConfig) -> Self {
        self.config = Some(config);
        self
    }

    /// Constructs a ready viewer from parsed model data for in-module tests.
    #[cfg(test)]
    pub fn from_model(mesh: Model3d, path: Option<PathBuf>) -> Self {
        Self::from_shared_model(Arc::new(mesh), path)
    }

    /// Constructs an immediate ready viewer from shared parsed model data.
    pub fn from_shared_model(mesh: Arc<Model3d>, path: Option<PathBuf>) -> Self {
        let title = path
            .as_ref()
            .and_then(|p| p.file_name())
            .and_then(|s| s.to_str())
            .unwrap_or("3D Model")
            .to_string();

        let up_axis = if mesh.format == ModelFormat::Stl {
            UpAxis::ZUp
        } else {
            UpAxis::YUp
        };

        Self {
            path,
            title,
            status: ModelLoadingStatus::Ready,
            mesh: Some(mesh),
            rx: None,
            load_ticket: None,
            pitch: 0.4,
            yaw: 0.6,
            zoom: 1.0,
            pan_offset: Vec2::ZERO,
            auto_rotate: true,
            wireframe: false,
            up_axis,
            status_toast: StatusToast::new(),
            config: None,
        }
    }

    fn poll_loader(&mut self) {
        if let Some(ref rx) = self.rx {
            if let Ok(res) = rx.try_recv() {
                match res {
                    Ok(Ok(m)) => {
                        self.up_axis = if m.format == ModelFormat::Stl {
                            UpAxis::ZUp
                        } else {
                            UpAxis::YUp
                        };
                        self.mesh = Some(m);
                        self.status = ModelLoadingStatus::Ready;
                    }
                    Ok(Err(e)) => {
                        self.status = ModelLoadingStatus::Error(e);
                    }
                    Err(error) => self.status = ModelLoadingStatus::Error(error.to_string()),
                }
                self.rx = None;
                self.load_ticket = None;
            }
        }
    }

    pub fn render_ui(&mut self, ui: &mut egui::Ui, palette: &AppPalette, close: &mut bool) {
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            *close = true;
            return;
        }

        self.poll_loader();

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs_f64();

        let bg = crate::hex_to_c32(&palette.colors.canvas);
        let b_low = crate::hex_to_c32(&palette.colors.surface_recessed);
        let f_high = crate::hex_to_c32(&palette.colors.text_primary);

        match &self.status {
            ModelLoadingStatus::Loading => {
                self.render_loading(ui, palette, close, bg, b_low, f_high);
            }
            ModelLoadingStatus::Error(err) => {
                let err_msg = err.clone();
                self.render_error(ui, &err_msg, palette, close, now, bg, b_low, f_high);
            }
            ModelLoadingStatus::Ready => {
                self.render_ready(ui, palette, close, now, bg, b_low);
            }
        }
    }

    fn render_loading(
        &self,
        ui: &mut egui::Ui,
        palette: &AppPalette,
        close: &mut bool,
        bg: Color32,
        b_low: Color32,
        f_high: Color32,
    ) {
        ui.ctx().request_repaint_after(Duration::from_millis(50));
        egui::Frame::none()
            .fill(bg)
            .inner_margin(egui::Margin::same(crate::ui_ext::SPACE_MD))
            .show(ui, |ui| {
                let mut shell = StudioShell::new(&self.title, Icon::Model3d, palette, close)
                    .with_badge("Parsing 3D Model...".to_string(), None)
                    .with_footer_hints("Loading asset geometry into memory...");
                let _ = shell.render_header(ui, |_ui| {});

                ui.add_space(20.0);
                let available_size = ui.available_size();
                let (rect, _) = ui.allocate_exact_size(available_size, egui::Sense::hover());
                let painter = ui.painter_at(rect);
                painter.rect_filled(
                    rect,
                    Rounding::same(crate::ui_ext::PANEL_CORNER_RADIUS),
                    b_low,
                );

                let center = rect.center();
                let text = "Parsing 3D mesh geometry with Scenix...";
                painter.text(
                    center,
                    egui::Align2::CENTER_CENTER,
                    text,
                    egui::FontId::proportional(crate::ui_ext::BODY_FONT_SIZE),
                    f_high,
                );
            });
    }

    fn render_error(
        &mut self,
        ui: &mut egui::Ui,
        err: &str,
        palette: &AppPalette,
        close: &mut bool,
        now: f64,
        bg: Color32,
        b_low: Color32,
        f_high: Color32,
    ) {
        egui::Frame::none()
            .fill(bg)
            .inner_margin(egui::Margin::same(crate::ui_ext::SPACE_MD))
            .show(ui, |ui| {
                let mut shell = StudioShell::new(&self.title, Icon::Alert, palette, close)
                    .with_badge("Failed to load".to_string(), None)
                    .with_escape_hatch(
                        EscapeHatch::primary(
                            "Blender",
                            "blender",
                            "Open 3D model in Blender / FreeCAD",
                        )
                        .with_icon(Icon::Model3d),
                    )
                    .with_footer_hints("Press Esc to close");

                let clicked_hatch = shell.render_header(ui, |_ui| {});
                if let Some(0) = clicked_hatch {
                    if let Some(ref p) = self.path {
                        EscapeHatchLauncher::launch(
                            "3D Model Viewer",
                            "blender",
                            p,
                            &mut self.status_toast,
                            now,
                        );
                    }
                }

                ui.add_space(20.0);
                let available_size = ui.available_size();
                let (rect, _) = ui.allocate_exact_size(available_size, egui::Sense::hover());
                let painter = ui.painter_at(rect);
                painter.rect_filled(
                    rect,
                    Rounding::same(crate::ui_ext::PANEL_CORNER_RADIUS),
                    b_low,
                );

                let text = format!("Unable to load 3D model:\n\n{}", err);
                painter.text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    text,
                    egui::FontId::proportional(crate::ui_ext::BODY_FONT_SIZE),
                    f_high,
                );
            });
    }

    fn render_ready(
        &mut self,
        ui: &mut egui::Ui,
        palette: &AppPalette,
        close: &mut bool,
        now: f64,
        bg: Color32,
        b_low: Color32,
    ) {
        let Some(mesh) = &self.mesh else {
            return;
        };

        // Frame-rate independent auto-rotation
        let dt = ui.input(|i| i.stable_dt).min(0.1);
        if self.auto_rotate {
            self.yaw += 0.9 * dt;
            ui.ctx().request_repaint();
        }

        let tri_count = mesh.triangles.len();
        let size_x = mesh.size.x;
        let size_y = mesh.size.y;
        let size_z = mesh.size.z;
        let auto_rotate = self.auto_rotate;
        let wireframe = self.wireframe;
        let format_badge = mesh.format.badge_label();

        let b_med = crate::hex_to_c32(&palette.colors.surface_panel);
        let accent = crate::hex_to_c32(&palette.harmonies.base);

        let mut auto_rot_clicked = false;
        let mut wire_clicked = false;
        let mut reset_clicked = false;

        egui::Frame::none()
            .fill(bg)
            .inner_margin(egui::Margin::same(crate::ui_ext::SPACE_MD))
            .show(ui, |ui| {
                let info_badge = format!(
                    "{} • {} triangles • {:.1}×{:.1}×{:.1} mm",
                    format_badge, tri_count, size_x, size_y, size_z
                );

                let mut shell = StudioShell::new(&self.title, Icon::Model3d, palette, close)
                    .with_badge(info_badge, None)
                    .with_escape_hatch(EscapeHatch::primary(
                        "Blender",
                        "blender",
                        "Open 3D model in Blender / FreeCAD",
                    ).with_icon(Icon::Model3d))
                    .with_footer_hints(
                        "Drag: Orbit • Right/Mid: Pan • Scroll: Zoom • Esc: Close",
                    )
                    .with_footer_readout(format!(
                        "Pitch {:.1}° • Yaw {:.1}° • Zoom {:.1}×",
                        self.pitch.to_degrees(),
                        self.yaw.to_degrees(),
                        self.zoom
                    ));

                let clicked_hatch = shell.render_header(ui, |_ui| {});
                if let Some(0) = clicked_hatch {
                    if let Some(ref p) = self.path {
                        EscapeHatchLauncher::launch(
                            "3D Model Viewer",
                            "blender",
                            p,
                            &mut self.status_toast,
                            now,
                        );
                    } else {
                        self.status_toast
                            .set("No on-disk file path for this 3D model", now);
                    }
                }

                ui.add_space(crate::ui_ext::SPACE_XS);

                // Toolbar
                egui::Frame::none()
                    .fill(b_low)
                    .stroke(Stroke::new(1.0_f32, b_med))
                    .inner_margin(egui::Margin::symmetric(
                        crate::ui_ext::PANEL_INSET,
                        crate::ui_ext::CONTROL_GROUP_INSET_Y,
                    ))
                    .show(ui, |ui| {
                        // Keep every viewer action one click away while allowing
                        // the controls to wrap on narrower fixed popup sizes.
                        ui.horizontal_wrapped(|ui| {
                            let auto_rot_bg = if auto_rotate { b_med } else { accent };
                            let (auto_rot_label, auto_rot_icon) = if auto_rotate {
                                ("Pause Rotation", Icon::Pause)
                            } else {
                                ("Auto-Rotate", Icon::Play)
                            };
                            let auto_rot_btn = crate::ui_ext::action_button_icon(
                                Some(auto_rot_icon),
                                auto_rot_label,
                                auto_rot_bg,
                                palette,
                            );
                            if ui.add(auto_rot_btn).clicked() {
                                auto_rot_clicked = true;
                            }

                            let wire_bg = if wireframe { accent } else { b_med };
                            let (wire_label, wire_icon) = if wireframe {
                                ("Shaded", Icon::Palette)
                            } else {
                                ("Wireframe", Icon::Teapot)
                            };
                            let wire_btn = crate::ui_ext::action_button_icon(
                                Some(wire_icon),
                                wire_label,
                                wire_bg,
                                palette,
                            );
                            if ui.add(wire_btn).clicked() {
                                wire_clicked = true;
                            }

                            let axis_label = match self.up_axis {
                                UpAxis::ZUp => "Axis: Z-Up",
                                UpAxis::YUp => "Axis: Y-Up",
                            };
                            let axis_btn = crate::ui_ext::secondary_button_icon(
                                Some(Icon::ArrowUp),
                                axis_label,
                                b_med,
                                palette,
                            );
                            let mut axis_clicked = false;
                            if ui
                                .add(axis_btn)
                                .on_hover_text(
                                    "Toggle between Z-Up (standard 3D printing / CAD) and Y-Up (3D graphics convention)",
                                )
                                .clicked()
                            {
                                axis_clicked = true;
                            }

                            let reset_btn = crate::ui_ext::secondary_button_icon(
                                Some(Icon::RotateCcw),
                                "Reset View",
                                b_med,
                                palette,
                            );
                            if ui.add(reset_btn).clicked() {
                                reset_clicked = true;
                            }

                            if axis_clicked {
                                self.up_axis = match self.up_axis {
                                    UpAxis::ZUp => UpAxis::YUp,
                                    UpAxis::YUp => UpAxis::ZUp,
                                };
                                self.status_toast.set(
                                    match self.up_axis {
                                        UpAxis::ZUp => "Orientation set to Z-Up (CAD / 3D printing)",
                                        UpAxis::YUp => "Orientation set to Y-Up (3D Graphics)",
                                    },
                                    now,
                                );
                            }
                        });
                    });

                if auto_rot_clicked {
                    self.auto_rotate = !self.auto_rotate;
                }
                if wire_clicked {
                    self.wireframe = !self.wireframe;
                }
                if reset_clicked {
                    self.pitch = 0.4;
                    self.yaw = 0.6;
                    self.zoom = 1.0;
                    self.pan_offset = Vec2::ZERO;
                }

                ui.add_space(crate::ui_ext::SPACE_XS);
                ui.separator();
                ui.add_space(crate::ui_ext::SPACE_XS);

                let toast_tuple = self.status_toast.active(now).cloned();
                // Place the footer first in a bottom-up region. Its actual wrapped height is
                // reserved before allocating the canvas, so narrow hosts do not clip it and
                // wide hosts keep the extra height for the model.
                ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
                    shell.render_footer(ui, &toast_tuple, now);

                    // Interactive 3D Canvas
                    let available_size = ui.available_size();
                    let (canvas_rect, response) =
                        ui.allocate_exact_size(available_size, egui::Sense::click_and_drag());
                let painter = ui.painter_at(canvas_rect);

                // Background dark canvas
                painter.rect_filled(canvas_rect, Rounding::same(crate::ui_ext::PANEL_CORNER_RADIUS), b_low);

                // Mouse interaction
                if response.dragged_by(egui::PointerButton::Primary) {
                    let delta = response.drag_delta();
                    self.yaw += delta.x * 0.01;
                    self.pitch = (self.pitch + delta.y * 0.01).clamp(-1.5, 1.5);
                    self.auto_rotate = false;
                }

                if response.dragged_by(egui::PointerButton::Secondary)
                    || response.dragged_by(egui::PointerButton::Middle)
                {
                    self.pan_offset += response.drag_delta();
                }

                let scroll = ui.input(|i| i.raw_scroll_delta.y);
                if scroll.abs() > 0.1 && response.hovered() {
                    let zoom_factor = if scroll > 0.0 { 1.15 } else { 0.85 };
                    self.zoom = (self.zoom * zoom_factor).clamp(0.1, 20.0);
                }

                let budget = self
                    .config
                    .as_ref()
                    .map(|c| c.workbench_triangle_limit)
                    .unwrap_or(150_000);

                render_mesh_projection(
                    mesh,
                    &painter,
                    canvas_rect,
                    self.pan_offset,
                    self.zoom,
                    self.yaw,
                    self.pitch,
                    self.wireframe,
                    self.up_axis,
                    accent,
                    budget,
                );
                });
            });
    }
}

fn render_mesh_projection(
    mesh: &Model3d,
    painter: &egui::Painter,
    canvas_rect: egui::Rect,
    pan_offset: Vec2,
    zoom: f32,
    yaw: f32,
    pitch: f32,
    wireframe: bool,
    up_axis: UpAxis,
    accent: Color32,
    budget: usize,
) {
    let center = canvas_rect.center() + pan_offset;
    let max_dim = mesh.max_dimension.max(0.001);
    let base_scale = (canvas_rect.width().min(canvas_rect.height()) * 0.4) / max_dim;
    let scale = base_scale * zoom;

    let cy = yaw.cos();
    let sy = yaw.sin();
    let cx = pitch.cos();
    let sx = pitch.sin();

    let project = |v: &Vec3| -> (Pos2, f32) {
        let (off_x, off_y, off_z) = match up_axis {
            UpAxis::ZUp => (
                v.x - mesh.center.x,
                v.z - mesh.center.z,
                -(v.y - mesh.center.y),
            ),
            UpAxis::YUp => (
                v.x - mesh.center.x,
                v.y - mesh.center.y,
                v.z - mesh.center.z,
            ),
        };
        let rx = off_x * cy + off_z * sy;
        let rz1 = -off_x * sy + off_z * cy;
        let ry = off_y * cx - rz1 * sx;
        let rz = off_y * sx + rz1 * cx;

        let px = center.x + rx * scale;
        let py = center.y - ry * scale;
        (Pos2::new(px, py), rz)
    };

    let total_tris = mesh.triangles.len();
    let skip_step = triangle_sample_step(total_tris, budget);

    let mut projected_tris = Vec::with_capacity(total_tris / skip_step + 1);
    for i in (0..total_tris).step_by(skip_step) {
        let t = &mesh.triangles[i];
        let (p1, z1) = project(&t.v1);
        let (p2, z2) = project(&t.v2);
        let (p3, z3) = project(&t.v3);
        let avg_z = (z1 + z2 + z3) / 3.0;

        let (raw_nx, raw_ny, raw_nz) = match up_axis {
            UpAxis::ZUp => (t.normal.x, t.normal.z, -t.normal.y),
            UpAxis::YUp => (t.normal.x, t.normal.y, t.normal.z),
        };
        let nx = raw_nx * cy + raw_nz * sy;
        let nz1 = -raw_nx * sy + raw_nz * cy;
        let ny = raw_ny * cx - nz1 * sx;
        let nz = raw_ny * sx + nz1 * cx;
        let rotated_normal = Vec3::new(nx, ny, nz);

        if rotated_normal.z <= 0.0 {
            continue;
        }

        projected_tris.push((avg_z, [p1, p2, p3], rotated_normal, t.color));
    }

    projected_tris.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

    let light_dir = Vec3::new(0.4, 0.7, 0.6).normalize();

    if wireframe {
        let stroke = Stroke::new(1.0_f32, accent);
        for (_, pts, _, _) in &projected_tris {
            painter.line_segment([pts[0], pts[1]], stroke);
            painter.line_segment([pts[1], pts[2]], stroke);
            painter.line_segment([pts[2], pts[0]], stroke);
        }
    } else {
        let mut mesh_shape = egui::epaint::Mesh::default();
        for (_, pts, norm, tri_color) in &projected_tris {
            let dot = (norm.x * light_dir.x + norm.y * light_dir.y + norm.z * light_dir.z).max(0.0);
            let ambient = 0.25;
            let intensity = (ambient + dot * 0.75).clamp(0.0, 1.0);

            let (br, bg_val, bb) = if let Some(c) = tri_color {
                (c[0] * 255.0, c[1] * 255.0, c[2] * 255.0)
            } else {
                (accent.r() as f32, accent.g() as f32, accent.b() as f32)
            };
            let face_color = Color32::from_rgb(
                (br * intensity).clamp(0.0, 255.0) as u8,
                (bg_val * intensity).clamp(0.0, 255.0) as u8,
                (bb * intensity).clamp(0.0, 255.0) as u8,
            );

            let idx = mesh_shape.vertices.len() as u32;
            mesh_shape.vertices.push(egui::epaint::Vertex {
                pos: pts[0],
                uv: egui::epaint::WHITE_UV,
                color: face_color,
            });
            mesh_shape.vertices.push(egui::epaint::Vertex {
                pos: pts[1],
                uv: egui::epaint::WHITE_UV,
                color: face_color,
            });
            mesh_shape.vertices.push(egui::epaint::Vertex {
                pos: pts[2],
                uv: egui::epaint::WHITE_UV,
                color: face_color,
            });
            mesh_shape
                .indices
                .extend_from_slice(&[idx, idx + 1, idx + 2]);
        }

        painter.add(egui::Shape::mesh(mesh_shape));
    }
}

impl Drop for Model3dViewerState {
    fn drop(&mut self) {
        if let Some(ticket) = &self.load_ticket {
            ticket.cancel();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inline_test_mesh() -> Arc<Model3d> {
        let triangle = little_clip_core::presentation::model3d::Triangle::new(
            Vec3::new(0.0, 0.0, 1.0),
            Vec3::new(-0.5, -0.5, 0.0),
            Vec3::new(0.5, -0.5, 0.0),
            Vec3::new(0.0, 0.5, 0.0),
        );
        Arc::new(Model3d {
            name: "inline-shape.obj".to_owned(),
            format: ModelFormat::Obj,
            triangles: vec![triangle; INLINE_MODEL3D_TRIANGLE_BUDGET * 3],
            thumbnail_triangles: vec![triangle],
            min_bounds: Vec3::new(-0.5, -0.5, 0.0),
            max_bounds: Vec3::new(0.5, 0.5, 0.0),
            center: Vec3::ZERO,
            size: Vec3::new(1.0, 1.0, 0.0),
            max_dimension: 1.0,
        })
    }

    fn render_inline_pair(
        ctx: &egui::Context,
        first: &mut InlineModel3dPreview,
        second: &mut InlineModel3dPreview,
        events: Vec<egui::Event>,
        time: f64,
    ) -> (egui::FullOutput, [egui::Rect; 2]) {
        let palette = AppPalette::default();
        let mut rects = [egui::Rect::NOTHING; 2];
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    Pos2::ZERO,
                    Vec2::new(500.0, 240.0),
                )),
                events,
                time: Some(time),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        rects[0] = first.show(ui, &palette).rect;
                        rects[1] = second.show(ui, &palette).rect;
                    });
                });
            },
        );
        (output, rects)
    }

    #[test]
    fn inline_model3d_previews_share_mesh_with_independent_camera_and_bounded_paint() {
        let mesh = inline_test_mesh();
        let mut first = InlineModel3dPreview::new(Arc::clone(&mesh), Vec2::new(180.0, 140.0));
        let mut second = InlineModel3dPreview::new(Arc::clone(&mesh), Vec2::new(180.0, 140.0));
        assert!(Arc::ptr_eq(&first.mesh, &second.mesh));
        assert_ne!(first.id, second.id);

        let ctx = egui::Context::default();
        let (initial, rects) = render_inline_pair(&ctx, &mut first, &mut second, Vec::new(), 0.0);
        let mesh_shapes: Vec<_> = initial
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::epaint::Shape::Mesh(mesh) => Some((shape.clip_rect, mesh)),
                _ => None,
            })
            .collect();
        assert_eq!(mesh_shapes.len(), 2, "each visible host paints its mesh");
        for (clip_rect, mesh_shape) in mesh_shapes {
            assert!(rects.iter().any(|host| {
                clip_rect.intersects(*host)
                    && clip_rect.min.x >= host.min.x
                    && clip_rect.max.x <= host.max.x
                    && clip_rect.min.y >= host.min.y
                    && clip_rect.max.y <= host.max.y
            }));
            assert!(
                mesh_shape.vertices.len() <= INLINE_MODEL3D_TRIANGLE_BUDGET * 3,
                "preview mesh paint must respect its triangle budget"
            );
        }

        let start = rects[0].center();
        let yaw = first.yaw;
        let pitch = first.pitch;
        render_inline_pair(
            &ctx,
            &mut first,
            &mut second,
            vec![
                egui::Event::PointerMoved(start),
                egui::Event::PointerButton {
                    pos: start,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
            0.1,
        );
        render_inline_pair(
            &ctx,
            &mut first,
            &mut second,
            vec![egui::Event::PointerMoved(start + Vec2::new(20.0, 10.0))],
            0.2,
        );
        render_inline_pair(
            &ctx,
            &mut first,
            &mut second,
            vec![egui::Event::PointerButton {
                pos: start + Vec2::new(20.0, 10.0),
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
            0.3,
        );
        assert_ne!((first.yaw, first.pitch), (yaw, pitch));
        assert_eq!((second.yaw, second.pitch), (0.6, 0.4));

        let zoom_before = first.zoom;
        render_inline_pair(
            &ctx,
            &mut first,
            &mut second,
            vec![
                egui::Event::PointerMoved(rects[0].center()),
                egui::Event::Scroll(Vec2::new(0.0, 80.0)),
            ],
            0.4,
        );
        assert!(first.zoom > zoom_before);
        assert_eq!(second.zoom, 1.0);
    }

    fn test_mesh() -> Model3d {
        Model3d {
            name: "shape.obj".to_owned(),
            format: ModelFormat::Obj,
            triangles: vec![],
            thumbnail_triangles: vec![],
            min_bounds: Vec3::ZERO,
            max_bounds: Vec3::ZERO,
            center: Vec3::ZERO,
            size: Vec3::ZERO,
            max_dimension: 1.0,
        }
    }

    #[test]
    fn independent_model_views_share_mesh_and_keep_camera_state_separate() {
        let source = Arc::new(test_mesh());
        let mut first = Model3dViewerState::from_shared_model(Arc::clone(&source), None);
        let second = Model3dViewerState::from_shared_model(Arc::clone(&source), None);

        assert!(Arc::ptr_eq(
            first.mesh.as_ref().unwrap(),
            second.mesh.as_ref().unwrap()
        ));
        assert!(Arc::ptr_eq(first.mesh.as_ref().unwrap(), &source));
        first.pitch = 1.1;
        first.yaw = -0.3;
        first.zoom = 2.0;
        first.pan_offset = Vec2::new(12.0, -8.0);
        first.wireframe = true;

        assert_eq!(second.pitch, 0.4);
        assert_eq!(second.yaw, 0.6);
        assert_eq!(second.zoom, 1.0);
        assert_eq!(second.pan_offset, Vec2::ZERO);
        assert!(!second.wireframe);
    }

    #[test]
    fn inline_model_source_seeds_independent_full_studios_without_loading_again() {
        let source = Arc::new(test_mesh());
        let inline = InlineModel3dPreview::new(Arc::clone(&source), Vec2::new(180.0, 140.0));
        let mut first = Model3dViewerState::from_shared_model(Arc::clone(&source), None);
        let second = Model3dViewerState::from_shared_model(Arc::clone(&source), None);

        assert!(Arc::ptr_eq(&inline.mesh, &source));
        assert!(Arc::ptr_eq(first.mesh.as_ref().unwrap(), &source));
        assert!(Arc::ptr_eq(second.mesh.as_ref().unwrap(), &source));
        assert_eq!(first.status, ModelLoadingStatus::Ready);
        assert!(first.rx.is_none() && first.load_ticket.is_none());
        assert!(second.rx.is_none() && second.load_ticket.is_none());

        first.pitch = 1.0;
        first.yaw = -0.5;
        first.zoom = 3.0;
        first.wireframe = true;
        assert_eq!(second.pitch, 0.4);
        assert_eq!(second.yaw, 0.6);
        assert_eq!(second.zoom, 1.0);
        assert!(!second.wireframe);
        assert_eq!((inline.pitch, inline.yaw, inline.zoom), (0.4, 0.6, 1.0));
    }

    #[test]
    fn file_and_byte_loaders_keep_distinct_source_identity() {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().join("cached_shape.stl");
        let bytes = br#"solid test_cache
  facet normal 0 0 1
    outer loop
      vertex 0 0 0
      vertex 1 0 0
      vertex 0 1 0
    endloop
  endfacet
endsolid test_cache
"#;
        std::fs::write(&path, bytes).unwrap();
        let viewer = Model3dViewerState::from_file(path);
        let loaded = viewer
            .rx
            .as_ref()
            .unwrap()
            .recv_timeout(Duration::from_secs(5))
            .expect("file loader should finish")
            .expect("worker should not panic")
            .expect("valid STL should load");
        let from_bytes =
            Arc::new(Model3d::load_bytes(bytes, "stl", "cached_shape".to_owned()).unwrap());
        assert_eq!(loaded.name, "cached_shape");
        assert!(!Arc::ptr_eq(&loaded, &from_bytes));
    }

    fn render_text_at_width(
        viewer: &mut Model3dViewerState,
        width: f32,
    ) -> Vec<(String, egui::Rect, bool, egui::Rect)> {
        render_text_at_size(viewer, width, 720.0)
    }

    fn render_text_at_size(
        viewer: &mut Model3dViewerState,
        width: f32,
        height: f32,
    ) -> Vec<(String, egui::Rect, bool, egui::Rect)> {
        let ctx = egui::Context::default();
        crate::ui_ext::configure_workbench_style(&ctx);
        let palette = AppPalette::default();
        let host = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(width, height));
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(host),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let mut close = false;
                    viewer.render_ui(ui, &palette, &mut close);
                });
            },
        );

        output
            .shapes
            .iter()
            .filter_map(|clipped| {
                let egui::epaint::Shape::Text(text) = &clipped.shape else {
                    return None;
                };
                let bounds = text.galley.rect.translate(text.pos.to_vec2());
                Some((
                    text.galley.job.text.clone(),
                    bounds,
                    text.galley.elided,
                    clipped.clip_rect,
                ))
            })
            .collect()
    }

    fn render_interaction_frame(
        ctx: &egui::Context,
        viewer: &mut Model3dViewerState,
        time: f64,
        events: Vec<egui::Event>,
    ) -> egui::Rect {
        let palette = AppPalette::default();
        let canvas_fill = crate::hex_to_c32(&palette.colors.surface_recessed);
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(900.0, 700.0),
                )),
                time: Some(time),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let mut close = false;
                    viewer.render_ui(ui, &palette, &mut close);
                });
            },
        );
        output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::epaint::Shape::Rect(rect) if rect.fill == canvas_fill => Some(rect.rect),
                _ => None,
            })
            .max_by(|left, right| left.area().total_cmp(&right.area()))
            .expect("ready viewer should paint its canvas")
    }

    fn pointer_button_event(pos: Pos2, button: egui::PointerButton, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button,
            pressed,
            modifiers: egui::Modifiers::NONE,
        }
    }

    #[test]
    fn test_triangle_sample_step_preserves_full_density() {
        let budget = 120_000;
        // Normal models (including complex 30k-100k triangle meshes like the Utah teapot)
        // should never drop triangles or create checkerboard holes.
        assert_eq!(triangle_sample_step(0, budget), 1);
        assert_eq!(triangle_sample_step(1_000, budget), 1);
        assert_eq!(triangle_sample_step(15_000, budget), 1);
        assert_eq!(triangle_sample_step(30_000, budget), 1);
        assert_eq!(triangle_sample_step(60_000, budget), 1);
        assert_eq!(triangle_sample_step(120_000, budget), 1);

        // Safety fallback only kicks in for extreme multi-hundred-thousand facet scans
        assert_eq!(triangle_sample_step(240_000, budget), 2);
    }

    #[test]
    fn test_up_axis_defaults_and_toggle() {
        let stl_model = Model3d {
            name: "test.stl".to_string(),
            format: ModelFormat::Stl,
            triangles: vec![],
            thumbnail_triangles: vec![],
            min_bounds: Vec3::ZERO,
            max_bounds: Vec3::ZERO,
            center: Vec3::ZERO,
            size: Vec3::ZERO,
            max_dimension: 1.0,
        };
        let obj_model = Model3d {
            name: "test.obj".to_string(),
            format: ModelFormat::Obj,
            triangles: vec![],
            thumbnail_triangles: vec![],
            min_bounds: Vec3::ZERO,
            max_bounds: Vec3::ZERO,
            center: Vec3::ZERO,
            size: Vec3::ZERO,
            max_dimension: 1.0,
        };

        let stl_viewer = Model3dViewerState::from_model(stl_model, None);
        assert_eq!(stl_viewer.up_axis, UpAxis::ZUp);

        let obj_viewer = Model3dViewerState::from_model(obj_model, None);
        assert_eq!(obj_viewer.up_axis, UpAxis::YUp);
    }

    #[test]
    fn dropping_file_load_viewer_cancels_queued_parse_publication() {
        use std::sync::atomic::{AtomicBool, Ordering};

        let jobs = little_clip_core::BackgroundJobs::new(1, 1);
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let (_blocker_ticket, blocker_result) = jobs.submit(Duration::MAX, move || {
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        });
        started_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("worker should reach blocker");

        let parse_ran = std::sync::Arc::new(AtomicBool::new(false));
        let parse_ran_worker = std::sync::Arc::clone(&parse_ran);
        let (ticket, rx) = jobs.submit(Duration::MAX, move || {
            parse_ran_worker.store(true, Ordering::SeqCst);
            Ok(Arc::new(test_mesh()))
        });
        let mut viewer = Model3dViewerState::from_model(test_mesh(), None);
        viewer.status = ModelLoadingStatus::Loading;
        viewer.rx = Some(rx);
        viewer.load_ticket = Some(ticket);

        drop(viewer);
        release_tx.send(()).unwrap();
        assert_eq!(
            blocker_result
                .recv_timeout(Duration::from_secs(2))
                .expect("blocker should finish"),
            Ok(())
        );

        // A following accepted job completes only after the canceled parse has
        // been dequeued, making the non-execution assertion deterministic.
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        loop {
            let (_ticket, result) = jobs.submit(Duration::MAX, || ());
            match result.recv_timeout(Duration::from_millis(10)) {
                Ok(Ok(())) => break,
                Ok(Err(little_clip_core::JobError::QueueFull))
                | Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "executor did not admit the follow-up job"
                    );
                }
                result => panic!("unexpected follow-up job result: {result:?}"),
            }
        }
        assert!(!parse_ran.load(Ordering::SeqCst));
    }

    #[test]
    fn dropping_file_load_viewer_drops_running_parse_result() {
        use std::sync::atomic::{AtomicBool, Ordering};

        let jobs = little_clip_core::BackgroundJobs::new(1, 1);
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let parse_finished = std::sync::Arc::new(AtomicBool::new(false));
        let parse_finished_worker = std::sync::Arc::clone(&parse_finished);
        let (ticket, rx) = jobs.submit(Duration::MAX, move || {
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            let parsed = Arc::new(test_mesh());
            parse_finished_worker.store(true, Ordering::Release);
            Ok(parsed)
        });
        started_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("parse worker should be running before viewer drop");

        let mut viewer = Model3dViewerState::from_model(test_mesh(), None);
        viewer.status = ModelLoadingStatus::Loading;
        viewer.rx = Some(rx);
        viewer.load_ticket = Some(ticket);
        drop(viewer);

        release_tx
            .send(())
            .expect("running parse should be waiting for release");
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        loop {
            if parse_finished.load(Ordering::Acquire) {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "running parse did not finish after viewer drop"
            );
            std::thread::sleep(Duration::from_millis(2));
        }

        // A sentinel accepted after the parse proves its successful result
        // returned through the executor after the only receiver was dropped.
        let (_sentinel_ticket, sentinel_rx) = jobs.submit(Duration::MAX, || ());
        sentinel_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("executor should finish parse and sentinel")
            .expect("sentinel should run");
    }

    #[test]
    fn primary_drag_orbits_and_disables_auto_rotation() {
        let ctx = egui::Context::default();
        crate::ui_ext::configure_workbench_style(&ctx);
        let mut viewer = Model3dViewerState::from_model(test_mesh(), None);
        let canvas = render_interaction_frame(&ctx, &mut viewer, 0.0, Vec::new());
        let start = canvas.center();
        let end = start + Vec2::new(32.0, 20.0);

        render_interaction_frame(
            &ctx,
            &mut viewer,
            0.01,
            vec![
                egui::Event::PointerMoved(start),
                pointer_button_event(start, egui::PointerButton::Primary, true),
            ],
        );
        let yaw_before_drag = viewer.yaw;
        render_interaction_frame(
            &ctx,
            &mut viewer,
            0.02,
            vec![egui::Event::PointerMoved(end)],
        );
        render_interaction_frame(
            &ctx,
            &mut viewer,
            0.03,
            vec![pointer_button_event(
                end,
                egui::PointerButton::Primary,
                false,
            )],
        );

        assert!((viewer.pitch - 0.6).abs() < 0.02, "pitch={}", viewer.pitch);
        assert!(viewer.yaw > yaw_before_drag + 0.25, "yaw={}", viewer.yaw);
        assert!(
            !viewer.auto_rotate,
            "primary drag should pause auto-rotation"
        );
        let yaw_after_drag = viewer.yaw;
        render_interaction_frame(&ctx, &mut viewer, 0.04, Vec::new());
        assert_eq!(viewer.yaw, yaw_after_drag, "auto-rotation remains paused");
    }

    #[test]
    fn secondary_and_middle_drags_pan_without_orbiting() {
        for button in [egui::PointerButton::Secondary, egui::PointerButton::Middle] {
            let ctx = egui::Context::default();
            crate::ui_ext::configure_workbench_style(&ctx);
            let mut viewer = Model3dViewerState::from_model(test_mesh(), None);
            viewer.auto_rotate = false;
            let canvas = render_interaction_frame(&ctx, &mut viewer, 0.0, Vec::new());
            let start = canvas.center();
            let end = start + Vec2::new(24.0, -13.0);

            render_interaction_frame(
                &ctx,
                &mut viewer,
                0.01,
                vec![
                    egui::Event::PointerMoved(start),
                    pointer_button_event(start, button, true),
                ],
            );
            render_interaction_frame(
                &ctx,
                &mut viewer,
                0.02,
                vec![egui::Event::PointerMoved(end)],
            );
            render_interaction_frame(
                &ctx,
                &mut viewer,
                0.03,
                vec![pointer_button_event(end, button, false)],
            );

            assert_eq!(viewer.pan_offset, Vec2::new(24.0, -13.0), "{button:?}");
            assert_eq!(viewer.yaw, 0.6, "{button:?} drag must not orbit yaw");
            assert_eq!(viewer.pitch, 0.4, "{button:?} drag must not orbit pitch");
        }
    }

    #[test]
    fn hovered_scroll_zoom_clamps_to_documented_limits() {
        let ctx = egui::Context::default();
        crate::ui_ext::configure_workbench_style(&ctx);
        let mut viewer = Model3dViewerState::from_model(test_mesh(), None);
        viewer.auto_rotate = false;
        let canvas = render_interaction_frame(&ctx, &mut viewer, 0.0, Vec::new());
        let canvas_pointer = canvas.center();

        for index in 0..32 {
            render_interaction_frame(
                &ctx,
                &mut viewer,
                (index + 1) as f64 * 0.01,
                vec![
                    egui::Event::PointerMoved(canvas_pointer),
                    egui::Event::Scroll(Vec2::new(0.0, 20.0)),
                ],
            );
        }
        assert_eq!(viewer.zoom, 20.0, "scroll-in should clamp at 20×");

        for index in 0..48 {
            render_interaction_frame(
                &ctx,
                &mut viewer,
                0.33 + index as f64 * 0.01,
                vec![
                    egui::Event::PointerMoved(canvas_pointer),
                    egui::Event::Scroll(Vec2::new(0.0, -20.0)),
                ],
            );
        }
        assert_eq!(viewer.zoom, 0.1, "scroll-out should clamp at 0.1×");
    }

    #[test]
    fn toolbar_loading_and_error_copy_fit_compact_and_wide_hosts() {
        for (size_name, width) in [("compact", 420.0), ("wide", 1400.0)] {
            let mut ready = Model3dViewerState::from_model(test_mesh(), None);
            let toolbar_text = render_text_at_width(&mut ready, width);
            for required in ["Pause Rotation", "Wireframe", "Axis: Y-Up", "Reset View"] {
                let (text, bounds, elided, _) = toolbar_text
                    .iter()
                    .find(|(text, _, _, _)| text.contains(required))
                    .unwrap_or_else(|| panic!("{required:?} missing at {size_name} width {width}"));
                assert!(!elided, "{required:?} elided at {size_name} width {width}");
                assert!(
                    egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(width, 720.0))
                        .contains_rect(*bounds),
                    "{required:?} ({text:?}) outside host at {size_name} width {width}: {bounds:?}"
                );
            }

            let mut loading = Model3dViewerState::from_model(test_mesh(), None);
            loading.status = ModelLoadingStatus::Loading;
            let loading_text = render_text_at_width(&mut loading, width);
            let (_, loading_bounds, loading_elided, _) = loading_text
                .iter()
                .find(|(text, _, _, _)| text.contains("Parsing 3D mesh geometry with Scenix"))
                .unwrap_or_else(|| panic!("loading copy missing at {size_name} width {width}"));
            assert!(
                !loading_elided,
                "loading copy elided at {size_name} width {width}"
            );
            assert!(
                egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(width, 720.0))
                    .contains_rect(*loading_bounds),
                "loading copy outside host at {size_name} width {width}: {loading_bounds:?}"
            );

            let mut error = Model3dViewerState::from_model(test_mesh(), None);
            error.status = ModelLoadingStatus::Error(
                "Unsupported geometry format in this model file".to_owned(),
            );
            let error_text = render_text_at_width(&mut error, width);
            for required in ["Unable to load 3D model:", "Unsupported geometry format"] {
                let (_, bounds, elided, _) = error_text
                    .iter()
                    .find(|(text, _, _, _)| text.contains(required))
                    .unwrap_or_else(|| panic!("{required:?} missing at {size_name} width {width}"));
                assert!(!elided, "{required:?} elided at {size_name} width {width}");
                assert!(
                    egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(width, 720.0))
                        .contains_rect(*bounds),
                    "{required:?} outside host at {size_name} width {width}: {bounds:?}"
                );
            }
        }
    }

    #[test]
    fn ready_footer_fits_short_compact_and_wide_hosts() {
        let hint = "Drag: Orbit • Right/Mid: Pan • Scroll: Zoom • Esc: Close";
        for (size_name, width, height) in [("compact", 588.0, 331.0), ("wide", 736.0, 414.0)] {
            let mut ready = Model3dViewerState::from_model(test_mesh(), None);
            let texts = render_text_at_size(&mut ready, width, height);
            for required in [
                "Drag: Orbit",
                "Right/Mid: Pan",
                "Scroll: Zoom",
                "Esc: Close",
            ] {
                let (text, bounds, elided, clip_rect) = texts
                    .iter()
                    .find(|(text, _, _, _)| text.contains(required))
                    .unwrap_or_else(|| panic!("{required:?} missing at {size_name}"));
                assert!(!elided, "{required:?} elided at {size_name}: {text:?}");
                assert!(
                    egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(width, height))
                        .contains_rect(*bounds),
                    "{required:?} outside {size_name} host: {bounds:?}"
                );
                assert!(
                    clip_rect.contains_rect(*bounds),
                    "{required:?} clipped at {size_name}: bounds={bounds:?}, clip={clip_rect:?}"
                );
            }

            assert_eq!(
                hint,
                texts
                    .iter()
                    .find(|(text, _, _, _)| text == hint)
                    .map(|(text, _, _, _)| text.as_str())
                    .unwrap_or_else(|| panic!("complete hint missing at {size_name}"))
            );
            for required in ["Pitch", "Yaw", "Zoom"] {
                let (text, bounds, elided, clip_rect) = texts
                    .iter()
                    .find(|(text, _, _, _)| text.contains(required))
                    .unwrap_or_else(|| panic!("{required:?} readout missing at {size_name}"));
                assert!(
                    !elided,
                    "{required:?} readout elided at {size_name}: {text:?}"
                );
                assert!(
                    egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(width, height))
                        .contains_rect(*bounds),
                    "{required:?} readout outside {size_name} host: {bounds:?}"
                );
                assert!(
                    clip_rect.contains_rect(*bounds),
                    "{required:?} readout clipped at {size_name}: bounds={bounds:?}, clip={clip_rect:?}"
                );
            }
        }
    }
}
