//! Shared geometry for painting, picking, dragging, reveals and audition trails.
use super::{ConstellationState, multisample};
use egui::{Color32, Pos2, Stroke, Vec2};

pub(super) const SET_MAX_ZOOM: f32 = 160.0;

/// Orthographic orbit camera: azimuth rotates around Z, elevation tilts toward the plane.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct MapCamera {
    pub azimuth: f32,
    pub elevation: f32,
}

impl Default for MapCamera {
    fn default() -> Self {
        Self {
            azimuth: 0.0,
            elevation: 46.0,
        }
    }
}

impl MapCamera {
    pub fn project(self, point: Pos2, height: f32) -> Vec2 {
        let (sin_yaw, cos_yaw) = self.azimuth.to_radians().sin_cos();
        let (sin_pitch, cos_pitch) = self.elevation.to_radians().sin_cos();
        Vec2::new(
            point.x * cos_yaw - point.y * sin_yaw,
            -(point.x * sin_yaw + point.y * cos_yaw) * sin_pitch - height * cos_pitch,
        )
    }

    fn unproject_plane(self, point: Vec2) -> Pos2 {
        let (sin_yaw, cos_yaw) = self.azimuth.to_radians().sin_cos();
        let depth = -point.y / self.elevation.to_radians().sin().max(0.01);
        Pos2::new(
            point.x * cos_yaw + depth * sin_yaw,
            -point.x * sin_yaw + depth * cos_yaw,
        )
    }

    /// Subdivide the original bounded C4 plane, targeting about 64 visible cells.
    /// Finer lines fade in between levels instead of jumping as a threshold is crossed.
    fn reference_grid(self, clip: egui::Rect, center: Pos2, scale: f32) -> Vec<(Pos2, Pos2, f32)> {
        if !clip.is_finite() || !clip.is_positive() || !scale.is_finite() || scale <= 0.0 {
            return Vec::new();
        }
        let corners = [
            clip.left_top(),
            clip.right_top(),
            clip.right_bottom(),
            clip.left_bottom(),
        ]
        .map(|point| self.unproject_plane((point - center) / scale));
        let bounds = egui::Rect::from_points(&corners);
        if !bounds.is_finite() || !bounds.is_positive() {
            return Vec::new();
        }
        let world_area = (clip.width() / scale) * (clip.height() / scale)
            / self.elevation.to_radians().sin().max(0.01);
        // Account for the empty corners of the world-axis bounding box when orbiting.
        let coverage = (world_area / bounds.area()).sqrt();
        let desired = (bounds.size() * (coverage / 8.0)).min(Vec2::splat(0.25));
        let plane = egui::Rect::from_min_max(Pos2::new(-1.0, -1.0), Pos2::new(1.0, 1.0));
        let bounds = bounds.intersect(plane);
        if !bounds.is_positive() {
            return Vec::new();
        }
        let mut lines = Vec::with_capacity(64);
        for axis in 0..2 {
            let coarse = 2.0_f32.powf(desired[axis].log2().ceil());
            let step = coarse * 0.5;
            let minor_opacity = (coarse / desired[axis] - 1.0).clamp(0.0, 1.0);
            let first = (bounds.min[axis] / step).floor() as i32;
            let last = (bounds.max[axis] / step).ceil() as i32;
            // Bound painting even for an exceptionally thin canvas or extreme pan.
            for index in first..=last.min(first.saturating_add(128)) {
                let opacity = if index % 2 == 0 { 1.0 } else { minor_opacity };
                if opacity < 0.01 {
                    continue;
                }
                let mut from = plane.min;
                let mut to = plane.max;
                from[axis] = index as f32 * step;
                to[axis] = from[axis];
                lines.push((
                    center + self.project(from, 0.0) * scale,
                    center + self.project(to, 0.0) * scale,
                    opacity,
                ));
            }
        }
        lines
    }

    pub fn orbit(&mut self, delta: Vec2) {
        self.azimuth = (self.azimuth + delta.x * 0.35).rem_euclid(360.0);
        self.elevation = (self.elevation + delta.y * 0.25).clamp(5.0, 90.0);
    }
}

