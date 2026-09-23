//! Standard multi-panel layout design tokens and per-session sidebar state.
//!
//! Provides standardized widths for left sidebars (e.g. presets, thumbnails, sheets),
//! right sidebars (e.g. inspectors, property sliders, diff lists), and bottom drawers,
//! replacing ad-hoc magic numbers across workbench tools.

use serde::{Deserialize, Serialize};

pub const PANEL_LEFT_DEFAULT_WIDTH: f32 = 240.0;
pub const PANEL_RIGHT_DEFAULT_WIDTH: f32 = 280.0;
pub const PANEL_BOTTOM_DEFAULT_HEIGHT: f32 = 96.0;

pub const PANEL_MIN_WIDTH: f32 = 160.0;
pub const PANEL_MAX_WIDTH: f32 = 500.0;
pub const PANEL_MIN_HEIGHT: f32 = 60.0;
pub const PANEL_MAX_HEIGHT: f32 = 300.0;

/// Interactive sizing and collapsed state for multi-panel workbench studio views.
/// Preferences remain in the owning studio session; disk persistence is separate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PanelState {
    pub left_width: f32,
    pub right_width: f32,
    pub bottom_height: f32,
    pub left_collapsed: bool,
    pub right_collapsed: bool,
    pub bottom_collapsed: bool,
}

impl Default for PanelState {
    fn default() -> Self {
        Self {
            left_width: PANEL_LEFT_DEFAULT_WIDTH,
            right_width: PANEL_RIGHT_DEFAULT_WIDTH,
            bottom_height: PANEL_BOTTOM_DEFAULT_HEIGHT,
            left_collapsed: false,
            right_collapsed: false,
            bottom_collapsed: false,
        }
    }
}

impl PanelState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Effective left panel width considering collapsed state.
    pub fn effective_left_width(&self) -> f32 {
        if self.left_collapsed {
            0.0
        } else {
            self.left_width.clamp(PANEL_MIN_WIDTH, PANEL_MAX_WIDTH)
        }
    }

    /// Effective right panel width considering collapsed state.
    pub fn effective_right_width(&self) -> f32 {
        if self.right_collapsed {
            0.0
        } else {
            self.right_width.clamp(PANEL_MIN_WIDTH, PANEL_MAX_WIDTH)
        }
    }

    /// Effective bottom drawer height considering collapsed state.
    pub fn effective_bottom_height(&self) -> f32 {
        if self.bottom_collapsed {
            0.0
        } else {
            self.bottom_height.clamp(PANEL_MIN_HEIGHT, PANEL_MAX_HEIGHT)
        }
    }

    /// Toggle left sidebar visibility.
    pub fn toggle_left(&mut self) {
        self.left_collapsed = !self.left_collapsed;
    }

    /// Toggle right sidebar visibility.
    pub fn toggle_right(&mut self) {
        self.right_collapsed = !self.right_collapsed;
    }

    /// Toggle bottom panel visibility.
    pub fn toggle_bottom(&mut self) {
        self.bottom_collapsed = !self.bottom_collapsed;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_panel_state_defaults() {
        let state = PanelState::default();
        assert_eq!(state.effective_left_width(), PANEL_LEFT_DEFAULT_WIDTH);
        assert_eq!(state.effective_right_width(), PANEL_RIGHT_DEFAULT_WIDTH);
        assert_eq!(state.effective_bottom_height(), PANEL_BOTTOM_DEFAULT_HEIGHT);
    }

    #[test]
    fn test_panel_collapse_toggles() {
        let mut state = PanelState::default();
        state.toggle_left();
        assert_eq!(state.effective_left_width(), 0.0);
        state.toggle_left();
        assert_eq!(state.effective_left_width(), PANEL_LEFT_DEFAULT_WIDTH);
    }
}
