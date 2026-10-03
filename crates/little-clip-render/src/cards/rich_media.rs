use super::*;

impl ScreenRenderer {
    pub(super) fn draw_midi_card(
        &self,
        pixmap: &mut Pixmap,
        midi: &midi::MidiData,
        rx: f32,
        ry: f32,
        rw: f32,
        rh: f32,
        is_hero: bool,
    ) {
        let content_x = rx + 14.0;
        let clip_right = rx + rw - 10.0;
        let mut y = ry + 124.0;

        let badge_bg = self.color_from_hex(&self.palette.harmonies.base);
        let badge_fg = self.contrast_color(
            badge_bg,
            Some(self.color_from_hex(&self.palette.colors.text_primary)),
        );
        self.draw_pill_badge(
            pixmap,
            "MIDI PIANO ROLL",
            content_x,
            y,
            14.0,
            badge_fg,
            badge_bg,
            clip_right,
        );
        y += 28.0;

        let info = format!(
            "{} • {} notes • {:.1}s • {:.0} BPM",
            midi.title, midi.total_notes, midi.duration_sec, midi.bpm
        );
        self.draw_text_clipped(
            pixmap,
            &info,
            content_x,
            y,
            if is_hero { 16.0 } else { 13.0 },
            self.color_from_hex(&self.palette.colors.text_secondary),
            false,
            clip_right,
        );
        y += 22.0;

        let roll_x = content_x;
        let roll_y = y;
        let roll_w = clip_right - content_x;
        let roll_h = (ry + rh - 30.0 - roll_y).max(40.0);

        if let Some(_rect) = self.draw_card_viewport_frame(pixmap, roll_x, roll_y, roll_w, roll_h) {
            let kb_w = 16.0f32;
            if let Some(kb_rect) = Rect::from_xywh(roll_x, roll_y, kb_w, roll_h) {
                let mut kb_paint = Paint::default();
                kb_paint.set_color(self.color_from_hex(&self.palette.colors.surface_panel));
                pixmap.fill_rect(kb_rect, &kb_paint, Transform::identity(), None);
            }

            let min_k = midi.min_key.min(midi.max_key) as f32;
            let max_k = midi.max_key.max(midi.min_key + 1) as f32;
            let key_span = (max_k - min_k + 1.0).max(1.0);
            let dur_span = midi.duration_sec.max(0.1);

            let grid_x = roll_x + kb_w + 2.0;
            let grid_w = roll_w - kb_w - 4.0;

            for note in &midi.notes {
                let norm_start = (note.start_sec / dur_span).clamp(0.0, 1.0);
                let norm_dur = (note.duration_sec / dur_span).clamp(0.005, 1.0);
                let norm_pitch = ((note.key as f32 - min_k) / key_span).clamp(0.0, 1.0);

                let nx = grid_x + norm_start * grid_w;
                let nw = (norm_dur * grid_w).max(3.0).min(grid_x + grid_w - nx);
                let note_h = (roll_h / key_span).clamp(3.0, 12.0);
                let ny = roll_y + (1.0 - norm_pitch) * (roll_h - note_h);

                if let Some(n_rect) = Rect::from_xywh(nx, ny, nw, note_h) {
                    let mut n_paint = Paint::default();
                    let (r, g, b) = self.palette.midi_note_color(
                        note.track,
                        note.channel,
                        midi.track_count,
                        note.key,
                        note.velocity,
                    );
                    n_paint.set_color(Color::from_rgba8(r, g, b, 240));
                    pixmap.fill_rect(n_rect, &n_paint, Transform::identity(), None);
                }
            }
        }

        let footer_y = ry + rh - 22.0;
        self.draw_text_clipped(
            pixmap,
            "> Space in Workbench to play audio & visualize",
            content_x,
            footer_y,
            12.0,
            self.color_from_hex(&self.palette.colors.text_muted),
            false,
            clip_right,
        );
    }