pub(super) struct MapPoint {
    pub index: usize,
    pub base: Pos2,
    pub position: Pos2,
    pub radius: f32,
    pub collapsed_set: Option<usize>,
}

impl ConstellationState {
    /// Fit, relax and reveal once; all map consumers receive the same camera geometry.
    pub(super) fn layout_map_points(
        &mut self,
        viewport: egui::Rect,
        visible: egui::Rect,
        base_star_radius: f32,
        query: &str,
    ) -> (f32, Pos2, Vec<MapPoint>) {
        let extent = self
            .samples
            .iter()
            .enumerate()
            .fold(1.0_f32, |extent, (index, sample)| {
                // A world-space bound keeps orbiting from changing the zoom scale.
                let height = multisample::pitch_height(self.sample_sets.note(index));
                extent.max(sample.x.hypot(sample.y) + height.abs())
            });
        let mut view_scale =
            (viewport.width().min(viewport.height()) * 0.45).max(1.0) / extent * self.zoom;
        let mut points = self.map_points(view_scale, base_star_radius, query);
        if let Some(set) = self.pending_set_focus.take() {
            let anchor = points
                .iter()
                .find(|point| point.index == self.sample_sets.sets[set].members[0])
                .map(|point| point.position)
                .unwrap_or_default();
            let mut bounds = self.set_point_bounds(&points, set, view_scale);
            // Fit symmetrically around the lowest sample, which remains the focus.
            bounds.extend_with(anchor + (anchor - bounds.min));
            bounds.extend_with(anchor + (anchor - bounds.max));
            if bounds.is_finite() {
                let fit = ((visible.width() - 60.0).max(1.0) / bounds.width().max(1.0))
                    .min((visible.height() - 60.0).max(1.0) / bounds.height().max(1.0))
                    .min(1.0);
                let old_zoom = self.zoom;
                self.zoom = (self.zoom * fit).clamp(0.2, SET_MAX_ZOOM);
                view_scale *= self.zoom / old_zoom;
                points = self.map_points(view_scale, base_star_radius, query);
                let anchor = anchor.to_vec2() * (self.zoom / old_zoom);
                self.pan = visible.center() - viewport.center() - anchor;
            }
        }
        let push_factor = super::ui::spacing_strength(self.zoom);
        if push_factor > 0.0 && self.expanded_set.is_none() {
            let positions = self.overlap_layout.positions(
                points.iter().map(|point| point.position).collect(),
                (base_star_radius * 3.6).max(7.5) * push_factor,
            );
            for (point, position) in points.iter_mut().zip(positions) {
                point.base += *position - point.position;
                point.position = *position;
            }
        }
        if let Some(path) = self.pending_map_reveal.take() {
            if let Some(point) = points.iter().find(|point| {
                self.samples[point.index].file_path == path
                    || point.collapsed_set.is_some_and(|set| {
                        self.sample_sets.sets[set]
                            .members
                            .iter()
                            .any(|index| self.samples[*index].file_path == path)
                    })
            }) {
                self.pan = visible.center() - viewport.center() - point.position.to_vec2();
            }
        }
        let center = viewport.center() + self.pan;
        (view_scale, center, points)
    }

    fn set_point_bounds(&self, points: &[MapPoint], set: usize, scale: f32) -> egui::Rect {
        let family = &self.sample_sets.sets[set];
        let radius = ring_radius(family.members.len() - 1) * scale;
        let origin = self.map_base(family.center) * scale;
        let corner = Vec2::new(
            radius,
            radius
                * if self.pitch_height_enabled {
                    self.camera.elevation.to_radians().sin()
                } else {
                    1.0
                },
        );
        let rings = egui::Rect::from_center_size(origin.to_pos2(), corner * 2.0);
        points
            .iter()
            .filter(|point| self.sample_sets.set_for(point.index) == Some(set))
            .fold(rings, |mut bounds, point| {
                bounds.extend_with(point.position);
                bounds
            })
    }

    pub(super) fn explore_set(&mut self, set: usize) {
        if self.overview_camera.is_none() {
            self.overview_camera =
                Some((self.camera, self.pan, self.zoom, self.selected_path.clone()));
        }
        let lowest = self.sample_sets.sets[set].members[0];
        self.selected_path = Some(self.samples[lowest].file_path.clone());
        self.expanded_set = Some(set);
        self.explore_selection = None;
        self.last_hovered_path = None;
        self.zoom = 12.0;
        self.pending_set_focus = Some(set);
    }

