//! Native Wayland and X11 URI drags on eframe's existing display.

mod io;
mod wayland;
mod x11;

use super::filesystem::Identity;
use super::search::SourceGuard;
use raw_window_handle::{HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle};
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

pub(super) enum Event {
    Drop(u64, egui::Pos2),
    Files(u64, Vec<PathBuf>, bool, HashMap<PathBuf, SourceGuard>),
    Remove(
        BTreeMap<PathBuf, Option<Identity>>,
        BTreeMap<PathBuf, Identity>,
        HashMap<PathBuf, SourceGuard>,
    ),
    Failed(u64, String),
    Notice(String),
    Refresh,
}

enum Backend {
    Wayland(Box<wayland::WaylandDrag>),
    X11(Box<x11::X11Drag>),
}

pub(super) struct NativeDrag {
    backend: Backend,
}

impl NativeDrag {
    pub(super) fn new(cc: &eframe::CreationContext<'_>) -> Option<Self> {
        let display = cc.display_handle().ok()?.as_raw();
        let window = cc.window_handle().ok()?.as_raw();
        let backend = match (display, window) {
            (RawDisplayHandle::Wayland(_), RawWindowHandle::Wayland(_)) => {
                Backend::Wayland(Box::new(wayland::WaylandDrag::new(display, window)?))
            }
            (
                RawDisplayHandle::Xlib(_) | RawDisplayHandle::Xcb(_),
                RawWindowHandle::Xlib(handle),
            ) => Backend::X11(Box::new(x11::X11Drag::new(
                u32::try_from(handle.window).ok()?,
            )?)),
            (
                RawDisplayHandle::Xlib(_) | RawDisplayHandle::Xcb(_),
                RawWindowHandle::Xcb(handle),
            ) => Backend::X11(Box::new(x11::X11Drag::new(handle.window.get())?)),
            _ => return None,
        };
        Some(Self { backend })
    }

    pub(super) fn set_reference_targets(&mut self, rects: Vec<egui::Rect>) {
        match &mut self.backend {
            Backend::Wayland(backend) => backend.set_reference_targets(rects),
            Backend::X11(backend) => backend.set_reference_targets(rects),
        }
    }

    pub(super) fn drag_position(&self) -> Option<egui::Pos2> {
        match &self.backend {
            Backend::Wayland(backend) => backend.drag_position(),
            Backend::X11(backend) => backend.drag_position(),
        }
    }

    pub(super) fn tick(&mut self) -> Vec<Event> {
        match &mut self.backend {
            Backend::Wayland(backend) => backend.tick(),
            Backend::X11(backend) => backend.tick(),
        }
    }

    pub(super) fn start(
        &mut self,
        identities: BTreeMap<PathBuf, Option<Identity>>,
        copy: bool,
        search_guards: HashMap<PathBuf, SourceGuard>,
    ) -> Result<bool, String> {
        match &mut self.backend {
            Backend::Wayland(backend) => backend.start(identities, copy, search_guards),
            Backend::X11(backend) => backend.start(identities, copy, search_guards),
        }
    }

    pub(super) fn finish(&mut self, token: u64, success: bool) {
        match &mut self.backend {
            Backend::Wayland(backend) => backend.finish(token, success),
            Backend::X11(backend) => backend.finish(token, success),
        }
    }

    pub(super) fn busy(&self) -> bool {
        match &self.backend {
            Backend::Wayland(backend) => backend.busy(),
            Backend::X11(backend) => backend.busy(),
        }
    }

    pub(super) fn raw_input(&mut self, ctx: &egui::Context, input: &mut egui::RawInput) {
        match &mut self.backend {
            Backend::Wayland(backend) => backend.raw_input(ctx, input),
            Backend::X11(backend) => backend.raw_input(ctx, input),
        }
    }
}