    pub(super) fn draw_model3d_card(
        &self,
        pixmap: &mut Pixmap,
        mesh: &model3d::Model3d,
        rx: f32,
        ry: f32,
        rw: f32,
        rh: f32,
        is_hero: bool,
    ) {
        let content_x = rx + 14.0;
        let clip_right = rx + rw - 10.0;
        let mut y = ry + 124.0;

        let badge_bg = self.color_from_hex(&self.palette.diff_mod);
        let badge_fg = self.contrast_color(
            badge_bg,
            Some(self.color_from_hex(&self.palette.colors.text_primary)),
        );
        let badge_label = format!("3D {} MODEL", mesh.format.badge_label());
        self.draw_pill_badge(
            pixmap,
            &badge_label,
            content_x,
            y,
            14.0,
            badge_fg,
            badge_bg,
            clip_right,
        );
        y += 28.0;

        let info = format!(
            "{} • {} facets • {:.1}×{:.1}×{:.1}mm",
            mesh.name,
            mesh.triangles.len(),
            mesh.size.x,
            mesh.size.y,
            mesh.size.z
        );
        self.draw_text_clipped(
            pixmap,
            &info,
            content_x,
            y,
            if is_hero { 16.0 } else { 13.0 },
            self.color_from_hex(&self.palette.colors.text_secondary),
            false,
            clip_right,
        );
        y += 22.0;

        let vp_x = content_x;
        let vp_y = y;
        let vp_w = clip_right - content_x;
        let vp_h = (ry + rh - 30.0 - vp_y).max(40.0);

        if let Some(_rect) = self.draw_card_viewport_frame(pixmap, vp_x, vp_y, vp_w, vp_h) {
            let now_millis = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0);
            // Continuous, perfectly smooth 360-degree rotation every 12 seconds with sub-millisecond precision
            let angle_y = ((now_millis % 12_000) as f32 / 12_000.0) * std::f32::consts::TAU;
            self.draw_model3d_viewport(pixmap, mesh, [vp_x, vp_y, vp_w, vp_h], angle_y);
        }

