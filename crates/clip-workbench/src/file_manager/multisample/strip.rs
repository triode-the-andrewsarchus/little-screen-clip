//! A single explicitly chosen inline instrument uses the studio's loader/player.
use super::{Task, Viewer, surface};
use crate::file_manager::filesystem::Entry;
use crate::ui_appearance::WorkbenchColors;
use little_clip_core::{BackgroundJobs, palette::AppPalette};
use std::{collections::BTreeSet, path::PathBuf, time::Duration};

#[derive(Default)]
pub(in crate::file_manager) struct Preview {
    source: Option<(PathBuf, u64)>,
    task: Option<Task<Result<Viewer, String>>>,
    viewer: Option<Viewer>,
    requested: Option<surface::Strike>,
    error: Option<String>,
    pub started: bool,
    focused: bool,
    clicked: bool,
}
impl Preview {
    pub fn clear(&mut self) {
        *self = Self::default();
    }
    pub fn retain(&mut self, paths: &BTreeSet<PathBuf>) {
        if self
            .source
            .as_ref()
            .is_some_and(|(path, _)| !paths.contains(path))
        {
            self.clear();
        }
    }
    pub fn focus(&mut self, entry: &Entry) {
        let source = (entry.path.clone(), entry.revision);
        if self.source.as_ref() != Some(&source) || (self.task.is_none() && self.viewer.is_none()) {
            self.clear();
            self.source = Some(source);
            let path = entry.path.clone();
            self.task = Some(Task::start(
                BackgroundJobs::global(),
                Duration::from_secs(35),
                move |token| Viewer::load(&path, token),
            ));
        }
        self.focused = true;
        self.clicked = true;
    }
    pub fn finish_frame(&mut self, ctx: &egui::Context) {
        if ctx.input(|i| i.pointer.any_pressed()) && !self.clicked {
            self.focused = false;
            if let Some(viewer) = &mut self.viewer {
                viewer.midi.disconnect();
            }
        }
        self.clicked = false;
    }
    fn request(&mut self, entry: &Entry, strike: surface::Strike) {
        self.focus(entry);
        self.started = true;
        self.requested = Some(strike);
        if let Some(viewer) = &mut self.viewer {
            viewer.trigger(strike);
            self.requested = None;
        }
    }
    fn poll(&mut self, ctx: &egui::Context) {
        if let Some(result) = self.task.as_ref().and_then(Task::poll) {
            self.task = None;
            match result.map_err(|e| e.to_string()).and_then(|result| result) {
                Ok(mut viewer) => {
                    if let Some(strike) = self.requested.take() {
                        viewer.trigger(strike);
                    }
                    self.viewer = Some(viewer);
                }
                Err(error) => self.error = Some(error),
            }
        }
        if let Some(viewer) = &mut self.viewer {
            viewer.poll(ctx);
            if self.focused {
                viewer.poll_midi(ctx);
                if viewer.midi.active() {
                    self.started = true;
                }
            }
        }
        if self.task.is_some() {
            ctx.request_repaint_after(Duration::from_millis(40));
        }
    }
    fn controls(
        &mut self,
        ui: &mut egui::Ui,
        bounds: [u8; 2],
        active: bool,
        colors: WorkbenchColors,
    ) {
        ui.spacing_mut().interact_size.y = 20.0;
        ui.horizontal(|ui| {
            ui.small(format!(
                "{}–{}",
                surface::note(bounds[0]),
                surface::note(bounds[1])
            ));
            if active && ui.small_button("Stop").clicked() {
                self.clear();
            }
        });
        if active && self.focused {
            ui.colored_label(
                colors.accent,
                egui::RichText::new("MIDI · soft ↓ · loud ↑").small(),
            );
        } else {
            ui.small("Soft ↓ · loud ↑");
        }
    }
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        rect: egui::Rect,
        entry: &Entry,
        document: Option<&little_clip_core::multisample::Document>,
        palette: &AppPalette,
    ) -> Option<(usize, usize)> {
        if self
            .source
            .as_ref()
            .is_some_and(|(path, revision)| path == &entry.path && *revision != entry.revision)
        {
            self.clear();
        }
        // Only the visible owning row may publish a completed audio request.
        // Other rows must not briefly start a retired offscreen instrument.
        if self
            .source
            .as_ref()
            .is_some_and(|(path, _)| path == &entry.path)
        {
            self.poll(ui.ctx());
        }
        let colors = WorkbenchColors::from_palette(palette);
        let document = document?;
        let instrument = document.instruments.first()?;
        let bounds = surface::bounds(&instrument.regions);
        let active = self
            .source
            .as_ref()
            .is_some_and(|(path, _)| path == &entry.path);
        let selected = if active {
            self.viewer
                .as_ref()
                .map_or_else(Vec::new, Viewer::sounding_keys)
        } else {
            Vec::new()
        };
        let id = ui.id().with((&entry.path, entry.revision, "inline-keys"));
        let mut hit = None;
        ui.push_id(id, |ui| {
            let controls = egui::Rect::from_min_size(
                rect.min,
                egui::vec2(165.0_f32.min(rect.width() * 0.3), rect.height()),
            );
            ui.allocate_ui_at_rect(controls, |ui| {
                ui.set_clip_rect(controls.intersect(ui.clip_rect()));
                self.controls(ui, bounds, active, colors);
            });
            let keys =
                egui::Rect::from_min_max(egui::pos2(controls.right() + 6.0, rect.top()), rect.max);
            hit = surface::strip_keys(ui, keys, &instrument.regions, &selected, palette);
            if active
                && crate::file_manager::ui::preview_hovered(ui, rect)
                && let Some(error) = self.error.clone().or_else(|| {
                    self.viewer
                        .as_ref()
                        .and_then(|v| v.error.clone().or_else(|| v.midi.error()))
                })
            {
                egui::show_tooltip(ui.ctx(), ui.id().with("sampler-error"), |ui| {
                    ui.colored_label(colors.danger, error);
                });
            }
        });
        if let Some(strike) = hit {
            self.request(entry, strike);
        }
        self.viewer
            .as_ref()
            .filter(|_| active)
            .map(|v| (v.instrument, v.selected))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_clicking_an_instrument_retains_the_midi_route() {
        let ctx = egui::Context::default();
        let mut preview = Preview {
            focused: true,
            clicked: true,
            ..Default::default()
        };
        ctx.begin_frame(egui::RawInput {
            events: vec![egui::Event::PointerButton {
                pos: egui::pos2(20.0, 20.0),
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: Default::default(),
            }],
            ..Default::default()
        });
        preview.finish_frame(&ctx);
        assert!(preview.focused);
        // The same press without an instrument claiming it retires routing.
        preview.finish_frame(&ctx);
        assert!(!preview.focused);
        let _ = ctx.end_frame();
    }
}
