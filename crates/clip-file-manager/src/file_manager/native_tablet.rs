//! UI-thread tablet ownership on eframe's borrowed display; never synthesize pen input.
use octotablet::{
    events::{Event, ToolEvent},
    tool,
};
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Pulse {
    pub point: Option<[f32; 2]>,
    pub pressure: Option<f32>,
    pub started: bool,
    pub ended: bool,
    pub contact: bool,
}
impl Pulse {
    pub fn pointer_events(
        self,
        zoom: f32,
        modifiers: egui::Modifiers,
        captured: bool,
    ) -> Vec<egui::Event> {
        let Some(point) = self.point else {
            return Vec::new();
        };
        let pos = egui::pos2(point[0] / zoom, point[1] / zoom);
        let mut events = vec![egui::Event::PointerMoved(pos)];
        for (changed, pressed) in [(self.started, true), (self.ended, false)] {
            if changed && !captured {
                events.push(egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers,
                });
            }
        }
        events
    }
}
pub(super) struct Frame {
    pub tool: tool::ID,
    pub pulse: Pulse,
}
#[derive(Default)]
struct ToolState {
    pulse: Pulse,
    proximity: bool,
    clear_pose: bool,
}
impl ToolState {
    // Events within a tablet frame are simultaneous; Down and Pose may arrive
    // in either order. Publish only at Frame, never on individual axis events.
    fn update(&mut self, event: ToolEvent<'_>) -> Option<Pulse> {
        match event {
            ToolEvent::In { .. } => self.proximity = true,
            ToolEvent::Down => {
                self.pulse.started |= !self.pulse.contact;
                self.pulse.contact = true;
            }
            ToolEvent::Up => {
                self.pulse.ended |= self.pulse.contact;
                self.pulse.contact = false;
            }
            ToolEvent::Out | ToolEvent::Removed => {
                self.proximity = false;
                self.clear_pose = true;
                self.pulse.ended |= self.pulse.contact;
                self.pulse.contact = false;
            }
            ToolEvent::Pose(pose) => {
                self.pulse.point = pose
                    .position
                    .iter()
                    .all(|v| v.is_finite())
                    .then_some(pose.position);
                self.pulse.pressure = pose
                    .pressure
                    .get()
                    .filter(|v| v.is_finite() && (0.0..=1.0).contains(v));
            }
            ToolEvent::Frame(_) => {
                let pulse = self.pulse;
                self.pulse.started = false;
                self.pulse.ended = false;
                if std::mem::take(&mut self.clear_pose) {
                    self.pulse.point = None;
                    self.pulse.pressure = None;
                }
                return Some(pulse);
            }
            _ => {}
        }
        None
    }
}
fn pressure_pen(tool: &tool::Tool) -> bool {
    tool.axes.pressure.is_some()
        && matches!(
            tool.tool_type,
            Some(tool::Type::Pen | tool::Type::Pencil | tool::Type::Brush | tool::Type::Airbrush)
        )
}

