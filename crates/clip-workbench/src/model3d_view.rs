//! 3D model inspection and orbital viewport workbench view.
//!
//! Renders 3D triangle meshes (STL, OBJ, glTF/GLB) using software projection,
//! painter's algorithm depth sorting, directional shading with material tinting,
//! wireframe mode, and background async asset parsing.

use crate::shell::{EscapeHatch, EscapeHatchLauncher, StudioShell};
use crate::ui_ext::StatusToast;
use eframe::egui::{self, Color32, Pos2, Rounding, Stroke, Vec2};
use little_clip_core::presentation::model3d::{Model3d, ModelFormat, Vec3};
use little_clip_core::theme::AppPalette;
use std::path::PathBuf;
use std::sync::mpsc::Receiver;

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

/// Interactive 3D orbital viewer state for 3D meshes (STL, OBJ, glTF).
pub struct Model3dViewerState {
    pub path: Option<PathBuf>,
    pub title: String,
    pub status: ModelLoadingStatus,
    pub mesh: Option<Model3d>,
    pub rx: Option<Receiver<Result<Result<Model3d, String>, little_clip_core::JobError>>>,
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
        let (_, rx) = little_clip_core::BackgroundJobs::global()
            .submit(std::time::Duration::from_secs(30), move || {
                Model3d::load_file(&load_path)
            });

