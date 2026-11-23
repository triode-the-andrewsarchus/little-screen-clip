//! Mixed-signal analysis on a shared timeline with native source clocks.

mod alignment;
mod derived;
pub(super) mod filter_editor;
pub(super) mod frequency_plot;
mod inputs;
pub(super) mod spectra;
pub(super) mod timeline;

use super::{
    DocumentPosition, document_format::Format, logic_view, preview_copy::Copies, preview_job::Task,
    scope_view,
};
use crate::file_source::ReadCheck;
use egui::{Context, Ui};
use little_clip_core::{engineering::waveforms::display_value, palette::AppPalette};
use std::{
    ops::Range,
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
};
use timeline::{Link, translate};

pub(super) const MAX_SOURCES: usize = 8;

pub(super) struct Viewer {
    sources: Vec<Source>,
    derived: derived::Derived,
    alignment: alignment::Alignment,
    domain: Range<f64>,
    span: Range<f64>,
    probe: Option<f64>,
    reference: Option<Reference>,
    error: Option<String>,
}

struct Source {
    id: u64,
    path: PathBuf,
    offset: f64,
    capture: Capture,
}

struct Reference {
    path: PathBuf,
    native_time: f64,
}

enum SourceAction {
    Pin(f64),
    Align(f64),
    ClearReference,
    Remove,
}

enum Capture {
    Analog(Box<scope_view::Viewer>),
    Digital(Box<logic_view::Viewer>),
}

pub(super) fn supports_path(path: &Path) -> bool {
    matches!(
        Format::for_path(path),
        Some(Format::Oscilloscope | Format::PowerRecording | Format::LogicCapture)
    ) || path.extension().is_some_and(|ext| {
        ["bin", "csv", "dsl", "wav"]
            .iter()
            .any(|hint| ext.eq_ignore_ascii_case(hint))
    })
}

// A saturated read queue is retried by the caller with its current query.
pub(super) fn take_query_result<K, T: Send + 'static>(
    pending: &mut Option<(K, Task<Result<T, String>>)>,
) -> Option<(K, Result<T, String>)> {
    let result = pending.as_ref()?.1.poll()?;
    let (key, _) = pending.take()?;
    if matches!(result, Err(little_clip_core::JobError::QueueFull)) {
        return None;
    }
    let result = result
        .map_err(|error| error.to_string())
        .and_then(|result| result);
    Some((key, result))
}

impl Viewer {
    pub(super) fn load(path: &Path, format: Format, cancel: &AtomicBool) -> Result<Self, String> {
        let source = Source::load(path, Some(format), cancel)?;
        Ok(Self::new(vec![source]))
    }

    pub(super) fn load_sources(paths: Vec<PathBuf>, cancel: &AtomicBool) -> Result<Self, String> {
        if !(2..=MAX_SOURCES).contains(&paths.len()) {
            return Err(format!("Select between two and {MAX_SOURCES} recordings."));
        }
        paths
            .into_iter()
            .map(|path| {
                Source::load(&path, None, cancel)
                    .map_err(|error| format!("{}: {error}", path.display()))
            })
            .collect::<Result<_, _>>()
            .map(Self::new)
    }

    fn new(mut sources: Vec<Source>) -> Self {
        for (index, source) in sources.iter_mut().enumerate() {
            source.id = index as u64 + 1;
        }
        let domain = extent(&sources);
        Self {
            sources,
            derived: derived::Derived::default(),
            alignment: alignment::Alignment::default(),
            span: domain.clone(),
            domain,
            probe: None,
            reference: None,
            error: None,
        }
    }

    pub(super) fn set_check(&mut self, check: ReadCheck) {
        if let [source] = &mut self.sources[..] {
            match &mut source.capture {
                Capture::Analog(view) => view.set_check(check),
                Capture::Digital(view) => view.set_check(check),
            }
        }
    }

    pub(super) fn focus(&mut self, position: DocumentPosition) {
        let [source] = &mut self.sources[..] else {
            return;
        };
        match (&mut source.capture, position) {
            (Capture::Analog(view), DocumentPosition::ScopeSamples { first, end, .. }) => {
                view.focus_samples(first, end);
            }
            (
                Capture::Digital(view),
                DocumentPosition::LogicRegion {
                    start_bits,
                    end_bits,
                    ..
                },
            ) => {
                view.focus_time(f64::from_bits(start_bits), f64::from_bits(end_bits));
            }
            _ => {}
        }
    }

