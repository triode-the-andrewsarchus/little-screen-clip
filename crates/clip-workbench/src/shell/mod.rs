//! Workbench mirror host and compatibility exports for the shared studio shell.
pub use clip_studios::shell::*;
pub mod screen_mirror;
pub use screen_mirror::{MirrorAction, ScreenMirrorWidget, slot_at_x, slot_rects};

pub(crate) mod native_mirror;
