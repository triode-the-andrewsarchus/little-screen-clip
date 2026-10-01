#[cfg(feature = "compute-gpu")]
use tracing::info;

#[cfg(feature = "compute-gpu")]
use std::sync::OnceLock;

#[cfg(feature = "compute-gpu")]
pub struct GpuContext {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub adapter_info: wgpu::AdapterInfo,
}

#[cfg(feature = "compute-gpu")]
pub struct GpuBuffer {
    pub buffer: wgpu::Buffer,
    pub width: u32,
    pub height: u32,
    pub size: u64,
}

#[cfg(all(feature = "compute-gpu", not(target_arch = "wasm32")))]
static GPU_CONTEXT: OnceLock<Option<GpuContext>> = OnceLock::new();

#[cfg(all(feature = "compute-gpu", not(target_arch = "wasm32")))]
pub fn get_gpu_context() -> Option<&'static GpuContext> {
    // Pipelines are cached for this device, so concurrent renders must share
    // the same initialization, including a failed adapter lookup.
    GPU_CONTEXT
        .get_or_init(|| block_on(GpuContext::new()))
        .as_ref()
}

// WASM Implementation using Unsafe Wrapper to bypass Send/Sync check
// Safety: This is only safe if accessed from the single worker thread where it was initialized.
#[cfg(all(feature = "compute-gpu", target_arch = "wasm32"))]
struct WasmGpuHolder(GpuContext);

#[cfg(all(feature = "compute-gpu", target_arch = "wasm32"))]
unsafe impl Send for WasmGpuHolder {}

#[cfg(all(feature = "compute-gpu", target_arch = "wasm32"))]
unsafe impl Sync for WasmGpuHolder {}

#[cfg(all(feature = "compute-gpu", target_arch = "wasm32"))]
static WASM_GPU_CONTEXT: OnceLock<WasmGpuHolder> = OnceLock::new();

#[cfg(all(feature = "compute-gpu", target_arch = "wasm32"))]
pub fn init_gpu_context(context: GpuContext) {
    let _ = WASM_GPU_CONTEXT.set(WasmGpuHolder(context));
}

#[cfg(all(feature = "compute-gpu", target_arch = "wasm32"))]
pub fn get_gpu_context() -> Option<&'static GpuContext> {
    WASM_GPU_CONTEXT.get().map(|h| &h.0)
}

#[cfg(feature = "compute-gpu")]
impl GpuContext {
    pub fn supports_rgb_image(&self, width: u32, height: u32) -> bool {
        let limits = self.device.limits();
        let size = u64::from(width)
            .checked_mul(u64::from(height))
            .and_then(|n| n.checked_mul(12));
        size.is_some_and(|size| {
            size > 0
                && size <= u64::from(limits.max_storage_buffer_binding_size)
                && size <= limits.max_buffer_size
                && width.div_ceil(16) <= limits.max_compute_workgroups_per_dimension
                && height.div_ceil(16) <= limits.max_compute_workgroups_per_dimension
        })
    }

    pub async fn new() -> Option<Self> {
        info!("Initializing WGPU context");
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
            .ok()?;

        let adapter_info = adapter.get_info();
        info!("Using GPU adapter: {:?}", adapter_info);

        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("Filmr Compute Device"),
                required_features: wgpu::Features::empty(),
                required_limits: adapter.limits(),
                memory_hints: Default::default(),
                experimental_features: Default::default(),
                trace: Default::default(),
            })
            .await
            .ok()?;

        Some(Self {
            device,
            queue,
            adapter_info,
        })
    }
}

/// Helper to run future synchronously on current thread (if possible)
#[cfg(feature = "compute-gpu")]
pub fn block_on<F: std::future::Future>(future: F) -> F::Output {
    pollster::block_on(future)
}
