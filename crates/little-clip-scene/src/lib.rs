//! Shared native scene rendering for workbench viewports and finite display previews.
//!
//! GPU projection, spherical harmonics, sorting and blending come from
//! `wgpu-3dgs-viewer`. Callers run loading and readback on background workers.

mod glb;
mod orbit;
mod renderer;
mod sh_rotation;
mod sog;
mod spark;
mod splat;
mod spz;

pub use orbit::{ORBIT_FRAME_COUNT, OrbitLoop};
pub use renderer::{OrbitCamera, SplatRenderer};
pub use splat::SplatScene;

/// Reads a container without allowing a growing file to exceed its byte budget.
pub(crate) fn read_bounded(
    reader: impl std::io::Read,
    limit: u64,
    format: &str,
) -> Result<Vec<u8>, SceneError> {
    use std::io::Read;
    let mut bytes = Vec::new();
    reader.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(SceneError::Invalid(format!(
            "{format} exceeds its {limit}-byte import budget"
        )));
    }
    Ok(bytes)
}

/// Returns whether a path uses a Gaussian-capable scene container extension.
///
/// GLB may instead contain meshes; loading classifies it on the worker thread.
pub fn is_splat_file(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|value| value.to_str())
        .is_some_and(|extension| {
            ["ply", "splat", "sog", "spz", "glb"]
                .iter()
                .any(|candidate| extension.eq_ignore_ascii_case(candidate))
        })
}

/// Scene loading or rendering failure.
#[derive(Debug, thiserror::Error)]
pub enum SceneError {
    #[error("Scene I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("Invalid scene: {0}")]
    Invalid(String),
    #[error("3D rendering unavailable: {0}")]
    Gpu(String),
    #[error("GLB contains ordinary mesh data")]
    MeshGlb,
    #[error("Scene rendering cancelled")]
    Cancelled,
}
