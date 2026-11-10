//! UI-independent search mechanisms and owned external model processes.
//!
//! Browser navigation, presentation and result composition stay with the host.
//! Guarded source/index orchestration is being extracted incrementally.
pub mod range;
pub mod runtime;

/// Dimension of the existing embedding protocol and persisted search vectors.
pub const EMBEDDING_DIMENSION: usize = 768;

/// Notifies a consumer after publication, outside the search snapshot lock.
///
/// This is a wake-up hint rather than a completion payload. Consumers read the
/// request-scoped snapshot and retain its usual generation/source checks. The
/// callback should return promptly and may inspect the snapshot.
pub type ChangeNotifier = std::sync::Arc<dyn Fn() + Send + Sync + 'static>;
