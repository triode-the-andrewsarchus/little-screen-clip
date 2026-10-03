//! Workbench orchestration, organized by lifecycle and interaction responsibility.

use super::*;

mod actions;
mod background;
pub(super) use background::{ClipboardSelection, MirrorJob, PaletteOpsJob};
mod appearance;
mod header;
mod image_sessions;
mod loaders;
mod rolling_editor;
mod shortcuts;
mod startup;
mod update;
mod viewers;