pub(super) struct NativeTablet {
    manager: octotablet::Manager,
    tools: HashMap<tool::ID, ToolState>,
    pub available: bool,
    pub connected: bool,
    pub proximity: bool,
    pub last_point: Option<[f32; 2]>,
}
impl NativeTablet {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Option<Self> {
        // SAFETY: eframe owns this display/window until after the app is dropped.
        // The manager stays on the app's UI thread, is explicitly taken in
        // on_exit, and its foreign backend never disconnects eframe's display.
        // No raw handle escapes into a worker or a separately owned window.
        let manager = unsafe {
            octotablet::builder::Builder::new()
                .emulate_tool_from_mouse(false)
                .build_raw(cc)
        }
        .ok()?;
        Some(Self {
            manager,
            tools: HashMap::new(),
            available: false,
            connected: false,
            proximity: false,
            last_point: None,
        })
    }
    pub fn pump(&mut self) -> Result<Vec<Frame>, String> {
        let mut frames = Vec::new();
        for event in self.manager.pump().map_err(|error| error.to_string())? {
            let Event::Tool { tool, event } = event else {
                continue;
            };
            if !pressure_pen(tool) {
                continue;
            }
            let id = tool.id();
            if !self.tools.contains_key(&id) && self.tools.len() >= 16 {
                continue;
            }
            let state = self.tools.entry(id.clone()).or_default();
            if let Some(pulse) = state.update(event) {
                if frames.len() >= 1024 {
                    return Err("Tablet input exceeded its frame budget".into());
                }
                self.last_point = pulse.point.or(self.last_point);
                frames.push(Frame {
                    tool: id.clone(),
                    pulse,
                });
            }
            if matches!(event, ToolEvent::Removed) {
                if state.pulse.ended {
                    frames.push(Frame {
                        tool: id.clone(),
                        pulse: state.pulse,
                    });
                }
                self.tools.remove(&id);
            }
        }
        self.available = self.manager.tools().iter().any(pressure_pen);
        self.connected = !self.manager.tablets().is_empty();
        self.proximity = self.tools.values().any(|tool| tool.proximity);
        Ok(frames)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn pose(pressure: f32) -> ToolEvent<'static> {
        ToolEvent::Pose(octotablet::axis::Pose {
            position: [12., 34.],
            pressure: Some(pressure).try_into().unwrap(),
            ..Default::default()
        })
    }
    #[test]
    fn pressure_and_contact_publish_together_regardless_of_event_order() {
        for pose_first in [false, true] {
            let mut state = ToolState::default();
            if pose_first {
                state.update(pose(0.2));
            }
            state.update(ToolEvent::Down);
            if !pose_first {
                state.update(pose(0.2));
            }
            let down = state.update(ToolEvent::Frame(None)).unwrap();
            assert!(down.started && down.contact);
            assert_eq!(down.point, Some([12., 34.]));
            assert_eq!(down.pressure, Some(0.2));
            state.update(pose(0.9));
            let motion = state.update(ToolEvent::Frame(None)).unwrap();
            assert!(!motion.started && motion.contact);
            assert_eq!(motion.pressure, Some(0.9));
            state.update(ToolEvent::Up);
            let up = state.update(ToolEvent::Frame(None)).unwrap();
            assert!(up.ended && !up.contact);
            assert!(!state.update(ToolEvent::Frame(None)).unwrap().ended);
        }
    }
    #[test]
    fn uncaptured_pen_controls_use_window_coordinates_and_a_balanced_click() {
        let pulse = Pulse {
            point: Some([40., 80.]),
            started: true,
            ended: true,
            ..Default::default()
        };
        let events = pulse.pointer_events(2., egui::Modifiers::NONE, false);
        assert_eq!(events.len(), 3);
        assert_eq!(
            pulse.pointer_events(2., egui::Modifiers::NONE, true).len(),
            1
        );
        assert!(matches!(events[0],egui::Event::PointerMoved(p) if p==egui::pos2(20.,40.)));
        assert!(matches!(
            events[1],
            egui::Event::PointerButton { pressed: true, .. }
        ));
        assert!(matches!(
            events[2],
            egui::Event::PointerButton { pressed: false, .. }
        ));
        assert!(
            Pulse::default()
                .pointer_events(1., egui::Modifiers::NONE, false)
                .is_empty()
        );
    }
    #[test]
    fn missing_pressure_is_never_replaced_by_a_mouse_like_constant() {
        let mut state = ToolState::default();
        state.update(ToolEvent::Pose(octotablet::axis::Pose::default()));
        state.update(ToolEvent::Down);
        assert!(
            state
                .update(ToolEvent::Frame(None))
                .unwrap()
                .pressure
                .is_none()
        );
        state.update(ToolEvent::Out);
        assert!(state.update(ToolEvent::Frame(None)).unwrap().ended);
        assert!(
            state
                .update(ToolEvent::Frame(None))
                .unwrap()
                .point
                .is_none()
        );
    }
}