    pub(super) fn ui(
        &mut self,
        ui: &mut Ui,
        close: &mut bool,
        palette: &AppPalette,
        copies: &mut Copies,
    ) {
        let mut shell = crate::shell::StudioShell::new(
            "Signal analysis",
            crate::icons::Icon::Waves,
            palette,
            close,
        );
        shell.render_header(ui, |_| {});
        self.controls(ui);
        self.align_recordings(ui.ctx());
        let inputs = if self.derived.editor_open() {
            self.sources.iter().flat_map(Source::inputs).collect()
        } else {
            Vec::new()
        };
        self.derived.editor(ui, &inputs, &self.span);
        if let [source] = &mut self.sources[..]
            && source.offset == 0.0
            && !self.derived.active()
        {
            source.capture.content(ui, palette, copies, None);
            self.span = source.capture.span().clone();
            self.domain = source.capture.domain();
        } else {
            self.linked(ui, palette, copies);
        }
    }

    fn controls(&mut self, ui: &mut Ui) {
        ui.horizontal_wrapped(|ui| {
            self.derived.button(ui);
            if self.sources.len() > 1 {
                if ui.button("Match timing").clicked() {
                    let pinned = self.reference_time();
                    self.alignment.open_at(&self.span, pinned);
                }
                if ui.button("Fit all").clicked() {
                    self.span = extent(&self.sources);
                }
                if let Some(probe) = self.probe {
                    ui.monospace(display_value(probe, "s"));
                }
            }
            if let Some(error) = &self.error {
                ui.colored_label(ui.visuals().error_fg_color, error);
            }
        });
    }

    fn align_recordings(&mut self, ctx: &Context) {
        if !self.alignment.open {
            return;
        }
        let entries = self
            .sources
            .iter()
            .flat_map(|source| {
                source
                    .inputs()
                    .into_iter()
                    .filter(|input| !input.is_awaiting_filter())
                    .map(|input| alignment::Entry {
                        source: source.id,
                        input,
                    })
            })
            .collect::<Vec<_>>();
        let pinned = self.reference_time();
        let Some(suggestion) = self.alignment.window(ctx, &entries, &self.span, pinned) else {
            return;
        };
        let applied = self
            .sources
            .iter_mut()
            .find(|source| source.id == suggestion.source)
            .filter(|source| source.offset == suggestion.base)
            .is_some_and(|source| source.set_offset(suggestion.offset));
        if !applied {
            self.error =
                Some("This offset cannot be represented on the current source timeline.".into());
        }
        ctx.request_repaint();
    }

