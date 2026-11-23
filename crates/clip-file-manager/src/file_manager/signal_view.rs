//! Mixed-signal analysis on a shared timeline with native source clocks.

mod alignment;
mod comparison;
mod derived;
pub(super) mod filter_editor;
pub(super) mod frequency_plot;
mod inputs;
mod interval;
pub(super) mod session;
pub(super) mod spectra;
pub(super) mod timeline;

use super::{
    DocumentPosition, document_format::Format, logic_view, preview_copy::Copies, preview_job::Task,
    scope_view,
};
use crate::file_source::ReadCheck;
use egui::Ui;
use little_clip_core::{
    engineering::{analysis::Clock, waveforms::display_value},
    palette::AppPalette,
};
use std::{
    ops::Range,
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
};
use timeline::{Interaction, Link};

pub(super) const MAX_SOURCES: usize = 8;

pub(super) struct Viewer {
    sources: Vec<Source>,
    derived: derived::Derived,
    alignment: alignment::Alignment,
    comparison: comparison::Comparison,
    domain: Range<f64>,
    span: Range<f64>,
    probe: Option<f64>,
    reference: Option<Reference>,
    shared: interval::Shared,
    reveal_source: Option<u64>,
    session: session::Session,
    error: Option<String>,
}

#[derive(Clone, Copy)]
struct LinkedTimeline<'a> {
    preview: Option<alignment::Suggestion>,
    shared: Option<&'a Range<f64>>,
    reference: Option<f64>,
}

