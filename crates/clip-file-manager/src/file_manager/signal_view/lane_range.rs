use crate::file_manager::scope_view::plot;
use egui::{Painter, PointerButton, Pos2, Rect, Response, Ui};
use serde::{Deserialize, Serialize};
use std::ops::Range;

#[derive(Clone, Copy, Debug)]
pub(in crate::file_manager) struct Drag {
    pub start: Pos2,
    pub end: Pos2,
    pub all: bool,
    pub finished: bool,
}

impl Drag {
    pub fn read(ui: &Ui, response: &Response) -> Option<Self> {
        let finished = response.drag_stopped_by(PointerButton::Primary);
        if !finished && !response.dragged_by(PointerButton::Primary) {
            return None;
        }
        let (origin, latest, all) = ui.input(|input| {
            (
                input.pointer.press_origin(),
                input.pointer.latest_pos(),
                input.modifiers.shift,
            )
        });
        let id = response.id.with("lane-range-origin");
        let start = ui.data_mut(|data| {
            if finished {
                let saved = data.get_temp::<Pos2>(id);
                data.remove::<Pos2>(id);
                saved.or(origin)
            } else {
                let start = origin?;
                data.insert_temp(id, start);
                Some(start)
            }
        })?;
        let end = response.interact_pointer_pos().or(latest)?;
        Some(Self {
            start,
            end,
            all,
            finished,
        })
    }

    pub fn times(self, rect: Rect, visible: &Range<f64>) -> [f64; 2] {
        [
            plot::at(rect, visible, self.start.x),
            plot::at(rect, visible, self.end.x),
        ]
    }

    pub fn targets<K: Copy>(self, plots: impl IntoIterator<Item = (K, Rect)>) -> Vec<K> {
        let top = self.start.y.min(self.end.y);
        let bottom = self.start.y.max(self.end.y);
        plots
            .into_iter()
            .filter(|(_, rect)| self.all || (rect.top() <= bottom && rect.bottom() >= top))
            .map(|(key, _)| key)
            .collect()
    }
}

pub(in crate::file_manager) struct Draft<K> {
    pub times: [f64; 2],
    pub lanes: Vec<K>,
}

impl<K: Copy> Draft<K> {
    pub fn new(
        drag: Drag,
        rect: Rect,
        span: &Range<f64>,
        plots: impl IntoIterator<Item = (K, Rect)>,
    ) -> Self {
        Self {
            times: drag.times(rect, span),
            lanes: drag.targets(plots),
        }
    }

    pub fn interval(&self) -> Range<f64> {
        interval(self.times)
    }
}

