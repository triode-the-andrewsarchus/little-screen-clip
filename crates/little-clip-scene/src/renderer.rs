use image::RgbaImage;
use std::sync::{Arc, OnceLock, mpsc};
use std::time::Duration;
use wgpu_3dgs_viewer::{
    self as gs,
    core::glam::{UVec2, Vec2, Vec3},
};

use crate::{SceneError, SplatScene};

/// Number of distinct views in a pre-rendered display orbit.
pub const ORBIT_FRAME_COUNT: usize = 30;

struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
}

static GPU: OnceLock<Result<Arc<Gpu>, String>> = OnceLock::new();

impl Gpu {
    fn shared() -> Result<Arc<Self>, SceneError> {
        GPU.get_or_init(|| pollster::block_on(Self::create()).map(Arc::new))
            .clone()
            .map_err(SceneError::Gpu)
    }

    async fn create() -> Result<Self, String> {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::from_env().unwrap_or(wgpu::Backends::all()),
            ..Default::default()
        });
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                ..Default::default()
            })
            .await
            .map_err(|e| e.to_string())?;
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("Little Clip scene renderer"),
                required_limits: adapter.limits(),
                ..Default::default()
            })
            .await
            .map_err(|e| e.to_string())?;
        Ok(Self { device, queue })
    }
}

/// Orbit camera parameters relative to the scene's initial framing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OrbitCamera {
    pub yaw: f32,
    pub pitch: f32,
    pub zoom: f32,
    /// Horizontal and vertical pan as fractions of the scene radius.
    pub pan: [f32; 2],
}

impl Default for OrbitCamera {
    fn default() -> Self {
        Self {
            yaw: 0.6,
            pitch: 0.35,
            zoom: 1.0,
            pan: [0.0; 2],
        }
    }
}

impl OrbitCamera {
    fn camera(self, scene: &SplatScene, aspect: f32) -> Result<gs::Camera, SceneError> {
        if !self.yaw.is_finite()
            || !self.pitch.is_finite()
            || !self.zoom.is_finite()
            || self.pan.iter().any(|x| !x.is_finite())
            || self.zoom <= 0.0
        {
            return Err(SceneError::Invalid("Invalid camera parameters".into()));
        }
        let fov = 50.0f32.to_radians();
        let limiting_fov = (fov * 0.5).tan().min((fov * 0.5).tan() * aspect).atan();
        let distance = scene.radius / limiting_fov.sin() / self.zoom.clamp(0.05, 50.0);
        let mut camera =
            gs::Camera::new(scene.radius * 0.001..distance + scene.radius * 100.0, fov);
        camera.pitch = -self.pitch.clamp(-1.5, 1.5);
        camera.yaw = self.yaw + std::f32::consts::PI;
        let pan = Vec2::from_array(self.pan) * scene.radius;
        let target = scene.center + camera.get_right() * pan.x + Vec3::Y * pan.y;
        camera.pos = target - camera.get_forward() * distance;
        Ok(camera)
    }
}

/// CPU-owned frames whose playback requires no renderer or GPU scene allocation.
pub struct OrbitLoop {
    frames: Vec<RgbaImage>,
}

impl OrbitLoop {
    /// Validates and retains a finite orbit produced by another scene renderer.
    ///
    /// # Errors
    /// Returns an error unless all 30 frames have identical nonzero dimensions
    /// and fit the preview pixel budget.
    pub fn from_frames(frames: Vec<RgbaImage>) -> Result<Self, SceneError> {
        let Some(first) = frames.first() else {
            return Err(SceneError::Invalid("Orbit has no frames".into()));
        };
        validate_orbit_size(first.width(), first.height())?;
        if frames.len() != ORBIT_FRAME_COUNT
            || frames
                .iter()
                .any(|frame| frame.dimensions() != first.dimensions())
        {
            return Err(SceneError::Invalid(
                "Orbit requires 30 equally sized frames".into(),
            ));
        }
        Ok(Self { frames })
    }

    /// Returns all 30 frames in orbit order, without a duplicate endpoint.
    pub fn frames(&self) -> &[RgbaImage] {
        &self.frames
    }

    /// Selects a cached frame from a 12-second repeating orbit.
    pub fn frame_at(&self, elapsed: Duration) -> &RgbaImage {
        let index = ((elapsed.as_millis() % 12_000) * ORBIT_FRAME_COUNT as u128 / 12_000) as usize;
        &self.frames[index]
    }
}