        Self {
            path: Some(path),
            title,
            status: ModelLoadingStatus::Loading,
            mesh: None,
            rx: Some(rx),
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

    /// Constructs an immediate ready viewer from an already-parsed model.
    pub fn from_model(mesh: Model3d, path: Option<PathBuf>) -> Self {
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

    /// Parses raw bytes synchronously for a given extension.
    #[allow(dead_code)]
    pub fn from_bytes(bytes: &[u8], ext: &str, title: String) -> Result<Self, String> {
        let mesh = Model3d::load_bytes(bytes, ext, title)?;
        Ok(Self::from_model(mesh, None))
    }

    #[allow(dead_code)]
    pub fn reset_view(&mut self) {
        self.pitch = 0.4;
        self.yaw = 0.6;
        self.zoom = 1.0;
        self.pan_offset = Vec2::ZERO;
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

        let bg = crate::hex_to_c32(&palette.theme.background);
        let b_low = crate::hex_to_c32(&palette.theme.b_low);
        let f_high = crate::hex_to_c32(&palette.theme.f_high);

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
        ui.ctx().request_repaint();
        egui::Frame::none()
            .fill(bg)
            .inner_margin(egui::Margin::same(12.0))
            .show(ui, |ui| {
                let mut shell = StudioShell::new(&self.title, "🧊", palette, close)
                    .with_badge("Parsing 3D Model...".to_string(), None)
                    .with_footer_hints("Loading asset geometry into memory...");
                let _ = shell.render_header(ui, |_ui| {});

                ui.add_space(20.0);
                let available_size = ui.available_size();
                let (rect, _) = ui.allocate_exact_size(available_size, egui::Sense::hover());
                let painter = ui.painter_at(rect);
                painter.rect_filled(rect, Rounding::same(6.0), b_low);

                let center = rect.center();
                let text = "⏳ Parsing 3D mesh geometry with Scenix...";
                painter.text(
                    center,
                    egui::Align2::CENTER_CENTER,
                    text,
                    egui::FontId::proportional(16.0),
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
            .inner_margin(egui::Margin::same(12.0))
            .show(ui, |ui| {
                let mut shell = StudioShell::new(&self.title, "⚠️", palette, close)
                    .with_badge("Failed to load".to_string(), None)
                    .with_escape_hatch(EscapeHatch::primary(
                        "🧊 Blender",
                        "blender",
                        "Open 3D model in Blender / FreeCAD",
                    ))
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
                painter.rect_filled(rect, Rounding::same(6.0), b_low);

                let text = format!("⚠️ Unable to load 3D model:\n\n{}", err);
                painter.text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    text,
                    egui::FontId::proportional(15.0),
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

        let b_med = crate::hex_to_c32(&palette.theme.b_med);
        let f_high = crate::hex_to_c32(&palette.theme.f_high);
        let f_med = crate::hex_to_c32(&palette.theme.f_med);
        let accent = crate::hex_to_c32(&palette.harmonies.base);

        let mut auto_rot_clicked = false;
        let mut wire_clicked = false;
        let mut reset_clicked = false;

        egui::Frame::none()
            .fill(bg)
            .inner_margin(egui::Margin::same(12.0))
            .show(ui, |ui| {
                let info_badge = format!(
                    "{} • {} triangles • {:.1}×{:.1}×{:.1} mm",
                    format_badge, tri_count, size_x, size_y, size_z
                );

                let mut shell = StudioShell::new(&self.title, "🧊", palette, close)
                    .with_badge(info_badge, None)
                    .with_escape_hatch(EscapeHatch::primary(
                        "🧊 Blender",
                        "blender",
                        "Open 3D model in Blender / FreeCAD",
                    ))
                    .with_footer_hints(
                        "Left Drag: Orbit • Right/Mid Drag: Pan • Scroll: Zoom • Esc: Close",
                    )
                    .with_footer_readout(format!(
                        "Pitch: {:.1}° • Yaw: {:.1}° • Zoom: {:.1}x",
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
                            .set("⚠️ No on-disk file path for this 3D model", now);
                    }
                }

                ui.add_space(4.0);

                // Toolbar
                egui::Frame::none()
                    .fill(b_low)
                    .stroke(Stroke::new(1.0, b_med))
                    .inner_margin(egui::Margin::symmetric(14.0, 6.0))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let auto_rot_bg = if auto_rotate { b_med } else { accent };
                            let auto_rot_fg =
                                crate::contrast_text_c32(auto_rot_bg, Some(f_high), palette);
                            let auto_rot_btn = egui::Button::new(
                                egui::RichText::new(if auto_rotate {
                                    "⏸ Pause Rotation"
                                } else {
                                    "▶ Auto-Rotate"
                                })
                                .small()
                                .strong()
                                .color(auto_rot_fg),
                            )
                            .fill(auto_rot_bg)
                            .stroke(Stroke::new(1.0, b_low))
                            .rounding(Rounding::same(4.0));
                            if ui.add(auto_rot_btn).clicked() {
                                auto_rot_clicked = true;
                            }

                            let wire_bg = if wireframe { accent } else { b_med };
                            let wire_fg = crate::contrast_text_c32(wire_bg, Some(f_high), palette);
                            let wire_btn = egui::Button::new(
                                egui::RichText::new(if wireframe {
                                    "🎨 Shaded"
                                } else {
                                    "📐 Wireframe"
                                })
                                .small()
                                .strong()
                                .color(wire_fg),
                            )
                            .fill(wire_bg)
                            .stroke(Stroke::new(1.0, b_low))
                            .rounding(Rounding::same(4.0));
                            if ui.add(wire_btn).clicked() {
                                wire_clicked = true;
                            }

                            let axis_label = match self.up_axis {
                                UpAxis::ZUp => "⬆ Axis: Z-Up",
                                UpAxis::YUp => "⬆ Axis: Y-Up",
                            };
                            let axis_fg = crate::contrast_text_c32(b_med, Some(f_high), palette);
                            let axis_btn = egui::Button::new(
                                egui::RichText::new(axis_label)
                                    .small()
                                    .strong()
                                    .color(axis_fg),
                            )
                            .fill(b_med)
                            .stroke(Stroke::new(1.0, b_low))
                            .rounding(Rounding::same(4.0));
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

                            let reset_fg = crate::contrast_text_c32(b_med, Some(f_med), palette);
                            let reset_btn = egui::Button::new(
                                egui::RichText::new("↺ Reset View").small().color(reset_fg),
                            )
                            .fill(b_med)
                            .stroke(Stroke::new(1.0, b_low))
                            .rounding(Rounding::same(4.0));
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

                ui.add_space(4.0);
                ui.separator();
                ui.add_space(4.0);

                // Interactive 3D Canvas
                let mut available_size = ui.available_size();
                available_size.y = (available_size.y - 32.0).max(100.0);
                let (canvas_rect, response) =
                    ui.allocate_exact_size(available_size, egui::Sense::click_and_drag());
                let painter = ui.painter_at(canvas_rect);

                // Background dark canvas
                painter.rect_filled(canvas_rect, Rounding::same(6.0), b_low);

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

                let toast_tuple = self.status_toast.message(now).map(|m| (m.to_string(), now));
                shell.render_footer(ui, &toast_tuple, now);
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
        let stroke = Stroke::new(1.0, accent);
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
