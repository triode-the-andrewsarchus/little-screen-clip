//! Node graph pipeline system.
//!
//! Provides a visual DAG computation engine with customizable nodes (transforms, sources, sinks),
//! typed data flow ports (`PortType`, `PortValue`), serializable recipes, and parallel graph execution.

pub mod catalog;
pub mod compiler;
pub mod executor;
pub mod recipes;
pub mod types;

pub use catalog::*;
pub use compiler::*;
pub use executor::*;
pub use recipes::*;
pub use types::*;
