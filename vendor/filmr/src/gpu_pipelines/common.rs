#[cfg(feature = "compute-gpu")]
use crate::gpu::{GpuBuffer, GpuContext};
#[cfg(feature = "compute-gpu")]
use futures::channel::oneshot;

// Native GPU handles are Send + Sync and share a process cache. Browser GPU
// handles are thread-affine: cache them on their worker instead of overriding
// their thread-safety traits. A successful cache deliberately lives for the
// worker's lifetime, matching the existing static-reference API.
#[cfg(feature = "compute-gpu")]
macro_rules! pipeline_cache {
    ($visibility:vis $getter:ident, $cache:ident, $ty:ty, $context:ident => $init:expr) => {
        #[cfg(not(target_arch = "wasm32"))]
        $visibility fn $getter($context: &crate::gpu::GpuContext) -> &'static $ty {
            static $cache: std::sync::OnceLock<$ty> = std::sync::OnceLock::new();
            $cache.get_or_init(|| $init)
        }
        #[cfg(target_arch = "wasm32")]
        $visibility fn $getter($context: &crate::gpu::GpuContext) -> &'static $ty {
            thread_local! {
                static $cache: std::cell::OnceCell<&'static $ty> = const { std::cell::OnceCell::new() };
            }
            $cache.with(|cache| *cache.get_or_init(|| Box::leak(Box::new($init))))
        }
    };
}
#[cfg(feature = "compute-gpu")]
pub(super) use pipeline_cache;

#[cfg(feature = "compute-gpu")]
pub async fn read_gpu_buffer(
    context: &GpuContext,
    gpu_buffer: &GpuBuffer,
) -> Option<image::ImageBuffer<image::Rgb<f32>, Vec<f32>>> {
    let size = gpu_buffer.size;

    // Create a staging buffer for reading back data
    let staging_buffer = context.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Staging Buffer"),
        size,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    // Copy from GPU buffer to staging buffer
    let mut encoder = context
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Readback Copy Encoder"),
        });
    encoder.copy_buffer_to_buffer(&gpu_buffer.buffer, 0, &staging_buffer, 0, size);
    let submission = context.queue.submit(Some(encoder.finish()));

    let buffer_slice = staging_buffer.slice(..);

    let (sender, receiver) = oneshot::channel();
    buffer_slice.map_async(wgpu::MapMode::Read, move |v| {
        let _ = sender.send(v);
    });

    #[cfg(not(target_arch = "wasm32"))]
    {
        context
            .device
            .poll(wgpu::PollType::Wait {
                submission_index: Some(submission),
                timeout: Some(std::time::Duration::from_secs(30)),
            })
            .ok()?;
        let mut receiver = receiver;
        receiver.try_recv().ok()??.ok()?;
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = submission;
        receiver.await.ok()?.ok()?;
    }
    let data = buffer_slice.get_mapped_range().ok()?;
    let result: Vec<f32> = bytemuck::cast_slice(&data).to_vec();
    drop(data);
    staging_buffer.unmap();
    image::ImageBuffer::from_raw(gpu_buffer.width, gpu_buffer.height, result)
}