    fn linked(&mut self, ui: &mut Ui, palette: &AppPalette, copies: &mut Copies) {
        let domain = extent(&self.sources);
        if self.span == self.domain {
            self.span = domain.clone();
        }
        self.domain = domain;
        let mut next_probe = None;
        let mut action = None;
        let reference_time = self.reference_time();
        let preview = self.alignment.preview();
        let may_remove = self.sources.len() > 1;
        let height = (ui.available_height() / self.sources.len() as f32 - 50.0).max(205.0);
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for (index, source) in self.sources.iter_mut().enumerate() {
                    let id = ui.make_persistent_id(&source.path);
                    ui.push_id(id, |ui| {
                        let is_reference = self
                            .reference
                            .as_ref()
                            .is_some_and(|pin| pin.path == source.path);
                        if let Some(request) =
                            source.controls(ui, may_remove, reference_time, is_reference, preview)
                        {
                            action = Some((index, request));
                        }
                        let offset = source.display_offset(preview);
                        let previous = translate(&self.span, -offset);
                        *source.capture.span() = previous.clone();
                        let link = Link {
                            domain: translate(&self.domain, -offset),
                            offset,
                            probe: self.probe.map(|probe| probe - offset),
                            reference: reference_time.map(|time| time - offset),
                        };
                        let height = height.max(source.capture.min_height());
                        ui.allocate_ui(egui::vec2(ui.available_width(), height), |ui| {
                            if let Some(probe) =
                                source.capture.content(ui, palette, copies, Some(&link))
                            {
                                next_probe = Some(probe + offset);
                            }
                        });
                        let view = source.capture.span();
                        if *view != previous {
                            self.span = translate(view, offset);
                            ui.ctx().request_repaint();
                        }
                        ui.separator();
                    });
                }
                let inputs = if self.derived.active() {
                    self.sources.iter().flat_map(Source::inputs).collect()
                } else {
                    Vec::new()
                };
                next_probe = self
                    .derived
                    .content(
                        ui,
                        &mut self.span,
                        &self.domain,
                        &inputs,
                        self.probe,
                        reference_time,
                    )
                    .or(next_probe);
            });
        if next_probe != self.probe {
            self.probe = next_probe;
            ui.ctx().request_repaint();
        }
        if let Some((index, request)) = action {
            self.apply_source_action(index, request);
            ui.ctx().request_repaint();
        }
    }

    fn reference_time(&self) -> Option<f64> {
        let pin = self.reference.as_ref()?;
        let source = self.sources.iter().find(|source| source.path == pin.path)?;
        Some(pin.native_time + source.offset)
    }

    fn apply_source_action(&mut self, index: usize, action: SourceAction) {
        match action {
            SourceAction::Pin(native_time) => {
                self.reference = Some(Reference {
                    path: self.sources[index].path.clone(),
                    native_time,
                });
            }
            SourceAction::Align(offset) => {
                if !self.sources[index].set_offset(offset) {
                    self.error =
                        Some("The reference is outside this source's time precision.".into());
                }
            }
            SourceAction::ClearReference => self.reference = None,
            SourceAction::Remove => {
                let removed = self.sources.remove(index);
                if self
                    .reference
                    .as_ref()
                    .is_some_and(|pin| pin.path == removed.path)
                {
                    self.reference = None;
                }
                if let [source] = &mut self.sources[..] {
                    let previous = translate(&self.span, -source.offset);
                    *source.capture.span() = timeline::fit(&source.capture.domain(), previous, 0.0);
                    self.probe = None;
                }
            }
        }
    }
}