struct Source {
    id: u64,
    path: PathBuf,
    clock: Clock,
    capture: Capture,
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct Reference {
    source: u64,
    native_time: f64,
}

enum SourceAction {
    Pin(f64),
    Share(Range<f64>),
    Align(Clock),
    ClearReference,
    Remove,
    Relink(Source),
}

enum Capture {
    Analog(Box<scope_view::Viewer>),
    Digital(Box<logic_view::Viewer>),
    Unavailable(Box<session::Unavailable>),
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
            comparison: comparison::Comparison::default(),
            span: domain.clone(),
            domain,
            probe: None,
            reference: None,
            shared: interval::Shared::default(),
            reveal_source: None,
            session: session::Session::default(),
            error: None,
        }
    }

    pub(super) fn set_check(&mut self, check: ReadCheck) {
        if self.session_path().is_none()
            && let [source] = &mut self.sources[..]
        {
            match &mut source.capture {
                Capture::Analog(view) => view.set_check(check),
                Capture::Digital(view) => view.set_check(check),
                Capture::Unavailable(_) => {}
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
        self.align_recordings(ui);
        if let [source] = &mut self.sources[..]
            && source.clock == Clock::default()
            && !self.derived.open()
            && self.reference.is_none()
            && self.shared.pin.is_none()
            && !matches!(source.capture, Capture::Unavailable(_))
        {
            if self.session.file.is_some() {
                ui.weak(
                    source
                        .path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy(),
                )
                .on_hover_text(little_clip_core::path_address::display_path(&source.path));
            }
            source.capture.content(ui, palette, copies, None);
            self.span = source.capture.span().clone();
            self.domain = source.capture.domain();
        } else {
            self.linked(ui, palette, copies);
        }
    }

    fn controls(&mut self, ui: &mut Ui) {
        let grouped = self.compares_analog();
        if grouped {
            self.comparison.reconcile(&self.sources);
        }
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().interact_size.y = 24.0;
            ui.spacing_mut().button_padding.y = 2.0;
            self.derived.button(ui);
            self.save_buttons(ui);
            let unavailable = self
                .sources
                .iter()
                .filter(|source| matches!(source.capture, Capture::Unavailable(_)));
            if let Some(first) = unavailable.clone().next() {
                let count = unavailable.count();
                if ui.button(format!("Review {count} unavailable")).clicked() {
                    self.reveal_source = Some(first.id);
                }
            }
            if self.sources.len() > 1 {
                if ui
                    .add(egui::Button::selectable(
                        self.alignment.open,
                        "Clock alignment",
                    ))
                    .clicked()
                {
                    let pinned = self.reference_time();
                    self.alignment.toggle(&self.span, pinned);
                }
                if ui.button("Fit all").clicked() {
                    self.span = extent_on(&self.sources, self.alignment.preview(&self.sources));
                }
                if grouped {
                    self.comparison_toolbar(ui);
                }
                if let Some(probe) = self.probe {
                    ui.monospace(display_value(probe, "s"));
                }
            }
            if let Some(error) = &self.error {
                ui.colored_label(ui.visuals().error_fg_color, error);
            }
        });
        self.save_form(ui);
    }

    fn align_recordings(&mut self, ui: &mut Ui) {
        let pinned = self.reference_time();
        match self.alignment.ui(ui, &self.sources, &self.span, pinned) {
            Some(alignment::Action::Apply(suggestion)) => {
                if let Some(source) = self
                    .sources
                    .iter_mut()
                    .find(|source| source.id == suggestion.target())
                {
                    source.set_clock(suggestion.clock);
                }
                ui.ctx().request_repaint();
            }
            Some(alignment::Action::Reveal(visit)) => {
                self.reveal_landmarks(visit);
                ui.ctx().request_repaint();
            }
            None => {}
        }
    }

    fn reveal_landmarks(&mut self, visit: alignment::Visit) {
        let preview = self.alignment.preview(&self.sources);
        let times = visit.endpoints.map(|(id, native)| {
            self.sources
                .iter()
                .find(|source| source.id == id)
                .map(|source| source.display_clock(preview).to_workspace(native))
        });
        if let [Some(first), Some(second)] = times {
            let start = first.min(second);
            let end = first.max(second);
            let domain = extent_on(&self.sources, preview);
            if start < self.span.start || end >= self.span.end {
                let half = ((self.span.end - self.span.start) * 0.5).max((end - start) * 0.65);
                let center = start.midpoint(end);
                self.span = timeline::fit(&domain, center - half..center + half, 0.0);
            }
            self.reveal_source = Some(visit.source);
        }
    }

    fn compares_analog(&self) -> bool {
        self.sources.len() > 1
            && self
                .sources
                .iter()
                .any(|source| matches!(source.capture, Capture::Analog(_)))
    }

    fn linked(&mut self, ui: &mut Ui, palette: &AppPalette, copies: &mut Copies) {
        let preview = self.alignment.preview(&self.sources);
        let domain = extent_on(&self.sources, preview);
        if self.span == self.domain {
            self.span = domain.clone();
        }
        self.domain = domain;
        let shared = self
            .shared
            .controls(ui, &self.sources, preview, &mut self.span, &self.domain);
        let mut next_probe = None;
        let mut action = None;
        let timeline = LinkedTimeline {
            preview,
            shared: shared.as_ref(),
            reference: self.reference_time_on(preview),
        };
        let height = (ui.available_height() / self.sources.len() as f32 - 50.0).max(205.0);
        let grouped = self.compares_analog();
        let scroll = self.session.scroll.area(ui).show(ui, |ui| {
            if grouped {
                (next_probe, action) = self.analog_comparison(ui, copies, timeline);
            }
            for index in 0..self.sources.len() {
                if grouped && matches!(self.sources[index].capture, Capture::Analog(_)) {
                    continue;
                }
                let id = self.sources[index].id;
                let (probe, request) = ui
                    .push_id(id, |ui| {
                        self.linked_source(ui, palette, copies, index, height, timeline)
                    })
                    .inner;
                next_probe = probe.or(next_probe);
                action = request.map(|request| (index, request)).or(action.take());
            }
            let inputs = if self.derived.open() {
                self.sources
                    .iter()
                    .flat_map(|source| source.inputs_on(source.display_clock(preview)))
                    .collect()
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
                    timeline.reference,
                    timeline.shared,
                )
                .or(next_probe);
        });
        self.session
            .scroll
            .finish(ui.ctx(), scroll.state.clamped_offset().y, || {
                self.derived.calculating()
                || self.sources.iter().any(|source| {
                    matches!(&source.capture, Capture::Digital(view) if view.restoring_details())
                })
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

    fn linked_source(
        &mut self,
        ui: &mut Ui,
        palette: &AppPalette,
        copies: &mut Copies,
        index: usize,
        height: f32,
        timeline: LinkedTimeline<'_>,
    ) -> (Option<f64>, Option<SourceAction>) {
        let may_remove = self.sources.len() > 1;
        let source = &mut self.sources[index];
        let reveal = self.reveal_source.take_if(|id| *id == source.id).is_some();
        let start = ui.next_widget_position();
        if let Capture::Unavailable(recording) = &mut source.capture {
            let action = recording.ui(ui, &source.path, may_remove, reveal);
            ui.separator();
            return (None, action);
        }
        let is_reference = self
            .reference
            .as_ref()
            .is_some_and(|pin| pin.source == source.id);
        let mut action = source.controls(
            ui,
            may_remove,
            timeline.reference,
            is_reference,
            timeline.preview,
            &mut self.alignment,
        );
        let link = self.link_source(ui, index, timeline);
        let source = &mut self.sources[index];
        let previous = link.clock.native_range(&self.span);
        *source.capture.span() = previous.clone();
        let height = height.max(source.capture.min_height());
        let interaction = ui
            .allocate_ui(egui::vec2(ui.available_width(), height), |ui| {
                source.capture.content(ui, palette, copies, Some(&link))
            })
            .inner;
        if let Some(range) = interaction.share {
            action = Some(SourceAction::Share(range));
        }
        if reveal {
            ui.scroll_to_rect(
                egui::Rect::from_min_size(start, egui::vec2(1.0, 22.0)),
                Some(egui::Align::TOP),
            );
        }
        let view = source.capture.span();
        if *view != previous {
            self.span = link.clock.workspace_range(view);
            ui.ctx().request_repaint();
        }
        ui.separator();
        (
            interaction
                .probe
                .map(|probe| link.clock.to_workspace(probe)),
            action,
        )
    }

    fn link_source(&mut self, ui: &mut Ui, index: usize, timeline: LinkedTimeline<'_>) -> Link {
        let source = &self.sources[index];
        let clock = source.display_clock(timeline.preview);
        let shared = self.shared.source(
            ui,
            source,
            timeline.shared,
            clock,
            &mut self.span,
            &self.domain,
        );
        Link {
            domain: clock.native_range(&self.domain),
            clock,
            probe: self.probe.map(|time| clock.to_native(time)),
            reference: timeline.reference.map(|time| clock.to_native(time)),
            landmark: self.alignment.marker(source.id),
            shared,
            share_selection: self.sources.len() > 1,
        }
    }

    fn reference_time(&self) -> Option<f64> {
        self.reference_time_on(None)
    }

    fn reference_time_on(&self, preview: Option<alignment::Suggestion>) -> Option<f64> {
        let pin = self.reference.as_ref()?;
        let source = self.sources.iter().find(|source| {
            source.id == pin.source && !matches!(source.capture, Capture::Unavailable(_))
        })?;
        Some(source.display_clock(preview).to_workspace(pin.native_time))
    }

    fn apply_source_action(&mut self, index: usize, action: SourceAction) {
        match action {
            SourceAction::Pin(native_time) => {
                self.reference = Some(Reference {
                    source: self.sources[index].id,
                    native_time,
                });
            }
            SourceAction::Share(native) => self.shared.set(Some(interval::Pin {
                source: self.sources[index].id,
                native,
            })),
            SourceAction::Align(clock) => {
                if !self.sources[index].set_clock(clock) {
                    self.error =
                        Some("The reference is outside this source's time precision.".into());
                }
            }
            SourceAction::ClearReference => self.reference = None,
            SourceAction::Relink(source) => {
                self.sources[index] = source;
                self.comparison.initialized = false;
            }
            SourceAction::Remove => {
                let removed = self.sources.remove(index);
                self.comparison.initialized = false;
                self.alignment.forget(removed.id);
                self.shared.remove_source(removed.id);
                if self
                    .reference
                    .as_ref()
                    .is_some_and(|pin| pin.source == removed.id)
                {
                    self.reference = None;
                }
                if let [source] = &mut self.sources[..]
                    && !matches!(source.capture, Capture::Unavailable(_))
                {
                    let previous = source.clock.native_range(&self.span);
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
            clock: Clock::default(),
            capture,
        })
    }

    fn inputs(&self) -> Vec<inputs::Input> {
        self.inputs_on(self.clock)
    }

    fn inputs_on(&self, clock: Clock) -> Vec<inputs::Input> {
        let label = self.name();
        match &self.capture {
            Capture::Unavailable(source) => source.inputs(clock),
            Capture::Analog(view) => {
                let recording = view.recording();
                let mut inputs = inputs::Input::analog(self.id, clock, &label, recording);
                inputs.extend(view.filtered_channels().filter_map(|filtered| {
                    inputs::Input::filtered(self.id, clock, &label, recording.channels(), filtered)
                }));
                inputs
            }
            Capture::Digital(view) => {
                let (recording, lanes) = view.recording();
                let mut inputs = inputs::Input::digital(self.id, clock, &label, recording, lanes);
                if let Some(pins) = view.decoded_pins() {
                    inputs.extend(pins.pcm().map(|(channel, generation, lane)| {
                        inputs::Input::pcm(self.id, clock, &label, channel, generation, lane)
                    }));
                    if let Some((generation, lane)) = pins.pwm() {
                        inputs.extend(inputs::Input::pwm(self.id, clock, &label, generation, lane));
                    }
                }
                inputs
            }
        }
    }

    fn name(&self) -> std::borrow::Cow<'_, str> {
        self.path
            .file_name()
            .unwrap_or(self.path.as_os_str())
            .to_string_lossy()
    }

    fn controls(
        &mut self,
        ui: &mut Ui,
        may_remove: bool,
        reference: Option<f64>,
        is_reference: bool,
        preview: Option<alignment::Suggestion>,
        alignment: &mut alignment::Alignment,
    ) -> Option<SourceAction> {
        ui.horizontal_wrapped(|ui| {
            ui.strong(self.name().as_ref())
                .on_hover_text(self.path.to_string_lossy());
            self.clock_control(ui, preview);
            let cursor = self
                .capture
                .cursor()
                .filter(|time| self.capture.domain().contains(time));
            alignment.cursor_button(ui, self.id, cursor);
            let mut action = self.pin_controls(ui, reference, is_reference, cursor);
            if self.clock != Clock::default() && ui.small_button("Reset clock").clicked() {
                action = Some(SourceAction::Align(Clock::default()));
            }
            if may_remove
                && ui
                    .small_button("Remove")
                    .on_hover_text("Remove source from this session")
                    .clicked()
            {
                action = Some(SourceAction::Remove);
            }
            action
        })
        .inner
    }

    fn clock_control(&mut self, ui: &mut Ui, preview: Option<alignment::Suggestion>) {
        let mut offset = self.clock.epoch_offset();
        let domain = self.capture.domain();
        let speed = ((domain.end - domain.start) * 0.001).clamp(1e-12, 0.1);
        ui.label("Offset");
        if ui
            .add(
                egui::DragValue::new(&mut offset)
                    .speed(speed)
                    .max_decimals(12)
                    .suffix(" s")
                    .update_while_editing(false),
            )
            .on_hover_text("Displacement at the native reference epoch")
            .changed()
        {
            self.set_clock(self.clock.shifted(offset - self.clock.epoch_offset()));
        }
        let displayed = self.display_clock(preview);
        let ppm = (displayed.scale() - 1.0) * 1e6;
        if displayed != self.clock {
            ui.monospace(format!(
                "Preview {} · {ppm:+.3} ppm",
                display_value(displayed.epoch_offset(), "s")
            ));
        } else if ppm != 0.0 {
            ui.monospace(format!("{ppm:+.3} ppm"));
        }
    }

    fn pin_controls(
        &self,
        ui: &mut Ui,
        reference: Option<f64>,
        is_reference: bool,
        cursor: Option<f64>,
    ) -> Option<SourceAction> {
        if is_reference {
            return ui
                .small_button("Clear pin")
                .clicked()
                .then_some(SourceAction::ClearReference);
        }
        if let Some((reference, cursor)) = reference.zip(cursor)
            && ui.small_button("Align to pin").clicked()
        {
            let clock = self
                .clock
                .shifted(reference - self.clock.to_workspace(cursor));
            return Some(SourceAction::Align(clock));
        }
        if ui
            .add_enabled(cursor.is_some(), egui::Button::new("Pin cursor").small())
            .clicked()
        {
            return cursor.map(SourceAction::Pin);
        }
        None
    }

    fn display_clock(&self, preview: Option<alignment::Suggestion>) -> Clock {
        preview
            .filter(|preview| preview.target() == self.id && preview.base == self.clock)
            .filter(|preview| self.accepts_clock(preview.clock))
            .map_or(self.clock, |preview| preview.clock)
    }

    fn accepts_clock(&self, clock: Clock) -> bool {
        clock.maps_range(&self.capture.domain())
    }

    fn set_clock(&mut self, clock: Clock) -> bool {
        if !self.accepts_clock(clock) {
            return false;
        }
        self.clock = clock;
        true
    }
}

impl Capture {
    fn span(&mut self) -> &mut Range<f64> {
        match self {
            Self::Analog(view) => view.span(),
            Self::Digital(view) => view.span(),
            Self::Unavailable(source) => source.span(),
        }
    }

    fn domain(&self) -> Range<f64> {
        match self {
            Self::Analog(view) => view.domain(),
            Self::Digital(view) => view.domain(),
            Self::Unavailable(source) => source.domain(),
        }
    }

    fn cursor(&self) -> Option<f64> {
        match self {
            Self::Analog(view) => view.cursor(),
            Self::Digital(view) => view.cursor(),
            Self::Unavailable(_) => None,
        }
    }

    fn min_height(&self) -> f32 {
        match self {
            Self::Analog(view) => view.min_height(),
            Self::Digital(view) => view.min_height(),
            Self::Unavailable(_) => 0.0,
        }
    }

    fn content(
        &mut self,
        ui: &mut Ui,
        palette: &AppPalette,
        copies: &mut Copies,
        link: Option<&Link>,
    ) -> Interaction {
        match self {
            Self::Analog(view) => view.content(ui, palette, copies, link),
            Self::Digital(view) => view.content(ui, copies, link),
            Self::Unavailable(_) => Interaction::default(),
        }
    }
}

fn extent(sources: &[Source]) -> Range<f64> {
    extent_on(sources, None)
}

fn extent_on(sources: &[Source], preview: Option<alignment::Suggestion>) -> Range<f64> {
    sources
        .iter()
        .map(|source| {
            source
                .display_clock(preview)
                .workspace_range(&source.capture.domain())
        })
        .fold(f64::INFINITY..f64::NEG_INFINITY, |extent, span| {
            extent.start.min(span.start)..extent.end.max(span.end)
        })
}
