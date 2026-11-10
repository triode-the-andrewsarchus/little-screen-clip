//! Visible thumbnails take priority over speculative neighbors and quality upgrades.
use super::*;
use crate::file_manager::{filesystem::Entry, previews::EntryPreviews};

pub(super) struct Request<'a> {
    pub entry: &'a Entry,
    pub edge: u32,
    pub orientation: u8,
}

impl<'a> Request<'a> {
    pub fn from_tile(
        tile: &crate::file_manager::gallery::Placement,
        entry: &'a Entry,
        unit: f32,
        dpi: f32,
    ) -> Option<Self> {
        (tile.ready && entry.is_photo()).then(|| Self {
            entry,
            edge: tile.thumbnail_edge(unit, dpi),
            orientation: tile.orientation,
        })
    }
}

pub(super) fn schedule(
    previews: &mut EntryPreviews,
    ctx: &egui::Context,
    visible: &[Request<'_>],
    neighbors: &[Request<'_>],
    seconds_since_scroll: f64,
) {
    for request in visible {
        previews.request_gallery(request.entry, request.edge, request.orientation);
    }
    if !visible
        .iter()
        .all(|request| previews.is_ready(request.entry))
    {
        ctx.request_repaint_after(std::time::Duration::from_millis(16));
        return;
    }
    for request in neighbors {
        previews.request_gallery(request.entry, request.edge, request.orientation);
    }
    if seconds_since_scroll < SCROLL_SETTLE_SECONDS {
        ctx.request_repaint_after(std::time::Duration::from_secs_f64(
            (SCROLL_SETTLE_SECONDS - seconds_since_scroll).max(0.001),
        ));
        return;
    }
    let mut upgrading = false;
    for request in visible {
        upgrading |= previews.gallery_size(&request.entry.path) < 1024;
        previews.request_gallery(request.entry, 1024, request.orientation);
    }
    if upgrading || previews.has_pending() {
        ctx.request_repaint_after(std::time::Duration::from_millis(16));
    }
}
