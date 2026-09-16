//! 3D STL mesh inspection and orbital viewport workbench view.
//!
//! Renders 3D triangle meshes using software projection, painter's algorithm depth sorting,
//! directional shading, wireframe mode, and auto-rotation.

use crate::shell::{EscapeHatch, EscapeHatchLauncher, StudioShell};
use crate::ui_ext::StatusToast;
use eframe::egui::{self, Color32, Pos2, Rounding, Stroke, Vec2};
use little_clip_ops::{StlMesh, Vec3};
use std::path::PathBuf;

/// Caps the number of facets submitted per frame; this is intentionally pure
/// so a regression test can protect interactive frame cost without a GPU.
fn triangle_sample_step(total_triangles: usize, dragging: bool) -> usize {
    let budget = if dragging { 5_000 } else { 15_000 };
    total_triangles.div_ceil(budget).max(1)
}

/// Interactive 3D orbital viewer state for STL meshes.
pub struct StlViewerState {
    pub path: Option<PathBuf>,
    pub title: String,
    pub mesh: StlMesh,
    pub pitch: f32, // X rotation (radians)
    pub yaw: f32,   // Y rotation (radians)
    pub zoom: f32,
    pub pan_offset: Vec2,
    pub auto_rotate: bool,
    pub wireframe: bool,
    pub status_toast: StatusToast,
}

impl StlViewerState {
    /// Loads an STL mesh from disk into orbital viewer state.
    pub fn from_file(path: PathBuf) -> Result<Self, String> {
        let mesh = StlMesh::parse_file(&path)?;
        let title = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("3D Model")
            .to_string();

        Ok(Self {
            path: Some(path),
            title,
            mesh,
            pitch: 0.4,
            yaw: 0.6,
            zoom: 1.0,
            pan_offset: Vec2::ZERO,
            auto_rotate: true,
            wireframe: true, // User prefers wireframe by default
            status_toast: StatusToast::new(),
        })
    }

    #[allow(dead_code)]
    pub fn from_bytes(bytes: &[u8], title: String) -> Result<Self, String> {
        let mesh = StlMesh::parse_bytes(bytes, title.clone())?;
        Ok(Self {
            path: None,
            title,
            mesh,
            pitch: 0.4,
            yaw: 0.6,
            zoom: 1.0,
            pan_offset: Vec2::ZERO,
            auto_rotate: true,
            wireframe: true, // User prefers wireframe by default
            status_toast: StatusToast::new(),
        })
    }

    pub fn reset_view(&mut self) {
        self.pitch = 0.4;
        self.yaw = 0.6;
        self.zoom = 1.0;
        self.pan_offset = Vec2::ZERO;
    }

    pub fn render_ui(
        &mut self,
        ui: &mut egui::Ui,
        palette: &little_clip_core::theme::AppPalette,
        close: &mut bool,
    ) {
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            *close = true;
            return;
        }

        // Frame-rate independent auto-rotation
        let dt = ui.input(|i| i.stable_dt).min(0.1);
        if self.auto_rotate {
            self.yaw += 0.9 * dt;
            ui.ctx().request_repaint();
        }

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs_f64();

        let bg = crate::hex_to_c32(&palette.theme.background);
        let b_low = crate::hex_to_c32(&palette.theme.b_low);
        let b_med = crate::hex_to_c32(&palette.theme.b_med);
        let f_high = crate::hex_to_c32(&palette.theme.f_high);
        let f_med = crate::hex_to_c32(&palette.theme.f_med);
        let _f_low = crate::hex_to_c32(&palette.theme.f_low);
        let accent = crate::hex_to_c32(&palette.harmonies.base);

