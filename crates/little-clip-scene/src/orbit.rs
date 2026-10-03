//! Finite CPU playback with an owned temporary frame directory.

use crate::SceneError;
use image::RgbaImage;
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

/// Number of distinct views: 20 frames per second over a 12-second orbit.
pub const ORBIT_FRAME_COUNT: usize = 240;

/// Temporary PNG frames whose playback requires no GPU scene allocation.
pub struct OrbitLoop {
    directory: tempfile::TempDir,
    frames: Vec<PathBuf>,
    disk_bytes: usize,
    decoded: Mutex<Option<(usize, Arc<RgbaImage>)>>,
}

impl OrbitLoop {
    /// Bakes one frame at a time into an owned directory explicitly under `/tmp`.
    ///
    /// # Errors
    /// Returns an error for excessive dimensions, inconsistent frames, rendering
    /// failures or temporary-file errors. Failure removes partially baked frames.
    pub fn bake_frames(
        width: u32,
        height: u32,
        render: impl FnMut(usize) -> Result<RgbaImage, SceneError>,
    ) -> Result<Self, SceneError> {
        let bytes = u64::from(width)
            .checked_mul(u64::from(height))
            .and_then(|bytes| bytes.checked_mul(4));
        if !bytes.is_some_and(|bytes| {
            bytes > 0 && bytes <= (256 * 1024 * 1024) / ORBIT_FRAME_COUNT as u64
        }) {
            return Err(SceneError::Invalid(
                "Orbit exceeds the 256 MiB raster budget".into(),
            ));
        }
        let directory = tempfile::Builder::new()
            .prefix("little-clip-orbit-")
            .tempdir_in("/tmp")?;
        Self::bake_in(directory, width, height, render)
    }

    fn bake_in(
        directory: tempfile::TempDir,
        width: u32,
        height: u32,
        mut render: impl FnMut(usize) -> Result<RgbaImage, SceneError>,
    ) -> Result<Self, SceneError> {
        let mut orbit = Self {
            directory,
            frames: Vec::with_capacity(ORBIT_FRAME_COUNT),
            disk_bytes: 0,
            decoded: Mutex::new(None),
        };
        for index in 0..ORBIT_FRAME_COUNT {
            let image = render(index)?;
            if image.dimensions() != (width, height) {
                return Err(SceneError::Invalid("Orbit frame dimensions changed".into()));
            }
            let path = orbit.directory.path().join(format!("{index:03}.png"));
            image
                .save(&path)
                .map_err(|error| SceneError::Invalid(error.to_string()))?;
            orbit.disk_bytes += path.metadata()?.len() as usize;
            orbit.frames.push(path);
        }
        Ok(orbit)
    }

    /// Retains a finite orbit produced by another renderer in temporary storage.
    ///
    /// # Errors
    /// Returns an error unless all frames have identical nonzero dimensions and
    /// the required frame count, or writing their temporary files fails.
    pub fn from_frames(frames: Vec<RgbaImage>) -> Result<Self, SceneError> {
        let size = frames
            .first()
            .ok_or_else(|| SceneError::Invalid("Orbit has no frames".into()))?
            .dimensions();
        if frames.len() != ORBIT_FRAME_COUNT {
            return Err(SceneError::Invalid(format!(
                "Orbit requires {ORBIT_FRAME_COUNT} frames"
            )));
        }
        let mut frames = frames.into_iter();
        Self::bake_frames(size.0, size.1, |_| {
            Ok(frames.next().expect("validated frame count"))
        })
    }

    /// Returns the ordered temporary frame paths, without a duplicated endpoint.
    pub fn frames(&self) -> &[PathBuf] {
        &self.frames
    }

    /// Returns the compressed bytes retained in temporary storage.
    pub fn disk_bytes(&self) -> usize {
        self.disk_bytes
    }

    /// Selects a frame index over a 12-second repeating orbit.
    pub fn frame_index(elapsed: Duration) -> usize {
        ((elapsed.as_millis() % 12_000) * ORBIT_FRAME_COUNT as u128 / 12_000) as usize
    }

    /// Reads a CPU frame, keeping only the most recently decoded raster in RAM.
    ///
    /// Call on a rendering worker because a changed frame requires PNG decoding.
    ///
    /// # Errors
    /// Returns an error for an invalid index or an unreadable temporary frame.
    pub fn read_frame(&self, index: usize) -> Result<Arc<RgbaImage>, SceneError> {
        let mut decoded = self
            .decoded
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some((previous, image)) = &*decoded
            && *previous == index
        {
            return Ok(Arc::clone(image));
        }
        let path = self
            .frames
            .get(index)
            .ok_or_else(|| SceneError::Invalid("Invalid orbit frame index".into()))?;
        let image = Arc::new(
            image::open(path)
                .map_err(|error| SceneError::Invalid(error.to_string()))?
                .into_rgba8(),
        );
        *decoded = Some((index, Arc::clone(&image)));
        Ok(image)
    }

    /// Reads the cached CPU view for a point in the repeating orbit.
    ///
    /// # Errors
    /// Returns an error if its temporary frame cannot be read.
    pub fn frame_at(&self, elapsed: Duration) -> Result<Arc<RgbaImage>, SceneError> {
        self.read_frame(Self::frame_index(elapsed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orbit_uses_tmp_wraps_at_twenty_fps_and_removes_files_on_drop() {
        let orbit = OrbitLoop::bake_frames(1, 1, |i| {
            Ok(RgbaImage::from_pixel(
                1,
                1,
                image::Rgba([i as u8, 0, 0, 255]),
            ))
        })
        .unwrap();
        let directory = orbit.directory.path().to_owned();
        assert_eq!(directory.parent(), Some(std::path::Path::new("/tmp")));
        assert_eq!(orbit.frames().len(), ORBIT_FRAME_COUNT);
        assert_eq!(OrbitLoop::frame_index(Duration::from_millis(49)), 0);
        assert_eq!(OrbitLoop::frame_index(Duration::from_millis(50)), 1);
        assert_eq!(
            orbit.frame_at(Duration::from_millis(11_999)).unwrap()[(0, 0)][0],
            239
        );
        assert_eq!(
            orbit.frame_at(Duration::from_secs(12)).unwrap()[(0, 0)][0],
            0
        );
        let first = orbit.read_frame(0).unwrap();
        assert!(Arc::ptr_eq(&first, &orbit.read_frame(0).unwrap()));
        drop(orbit);
        assert!(!directory.exists());
        assert_eq!(first[(0, 0)][0], 0);
    }

    #[test]
    fn cancelled_or_invalid_bake_removes_partially_written_frames() {
        for failure in [
            SceneError::Cancelled,
            SceneError::Invalid("render failed".into()),
        ] {
            let directory = tempfile::tempdir_in("/tmp").unwrap();
            let path = directory.path().to_owned();
            let mut failure = Some(failure);
            let result = OrbitLoop::bake_in(directory, 1, 1, |index| {
                if index == 3 {
                    return Err(failure.take().unwrap());
                }
                Ok(RgbaImage::from_pixel(1, 1, image::Rgba([0, 0, 0, 255])))
            });
            assert!(result.is_err());
            assert!(!path.exists());
        }
        assert!(OrbitLoop::bake_frames(u32::MAX, u32::MAX, |_| unreachable!()).is_err());
    }
}
