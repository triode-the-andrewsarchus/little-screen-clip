//! 3D STL mesh inspection and orbital viewport workbench view.
//!
//! Renders 3D triangle meshes using software projection, painter's algorithm depth sorting,
//! directional shading, wireframe mode, and auto-rotation.

use eframe::egui::{self, Color32, Pos2, Rounding, Stroke, Vec2};
use little_clip_ops::{StlMesh, Vec3};
use std::path::PathBuf;

/// Interactive 3D orbital viewer state for STL meshes.
pub struct StlViewerState {
    #[allow(dead_code)]
    pub path: Option<PathBuf>,
    pub title: String,
    pub mesh: StlMesh,
    pub pitch: f32, // X rotation (radians)
    pub yaw: f32,   // Y rotation (radians)
    pub zoom: f32,
    pub pan_offset: Vec2,
    pub auto_rotate: bool,
    pub wireframe: bool,
    pub status_toast: Option<(String, f64)>,
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
            status_toast: None,
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
            status_toast: None,
        })
    }

    pub fn reset_view(&mut self) {
        self.pitch = 0.4;
        self.yaw = 0.6;
        self.zoom = 1.0;
        self.pan_offset = Vec2::ZERO;
    }

    pub fn render_ui(&mut self, ui: &mut egui::Ui, palette: &little_clip_core::theme::AppPalette, close: &mut bool) {
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            *close = true;
            return;
        }

        if self.auto_rotate {
            self.yaw += 0.015;
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
        let f_low = crate::hex_to_c32(&palette.theme.f_low);
        let accent = crate::hex_to_c32(&palette.harmonies.base);
        let success_accent = crate::hex_to_c32(&palette.diff_add);

        egui::Frame::none()
            .fill(bg)
            .inner_margin(egui::Margin::same(12.0))
            .show(ui, |ui| {
                // 1. Top Control Bar
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("🧊").size(20.0));
                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new(&self.title)
                            .strong()
                            .size(18.0)
                            .color(f_high),
                    );

                    ui.add_space(8.0);
                    let info_badge = format!(
                        "{} triangles • {:.1}×{:.1}×{:.1} mm",
                        self.mesh.triangles.len(),
                        self.mesh.size.x,
                        self.mesh.size.y,
                        self.mesh.size.z
                    );
                    ui.label(
                        egui::RichText::new(info_badge)
                            .small()
                            .color(f_low),
                    );

                    ui.add_space(16.0);
                    // Controls
                    let auto_rot_btn = egui::Button::new(
                        egui::RichText::new(if self.auto_rotate { "⏸ Pause Rotation" } else { "▶ Auto-Rotate" })
                            .small()
                            .strong()
                            .color(f_high),
                    )
                    .fill(if self.auto_rotate { b_med } else { accent })
                    .stroke(Stroke::new(1.0, b_low))
                    .rounding(Rounding::same(4.0));
                    if ui.add(auto_rot_btn).clicked() {
                        self.auto_rotate = !self.auto_rotate;
                    }

                    let wire_btn = egui::Button::new(
                        egui::RichText::new(if self.wireframe { "🎨 Shaded" } else { "📐 Wireframe" })
                            .small()
                            .strong()
                            .color(f_high),
                    )
                    .fill(if self.wireframe { accent } else { b_med })
                    .stroke(Stroke::new(1.0, b_low))
                    .rounding(Rounding::same(4.0));
                    if ui.add(wire_btn).clicked() {
                        self.wireframe = !self.wireframe;
                    }

                    let reset_btn = egui::Button::new(
                        egui::RichText::new("↺ Reset View")
                            .small()
                            .color(f_med),
                    )
                    .fill(b_med)
                    .stroke(Stroke::new(1.0, b_low))
                    .rounding(Rounding::same(4.0));
                    if ui.add(reset_btn).clicked() {
                        self.reset_view();
                    }

                    // Right aligned Close button
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let close_btn = egui::Button::new(
                            egui::RichText::new("✕ Close (Esc)")
                                .strong()
                                .color(crate::hex_to_c32(&palette.undo_border)),
                        )
                        .fill(crate::hex_to_c32(&palette.undo_card_bg))
                        .stroke(Stroke::new(1.0, crate::hex_to_c32(&palette.undo_border)))
                        .rounding(Rounding::same(6.0));
                        if ui.add(close_btn).clicked() {
                            *close = true;
                        }

                        if let Some((msg, ts)) = &self.status_toast {
                            if now - ts < 4.0 {
                                ui.label(
                                    egui::RichText::new(msg)
                                        .small()
                                        .color(success_accent),
                                );
                            }
                        }
                    });
                });

                ui.add_space(8.0);
                ui.separator();
                ui.add_space(8.0);

                // 2. Interactive 3D Canvas
                let available_size = ui.available_size();
                let (canvas_rect, response) = ui.allocate_exact_size(available_size, egui::Sense::click_and_drag());
                let painter = ui.painter_at(canvas_rect);

                // Background dark canvas
                painter.rect_filled(canvas_rect, Rounding::same(6.0), b_low);
                painter.rect_stroke(canvas_rect, Rounding::same(6.0), Stroke::new(1.0, b_med));

                // Handle mouse interaction
                if response.dragged_by(egui::PointerButton::Primary) {
                    let delta = response.drag_delta();
                    self.yaw += delta.x * 0.01;
                    self.pitch = (self.pitch + delta.y * 0.01).clamp(-1.5, 1.5);
                    self.auto_rotate = false;
                } else if response.dragged_by(egui::PointerButton::Secondary) || response.dragged_by(egui::PointerButton::Middle) {
                    let delta = response.drag_delta();
                    self.pan_offset += delta;
                }

                // Handle zoom with scroll wheel
                let scroll_delta = ui.input(|i| i.raw_scroll_delta.y);
                if scroll_delta != 0.0 && response.hovered() {
                    let zoom_factor = if scroll_delta > 0.0 { 1.1 } else { 0.9 };
                    self.zoom = (self.zoom * zoom_factor).clamp(0.1, 20.0);
                }

                // 3D Projection & Rendering
                let cx = canvas_rect.center().x + self.pan_offset.x;
                let cy = canvas_rect.center().y + self.pan_offset.y;

                let sin_yaw = self.yaw.sin();
                let cos_yaw = self.yaw.cos();
                let sin_pitch = self.pitch.sin();
                let cos_pitch = self.pitch.cos();

                let center = self.mesh.center;
                let max_dim = self.mesh.max_dimension.max(0.001);
                let base_scale = (canvas_rect.width().min(canvas_rect.height()) * 0.45) / (max_dim * 0.5);
                let scale = base_scale * self.zoom;

                let light_dir = Vec3::new(0.4, 0.6, 0.7).normalize();

                // Stable detail scaling: target ~60,000 triangles to maintain high interactive framerates
                // without geometric popping or flickering when transitioning between dragged and static states.
                let step = (self.mesh.triangles.len() / 60000).max(1);

                let mut projected: Vec<(f32, f32, [Pos2; 3])> = Vec::with_capacity(self.mesh.triangles.len() / step);

                let transform_v = |v: Vec3| -> (Pos2, f32) {
                    let dx = v.x - center.x;
                    let dy = v.y - center.y;
                    let dz = v.z - center.z;

                    // Rotate around Y (Yaw)
                    let rx = dx * cos_yaw + dz * sin_yaw;
                    let rz = -dx * sin_yaw + dz * cos_yaw;
                    let ry = dy;

                    // Rotate around X (Pitch)
                    let fy = ry * cos_pitch - rz * sin_pitch;
                    let fz = ry * sin_pitch + rz * cos_pitch;

                    (Pos2::new(cx + rx * scale, cy - fy * scale), fz)
                };

                for (idx, tri) in self.mesh.triangles.iter().enumerate() {
                    if idx % step != 0 {
                        continue;
                    }

                    // Rotated normal lighting & camera-space front-face culling
                    let normal = tri.normal;
                    let rotated_normal = Vec3::new(
                        normal.x * cos_yaw + normal.z * sin_yaw,
                        normal.y * cos_pitch - (-normal.x * sin_yaw + normal.z * cos_yaw) * sin_pitch,
                        normal.y * sin_pitch + (-normal.x * sin_yaw + normal.z * cos_yaw) * cos_pitch,
                    ).normalize();

                    // Discard faces oriented away from camera (camera-space front-facing test)
                    if rotated_normal.z <= 0.02 {
                        continue;
                    }

                    let (p1, z1) = transform_v(tri.v1);
                    let (p2, z2) = transform_v(tri.v2);
                    let (p3, z3) = transform_v(tri.v3);

                    let avg_z = (z1 + z2 + z3) / 3.0;
                    let diff = rotated_normal.dot(light_dir).max(0.15).min(1.0);
                    projected.push((avg_z, diff, [p1, p2, p3]));
                }

                // Painter's algorithm depth sort
                projected.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

                if self.wireframe {
                    let stroke = Stroke::new(1.0, accent);
                    for (_, _, pts) in projected {
                        painter.line_segment([pts[0], pts[1]], stroke);
                        painter.line_segment([pts[1], pts[2]], stroke);
                        painter.line_segment([pts[2], pts[0]], stroke);
                    }
                } else {
                    let mut mesh_shape = egui::Mesh::default();
                    mesh_shape.reserve_triangles(projected.len());
                    mesh_shape.reserve_vertices(projected.len() * 3);

                    for (_, diff, pts) in projected {
                        let r = ((accent.r() as f32) * (0.2 + 0.8 * diff)).clamp(0.0, 255.0) as u8;
                        let g = ((accent.g() as f32) * (0.2 + 0.8 * diff)).clamp(0.0, 255.0) as u8;
                        let b = ((accent.b() as f32) * (0.2 + 0.8 * diff)).clamp(0.0, 255.0) as u8;
                        let face_color = Color32::from_rgb(r, g, b);

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
                        mesh_shape.indices.extend_from_slice(&[idx, idx + 1, idx + 2]);
                    }

                    painter.add(egui::Shape::mesh(mesh_shape));
                }

                // Navigation help in bottom-left corner
                let help_text = "Left Drag: Orbit • Right/Mid Drag: Pan • Scroll: Zoom";
                painter.text(
                    Pos2::new(canvas_rect.min.x + 12.0, canvas_rect.max.y - 14.0),
                    egui::Align2::LEFT_BOTTOM,
                    help_text,
                    egui::FontId::proportional(11.0),
                    f_low,
                );
            });
    }
}
