//! Node graph pipeline system.
//!
//! Provides a visual DAG computation engine with customizable nodes (transforms, sources, sinks),
//! typed data flow ports (`PortType`, `PortValue`), serializable recipes, and parallel graph execution.

const EXTERNAL_COMMAND_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

mod archive;
mod artifacts;
pub mod catalog;
pub mod cdp_catalog;
pub mod compiler;
mod content;
pub mod executor;
pub mod optimizer;
pub mod recipes;
mod save;
mod telemetry;
pub mod types;

pub use catalog::*;
pub use cdp_catalog::*;
pub use compiler::*;
pub use executor::*;
pub use optimizer::*;
pub use recipes::*;
pub use types::*;