        egui::Frame::none()
            .fill(bg)
            .inner_margin(egui::Margin::same(12.0))
            .show(ui, |ui| {
                let tri_count = self.mesh.triangles.len();
                let size_x = self.mesh.size.x;
                let size_y = self.mesh.size.y;
                let size_z = self.mesh.size.z;
                let auto_rotate = self.auto_rotate;
                let wireframe = self.wireframe;

                let mut auto_rot_clicked = false;
                let mut wire_clicked = false;
                let mut reset_clicked = false;

                let info_badge = format!(
                    "{} triangles • {:.1}×{:.1}×{:.1} mm",
                    tri_count, size_x, size_y, size_z
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
                            .set("⚠️ No on-disk file path for this STL model", now);
                    }
                }

                ui.add_space(4.0);

                // Zone 2b: 3D Toolbar
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
                        });
                    });

                if auto_rot_clicked {
                    self.auto_rotate = !self.auto_rotate;
                }
                if wire_clicked {
                    self.wireframe = !self.wireframe;
                }
                if reset_clicked {
                    self.reset_view();
                }

                ui.add_space(4.0);
                ui.separator();
                ui.add_space(4.0);

                // 2. Interactive 3D Canvas
                let mut available_size = ui.available_size();
                available_size.y = (available_size.y - 32.0).max(100.0);
                let (canvas_rect, response) =
                    ui.allocate_exact_size(available_size, egui::Sense::click_and_drag());
                let painter = ui.painter_at(canvas_rect);

                // Background dark canvas
                painter.rect_filled(canvas_rect, Rounding::same(6.0), b_low);

                // Mouse interaction for orbit & pan
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

                // Project & render 3D mesh
                let center = canvas_rect.center() + self.pan_offset;
                let max_dim = self
                    .mesh
                    .size
                    .x
                    .max(self.mesh.size.y)
                    .max(self.mesh.size.z)
                    .max(0.001);
                let base_scale = (canvas_rect.width().min(canvas_rect.height()) * 0.4) / max_dim;
                let scale = base_scale * self.zoom;

                // Rotation matrices (Yaw around Y, Pitch around X)
                let cy = self.yaw.cos();
                let sy = self.yaw.sin();
                let cx = self.pitch.cos();
                let sx = self.pitch.sin();

                let project = |v: &Vec3| -> (Pos2, f32) {
                    let off = Vec3::new(
                        v.x - self.mesh.center.x,
                        v.y - self.mesh.center.y,
                        v.z - self.mesh.center.z,
                    );
                    let rx = off.x * cy + off.z * sy;
                    let rz1 = -off.x * sy + off.z * cy;
                    let ry = off.y * cx - rz1 * sx;
                    let rz = off.y * sx + rz1 * cx;

                    let px = center.x + rx * scale;
                    let py = center.y - ry * scale;
                    (Pos2::new(px, py), rz)
                };

                let total_tris = self.mesh.triangles.len();
                // Painter's algorithm rendering is CPU bound. Keep the visual
                // triangle budget stable at interactive rates; dragging uses a
                // lower temporary budget and restores detail on release.
                let skip_step = triangle_sample_step(total_tris, response.dragged());

                let mut projected_tris = Vec::with_capacity(total_tris / skip_step + 1);
                for i in (0..total_tris).step_by(skip_step) {
                    let t = &self.mesh.triangles[i];
                    let (p1, z1) = project(&t.v1);
                    let (p2, z2) = project(&t.v2);
                    let (p3, z3) = project(&t.v3);
                    let avg_z = (z1 + z2 + z3) / 3.0;

                    let nx = t.normal.x * cy + t.normal.z * sy;
                    let nz1 = -t.normal.x * sy + t.normal.z * cy;
                    let ny = t.normal.y * cx - nz1 * sx;
                    let nz = t.normal.y * sx + nz1 * cx;
                    let rotated_normal = Vec3::new(nx, ny, nz);

                    // Back-face culling avoids projection, sort, and paint work
                    // for triangles that cannot contribute to this orthographic view.
                    if rotated_normal.z <= 0.0 {
                        continue;
                    }

                    projected_tris.push((avg_z, [p1, p2, p3], rotated_normal));
                }

                projected_tris
                    .sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

                let light_dir = Vec3::new(0.4, 0.7, 0.6).normalize();

                if self.wireframe {
                    let stroke = Stroke::new(1.0, accent);
                    for (_, pts, _) in &projected_tris {
                        painter.line_segment([pts[0], pts[1]], stroke);
                        painter.line_segment([pts[1], pts[2]], stroke);
                        painter.line_segment([pts[2], pts[0]], stroke);
                    }
                } else {
                    let mut mesh_shape = egui::epaint::Mesh::default();
                    for (_, pts, norm) in &projected_tris {
                        let dot =
                            (norm.x * light_dir.x + norm.y * light_dir.y + norm.z * light_dir.z)
                                .max(0.0);
                        let ambient = 0.25;
                        let intensity = (ambient + dot * 0.75).clamp(0.0, 1.0);

                        let (br, bg_val, bb) =
                            (accent.r() as f32, accent.g() as f32, accent.b() as f32);
                        let face_color = Color32::from_rgb(
                            (br * intensity) as u8,
                            (bg_val * intensity) as u8,
                            (bb * intensity) as u8,
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

                // 4. Footer
                let toast_tuple = self.status_toast.message(now).map(|m| (m.to_string(), now));
                shell.render_footer(ui, &toast_tuple, now);
            });
    }
}

#[cfg(test)]
mod tests {
    use super::triangle_sample_step;

    #[test]
    fn large_meshes_have_a_bounded_interactive_frame_budget() {
        let total = 1_000_000;
        let idle_step = triangle_sample_step(total, false);
        let drag_step = triangle_sample_step(total, true);
        assert!(total.div_ceil(idle_step) <= 15_000);
        assert!(total.div_ceil(drag_step) <= 5_000);
        assert!(drag_step >= idle_step);
    }
}