impl Source {
    fn load(path: &Path, format: Option<Format>, cancel: &AtomicBool) -> Result<Self, String> {
        let format = format
            .or_else(|| little_clip_core::file_format::recognize_engineering(path, cancel))
            .or_else(|| Format::for_path(path));
        let capture = match format {
            Some(Format::Oscilloscope | Format::PowerRecording) => {
                Capture::Analog(Box::new(scope_view::Viewer::load(path, cancel)?))
            }
            Some(Format::LogicCapture) => {
                Capture::Digital(Box::new(logic_view::Viewer::load(path, cancel)?))
            }
            None if path
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("wav")) =>
            {
                Capture::Analog(Box::new(scope_view::Viewer::load(path, cancel)?))
            }
            _ => {
                return Err("Unsupported signal recording.".into());
            }
        };
        Ok(Self {
            id: 0,
            path: path.to_path_buf(),
            offset: 0.0,
            capture,
        })
    }

    fn inputs(&self) -> Vec<inputs::Input> {
        let label = self
            .path
            .file_name()
            .unwrap_or(self.path.as_os_str())
            .to_string_lossy();
        match &self.capture {
            Capture::Analog(view) => {
                let recording = view.recording();
                let mut inputs = inputs::Input::analog(self.id, self.offset, &label, recording);
                inputs.extend(view.filtered_channels().filter_map(|filtered| {
                    inputs::Input::filtered(
                        self.id,
                        self.offset,
                        &label,
                        recording.channels(),
                        filtered,
                    )
                }));
                inputs
            }
            Capture::Digital(view) => {
                let (recording, lanes) = view.recording();
                let mut inputs =
                    inputs::Input::digital(self.id, self.offset, &label, recording, lanes);
                if let Some(pins) = view.decoded_pins() {
                    inputs.extend(pins.pcm().map(|(channel, generation, lane)| {
                        inputs::Input::pcm(self.id, self.offset, &label, channel, generation, lane)
                    }));
                    if let Some((generation, lane)) = pins.pwm() {
                        inputs.extend(inputs::Input::pwm(
                            self.id,
                            self.offset,
                            &label,
                            generation,
                            lane,
                        ));
                    }
                }
                inputs
            }
        }
    }

    fn controls(
        &mut self,
        ui: &mut Ui,
        may_remove: bool,
        reference: Option<f64>,
        is_reference: bool,
        preview: Option<alignment::Suggestion>,
    ) -> Option<SourceAction> {
        ui.horizontal(|ui| {
            let name = self
                .path
                .file_name()
                .unwrap_or(self.path.as_os_str())
                .to_string_lossy();
            ui.strong(name.as_ref())
                .on_hover_text(self.path.to_string_lossy());
            let mut proposed = self.offset;
            let domain = self.capture.domain();
            let interval = ((domain.end - domain.start) * 0.001).clamp(1e-12, 0.1);
            ui.label("Offset");
            if ui
                .add(
                    egui::DragValue::new(&mut proposed)
                        .speed(interval)
                        .max_decimals(12)
                        .suffix(" s"),
                )
                .on_hover_text("Positive offsets move this source later on the shared timeline.")
                .changed()
            {
                self.set_offset(proposed);
            }
            let displayed = self.display_offset(preview);
            if displayed != self.offset {
                ui.monospace(format!("Preview {}", display_value(displayed, "s")));
            }
            let align = ui
                .menu_button(if is_reference { "Reference" } else { "Align" }, |ui| {
                    let cursor = self.capture.cursor();
                    for (text, action) in [
                        ("Pin cursor as reference", cursor.map(SourceAction::Pin)),
                        (
                            "Align cursor to reference",
                            reference
                                .zip(cursor)
                                .filter(|_| !is_reference)
                                .map(|(time, cursor)| SourceAction::Align(time - cursor)),
                        ),
                        (
                            "Reset offset",
                            (self.offset != 0.0).then_some(SourceAction::Align(0.0)),
                        ),
                        (
                            "Clear reference",
                            is_reference.then_some(SourceAction::ClearReference),
                        ),
                    ] {
                        if ui
                            .add_enabled(action.is_some(), egui::Button::new(text))
                            .clicked()
                        {
                            ui.close();
                            return action;
                        }
                    }
                    None
                })
                .inner
                .flatten();
            if may_remove
                && ui
                    .small_button("Remove")
                    .on_hover_text("Remove source from this session")
                    .clicked()
            {
                return Some(SourceAction::Remove);
            }
            align
        })
        .inner
    }

    fn display_offset(&self, preview: Option<alignment::Suggestion>) -> f64 {
        preview
            .filter(|preview| preview.source == self.id && preview.base == self.offset)
            .filter(|preview| self.accepts_offset(preview.offset))
            .map_or(self.offset, |preview| preview.offset)
    }

    fn accepts_offset(&self, offset: f64) -> bool {
        let extent = translate(&self.capture.domain(), offset);
        !extent.is_empty() && extent.start.is_finite() && extent.end.is_finite()
    }

    fn set_offset(&mut self, offset: f64) -> bool {
        if !self.accepts_offset(offset) {
            return false;
        }
        self.offset = offset;
        true
    }
}

impl Capture {
    fn span(&mut self) -> &mut Range<f64> {
        match self {
            Self::Analog(view) => view.span(),
            Self::Digital(view) => view.span(),
        }
    }

    fn domain(&self) -> Range<f64> {
        match self {
            Self::Analog(view) => view.domain(),
            Self::Digital(view) => view.domain(),
        }
    }

    fn cursor(&self) -> Option<f64> {
        match self {
            Self::Analog(view) => view.cursor(),
            Self::Digital(view) => view.cursor(),
        }
    }

    fn min_height(&self) -> f32 {
        match self {
            Self::Analog(view) => view.min_height(),
            Self::Digital(view) => view.min_height(),
        }
    }

    fn content(
        &mut self,
        ui: &mut Ui,
        palette: &AppPalette,
        copies: &mut Copies,
        link: Option<&Link>,
    ) -> Option<f64> {
        match self {
            Self::Analog(view) => view.content(ui, palette, copies, link),
            Self::Digital(view) => view.content(ui, copies, link),
        }
    }
}

fn extent(sources: &[Source]) -> Range<f64> {
    sources
        .iter()
        .map(|source| translate(&source.capture.domain(), source.offset))
        .fold(f64::INFINITY..f64::NEG_INFINITY, |extent, span| {
            extent.start.min(span.start)..extent.end.max(span.end)
        })
}
