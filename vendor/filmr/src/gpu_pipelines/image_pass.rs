use crate::gpu::{GpuBuffer, GpuContext};
use image::{ImageBuffer, Rgb};
use std::sync::OnceLock;
use wgpu::util::DeviceExt;

#[derive(Clone, Copy)]
pub enum ImagePassKind {
    Exposure,
    Motion,
    SpectralOutput,
}

pub struct ImagePass {
    pipeline: wgpu::ComputePipeline,
    layout: wgpu::BindGroupLayout,
}

impl ImagePass {
    fn new(context: &GpuContext, entry: &str) -> Self {
        let shader = context
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("Accurate Film Image Pass"),
                source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/image_pass.wgsl").into()),
            });
        let entries: Vec<_> = (0..4)
            .map(|binding| wgpu::BindGroupLayoutEntry {
                binding,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: if binding == 3 {
                        wgpu::BufferBindingType::Uniform
                    } else {
                        wgpu::BufferBindingType::Storage {
                            read_only: binding != 1,
                        }
                    },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            })
            .collect();
        let layout = context
            .device
            .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Accurate Film Image Layout"),
                entries: &entries,
            });
        let pipeline_layout =
            context
                .device
                .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: Some("Accurate Film Image Pipeline Layout"),
                    bind_group_layouts: &[&layout],
                    immediate_size: 0,
                });
        let create = |entry: &str| {
            context
                .device
                .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                    label: Some(entry),
                    layout: Some(&pipeline_layout),
                    module: &shader,
                    entry_point: Some(entry),
                    compilation_options: Default::default(),
                    cache: None,
                })
        };
        let pipeline = create(entry);
        Self { pipeline, layout }
    }

    pub fn process(
        &self,
        context: &GpuContext,
        image: &ImageBuffer<Rgb<f32>, Vec<f32>>,
        parameters: &[f32],
    ) -> Option<ImageBuffer<Rgb<f32>, Vec<f32>>> {
        let (width, height) = image.dimensions();
        if !context.supports_rgb_image(width, height) {
            return None;
        }
        let size = image.as_raw().len() as u64 * 4;
        let input = context
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Accurate Film Input"),
                contents: bytemuck::cast_slice(image.as_raw()),
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            });
        let output = context.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Accurate Film Output"),
            size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let params = context
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Accurate Film Parameters"),
                contents: bytemuck::cast_slice(parameters),
                usage: wgpu::BufferUsages::STORAGE,
            });
        let dimensions = context
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Accurate Film Dimensions"),
                contents: bytemuck::cast_slice(&[width, height]),
                usage: wgpu::BufferUsages::UNIFORM,
            });
        let bind = |source: &wgpu::Buffer, destination: &wgpu::Buffer| {
            let entries: Vec<_> = [source, destination, &params, &dimensions]
                .iter()
                .enumerate()
                .map(|(binding, buffer)| wgpu::BindGroupEntry {
                    binding: binding as u32,
                    resource: buffer.as_entire_binding(),
                })
                .collect();
            context
                .device
                .create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("Accurate Film Bindings"),
                    layout: &self.layout,
                    entries: &entries,
                })
        };
        let bindings = bind(&input, &output);
        let mut encoder = context
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Accurate Film Encoder"),
            });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("Accurate Film Pass"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &bindings, &[]);
            pass.dispatch_workgroups(width.div_ceil(16), height.div_ceil(16), 1);
        }
        context.queue.submit(Some(encoder.finish()));
        crate::gpu::block_on(super::read_gpu_buffer(
            context,
            &GpuBuffer {
                buffer: output,
                width,
                height,
                size,
            },
        ))
    }
}

pub fn process_image_pass(
    kind: ImagePassKind,
    image: &ImageBuffer<Rgb<f32>, Vec<f32>>,
    params: &[f32],
) -> Option<ImageBuffer<Rgb<f32>, Vec<f32>>> {
    static EXPOSURE: OnceLock<ImagePass> = OnceLock::new();
    static MOTION: OnceLock<ImagePass> = OnceLock::new();
    static OUTPUT: OnceLock<ImagePass> = OnceLock::new();
    let context = crate::gpu::get_gpu_context()?;
    let (cache, entry) = match kind {
        ImagePassKind::Exposure => (&EXPOSURE, "exposure"),
        ImagePassKind::Motion => (&MOTION, "motion"),
        ImagePassKind::SpectralOutput => (&OUTPUT, "spectral_output"),
    };
    cache
        .get_or_init(|| ImagePass::new(context, entry))
        .process(context, image, params)
}