/// Offscreen Gaussian renderer backed by one process-wide GPU device.
pub struct SplatRenderer {
    gpu: Arc<Gpu>,
    viewer: gs::Viewer,
    scene: Arc<SplatScene>,
}

impl SplatRenderer {
    /// Uploads a scene once for interactive rendering or a finite orbit bake.
    ///
    /// # Errors
    /// Returns an error when no compatible GPU exists or the scene exceeds its limits.
    pub fn new(scene: Arc<SplatScene>) -> Result<Self, SceneError> {
        let gpu = Gpu::shared()?;
        let bytes = scene
            .len()
            .checked_mul(std::mem::size_of::<gs::DefaultGaussianPod>())
            .ok_or_else(|| SceneError::Invalid("Scene allocation overflow".into()))?
            as u64;
        let limits = gpu.device.limits();
        if bytes > limits.max_buffer_size
            || bytes > u64::from(limits.max_storage_buffer_binding_size)
            || scene.len() > u32::MAX as usize
        {
            return Err(SceneError::Gpu("Scene exceeds GPU buffer limits".into()));
        }
        let mut viewer = gs::Viewer::new(
            &gpu.device,
            wgpu::TextureFormat::Rgba8Unorm,
            &scene.gaussians,
        )
        .map_err(|e| SceneError::Gpu(e.to_string()))?;
        viewer.update_model_transform(
            &gpu.queue,
            Vec3::ZERO,
            gs::core::glam::Quat::IDENTITY,
            Vec3::ONE,
        );
        viewer.update_gaussian_transform_with_pod(
            &gpu.queue,
            &gs::core::GaussianTransformPod::default(),
        );
        Ok(Self { gpu, viewer, scene })
    }

    /// Renders one view and reads RGBA pixels back to CPU memory.
    ///
    /// Call this on a worker, because GPU readback blocks until completion.
    ///
    /// # Errors
    /// Returns an error for invalid dimensions or camera settings, or a GPU failure.
    pub fn render(
        &mut self,
        camera: OrbitCamera,
        width: u32,
        height: u32,
    ) -> Result<RgbaImage, SceneError> {
        let limits = self.gpu.device.limits();
        if width == 0
            || height == 0
            || width > limits.max_texture_dimension_2d
            || height > limits.max_texture_dimension_2d
        {
            return Err(SceneError::Invalid("Invalid scene viewport size".into()));
        }
        let row_bytes = width * 4;
        let padded_row_bytes = row_bytes.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
            * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let buffer_bytes = u64::from(padded_row_bytes) * u64::from(height);
        if buffer_bytes > limits.max_buffer_size {
            return Err(SceneError::Gpu(
                "Viewport exceeds GPU readback limits".into(),
            ));
        }
        let camera = camera.camera(&self.scene, width as f32 / height as f32)?;
        self.viewer
            .update_camera(&self.gpu.queue, &camera, UVec2::new(width, height));
        let texture = self.gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Scene frame"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let staging = self.gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Scene readback"),
            size: buffer_bytes,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self
            .gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        self.viewer.render(
            &mut encoder,
            &texture.create_view(&wgpu::TextureViewDescriptor::default()),
        );
        encoder.copy_texture_to_buffer(
            texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &staging,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_row_bytes),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        let submission = self.gpu.queue.submit([encoder.finish()]);
        let (tx, rx) = mpsc::sync_channel(1);
        staging
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                let _ = tx.send(result);
            });
        self.gpu
            .device
            .poll(wgpu::PollType::Wait {
                submission_index: Some(submission),
                timeout: Some(Duration::from_secs(30)),
            })
            .map_err(|e| SceneError::Gpu(e.to_string()))?;
        rx.recv_timeout(Duration::from_secs(1))
            .map_err(|e| SceneError::Gpu(e.to_string()))?
            .map_err(|e| SceneError::Gpu(e.to_string()))?;
        let mapped = staging.slice(..).get_mapped_range();
        let mut pixels = Vec::with_capacity(row_bytes as usize * height as usize);
        for row in mapped.chunks_exact(padded_row_bytes as usize) {
            pixels.extend_from_slice(&row[..row_bytes as usize]);
        }
        drop(mapped);
        staging.unmap();
        if self.scene.linear_color {
            encode_linear_rgb_to_srgb(&mut pixels);
        }
        RgbaImage::from_raw(width, height, pixels)
            .ok_or_else(|| SceneError::Gpu("Invalid readback size".into()))
    }

    /// Bakes 30 orbit views, checking cancellation before each GPU submission.
    ///
    /// Drop the renderer after baking; [`OrbitLoop`] retains only CPU pixels.
    ///
    /// # Errors
    /// Returns an error on cancellation or if any view cannot be rendered.
    pub fn bake_orbit(
        &mut self,
        width: u32,
        height: u32,
        cancelled: impl Fn() -> bool,
    ) -> Result<OrbitLoop, SceneError> {
        validate_orbit_size(width, height)?;
        let mut frames = Vec::with_capacity(ORBIT_FRAME_COUNT);
        for index in 0..ORBIT_FRAME_COUNT {
            if cancelled() {
                return Err(SceneError::Cancelled);
            }
            let camera = OrbitCamera {
                yaw: index as f32 * std::f32::consts::TAU / ORBIT_FRAME_COUNT as f32,
                ..Default::default()
            };
            frames.push(self.render(camera, width, height)?);
        }
        OrbitLoop::from_frames(frames)
    }
}

