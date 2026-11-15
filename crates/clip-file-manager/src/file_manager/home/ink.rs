//! Home ink keeps measured pressure and paints the shared native freehand outline.
use super::super::native_tablet::Frame;
use little_clip_core::{JobError, jobs::task::Task};
use little_clip_render::diagram::{InkCommand, InkPath};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, sync::Arc, time::Duration};
use uuid::Uuid;

pub(super) const POINT_BUDGET: usize = 16_384;
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(in crate::file_manager) enum Color {
    #[default]
    Theme,
    Teal,
    Amber,
    Rose,
}
impl Color {
    fn paint(
        self,
        palette: &little_clip_core::palette::AppPalette,
        marker: bool,
    ) -> clip_ui::vectors::femtovg::Color {
        let [r, g, b] = match self {
            Self::Theme => {
                let ink =
                    clip_ui::ui_appearance::WorkbenchColors::from_palette(palette).text_primary;
                [ink.r(), ink.g(), ink.b()]
            }
            Self::Teal => [47, 159, 162],
            Self::Amber => [213, 155, 43],
            Self::Rose => [202, 89, 135],
        };
        clip_ui::vectors::femtovg::Color::rgba(r, g, b, if marker { 90 } else { 255 })
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::file_manager) struct Stroke {
    pub id: Uuid,
    pub origin: [f64; 2],
    pub points: Vec<[f64; 3]>,
    pub width: f64,
    pub color: Color,
    pub marker: bool,
}
impl Stroke {
    pub fn valid(&self) -> bool {
        !self.id.is_nil()
            && (2..=4096).contains(&self.points.len())
            && self.origin.iter().all(|v| v.is_finite() && v.abs() <= 1e8)
            && self.width.is_finite()
            && (0.1..=32.).contains(&self.width)
            && self.points.iter().all(|p| {
                p.iter().all(|v| v.is_finite())
                    && p[0].abs() <= 1e6
                    && p[1].abs() <= 1e6
                    && (0.0..=1.).contains(&p[2])
            })
    }
    pub fn bounds(&self) -> little_clip_core::diagram::drawing::Bounds {
        let mut min = [0.0_f64; 2];
        let mut max = [0.0_f64; 2];
        for p in &self.points {
            for axis in 0..2 {
                min[axis] = min[axis].min(p[axis]);
                max[axis] = max[axis].max(p[axis]);
            }
        }
        let margin = self.width * 5.;
        little_clip_core::diagram::drawing::Bounds {
            x: self.origin[0] + min[0] - margin,
            y: self.origin[1] + min[1] - margin,
            width: max[0] - min[0] + margin * 2.,
            height: max[1] - min[1] + margin * 2.,
            angle: 0.,
        }
    }
}
pub(super) fn validate(strokes: &[Stroke]) -> Result<(), String> {
    let mut ids = std::collections::BTreeSet::new();
    if strokes.len() > 256
        || strokes.iter().map(|s| s.points.len()).sum::<usize>() > POINT_BUDGET
        || strokes.iter().any(|s| !s.valid() || !ids.insert(s.id))
    {
        return Err("Home ink exceeds its stroke, coordinate or pressure budget".into());
    }
    Ok(())
}
struct Gesture {
    tool: octotablet::tool::ID,
    stroke: Stroke,
    path: Arc<InkPath>,
    dirty: bool,
    capacity: usize,
}
struct View {
    source: Stroke,
    path: Option<Arc<InkPath>>,
    pending: Option<Task<Result<InkPath, String>>>,
}
pub(in crate::file_manager) struct Ink {
    pub available: bool,
    pub enabled: bool,
    pub marker: bool,
    pub color: Color,
    pub width: f64,
    pub error: Option<String>,
    current: Option<Gesture>,
    views: BTreeMap<Uuid, View>,
}
impl Default for Ink {
    fn default() -> Self {
        Self {
            available: false,
            enabled: true,
            marker: false,
            color: Color::Theme,
            width: 1.5,
            error: None,
            current: None,
            views: BTreeMap::new(),
        }
    }
}
impl Ink {
    pub fn discard(&mut self) {
        self.current = None;
        self.error = None;
    }
    pub fn drawing(&self) -> bool {
        self.current.is_some()
    }
    fn begin(&mut self, frame: &Frame, world: [f64; 2], capacity: usize) {
        let Some(pressure) = frame.pulse.pressure else {
            return;
        };
        let stroke = Stroke {
            id: Uuid::new_v4(),
            origin: world,
            points: vec![[0., 0., f64::from(pressure)]; 2],
            width: if self.marker {
                self.width * 3.
            } else {
                self.width
            },
            color: self.color,
            marker: self.marker,
        };
        match InkPath::prepare(&stroke.points, stroke.width) {
            Ok(path) => {
                self.current = Some(Gesture {
                    tool: frame.tool.clone(),
                    stroke,
                    path: Arc::new(path),
                    dirty: false,
                    capacity,
                })
            }
            Err(error) => self.error = Some(error),
        }
    }
    fn sample(&mut self, frame: &Frame, world: [f64; 2], scale: f64) {
        let Some(current) = &mut self.current else {
            return;
        };
        if current.tool != frame.tool {
            return;
        }
        let Some(pressure) = frame.pulse.pressure else {
            return;
        };
        let p = [
            world[0] - current.stroke.origin[0],
            world[1] - current.stroke.origin[1],
            f64::from(pressure),
        ];
        if p[0].abs() > 1e6 || p[1].abs() > 1e6 {
            return;
        }
        let last = current
            .stroke
            .points
            .last_mut()
            .expect("A live stroke always has a starting dot");
        if (p[0] - last[0]).hypot(p[1] - last[1]) * scale < 0.7 {
            last[2] = p[2];
        } else if current.stroke.points.len() < current.capacity {
            current.stroke.points.push(p);
        }
        current.dirty = true;
    }
    fn finish(&mut self) -> Option<Stroke> {
        self.prepare_current();
        let current = self.current.take()?;
        self.views.insert(
            current.stroke.id,
            View {
                source: current.stroke.clone(),
                path: (!current.dirty).then_some(current.path),
                pending: None,
            },
        );
        Some(current.stroke)
    }
    fn prepare_current(&mut self) {
        if let Some(current) = &mut self.current
            && current.dirty
        {
            match InkPath::prepare(&current.stroke.points, current.stroke.width) {
                Ok(path) => {
                    current.path = Arc::new(path);
                    current.dirty = false;
                }
                Err(error) => self.error = Some(error),
            }
        }
    }
    pub fn retire_reads(&mut self) {
        self.views.retain(|_, view| view.path.is_some());
    }
    pub fn paint(
        &mut self,
        ui: &egui::Ui,
        viewport: &clip_studios::drawing_canvas::Viewport<'_>,
        strokes: &[Stroke],
        palette: &little_clip_core::palette::AppPalette,
    ) {
        self.prepare_current();
        let mut paths = Vec::new();
        self.views
            .retain(|id, _| strokes.iter().any(|stroke| stroke.id == *id));
        for stroke in strokes {
            if !viewport.bounds(stroke.bounds()).intersects(viewport.rect) {
                continue;
            }
            let view = self.views.entry(stroke.id).or_insert_with(|| View {
                source: stroke.clone(),
                path: None,
                pending: None,
            });
            if view.source != *stroke {
                *view = View {
                    source: stroke.clone(),
                    path: None,
                    pending: None,
                };
            }
            if let Some(result) = view.pending.as_ref().and_then(Task::poll) {
                view.pending = None;
                match result {
                    Ok(Ok(path)) => view.path = Some(Arc::new(path)),
                    Err(JobError::QueueFull) => {}
                    Ok(Err(error)) => self.error = Some(error),
                    Err(error) => self.error = Some(error.to_string()),
                }
            }
            if view.path.is_none() && view.pending.is_none() && self.error.is_none() {
                let source = stroke.clone();
                view.pending = Some(Task::start(
                    &super::artwork::READS,
                    Duration::from_secs(5),
                    move |_| InkPath::prepare(&source.points, source.width),
                ));
            }
            if view.pending.is_some() {
                ui.ctx().request_repaint_after(Duration::from_millis(16));
            }
            if let Some(path) = &view.path {
                paths.push((stroke, path.clone()));
            }
        }
        if let Some(current) = &self.current {
            paths.push((&current.stroke, current.path.clone()));
        }
        if let Err(error) = paint_paths(ui, viewport, paths, palette) {
            self.error = Some(error);
        }
    }
}
fn paint_paths(
    ui: &egui::Ui,
    viewport: &clip_studios::drawing_canvas::Viewport<'_>,
    paths: Vec<(&Stroke, Arc<InkPath>)>,
    palette: &little_clip_core::palette::AppPalette,
) -> Result<(), String> {
    if paths.is_empty() {
        return Ok(());
    }
    let scale = viewport.scale() as f32;
    let paths: Vec<_> = paths
        .into_iter()
        .map(|(stroke, path)| {
            (
                viewport.screen(stroke.origin),
                stroke.color.paint(palette, stroke.marker),
                path,
            )
        })
        .collect();
    clip_ui::vectors::paint(ui, viewport.rect, move |canvas| {
        use clip_ui::vectors::femtovg::{Paint, Path};
        for (origin, color, path) in &paths {
            let mut outline = Path::new();
            let point = |p: [f64; 2]| {
                (
                    origin.x + p[0] as f32 * scale,
                    origin.y + p[1] as f32 * scale,
                )
            };
            for command in &path.commands {
                match *command {
                    InkCommand::Move(p) => {
                        let (x, y) = point(p);
                        outline.move_to(x, y);
                    }
                    InkCommand::Line(p) => {
                        let (x, y) = point(p);
                        outline.line_to(x, y);
                    }
                    InkCommand::Quadratic { control, end } => {
                        let (cx, cy) = point(control);
                        let (x, y) = point(end);
                        outline.quad_to(cx, cy, x, y);
                    }
                    InkCommand::Close => outline.close(),
                }
            }
            canvas.fill_path(&outline, &Paint::color(*color));
        }
    })
}
fn live_at(board: &super::Board, world: [f64; 2]) -> bool {
    board.cards.iter().any(|card| {
        matches!(
            card.content,
            super::Content::Shortcut { .. }
                | super::Content::Folder { .. }
                | super::Content::Project { .. }
                | super::Content::Projects { .. }
        ) && {
            let b = card.bounds();
            [world[0] - b.x, world[1] - b.y]
                .into_iter()
                .zip([b.width, b.height])
                .all(|(value, size)| (0.0..=size).contains(&value))
        }
    })
}
impl super::Home {
    pub fn tablet_input(
        &mut self,
        frames: impl IntoIterator<Item = Frame>,
        zoom: f32,
        focused: bool,
    ) -> bool {
        let allowed = self.open
            && self.ink.enabled
            && !self.session.arrange
            && !self.region_ui.drafting()
            && self.editing_allowed()
            && focused;
        let mut suppress = self.ink.drawing();
        if !allowed {
            self.finish_ink();
            return suppress;
        }
        for frame in frames {
            suppress |= self.pen_frame(frame, zoom);
        }
        suppress
    }
    fn pen_frame(&mut self, frame: Frame, zoom: f32) -> bool {
        if self
            .ink
            .current
            .as_ref()
            .is_some_and(|g| g.tool != frame.tool)
        {
            return self.ink.drawing();
        }
        let point = frame
            .pulse
            .point
            .map(|p| egui::pos2(p[0] / zoom, p[1] / zoom));
        let world = point.and_then(|p| {
            self.drop_rect
                .and_then(|rect| self.session.canvas.world_in(rect, p))
        });
        if frame.pulse.started
            && let Some(world) = world
        {
            self.finish_ink();
            let live = live_at(&self.session.board, world);
            if !live {
                self.begin_ink(&frame, world);
            }
        }
        let suppress = self.ink.drawing();
        if frame.pulse.contact
            && let Some(world) = world
        {
            self.ink.sample(
                &frame,
                world,
                self.session.canvas.view().map_or(1., |v| v.scale),
            );
        }
        if frame.pulse.ended {
            self.finish_ink();
        } else if frame.pulse.contact
            && self
                .ink
                .current
                .as_ref()
                .is_some_and(|g| g.stroke.points.len() >= g.capacity)
        {
            self.finish_ink();
            if let Some(world) = world {
                self.begin_ink(&frame, world);
            }
        }
        suppress
    }
    fn begin_ink(&mut self, frame: &Frame, world: [f64; 2]) {
        let remaining = POINT_BUDGET.saturating_sub(
            self.session
                .board
                .ink
                .iter()
                .map(|s| s.points.len())
                .sum::<usize>(),
        );
        if remaining >= 2 && self.session.board.ink.len() < 256 && !self.ink.drawing() {
            self.ink.begin(frame, world, remaining.min(4096));
        } else {
            self.ink.error = Some("Home ink is full; Undo or clear ink before drawing more".into());
        }
    }
    pub fn finish_ink(&mut self) {
        let used = self
            .session
            .board
            .ink
            .iter()
            .map(|s| s.points.len())
            .sum::<usize>();
        if self.ink.current.as_ref().is_some_and(|g| {
            used + g.stroke.points.len() > POINT_BUDGET || self.session.board.ink.len() >= 256
        }) {
            self.ink.error = Some(
                "Home changed while drawing; keep or explicitly discard the unfinished stroke"
                    .into(),
            );
            return;
        }
        if let Some(stroke) = self.ink.finish() {
            self.session.edit(|board| board.ink.push(stroke));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn stroke() -> Stroke {
        Stroke {
            id: Uuid::new_v4(),
            origin: [20., 40.],
            points: vec![[0., 0., 0.1], [100., 0., 0.9]],
            width: 1.5,
            color: Color::Theme,
            marker: false,
        }
    }
    #[test]
    fn ink_persists_actual_pressure_and_layout_undo_keeps_it_independent_of_artwork() {
        let ink = stroke();
        assert!(ink.valid());
        let mut session = super::super::Session::default();
        session.install(&super::super::Board::default());
        let source = session.board.drawing.clone();
        session.edit(|board| board.ink.push(ink.clone()));
        let saved = serde_json::to_vec(&session.board).unwrap();
        let restored: super::super::Board = serde_json::from_slice(&saved).unwrap();
        assert_eq!(restored.ink, vec![ink.clone()]);
        restored.validate(&[]).unwrap();
        session.undo();
        assert!(session.board.ink.is_empty());
        session.redo();
        assert_eq!(session.board.ink, vec![ink]);
        assert_eq!(session.board.drawing, source);
    }
    #[test]
    fn invalid_pressure_and_duplicate_identities_cannot_enter_saved_home() {
        let ink = stroke();
        assert!(validate(&[ink.clone(), ink.clone()]).is_err());
        for pressure in [f64::NAN, -0.1, 1.1] {
            let mut invalid = ink.clone();
            invalid.points[0][2] = pressure;
            assert!(!invalid.valid());
        }
        let mut oversized = ink;
        oversized.points = vec![[0., 0., 0.5]; 4097];
        assert!(!oversized.valid());
    }
    #[test]
    fn pen_taps_operate_shortcuts_while_artwork_remains_a_drawing_surface() {
        let pin = Uuid::new_v4();
        let mut board = super::super::Board {
            cards: vec![super::super::Card {
                id: Uuid::new_v4(),
                rect: [0., 0., 100., 100.],
                content: super::super::Content::Shortcut {
                    pin,
                    label: "Folder".into(),
                },
            }],
            ..Default::default()
        };
        assert!(live_at(&board, [50., 50.]));
        board.cards[0].content = super::super::Content::Artwork {
            pin,
            label: "Artwork".into(),
        };
        assert!(!live_at(&board, [50., 50.]));
    }
    #[test]
    fn maximum_stroke_prepares_bounded_geometry_with_measured_pressure() {
        let points: Vec<_> = (0..4096)
            .map(|i| {
                [
                    f64::from(i) * 0.25,
                    (f64::from(i) * 0.05).sin() * 10.,
                    f64::from(i) / 4095.,
                ]
            })
            .collect();
        let start = std::time::Instant::now();
        let path = InkPath::prepare(&points, 9.).unwrap();
        eprintln!(
            "4096 measured ink points: {:?}, {} path commands",
            start.elapsed(),
            path.commands.len()
        );
        assert!(!path.commands.is_empty() && path.commands.len() <= 65_536);
    }
    #[test]
    fn neutral_ink_changes_only_presentation_when_switching_themes() {
        use clip_ui::ui_appearance;
        use little_clip_core::config::WorkbenchAppearance;
        let light = ui_appearance::palette(WorkbenchAppearance::Light);
        let dark = ui_appearance::palette(WorkbenchAppearance::Dark);
        assert_ne!(
            Color::Theme.paint(&light, false),
            Color::Theme.paint(&dark, false)
        );
        assert_eq!(
            Color::Teal.paint(&light, false),
            Color::Teal.paint(&dark, false)
        );
        assert_ne!(
            Color::Teal.paint(&light, false),
            Color::Teal.paint(&light, true)
        );
    }
}
