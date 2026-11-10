//! UI-independent search mechanisms and owned external model processes.
//!
//! Browser navigation, presentation and result composition stay with the host.
//! Query grammar, guarded source readers, transactional storage, bounded ranking,
//! inference and coordinator lifetimes share one headless owner. The host keeps
//! navigation/UI and explicitly dispatches reader/extraction worker entrypoints.
pub mod name_match;
pub mod policy;
pub mod query;
pub mod range;
pub mod runtime;
pub mod session;
pub mod source;
pub mod source_format;

/// Dimension of the existing embedding protocol and persisted search vectors.
pub const EMBEDDING_DIMENSION: usize = 768;

/// Notifies a consumer after publication, outside the search snapshot lock.
///
/// This is a wake-up hint rather than a completion payload. Consumers read the
/// request-scoped snapshot and retain its usual generation/source checks. The
/// callback should return promptly and may inspect the snapshot.
pub type ChangeNotifier = std::sync::Arc<dyn Fn() + Send + Sync + 'static>;

pub mod content;
pub mod explanation;
pub mod retrieval;
pub mod store;

pub mod results;
pub use results::Results;
pub mod answer;
mod monitor;
pub mod reader;
pub mod service;
mod walk;
