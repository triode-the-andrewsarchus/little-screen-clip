mod common;
pub mod develop;
pub mod gaussian;
pub mod grain;
pub mod halation;
#[cfg(feature = "compute-gpu")]
pub mod image_pass;
pub mod light_leak;
pub mod linearize;

pub use develop::DevelopPipeline;
pub use gaussian::GaussianPipeline;
pub use grain::{GrainPipeline, GrainUniforms};
pub use halation::HalationPipeline;
pub use light_leak::LightLeakPipeline;
pub use linearize::LinearizePipeline;

#[cfg(feature = "compute-gpu")]
pub use common::read_gpu_buffer;

#[cfg(feature = "compute-gpu")]
common::pipeline_cache!(get_linearize_pipeline, LINEARIZE_PIPELINE, LinearizePipeline, context => LinearizePipeline::new(context));
#[cfg(feature = "compute-gpu")]
common::pipeline_cache!(get_light_leak_pipeline, LIGHT_LEAK_PIPELINE, LightLeakPipeline, context => LightLeakPipeline::new(context));
#[cfg(feature = "compute-gpu")]
common::pipeline_cache!(get_halation_pipeline, HALATION_PIPELINE, HalationPipeline, context => HalationPipeline::new(context));
#[cfg(feature = "compute-gpu")]
common::pipeline_cache!(get_gaussian_pipeline, GAUSSIAN_PIPELINE, GaussianPipeline, context => GaussianPipeline::new(context));
#[cfg(feature = "compute-gpu")]
common::pipeline_cache!(get_grain_pipeline, GRAIN_PIPELINE, GrainPipeline, context => GrainPipeline::new(context));
#[cfg(feature = "compute-gpu")]
common::pipeline_cache!(get_develop_pipeline, DEVELOP_PIPELINE, DevelopPipeline, context => DevelopPipeline::new(context));
