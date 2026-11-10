//! Source-aware search observations handed to the host.
use crate::{explanation, source};

/// Observed entries and their retained root/availability explanations.
///
/// Hosts must keep the source descriptors with results and revalidate before
/// content/navigation actions; offline observations do not authorize access.
#[derive(Clone)]
pub struct Results {
    pub entries: Vec<little_clip_core::file_inventory::Entry>,
    pub message: String,
    pub sources: Vec<source::SourceRoot>,
    pub explanations: explanation::Explanations,
}
