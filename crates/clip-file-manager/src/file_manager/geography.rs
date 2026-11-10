//! Offline geographic maps with a linked supplied-elevation profile.
use super::{filesystem::Entry, media::PreviewData, shelf::preview::Ready};
use crate::ui_ext::fit_cartesian_point as project;
use crate::{
    icons::Icon,
    shell::{StudioPanelLayout, StudioShell},
    ui_appearance::WorkbenchColors,
};
use little_clip_core::{
    geography::{Document, Feature, Kind, MAX_BYTES, Sample},
    palette::AppPalette,
};
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

fn read(path: &Path, token: &AtomicBool) -> Result<Document, String> {
    let bytes = crate::file_source::read_file_snapshot(path, MAX_BYTES)?;
    if token.load(Ordering::Relaxed) {
        return Err("Geography opening cancelled".into());
    }
    if path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("geojson"))
    {
        Document::geojson(&bytes)
    } else {
        Document::gpx(&bytes)
    }
}
pub(super) fn load(path: &Path, token: &AtomicBool) -> PreviewData {
    let mut data = PreviewData::new(String::new(), "Geography".into(), Icon::Image);
    match read(path, token) {
        Ok(mut document) => {
            data.detail = format!(
                "{} shapes · {} positions · partial map preview",
                document.features.len(),
                document.points
            );
            // Keep source-order indices while retaining only bounded drawing data in row caches.
            document.features.truncate(32);
            let mut remaining = 512;
            for feature in &mut document.features {
                feature.name = feature.name.chars().take(80).collect();
                feature.properties.clear();
                feature.paths.truncate(32);
                for path in &mut feature.paths {
                    let step = path.len().div_ceil(remaining.max(1)).max(1);
                    *path = path
                        .iter()
                        .step_by(step)
                        .take(remaining)
                        .cloned()
                        .map(|mut sample| {
                            sample.point.time = String::new();
                            sample
                        })
                        .collect();
                    remaining = remaining.saturating_sub(path.len());
                }
                feature.paths.retain(|path| !path.is_empty());
            }
            document.samples = document
                .features
                .iter()
                .flat_map(|feature| &feature.paths)
                .map(Vec::len)
                .sum();
            document.title = String::new();
            document.notes.clear();
            data.geography = Some(Arc::new(document));
        }
        Err(error) => data.detail = error,
    }
    data
}
#[derive(Clone, Copy, PartialEq, Eq)]
struct Hit {
    feature: usize,
    path: usize,
    point: usize,
}
pub(super) struct Viewer {
    document: Document,
    selected: usize,
    current: Option<Hit>,
    zoom: f32,
    pan: egui::Vec2,
}
impl Viewer {
    pub fn load(path: &Path) -> Result<Self, String> {
        let document = read(path, &AtomicBool::new(false))?;
        let selected = document
            .features
            .iter()
            .enumerate()
            .max_by_key(|(_, f)| f.points)
            .map_or(0, |(i, _)| i);
        let mut viewer = Self {
            document,
            selected,
            current: None,
            zoom: 1.0,
            pan: egui::Vec2::ZERO,
        };
        viewer.select(selected);
        Ok(viewer)
    }
    pub fn select(&mut self, index: usize) {
        if index < self.document.features.len() {
            self.selected = index;
            self.current = self.document.features[index]
                .paths
                .first()
                .filter(|p| !p.is_empty())
                .map(|_| Hit {
                    feature: index,
                    path: 0,
                    point: 0,
                });
        }
    }
    pub fn ui(&mut self, ui: &mut egui::Ui, close: &mut bool, palette: &AppPalette) {
        StudioShell::new("Geography", Icon::Image, palette, close).render_header(ui, |_| {});
        let layout = StudioPanelLayout::default().with_left().allocate(ui);
        if let Some(left) = layout.left {
            ui.allocate_ui_at_rect(left, |ui| {
                ui.set_clip_rect(left.intersect(ui.clip_rect()));
                self.details(ui);
            });
        }
        ui.allocate_ui_at_rect(layout.center, |ui| {
            ui.set_clip_rect(layout.center.intersect(ui.clip_rect()));
            let colors = WorkbenchColors::from_palette(palette);
            let feature = &self.document.features[self.selected];
            let profile_visible = feature.kind != Kind::Point && feature.elevation.is_some() && feature.points > 1;
            let map_height = (ui.available_height() - if profile_visible { 190.0 } else { 26.0 }).max(80.0);
            let (rect, response) = ui.allocate_exact_size(
                egui::vec2(ui.available_width(), map_height),
                egui::Sense::click_and_drag(),
            );
            crate::ui_ext::pan_zoom_canvas(ui, rect, &response, &mut self.zoom, &mut self.pan, 16.0);
            let hit = paint_map(
                ui, rect, &self.document, Some(self.selected), self.zoom, self.pan, colors,
            );
            if response.clicked() {
                if let Some(hit) = hit {
                    self.selected = hit.feature;
                    self.current = Some(hit);
                }
            }
            if let Some(sample) = self.current.and_then(|hit| self.sample(hit)) {
                let point = project(rect, self.document.bounds, sample.xy, self.zoom, self.pan);
                if rect.contains(point) {
                    let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
                    painter.circle_stroke(point, 7.0, egui::Stroke::new(2.0_f32, colors.focus));
                    painter.circle_filled(point, 2.0, colors.text_primary);
                }
            }
            ui.label("Offline map · north up · drag to pan, scroll to zoom")
                .on_hover_text("Local equirectangular projection. Supplied positions only; no basemap or terrain information.");
            if !profile_visible { return; }
            let feature = &self.document.features[self.selected];
            let (profile, response) = ui.allocate_exact_size(
                egui::vec2(ui.available_width(), ui.available_height().max(1.0)),
                egui::Sense::hover(),
            );
            let point = paint_profile(ui, profile, feature, self.selected, self.current, colors);
            if response.hovered() && point.is_some() && point != self.current {
                self.current = point;
                ui.ctx().request_repaint();
            }
        });
    }
    fn sample(&self, hit: Hit) -> Option<&Sample> {
        self.document
            .features
            .get(hit.feature)?
            .paths
            .get(hit.path)?
            .get(hit.point)
    }
    fn details(&mut self, ui: &mut egui::Ui) {
        if !self.document.title.is_empty() {
            ui.heading(&self.document.title);
        }
        ui.label(format!(
            "{} shapes · {} positions",
            self.document.features.len(),
            self.document.points
        ));
        ui.horizontal(|ui| {
            if ui.button("Fit map").clicked() {
                self.zoom = 1.0;
                self.pan = egui::Vec2::ZERO;
            }
            ui.add(
                egui::Slider::new(&mut self.zoom, 1.0..=16.0)
                    .show_value(false)
                    .text("Zoom"),
            );
        });
        let height = crate::ui_ext::selectable_text_row_height(ui, 28.0);
        let mut chosen = None;
        egui::ScrollArea::vertical()
            .id_source("geography-features")
            .max_height((ui.available_height() * 0.3).max(height))
            .min_scrolled_height(0.0)
            .show_rows(ui, height, self.document.features.len(), |ui, rows| {
                for index in rows {
                    if crate::ui_ext::selectable_text_row(
                        ui,
                        &self.document.features[index].name,
                        index == self.selected,
                        height,
                    )
                    .clicked()
                        && index != self.selected
                    {
                        chosen = Some(index);
                    }
                }
            });
        if let Some(index) = chosen {
            self.select(index);
        }
        ui.separator();
        egui::ScrollArea::vertical()
            .id_source("geography-details")
            .auto_shrink([false, false])
            .min_scrolled_height(0.0)
            .show(ui, |ui| {
                let feature = &self.document.features[self.selected];
                ui.strong(&feature.name);
                ui.label(format!(
                    "{} positions · {} segments",
                    feature.points,
                    feature.paths.len()
                ));
                if matches!(feature.kind, Kind::Line | Kind::Polygon) {
                    ui.label(format!(
                        "Spherical length {:.2} km",
                        feature.distance_m / 1000.0
                    ));
                }
                if let Some([low, high]) = feature.elevation {
                    ui.label(format!("Supplied elevation {low:.1}–{high:.1} m"));
                }
                if let Some(sample) = self.current.and_then(|hit| self.sample(hit)) {
                    ui.separator();
                    ui.strong("Selected position");
                    ui.label(format!("Latitude {:.6}", sample.point.lat));
                    ui.label(format!("Longitude {:.6}", sample.point.lon));
                    if let Some(value) = sample.point.elevation {
                        ui.label(format!("Elevation {value:.1} m"));
                    }
                    if !sample.point.time.is_empty() {
                        ui.add(egui::Label::new(&sample.point.time).wrap(true));
                    }
                    ui.label(format!("Along feature {:.1} m", sample.distance_m));
                }
                for (name, value) in &feature.properties {
                    ui.strong(name);
                    ui.add(egui::Label::new(value).wrap(true));
                }
                for note in &self.document.notes {
                    ui.add(egui::Label::new(note).wrap(true));
                }
            });
    }
}
fn paint_map(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    doc: &Document,
    selected: Option<usize>,
    zoom: f32,
    pan: egui::Vec2,
    colors: WorkbenchColors,
) -> Option<Hit> {
    let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    painter.rect_filled(rect, 3.0, colors.recessed);
    let pointer = super::ui::preview_hovered(ui, rect)
        .then(|| ui.input(|i| i.pointer.hover_pos()))
        .flatten();
    let mut nearest = 64.0;
    let mut hit = None;
    for (index, feature) in doc.features.iter().enumerate() {
        let color = if selected == Some(index) {
            colors.accent
        } else {
            colors.text_secondary
        };
        for (path_index, path) in feature.paths.iter().enumerate() {
            let positions: Vec<_> = path
                .iter()
                .map(|sample| project(rect, doc.bounds, sample.xy, zoom, pan))
                .collect();
            if matches!(feature.kind, Kind::Line | Kind::Polygon) && positions.len() > 1 {
                painter.add(egui::Shape::line(
                    positions.clone(),
                    egui::Stroke::new(
                        if selected == Some(index) {
                            3.0_f32
                        } else {
                            1.5_f32
                        },
                        color,
                    ),
                ));
            }
            if feature.kind == Kind::Point || path.len() == 1 {
                for position in &positions {
                    painter.circle_filled(*position, 4.0, color);
                }
            }
            if let Some(pointer) = pointer {
                if let Some((point, distance)) = nearest_on_path(
                    &positions,
                    pointer,
                    matches!(feature.kind, Kind::Line | Kind::Polygon),
                ) {
                    if distance < nearest {
                        nearest = distance;
                        hit = Some(Hit {
                            feature: index,
                            path: path_index,
                            point,
                        });
                    }
                }
            }
        }
    }
    if let Some(hit) = hit {
        let sample = &doc.features[hit.feature].paths[hit.path][hit.point];
        egui::show_tooltip_at_pointer(ui.ctx(), egui::Id::new("geographic-position"), |ui| {
            ui.label(&doc.features[hit.feature].name);
            ui.label(format!("{:.6}, {:.6}", sample.point.lat, sample.point.lon));
        });
    }
    hit
}
fn nearest_on_path(
    positions: &[egui::Pos2],
    pointer: egui::Pos2,
    connected: bool,
) -> Option<(usize, f32)> {
    let mut best = positions
        .iter()
        .enumerate()
        .map(|(index, p)| (index, p.distance_sq(pointer)))
        .min_by(|a, b| a.1.total_cmp(&b.1))?;
    if connected {
        for (point, pair) in positions.windows(2).enumerate() {
            let (t, distance) = crate::ui_ext::segment_hit(pointer, pair[0], pair[1]);
            if distance < best.1 {
                best = (point + usize::from(t > 0.5), distance);
            }
        }
    }
    Some(best)
}
fn paint_profile(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    feature: &Feature,
    feature_index: usize,
    current: Option<Hit>,
    colors: WorkbenchColors,
) -> Option<Hit> {
    let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    painter.rect_filled(rect, 3.0, colors.recessed);
    let Some([low, high]) = feature.elevation else {
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            "No supplied elevations",
            egui::TextStyle::Body.resolve(ui.style()),
            colors.text_secondary,
        );
        return None;
    };
    painter.text(
        rect.left_top() + egui::vec2(5.0, 3.0),
        egui::Align2::LEFT_TOP,
        format!(
            "Supplied elevation · {low:.1}–{high:.1} m · {:.2} km",
            feature.distance_m / 1000.0
        ),
        egui::TextStyle::Small.resolve(ui.style()),
        colors.text_secondary,
    );
    let plot = egui::Rect::from_min_max(
        rect.min + egui::vec2(8.0, 28.0),
        rect.max - egui::vec2(8.0, 8.0),
    );
    let pointer = super::ui::preview_hovered(ui, plot)
        .then(|| ui.input(|i| i.pointer.hover_pos()))
        .flatten();
    let mut nearest = f32::INFINITY;
    let mut hit = None;
    for (path_index, path) in feature.paths.iter().enumerate() {
        let mut previous = None;
        for (index, sample) in path.iter().enumerate() {
            let Some(value) = sample.point.elevation else {
                previous = None;
                continue;
            };
            if sample.profile_break {
                previous = None;
            }
            let scale = low.abs().max(high.abs()).max(1.0);
            let fraction =
                (value / scale - low / scale) / (high / scale - low / scale).max(1.0 / scale);
            let position = egui::pos2(
                plot.left()
                    + plot.width() * (sample.distance_m / feature.distance_m.max(1.0)) as f32,
                plot.bottom() - plot.height() * fraction as f32,
            );
            if let Some(previous) = previous {
                painter.line_segment(
                    [previous, position],
                    egui::Stroke::new(2.0_f32, colors.accent),
                );
            }
            painter.circle_filled(position, 2.0, colors.accent);
            previous = Some(position);
            let candidate = Hit {
                feature: feature_index,
                path: path_index,
                point: index,
            };
            if current == Some(candidate) {
                painter.circle_stroke(position, 5.0, egui::Stroke::new(2.0_f32, colors.focus));
            }
            if let Some(pointer) = pointer {
                let distance = (position.x - pointer.x).abs();
                if distance < nearest {
                    nearest = distance;
                    hit = Some(candidate);
                }
            }
        }
    }
    hit
}
pub(super) fn show_list(
    ui: &mut egui::Ui,
    row: egui::Rect,
    entry: &Entry,
    columns: super::ui::ListColumns,
    ready: Option<&Ready>,
    colors: WorkbenchColors,
) -> Option<usize> {
    let rect = super::ui::information_strip(
        ui,
        row,
        entry,
        ready.map_or("Geographic paths", |r| &r.data.detail),
        Some(columns),
        colors,
    );
    ui.painter().rect_filled(rect, 3.0, colors.recessed);
    let document = ready.and_then(|r| r.data.geography.as_ref())?;
    let map = egui::Rect::from_min_max(
        rect.min,
        egui::pos2(rect.left() + rect.width().min(240.0), rect.bottom()),
    );
    let mut hovered =
        paint_map(ui, map, document, None, 1.0, egui::Vec2::ZERO, colors).map(|hit| hit.feature);
    let mut x = map.right() + 4.0;
    for (index, feature) in document.features.iter().enumerate().take(4) {
        let width = 180.0;
        if x + width > rect.right() {
            break;
        }
        let card = egui::Rect::from_min_size(
            egui::pos2(x, rect.top() + 4.0),
            egui::vec2(width, rect.height() - 8.0),
        );
        let painter = ui.painter().with_clip_rect(card);
        painter.rect_filled(card, 3.0, colors.accent.gamma_multiply(0.18));
        painter.text(
            card.left_top() + egui::vec2(5.0, 3.0),
            egui::Align2::LEFT_TOP,
            &feature.name,
            egui::TextStyle::Body.resolve(ui.style()),
            colors.text_primary,
        );
        painter.text(
            card.left_top() + egui::vec2(5.0, 26.0),
            egui::Align2::LEFT_TOP,
            format!("{} positions", feature.points),
            egui::TextStyle::Small.resolve(ui.style()),
            colors.text_secondary,
        );
        if super::ui::preview_hovered(ui, card) {
            hovered = Some(index);
        }
        x += width + 4.0;
    }
    hovered
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sampled_folder_features_and_viewer_selection_keep_source_indices() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../little-clip-core/tests/fixtures/geography/harbor.gpx");
        let data = load(&path, &AtomicBool::new(false));
        let summary = data.geography.unwrap();
        assert!(summary.samples <= 512);
        assert_eq!(summary.features[1].name, "Ferry route");
        let mut viewer = Viewer::load(&path).unwrap();
        viewer.select(1);
        assert_eq!(
            viewer.document.features[viewer.selected].name,
            "Ferry route"
        );
        assert_eq!(
            viewer
                .sample(viewer.current.unwrap())
                .unwrap()
                .point
                .elevation,
            Some(0.0)
        );
        assert!(read(&path, &AtomicBool::new(true)).is_err());
        assert!(
            (little_clip_core::file_format::Format::for_path(Path::new("TRACK.GPX"))
                == Some(little_clip_core::file_format::Format::Geography))
        );
    }
    #[test]
    fn geojson_preview_and_null_geometry_selection_preserve_source_order() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../little-clip-core/tests/fixtures/geography/harbor.geojson");
        assert!(
            (little_clip_core::file_format::Format::for_path(Path::new("MAP.GEOJSON"))
                == Some(little_clip_core::file_format::Format::Geography))
        );
        let data = load(&path, &AtomicBool::new(false));
        let summary = data.geography.unwrap();
        assert_eq!(summary.features[0].kind, Kind::Polygon);
        assert_eq!(summary.features[0].paths.len(), 2);
        assert!(summary.samples <= 512);
        let mut viewer = Viewer::load(&path).unwrap();
        viewer.select(3);
        assert_eq!(
            viewer.document.features[viewer.selected].name,
            "Unplaced record"
        );
        assert!(viewer.current.is_none());
        viewer.select(1);
        assert_eq!(
            viewer
                .sample(viewer.current.unwrap())
                .unwrap()
                .point
                .elevation,
            Some(12.0)
        );
    }
    #[test]
    fn line_hits_choose_actual_endpoints_and_unconnected_points_have_no_line_hits() {
        let path = [egui::pos2(10.0, 10.0), egui::pos2(110.0, 10.0)];
        assert_eq!(
            nearest_on_path(&path, egui::pos2(75.0, 11.0), true),
            Some((1, 1.0))
        );
        assert!(
            nearest_on_path(&path, egui::pos2(75.0, 11.0), false)
                .unwrap()
                .1
                > 1000.0
        );
        assert!(nearest_on_path(&[], egui::Pos2::ZERO, true).is_none());
    }
}