fn encode_linear_rgb_to_srgb(pixels: &mut [u8]) {
    for pixel in pixels.as_chunks_mut::<4>().0 {
        for channel in &mut pixel[..3] {
            let linear = f32::from(*channel) / 255.0;
            let srgb = if linear <= 0.0031308 {
                linear * 12.92
            } else {
                1.055 * linear.powf(1.0 / 2.4) - 0.055
            };
            *channel = (srgb * 255.0).round() as u8;
        }
    }
}

fn validate_orbit_size(width: u32, height: u32) -> Result<(), SceneError> {
    let bytes = u64::from(width)
        .checked_mul(u64::from(height))
        .and_then(|bytes| bytes.checked_mul(4));
    if !bytes
        .is_some_and(|bytes| bytes > 0 && bytes <= (128 * 1024 * 1024) / ORBIT_FRAME_COUNT as u64)
    {
        return Err(SceneError::Invalid(
            "Orbit frames exceed the 128 MiB preview budget".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orbit_playback_wraps_without_invoking_a_renderer() {
        let frames = (0..ORBIT_FRAME_COUNT)
            .map(|i| RgbaImage::from_pixel(1, 1, image::Rgba([i as u8, 0, 0, 255])))
            .collect();
        let orbit = OrbitLoop { frames };
        assert_eq!(orbit.frame_at(Duration::ZERO)[(0, 0)][0], 0);
        assert_eq!(orbit.frame_at(Duration::from_millis(11_999))[(0, 0)][0], 29);
        assert_eq!(orbit.frame_at(Duration::from_secs(12))[(0, 0)][0], 0);
    }

    #[test]
    #[ignore = "requires a native compute-capable GPU adapter"]
    fn gpu_renders_navigation_and_bakes_thirty_cpu_frames() {
        let mut renderer = SplatRenderer::new(Arc::new(crate::splat::tests::scene())).unwrap();
        let initial = renderer.render(OrbitCamera::default(), 97, 64).unwrap();
        assert!(initial.pixels().any(|p| p[0] > 20 || p[1] > 20));
        for camera in [
            OrbitCamera {
                yaw: 2.0,
                ..Default::default()
            },
            OrbitCamera {
                zoom: 2.0,
                ..Default::default()
            },
            OrbitCamera {
                pan: [0.4, 0.2],
                ..Default::default()
            },
        ] {
            assert_ne!(initial, renderer.render(camera, 97, 64).unwrap());
        }
        let orbit = renderer.bake_orbit(97, 64, || false).unwrap();
        drop(renderer);
        assert_eq!(orbit.frames().len(), 30);
        assert_ne!(orbit.frames()[0], orbit.frames()[15]);
        assert_eq!(orbit.frame_at(Duration::from_secs(12)), &orbit.frames()[0]);
        let mut renderer = SplatRenderer::new(Arc::new(crate::splat::tests::scene())).unwrap();
        assert!(matches!(
            renderer.bake_orbit(97, 64, || true),
            Err(SceneError::Cancelled)
        ));
    }
}