    pub(super) fn handle_explore_escape(&mut self, ctx: &egui::Context) {
        if self.expanded_set.is_some()
            && !self.browser_open
            && !ctx.memory(|memory| memory.any_popup_open())
            && ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
        {
            self.leave_set();
        }
    }

    pub(super) fn leave_set(&mut self) {
        self.expanded_set = None;
        self.last_hovered_path = None;
        self.map_hover_suspended = true;
        self.pending_set_focus = None;
        self.pending_map_reveal = None;
        if let Some((camera, pan, zoom, selected_path)) = self.overview_camera.take() {
            self.camera = camera;
            self.pan = pan;
            self.zoom = zoom;
            self.selected_path = selected_path;
        } else {
            self.zoom = 1.0;
            self.pan = Vec2::ZERO;
        }
        if let Some(path) = self.explore_selection.take() {
            self.selected_path = Some(path.clone());
            self.pending_map_reveal = Some(path);
        }
    }

    /// Hit the projected ring's entire circumference, independently of its dot angle.
    pub(super) fn ring_sample_at(&self, pointer: Pos2, center: Pos2, scale: f32) -> Option<usize> {
        let set = self.sample_sets.sets.get(self.expanded_set?)?;
        let origin = center + self.map_base(set.center) * scale;
        let delta = (pointer - origin) / scale;
        let tilt = if self.pitch_height_enabled {
            self.camera.elevation.to_radians().sin()
        } else {
            1.0
        };
        let distance = Vec2::new(delta.x, delta.y / tilt.max(0.01)).length();
        if distance > ring_radius(set.members.len() - 1) + 0.0125 {
            return None;
        }
        let ordinal = ((distance / 0.025).round() as usize)
            .saturating_sub(1)
            .min(set.members.len() - 1);
        Some(set.members[ordinal])
    }

    pub(super) fn selected_set(&self) -> Option<usize> {
        let path = self.selected_path.as_ref()?;
        let index = self
            .samples
            .iter()
            .position(|sample| sample.file_path == *path)?;
        self.sample_sets.set_for(index)
    }

    pub(super) fn map_base(&self, position: Pos2) -> Vec2 {
        if self.pitch_height_enabled {
            self.camera.project(position, 0.0)
        } else {
            Vec2::new(position.x, -position.y)
        }
    }

    pub(super) fn note_height(&self, index: usize) -> f32 {
        if self.pitch_height_enabled {
            multisample::pitch_height(self.sample_sets.note(index))
                * self.camera.elevation.to_radians().cos()
        } else {
            0.0
        }
    }

    pub(super) fn map_points(&self, scale: f32, radius: f32, query: &str) -> Vec<MapPoint> {
        let selected_set = self.expanded_set;
        let mut points = Vec::with_capacity(self.samples.len());
        for (index, sample) in self.samples.iter().enumerate() {
            if self.sample_sets.set_for(index).is_none() {
                let base = self.map_base(Pos2::new(sample.x, sample.y)) * scale;
                points.push(self.project_point(index, base, scale, radius, None));
            }
        }
        for (set_index, set) in self.sample_sets.sets.iter().enumerate() {
            let base = self.map_base(set.center) * scale;
            if Some(set_index) == selected_set {
                for (ordinal, &index) in set.members.iter().enumerate() {
                    let angle = ordinal as f32 * 2.399_963_1;
                    let offset = self.map_base(Pos2::new(angle.cos(), angle.sin()))
                        * ring_radius(ordinal)
                        * scale;
                    points.push(self.project_point(index, base + offset, scale, radius, None));
                }
            } else {
                let matching: Vec<_> = set
                    .members
                    .iter()
                    .copied()
                    .filter(|index| self.matches_filters(&self.samples[*index], query))
                    .collect();
                let index = if matching.len() == set.members.len() || matching.is_empty() {
                    set.representative
                } else {
                    matching[(matching.len() - 1) / 2]
                };
                points.push(self.project_point(index, base, scale, radius, Some(set_index)));
            }
        }
        points
    }