pub(in crate::file_manager) fn interval([a, b]: [f64; 2]) -> Range<f64> {
    a.min(b)..a.max(b)
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub(in crate::file_manager) struct Scope<K> {
    // An absent lane list preserves the representation of older recipes.
    pub lanes: Option<Vec<K>>,
    pub color: Option<usize>,
}

impl<K> Default for Scope<K> {
    fn default() -> Self {
        Self {
            lanes: None,
            color: None,
        }
    }
}

impl<K: Copy + PartialEq> Scope<K> {
    pub fn new(lanes: Vec<K>, color: Option<usize>) -> Self {
        Self {
            lanes: Some(lanes),
            color,
        }
    }

    pub fn includes(&self, key: K) -> bool {
        self.lanes.as_ref().is_none_or(|lanes| lanes.contains(&key))
    }

    /// Returns whether any part of the selection remains.
    pub fn remove(&mut self, key: K) -> bool {
        match self.lanes.as_mut() {
            Some(lanes) => {
                lanes.retain(|&lane| lane != key);
                !lanes.is_empty()
            }
            None => false,
        }
    }

    pub fn legacy(&self) -> bool {
        self.lanes.is_none() && self.color.is_none()
    }
}

#[derive(Clone, Serialize, Deserialize)]
struct Mark<K> {
    time: Range<f64>,
    scope: Scope<K>,
}

#[derive(Clone, Serialize, Deserialize)]
pub(in crate::file_manager) struct Marks<K> {
    pub active: Scope<K>,
    earlier: Vec<Mark<K>>,
}

impl<K> Default for Marks<K> {
    fn default() -> Self {
        Self {
            active: Scope::default(),
            earlier: Vec::new(),
        }
    }
}

impl<K: Copy + PartialEq> Marks<K> {
    pub fn archive(&mut self, selected: &mut Option<Range<f64>>) {
        let scope = std::mem::take(&mut self.active);
        if let Some(time) = selected.take() {
            self.earlier.push(Mark { time, scope });
        }
    }

    pub fn select(
        &mut self,
        selected: &mut Option<Range<f64>>,
        range: Range<f64>,
        lanes: Vec<K>,
        color: Option<usize>,
    ) -> bool {
        if range.is_empty() || lanes.is_empty() {
            return false;
        }
        let color = color.unwrap_or_else(|| self.next_color(selected.is_some()));
        self.archive(selected);
        *selected = Some(range);
        self.active = Scope::new(lanes, Some(color));
        true
    }

    pub fn clear_lane(&mut self, selected: &mut Option<Range<f64>>, key: K, time: f64) {
        if selected.as_ref().is_some_and(|range| contains(range, time)) && self.active.includes(key)
        {
            if !self.active.remove(key) {
                *selected = None;
                self.active = Scope::default();
            }
        } else if let Some(index) = self
            .earlier
            .iter()
            .rposition(|mark| mark.scope.includes(key) && contains(&mark.time, time))
            && !self.earlier[index].scope.remove(key)
        {
            self.earlier.remove(index);
        }
    }

    pub fn empty(&self) -> bool {
        self.earlier.is_empty() && self.active.legacy()
    }

    pub fn next_color(&self, selected: bool) -> usize {
        self.earlier.len() + usize::from(selected)
    }

    pub fn paint(
        &self,
        painter: &Painter,
        rect: Rect,
        visible: &Range<f64>,
        selection: Option<&Range<f64>>,
        key: K,
        draft: Option<&Draft<K>>,
    ) {
        let earlier = self.earlier.iter().map(|mark| (&mark.time, &mark.scope));
        for (index, (range, scope)) in earlier
            .chain(selection.map(|range| (range, &self.active)))
            .enumerate()
        {
            if scope.includes(key) {
                paint(painter, rect, visible, range, scope.color.unwrap_or(index));
            }
        }
        if let Some(draft) = draft.filter(|draft| draft.lanes.contains(&key)) {
            paint(
                painter,
                rect,
                visible,
                &draft.interval(),
                self.next_color(selection.is_some()),
            );
        }
    }
}

fn contains(range: &Range<f64>, time: f64) -> bool {
    range.start <= time && time <= range.end
}

pub(in crate::file_manager) fn paint(
    painter: &Painter,
    rect: Rect,
    visible: &Range<f64>,
    range: &Range<f64>,
    color: usize,
) {
    plot::highlight(
        &painter.with_clip_rect(rect),
        rect,
        visible,
        range.clone(),
        plot::color(color).gamma_multiply(0.25),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{pos2, vec2};

    fn plot_at(y: f32) -> Rect {
        Rect::from_min_size(pos2(10.0, y), vec2(100.0, 14.0))
    }

    #[test]
    fn diagonal_brush_uses_the_same_interval_on_each_crossed_lane_in_both_directions() {
        let plots = [
            (7, plot_at(20.0)),
            (2, plot_at(50.0)),
            (6, plot_at(80.0)),
            (4, plot_at(110.0)),
        ];
        let mut drag = Drag {
            start: pos2(30.0, 23.0),
            end: pos2(80.0, 86.0),
            all: false,
            finished: true,
        };
        let domain = 10.0..20.0;
        let span = interval(drag.times(plots[0].1, &domain));
        assert_eq!(drag.targets(plots), [7, 2, 6]);
        std::mem::swap(&mut drag.start, &mut drag.end);
        assert_eq!(drag.targets(plots), [7, 2, 6]);
        assert_eq!(interval(drag.times(plots[2].1, &domain)), span);
    }

    #[test]
    fn horizontal_drag_is_local_and_shift_uses_only_available_lanes() {
        let shown = [(0, plot_at(20.0)), (2, plot_at(50.0)), (3, plot_at(80.0))];
        let mut drag = Drag {
            start: pos2(30.0, 57.0),
            end: pos2(80.0, 57.0),
            all: false,
            finished: true,
        };
        assert_eq!(drag.targets(shown), [2]);
        drag.all = true;
        assert_eq!(drag.targets(shown), [0, 2, 3]);
    }

    #[test]
    fn each_lanes_part_of_an_archived_range_can_be_removed_independently() {
        let mut marks = Marks::default();
        let mut active = None;
        assert!(marks.select(&mut active, 1.0..2.0, vec![0, 2, 3], None));
        assert!(marks.select(&mut active, 3.0..4.0, vec![1], None));
        marks.clear_lane(&mut active, 2, 1.5);
        let restored: Marks<usize> =
            serde_json::from_value(serde_json::to_value(marks).unwrap()).unwrap();
        assert_eq!(
            restored.earlier[0].scope.lanes.as_deref(),
            Some([0, 3].as_slice())
        );
        assert_eq!(restored.active.lanes.as_deref(), Some([1].as_slice()));
        assert_eq!(active, Some(3.0..4.0));
    }
}
