//! Unified StudioShell UI system module.

pub mod escape_hatch;
pub mod panel;
pub mod screen_mirror;
pub mod shortcuts;
pub mod split_compare;
pub mod studio_controls;
pub mod studio_shell;

pub use escape_hatch::{EscapeHatch, EscapeHatchLauncher};
pub use panel::{
    PANEL_BOTTOM_DEFAULT_HEIGHT, PANEL_LEFT_DEFAULT_WIDTH, PANEL_MAX_HEIGHT, PANEL_MAX_WIDTH,
    PANEL_MIN_HEIGHT, PANEL_MIN_WIDTH, PANEL_RIGHT_DEFAULT_WIDTH, PanelState,
};
pub use screen_mirror::{MirrorAction, ScreenMirrorWidget, slot_at_x, slot_rects};
pub use shortcuts::{CommonShortcut, ShortcutDispatcher};
pub use split_compare::SplitCompareView;
pub use studio_controls::render_context_slider;
pub use studio_shell::{HeaderBadge, PanelBody, StudioPanelLayout, StudioPanelRects, StudioShell};