    fn project_point(
        &self,
        index: usize,
        base: Vec2,
        scale: f32,
        radius: f32,
        collapsed_set: Option<usize>,
    ) -> MapPoint {
        MapPoint {
            index,
            base: base.to_pos2(),
            position: (base - Vec2::Y * self.note_height(index) * scale).to_pos2(),
            radius: (radius * multisample::duration_radius(self.samples[index].duration_seconds))
                .clamp(0.35, 12.0),
            collapsed_set,
        }
    }

    pub(super) fn paint_reference_plane(&self, painter: &egui::Painter, center: Pos2, scale: f32) {
        if !self.pitch_height_enabled {
            return;
        }
        let color = painter
            .ctx()
            .style()
            .visuals
            .weak_text_color()
            .gamma_multiply(0.12);
        for (from, to, opacity) in self
            .camera
            .reference_grid(painter.clip_rect(), center, scale)
        {
            painter.line_segment(
                [from, to],
                Stroke::new(0.7_f32, color.gamma_multiply(opacity)),
            );
        }
    }

    pub(super) fn paint_pitch_rings(&self, painter: &egui::Painter, center: Pos2, scale: f32) {
        let Some(set_index) = self.expanded_set else {
            return;
        };
        let set = &self.sample_sets.sets[set_index];
        let query = self.search_query.to_lowercase();
        if !set
            .members
            .iter()
            .any(|index| self.matches_filters(&self.samples[*index], &query))
        {
            return;
        }
        let origin = center + self.map_base(set.center) * scale;
        for (ordinal, &index) in set.members.iter().enumerate() {
            if !self.matches_filters(&self.samples[index], &query) {
                continue;
            }
            let radius = ring_radius(ordinal) * scale;
            let bounds = egui::Rect::from_center_size(origin, Vec2::splat(radius * 2.0));
            if !bounds.intersects(painter.clip_rect()) {
                continue;
            }
            let ring: Vec<_> = (0..=96)
                .map(|step| {
                    let angle = step as f32 / 96.0 * std::f32::consts::TAU;
                    origin + self.map_base(Pos2::new(angle.cos(), angle.sin())) * radius
                })
                .collect();
            let active = self.last_hovered_path.as_ref() == Some(&self.samples[index].file_path);
            let color = if active {
                self.samples[index].category.color().gamma_multiply(0.8)
            } else {
                painter
                    .ctx()
                    .style()
                    .visuals
                    .weak_text_color()
                    .gamma_multiply(0.35)
            };
            painter.add(egui::Shape::line(
                ring,
                Stroke::new(if active { 2.0_f32 } else { 1.0_f32 }, color),
            ));
        }
    }
}

fn ring_radius(ordinal: usize) -> f32 {
    (ordinal + 1) as f32 * 0.025
}