        let footer_y = ry + rh - 22.0;
        self.draw_text_clipped(
            pixmap,
            "> Open in Workbench to orbit 3D model",
            content_x,
            footer_y,
            12.0,
            self.color_from_hex(&self.palette.colors.text_muted),
            false,
            clip_right,
        );
    }
    pub(crate) fn draw_model3d_viewport(
        &self,
        pixmap: &mut Pixmap,
        mesh: &model3d::Model3d,
        bounds: [f32; 4],
        angle_y: f32,
    ) {
        let [vp_x, vp_y, vp_w, vp_h] = bounds;
        let tilt_x = 0.35f32;

        let sin_y = angle_y.sin();
        let cos_y = angle_y.cos();
        let sin_x = tilt_x.sin();
        let cos_x = tilt_x.cos();

        let is_z_up = mesh.format == model3d::ModelFormat::Stl;
        let center = mesh.center;
        let max_dim = mesh.max_dimension.max(0.001);
        let scale = (vp_w.min(vp_h) * 0.42) / (max_dim * 0.5);

        let cx = vp_x + vp_w * 0.5;
        let cy = vp_y + vp_h * 0.5;
        let light_dir = model3d::Vec3::new(0.4, 0.6, 0.7).normalize();

        let budget = self.config.model3d.preview_triangle_limit.max(100);
        let source_triangles = if !mesh.thumbnail_triangles.is_empty() {
            &mesh.thumbnail_triangles
        } else {
            &mesh.triangles
        };
        let step = source_triangles.len().div_ceil(budget).max(1);

        let mut projected = Vec::with_capacity(source_triangles.len() / step + 1);
        for tri in source_triangles.iter().step_by(step) {
            let (nx, ny, nz) = if is_z_up {
                (tri.normal.x, tri.normal.z, -tri.normal.y)
            } else {
                (tri.normal.x, tri.normal.y, tri.normal.z)
            };

            let rotated_normal = model3d::Vec3::new(
                nx * cos_y + nz * sin_y,
                ny * cos_x - (-nx * sin_y + nz * cos_y) * sin_x,
                ny * sin_x + (-nx * sin_y + nz * cos_y) * cos_x,
            )
            .normalize();

            if rotated_normal.z <= 0.02 {
                continue;
            }

            let transform_v = |v: model3d::Vec3| -> (f32, f32, f32) {
                let (dx, dy, dz) = if is_z_up {
                    (v.x - center.x, v.z - center.z, -(v.y - center.y))
                } else {
                    (v.x - center.x, v.y - center.y, v.z - center.z)
                };

                let rx = dx * cos_y + dz * sin_y;
                let rz = -dx * sin_y + dz * cos_y;
                let ry = dy;

                let fy = ry * cos_x - rz * sin_x;
                let fz = ry * sin_x + rz * cos_x;

                (cx + rx * scale, cy - fy * scale, fz)
            };

            let (p1x, p1y, p1z) = transform_v(tri.v1);
            let (p2x, p2y, p2z) = transform_v(tri.v2);
            let (p3x, p3y, p3z) = transform_v(tri.v3);

            let cross_2d = (p2x - p1x) * (p3y - p1y) - (p2y - p1y) * (p3x - p1x);
            if cross_2d.abs() < 0.2 {
                continue;
            }

            let avg_z = (p1z + p2z + p3z) / 3.0;
            let diff = rotated_normal.dot(light_dir).max(0.18).min(1.0);
            projected.push((avg_z, diff, tri.color, (p1x, p1y), (p2x, p2y), (p3x, p3y)));
        }

        projected.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

        let base_c = self.color_from_hex(&self.palette.harmonies.base);
        let bg_c = self.color_from_hex(&self.palette.colors.surface_recessed);

        let mut line_stroke = Stroke::default();
        line_stroke.width = 1.2;
        let mut line_paint = Paint::default();
        let wire_c = self.color_from_hex(&self.palette.harmonies.base);
        line_paint.set_color(Color::from_rgba8(
            (wire_c.red() * 255.0) as u8,
            (wire_c.green() * 255.0) as u8,
            (wire_c.blue() * 255.0) as u8,
            220,
        ));

        for (_, diff, tri_color, (p1x, p1y), (p2x, p2y), (p3x, p3y)) in projected {
            let mut pb = PathBuilder::new();
            pb.move_to(p1x, p1y);
            pb.line_to(p2x, p2y);
            pb.line_to(p3x, p3y);
            pb.close();

            if let Some(t_path) = pb.finish() {
                let tri_base_c = if let Some(c) = tri_color {
                    Color::from_rgba(c[0], c[1], c[2], c[3]).unwrap_or(base_c)
                } else {
                    base_c
                };
                let r = ((bg_c.red() * (1.0 - diff * 0.7) + tri_base_c.red() * diff * 0.7) * 255.0)
                    .clamp(0.0, 255.0) as u8;
                let g = ((bg_c.green() * (1.0 - diff * 0.7) + tri_base_c.green() * diff * 0.7)
                    * 255.0)
                    .clamp(0.0, 255.0) as u8;
                let b = ((bg_c.blue() * (1.0 - diff * 0.7) + tri_base_c.blue() * diff * 0.7)
                    * 255.0)
                    .clamp(0.0, 255.0) as u8;

                let mut fill_paint = Paint::default();
                fill_paint.set_color(Color::from_rgba8(r, g, b, 235));
                pixmap.fill_path(
                    &t_path,
                    &fill_paint,
                    tiny_skia::FillRule::Winding,
                    Transform::identity(),
                    None,
                );

                pixmap.stroke_path(
                    &t_path,
                    &line_paint,
                    &line_stroke,
                    Transform::identity(),
                    None,
                );
            }
        }
    }
}
