//! 3D STL mesh backward-compatibility module.
//!
//! Re-exports [`Model3dViewerState`] as [`StlViewerState`].

pub use crate::model3d_view::Model3dViewerState;

/// Backwards-compatible type alias for [`Model3dViewerState`].
#[allow(dead_code)]
pub type StlViewerState = Model3dViewerState;