/// Batch dotted stems with the shared disk texture; clip before emitting vertices.
pub(super) fn append_stem(
    mesh: &mut egui::Mesh,
    clip: egui::Rect,
    base: Pos2,
    point: Pos2,
    color: Color32,
) {
    let delta = point - base;
    if delta.length_sq() < 9.0 || base.x < clip.left() || base.x > clip.right() {
        return;
    }
    let top = base.y.min(point.y).max(clip.top());
    let bottom = base.y.max(point.y).min(clip.bottom());
    let uv = egui::Rect::from_min_max(Pos2::ZERO, Pos2::new(0.5, 1.0));
    let mut y = top + (base.y - top).rem_euclid(6.0);
    while y <= bottom {
        mesh.add_rect_with_uv(
            egui::Rect::from_center_size(Pos2::new(base.x, y), Vec2::splat(1.6)),
            uv,
            color,
        );
        y += 6.0;
    }
    if clip.contains(base) {
        mesh.add_rect_with_uv(
            egui::Rect::from_center_size(base, Vec2::splat(3.0)),
            uv,
            color,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reference_grid_keeps_about_eight_cells_per_axis_through_zoom() {
        let camera = MapCamera {
            azimuth: 0.0,
            elevation: 90.0,
        };
        let clip = egui::Rect::from_min_size(Pos2::ZERO, Vec2::splat(800.0));
        for scale in [0.2, 80.0, 113.0, 400.0, 763.0, 1600.0, 1_000_000.0] {
            let lines = camera.reference_grid(clip, clip.center(), scale);
            let mut density = [0.0_f32; 2];
            for (from, to, opacity) in &lines {
                let axis = usize::from((from.x - to.x).abs() > 0.01);
                if (clip.min[axis]..=clip.max[axis]).contains(&from[axis]) {
                    density[axis] += opacity;
                }
            }
            assert!((7.0..=10.0).contains(&density[0]), "{scale}: {density:?}");
            assert!((7.0..=10.0).contains(&density[1]), "{scale}: {density:?}");
            assert!((49.0..=100.0).contains(&(density[0] * density[1])));
        }
    }

    #[test]
    fn reference_grid_is_world_anchored_and_bounded_when_panned_or_orbited() {
        let clip = egui::Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0));
        for azimuth in [0.0, 43.0, 137.0] {
            for elevation in [5.0, 46.0, 90.0] {
                let camera = MapCamera { azimuth, elevation };
                let world = Pos2::new(0.37, -0.86);
                assert!(
                    (camera.unproject_plane(camera.project(world, 0.0)) - world).length() < 0.00001
                );
                for scale in [0.2, 400.0, 100_000.0] {
                    let center = clip.center() + Vec2::new(600.0, -500.0);
                    let lines = camera.reference_grid(clip, center, scale);
                    assert!(lines.len() <= 258);
                    for (from, to, opacity) in lines {
                        assert!(from.is_finite() && to.is_finite());
                        assert!((0.0..=1.0).contains(&opacity));
                        for point in [from, to] {
                            let world = camera.unproject_plane((point - center) / scale);
                            assert!((-1.001..=1.001).contains(&world.x));
                            assert!((-1.001..=1.001).contains(&world.y));
                        }
                    }
                }
            }
        }
        let top = MapCamera {
            azimuth: 0.0,
            elevation: 90.0,
        };
        let finite = top.reference_grid(clip, clip.center(), 100.0);
        assert_eq!(finite.len(), 18, "the full bounded plane has 8 by 8 cells");
        assert!(
            top.reference_grid(clip, clip.center() + Vec2::splat(10_000.0), 400.0)
                .is_empty(),
            "panning away from the original plane must not tile new grid regions"
        );
        let camera = MapCamera::default();
        let pan = Vec2::new(19.0, -23.0);
        let before = camera.reference_grid(clip, clip.center(), 763.0);
        let after = camera.reference_grid(clip, clip.center() + pan, 763.0);
        for (from, to, opacity) in before {
            let axis = usize::from((from.x - to.x).abs() > 0.01);
            let shifted = from[axis] + pan[axis];
            if (clip.min[axis]..=clip.max[axis]).contains(&shifted) {
                assert!(
                    after.iter().any(|(a, b, alpha)| {
                        (a[axis] - b[axis]).abs() < 0.001
                            && (a[axis] - shifted).abs() < 0.001
                            && (alpha - opacity).abs() < 0.001
                    }),
                    "panning must translate existing world grid lines"
                );
            }
        }
    }

    #[test]
    fn orbit_rotates_plane_and_tilt_projects_pitch_height() {
        let camera = MapCamera {
            azimuth: 90.0,
            elevation: 30.0,
        };
        let point = camera.project(Pos2::new(1.0, 0.0), 0.0);
        assert!(point.x.abs() < 0.00001);
        assert!((point.y + 0.5).abs() < 0.00001);
        let raised = camera.project(Pos2::new(1.0, 0.0), 1.0);
        assert!((raised.y - point.y + 30.0_f32.to_radians().cos()).abs() < 0.00001);
        let top = MapCamera {
            elevation: 90.0,
            ..camera
        };
        assert!((top.project(Pos2::ZERO, 1.0) - top.project(Pos2::ZERO, 0.0)).length() < 0.00001);
        let mut orbit = camera;
        orbit.orbit(Vec2::new(2000.0, -1000.0));
        assert!((0.0..360.0).contains(&orbit.azimuth));
        assert_eq!(orbit.elevation, 5.0);
    }
}
