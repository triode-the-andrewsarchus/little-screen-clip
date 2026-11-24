//! Recipes for recorded signal workspaces.
use super::{
    Capture, Reference, Source, SourceAction, Viewer, derived, extent, inputs, logic_view,
    scope_view,
};
use crate::file_source::FileSource;
use egui::{Context, Ui};
use little_clip_core::{
    BackgroundJobs, JobError,
    engineering::analysis::Clock,
    file_source::{create_file_in, read_file_snapshot_with_source},
    kernel_interface::pin_content_directory,
    path_address::{decode_file_name, decode_path, display_path},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    ops::Range,
    os::fd::AsRawFd,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    time::Duration,
};

const VERSION: u32 = 2;
const MAX_BYTES: usize = 2 * 1024 * 1024;
pub(super) const EXTENSION: &str = "clip-analysis";

pub(in crate::file_manager) fn preview(
    path: &Path,
    cancel: &AtomicBool,
) -> super::super::shelf::preview::Data {
    let mut data = super::super::shelf::preview::Data::new(
        display_path(path),
        "Analysis session".into(),
        crate::icons::Icon::Waves,
    );
    if !cancel.load(Ordering::Relaxed) {
        data.detail = match Recipe::read(path) {
            Ok((recipe, _)) => format!(
                "Recordings {} · Equations {}",
                recipe.sources.len(),
                recipe.derived.equation_count(),
            ),
            Err(error) => error,
        };
    }
    data
}

#[derive(Serialize, Deserialize)]
struct Recipe {
    version: u32,
    sources: Vec<SavedSource>,
    span: Range<f64>,
    #[serde(default)]
    scroll_offset: f32,
    reference: Option<Reference>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    shared: Option<super::interval::Pin>,
    derived: derived::SessionSettings,
    #[serde(default)]
    alignment: super::alignment::Settings,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    trace_order: Vec<super::comparison::Trace>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    active_trace: Option<super::comparison::Trace>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    comparison_spectrum: bool,
}

#[derive(Deserialize)]
struct Header {
    version: u32,
}

#[derive(Clone, Serialize, Deserialize)]
struct SavedSource {
    id: u64,
    path: String,
    revision: Vec<u8>,
    channels: Vec<(String, String)>,
    native_domain: Range<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    clock: Option<Clock>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    offset: Option<f64>,
    inputs: Vec<inputs::SavedInput>,
    capture: Display,
}

#[derive(Clone, Serialize, Deserialize)]
enum Display {
    Scope(scope_view::SessionSettings),
    Logic(logic_view::SessionSettings),
}

pub(super) struct Unavailable {
    saved: SavedSource,
    error: String,
    replacement: String,
    pending: Option<super::Task<Result<Source, String>>>,
}

impl Unavailable {
    pub(super) fn domain(&self) -> Range<f64> {
        self.saved.native_domain.clone()
    }

    pub(super) fn span(&mut self) -> &mut Range<f64> {
        &mut self.saved.native_domain
    }

    pub(super) fn inputs(&self, clock: Clock) -> Vec<inputs::Input> {
        self.saved
            .inputs
            .iter()
            .map(|input| inputs::Input::missing(input, clock))
            .collect()
    }

    pub(super) fn ui(
        &mut self,
        ui: &mut Ui,
        original: &Path,
        may_remove: bool,
        reveal: bool,
    ) -> Option<SourceAction> {
        let start = ui.next_widget_position();
        ui.strong(
            original
                .file_name()
                .unwrap_or(original.as_os_str())
                .to_string_lossy(),
        );
        ui.colored_label(ui.visuals().error_fg_color, &self.error);
        if let Some(result) = self.pending.as_ref().and_then(super::Task::poll) {
            self.pending = None;
            match result
                .map_err(|error| error.to_string())
                .and_then(|result| result)
            {
                Ok(source) => return Some(SourceAction::Relink(source)),
                Err(error) => self.error = error,
            }
        }
        ui.horizontal_wrapped(|ui| {
            ui.label("Recording path");
            ui.add(egui::TextEdit::singleline(&mut self.replacement).desired_width(340.0));
            if ui
                .add_enabled(
                    self.pending.is_none(),
                    egui::Button::new("Relink recording"),
                )
                .clicked()
            {
                match decode_path(&self.replacement) {
                    Ok(path) if path.is_absolute() => {
                        let saved = self.saved.clone();
                        let ctx = ui.ctx().clone();
                        let job = super::Task::start(
                            BackgroundJobs::global(),
                            Duration::from_secs(90),
                            move |cancel| {
                                let result = saved.open(&path, true, cancel);
                                ctx.request_repaint();
                                result
                            },
                        );
                        self.pending = Some(job);
                    }
                    _ => self.error = "Enter the full path to the recording.".into(),
                }
            }
            if self.pending.is_some() {
                ui.spinner();
                ui.ctx().request_repaint_after(Duration::from_millis(35));
            }
        });
        let remove = may_remove && ui.small_button("Remove source").clicked();
        if reveal {
            ui.scroll_to_rect(
                egui::Rect::from_two_pos(start, ui.next_widget_position()),
                Some(egui::Align::TOP),
            );
        }
        remove.then_some(SourceAction::Remove)
    }
}

impl Recipe {
    fn read(path: &Path) -> Result<(Self, FileSource), String> {
        let (bytes, file) =
            read_file_snapshot_with_source(path, MAX_BYTES).map_err(|error| error.to_string())?;
        let header: Header = serde_json::from_slice(&bytes)
            .map_err(|error| format!("Invalid analysis session: {error}"))?;
        if !(1..=VERSION).contains(&header.version) {
            return Err(format!(
                "Analysis session version {} is unsupported.",
                header.version
            ));
        }
        let recipe: Self = serde_json::from_slice(&bytes)
            .map_err(|error| format!("Invalid analysis session: {error}"))?;
        let mut ids = BTreeSet::new();
        if !(1..=super::MAX_SOURCES).contains(&recipe.sources.len())
            || !valid_span(&recipe.span)
            || !recipe.scroll_offset.is_finite()
            || recipe.scroll_offset < 0.0
            || recipe
                .sources
                .iter()
                .any(|source| !ids.insert(source.id) || !source.valid(recipe.version))
        {
            return Err("Invalid source identifiers or timeline in the analysis session".into());
        }
        if recipe.reference.as_ref().is_some_and(|reference| {
            !ids.contains(&reference.source) || !reference.native_time.is_finite()
        }) {
            return Err("Invalid analysis reference marker".into());
        }
        if recipe
            .shared
            .as_ref()
            .is_some_and(|shared| !ids.contains(&shared.source) || !valid_span(&shared.native))
        {
            return Err("Invalid shared analysis range".into());
        }
        Ok((recipe, file))
    }

    fn take(viewer: &Viewer) -> Self {
        Self {
            version: VERSION,
            sources: viewer.sources.iter().map(SavedSource::take).collect(),
            span: viewer.span.clone(),
            scroll_offset: viewer.session.scroll.offset,
            reference: viewer.reference.clone(),
            shared: viewer.shared.pin.clone(),
            derived: viewer.derived.session_settings(),
            alignment: viewer.alignment.save(),
            trace_order: viewer.comparison.order.clone(),
            active_trace: viewer.comparison.active,
            comparison_spectrum: viewer.comparison.spectrum,
        }
    }

    fn write(mut self, target: Target) -> Result<FileSource, String> {
        let path = target.path();
        let parent = path.parent().ok_or("Choose a destination folder")?;
        let directory = pin_content_directory(parent).map_err(|error| error.to_string())?;
        let resolved = std::fs::read_link(format!("/proc/self/fd/{}", directory.as_raw_fd()))
            .map_err(|error| error.to_string())?;
        let path = resolved.join(path.file_name().ok_or("Choose a filename")?);
        for source in &mut self.sources {
            let original = decode_path(&source.path)?;
            source.path = display_path(original.strip_prefix(&resolved).unwrap_or(&original));
        }
        let bytes = serde_json::to_vec_pretty(&self).map_err(|error| error.to_string())?;
        if bytes.len() > MAX_BYTES {
            return Err("This analysis session exceeds the recipe size limit".into());
        }
        match target {
            Target::New(_) => FileSource::capture_file(&create_file_in(&path, &bytes, &directory)?),
            Target::Replace(file) => {
                if path != file.path() {
                    return Err(
                        "The analysis destination changed. Use Save As to choose a new file."
                            .into(),
                    );
                }
                file.write_in(&path, &bytes, &directory)
            }
        }
    }
}

fn valid_span(span: &Range<f64>) -> bool {
    span.start.is_finite() && span.end.is_finite() && !span.is_empty()
}

impl SavedSource {
    fn clock(&self) -> Clock {
        self.clock
            .unwrap_or_else(|| Clock::offset(self.offset.unwrap_or(0.0)))
    }

    fn valid(&self, version: u32) -> bool {
        let clock = self.clock();
        let prefix = format!("s{}_", self.id);
        self.id > 0
            && self.id < u64::MAX / 2
            && valid_span(&self.native_domain)
            && match version {
                1 => self.offset.is_some() && self.clock.is_none(),
                VERSION => self.clock.is_some() && self.offset.is_none(),
                _ => false,
            }
            && clock.maps_range(&self.native_domain)
            && self.revision.len() == 56
            && self.channels.len() <= 1024
            && self.inputs.len() <= 2048
            && self
                .inputs
                .iter()
                .all(|input| input.alias.starts_with(&prefix))
    }

    fn take(source: &Source) -> Self {
        if let Capture::Unavailable(unavailable) = &source.capture {
            let mut saved = unavailable.saved.clone();
            saved.path = display_path(&source.path);
            saved.clock = Some(source.clock);
            saved.offset = None;
            return saved;
        }
        let file = source.capture.file().expect("available capture");
        let capture = match &source.capture {
            Capture::Analog(view) => Display::Scope(view.session_settings()),
            Capture::Digital(view) => Display::Logic(view.session_settings()),
            Capture::Unavailable(_) => unreachable!("recipe retained for unavailable capture"),
        };
        Self {
            id: source.id,
            path: display_path(&source.path),
            revision: file.revision_key(),
            channels: source.capture.channels(),
            native_domain: source.capture.domain(),
            clock: Some(source.clock),
            offset: None,
            inputs: source
                .inputs()
                .iter()
                .map(inputs::Input::snapshot)
                .collect(),
            capture,
        }
    }

    fn open(&self, path: &Path, relink: bool, cancel: &AtomicBool) -> Result<Source, String> {
        let identity = FileSource::capture(path)?;
        if !relink && identity.revision_key() != self.revision {
            return Err(
                "Recording revision changed. Review the path and relink it to use this revision."
                    .into(),
            );
        }
        let mut source = Source::load(path, None, cancel)?;
        if source.capture.file() != Some(&identity) {
            return Err("The recording changed while opening".into());
        }
        if source.capture.channels() != self.channels {
            return Err("The recording channel names or units differ from the saved source".into());
        }
        match (&mut source.capture, self.capture.clone()) {
            (Capture::Analog(view), Display::Scope(settings)) => view.restore_session(settings)?,
            (Capture::Digital(view), Display::Logic(settings)) => {
                view.restore_session(settings, cancel)?
            }
            _ => return Err("The recording type differs from the saved source".into()),
        }
        source.id = self.id;
        if !source.set_clock(self.clock()) {
            return Err("The recording offset cannot be represented".into());
        }
        Ok(source)
    }

    fn unavailable(self, path: PathBuf, error: String) -> Source {
        let replacement = display_path(&path);
        Source {
            id: self.id,
            path,
            clock: self.clock(),
            capture: Capture::Unavailable(Box::new(Unavailable {
                saved: self,
                error,
                replacement,
                pending: None,
            })),
        }
    }
}

impl Capture {
    pub(super) fn file(&self) -> Option<&FileSource> {
        match self {
            Self::Analog(view) => Some(view.recording().source()),
            Self::Digital(view) => Some(view.recording().0.source()),
            Self::Unavailable(_) => None,
        }
    }

    fn channels(&self) -> Vec<(String, String)> {
        match self {
            Self::Analog(view) => view
                .recording()
                .channels()
                .iter()
                .map(|channel| (channel.name.clone(), channel.unit.clone()))
                .collect(),
            Self::Digital(view) => view
                .recording()
                .1
                .iter()
                .map(|channel| (channel.name.clone(), String::new()))
                .collect(),
            Self::Unavailable(source) => source.saved.channels.clone(),
        }
    }
}

enum Target {
    New(PathBuf),
    Replace(FileSource),
}

impl Target {
    fn path(&self) -> &Path {
        match self {
            Self::New(path) => path,
            Self::Replace(file) => file.path(),
        }
    }
}

struct Destination {
    folder: String,
    name: String,
    error: Option<String>,
}

impl Destination {
    fn target(&self) -> Result<Target, String> {
        let folder = decode_path(&self.folder)?;
        if !folder.is_absolute() {
            return Err("Enter the full path to the destination folder.".into());
        }
        let mut name = decode_file_name(&self.name)?;
        if Path::new(&name)
            .extension()
            .is_none_or(|ext| !ext.eq_ignore_ascii_case(EXTENSION))
        {
            name.push(format!(".{EXTENSION}"));
        }
        Ok(Target::New(folder.join(name)))
    }
}

type SaveResult = Result<Result<FileSource, String>, JobError>;

#[derive(PartialEq)]
struct AnalysisEdits {
    sources: Vec<SourceEdits>,
    reference: Option<Reference>,
    shared: Option<super::interval::Pin>,
    derived: derived::AnalysisEdits,
    alignment: super::alignment::Settings,
}

#[derive(PartialEq)]
struct SourceEdits {
    id: u64,
    path: PathBuf,
    clock: Clock,
    settings: CaptureEdits,
}

#[derive(PartialEq)]
enum CaptureEdits {
    Scope(scope_view::AnalysisEdits),
    Logic(logic_view::protocols::SessionSettings),
    Unavailable,
}

impl AnalysisEdits {
    fn take(viewer: &Viewer) -> Self {
        Self {
            sources: viewer
                .sources
                .iter()
                .map(|source| SourceEdits {
                    id: source.id,
                    path: source.path.clone(),
                    clock: source.clock,
                    settings: match &source.capture {
                        Capture::Analog(view) => CaptureEdits::Scope(view.analysis_edits()),
                        Capture::Digital(view) => CaptureEdits::Logic(view.analysis_edits()),
                        Capture::Unavailable(_) => CaptureEdits::Unavailable,
                    },
                })
                .collect(),
            reference: viewer.reference.clone(),
            shared: viewer.shared.pin.clone(),
            derived: viewer.derived.analysis_edits(),
            alignment: viewer.alignment.analysis_edits(),
        }
    }
}

struct PendingSave {
    receiver: mpsc::Receiver<SaveResult>,
    edits: AnalysisEdits,
}

#[derive(Default)]
pub(super) struct Session {
    pub(super) file: Option<FileSource>,
    pub(super) scroll: Scroll,
    form: Option<Destination>,
    pending: Option<PendingSave>,
    baseline: Option<AnalysisEdits>,
    message: Option<Result<String, String>>,
}

pub(super) struct Scroll {
    id: u64,
    offset: f32,
    restore: Option<f32>,
    settled: bool,
}

impl Default for Scroll {
    fn default() -> Self {
        static NEXT_ID: AtomicU64 = AtomicU64::new(0);
        Self {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            offset: 0.0,
            restore: None,
            settled: false,
        }
    }
}

impl Scroll {
    pub(super) fn offset(&self) -> f32 {
        self.offset
    }

    pub(super) fn area(&mut self, ui: &Ui) -> egui::ScrollArea {
        let area = egui::ScrollArea::vertical()
            .id_salt(("linked-recordings", self.id))
            .auto_shrink([false, false]);
        if ui.input(|input| input.smooth_scroll_delta.y != 0.0 || input.pointer.any_pressed()) {
            self.restore = None;
        }
        match self.restore {
            Some(offset) => area.vertical_scroll_offset(offset),
            None => area,
        }
    }

    pub(super) fn finish(&mut self, ctx: &Context, offset: f32, loading: impl FnOnce() -> bool) {
        self.offset = offset;
        if self.restore.is_some() {
            let ready = !loading();
            // A decoded table or the first derived frame can enlarge the scroll
            // area as it arrives. Apply the saved offset after that layout too.
            if ready && self.settled {
                self.restore = None;
            } else if ready {
                ctx.request_repaint();
            }
            self.settled = ready;
        }
    }
}

impl Session {
    fn poll(&mut self, ctx: &Context) {
        let Some(pending) = &self.pending else { return };
        let result = match pending.receiver.try_recv() {
            Err(mpsc::TryRecvError::Empty) => {
                ctx.request_repaint_after(Duration::from_millis(35));
                return;
            }
            result => result
                .map_err(|error| error.to_string())
                .and_then(|result| result.map_err(|error| error.to_string()))
                .and_then(|result| result),
        };
        let Some(pending) = self.pending.take() else {
            return;
        };
        self.message = Some(result.map(|source| {
            let name = source
                .path()
                .file_name()
                .unwrap_or_default()
                .to_string_lossy();
            let message = format!("Saved {name}");
            self.file = Some(source);
            self.baseline = Some(pending.edits);
            self.form = None;
            message
        }));
    }
}

impl Viewer {
    pub(in crate::file_manager) fn load_session(
        path: &Path,
        cancel: &AtomicBool,
    ) -> Result<Self, String> {
        let (recipe, file) = Recipe::read(path)?;
        let parent = file
            .path()
            .parent()
            .ok_or("Analysis session has no enclosing directory")?;
        let mut sources = Vec::with_capacity(recipe.sources.len());
        for saved in recipe.sources {
            if cancel.load(Ordering::Relaxed) {
                return Err("Opening analysis was cancelled".into());
            }
            let path = parent.join(decode_path(&saved.path)?);
            let source = match saved.open(&path, false, cancel) {
                Ok(source) => source,
                Err(error) => saved.unavailable(path, error),
            };
            sources.push(source);
        }
        let domain = extent(&sources);
        let mut viewer = Self {
            sources,
            domain,
            span: recipe.span,
            reference: recipe.reference,
            shared: super::interval::Shared::restore(recipe.shared),
            reveal_source: None,
            reveal_input: None,
            derived: derived::Derived::default(),
            alignment: super::alignment::Alignment::default(),
            comparison: super::comparison::Comparison {
                order: recipe.trace_order,
                active: recipe.active_trace,
                spectrum: recipe.comparison_spectrum,
                ..Default::default()
            },
            probe: None,
            error: None,
            session: Session {
                file: Some(file),
                scroll: Scroll {
                    offset: recipe.scroll_offset,
                    restore: Some(recipe.scroll_offset),
                    ..Default::default()
                },
                ..Default::default()
            },
        };
        viewer.derived.restore_session(recipe.derived)?;
        viewer
            .alignment
            .restore(recipe.alignment, &viewer.sources)?;
        if let [source] = &mut viewer.sources[..]
            && !matches!(source.capture, Capture::Unavailable(_))
        {
            *source.capture.span() = source.clock.native_range(&viewer.span);
        }
        viewer.capture_edit_baseline();
        Ok(viewer)
    }

    pub(in crate::file_manager) fn session_path(&self) -> Option<&Path> {
        self.session.file.as_ref().map(FileSource::path)
    }

    pub(in crate::file_manager) fn session_write_pending(&self) -> bool {
        self.session.pending.is_some()
    }

    pub(super) fn capture_edit_baseline(&mut self) {
        self.session.baseline = Some(AnalysisEdits::take(self));
    }

    pub(in crate::file_manager) fn session_dirty(&self) -> bool {
        self.derived.has_draft_edits()
            || self
                .session
                .baseline
                .as_ref()
                .is_some_and(|baseline| *baseline != AnalysisEdits::take(self))
    }

    pub(in crate::file_manager) fn tick_session(&mut self, ctx: &Context) {
        self.session.poll(ctx);
    }

    pub(in crate::file_manager) fn can_save_open_recipe(&self) -> bool {
        self.session.file.is_some()
            && self.session.pending.is_none()
            && self.session.form.is_none()
            && !self.derived.has_draft_edits()
    }

    pub(in crate::file_manager) fn save_open_recipe(&mut self, ctx: &Context) -> bool {
        if !self.can_save_open_recipe() {
            return false;
        }
        let Some(file) = self.session.file.clone() else {
            return false;
        };
        self.start_save(Target::Replace(file), ctx);
        true
    }

    pub(in crate::file_manager) fn save_error(&self) -> Option<&str> {
        self.session
            .message
            .as_ref()?
            .as_ref()
            .err()
            .map(String::as_str)
    }

    pub(super) fn save_buttons(&mut self, ui: &mut Ui) {
        let idle = self.session.pending.is_none();
        let unapplied = self.derived.has_draft_edits();
        let may_save = idle && self.session.form.is_none() && !unapplied;
        if ui
            .add_enabled(may_save, egui::Button::new("Save analysis"))
            .clicked()
        {
            if self.session.file.is_none() {
                self.begin_save_as();
            } else {
                self.save_open_recipe(ui.ctx());
            }
        }
        if self.session.file.is_some()
            && ui
                .add_enabled(may_save, egui::Button::new("Save as"))
                .clicked()
        {
            self.begin_save_as();
        }
        if !idle {
            ui.spinner();
            ui.weak("Saving analysis…");
        } else if unapplied {
            ui.weak("Apply or cancel the equation draft to save");
        }
    }

    fn begin_save_as(&mut self) {
        let original = self.session_path().unwrap_or(&self.sources[0].path);
        let folder = original.parent().map(display_path).unwrap_or_default();
        let name = original
            .file_stem()
            .map(|name| display_path(Path::new(name)))
            .unwrap_or_default();
        let name = if self.session.file.is_some() {
            format!("{name} copy")
        } else {
            name
        };
        self.session.form = Some(Destination {
            folder,
            name,
            error: None,
        });
        self.session.message = None;
    }

    pub(super) fn save_form(&mut self, ui: &mut Ui) {
        let may_save = !self.derived.has_draft_edits();
        if let Some(form) = &mut self.session.form {
            let pending = self.session.pending.is_some();
            let (save, cancel) = ui
                .add_enabled_ui(!pending, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.label("Filename");
                        ui.add(egui::TextEdit::singleline(&mut form.name).desired_width(160.0));
                        ui.label("Folder");
                        ui.add(egui::TextEdit::singleline(&mut form.folder).desired_width(270.0));
                        (
                            ui.add_enabled(may_save, egui::Button::new("Save"))
                                .clicked(),
                            ui.button("Cancel").clicked(),
                        )
                    })
                    .inner
                })
                .inner;
            if let Some(error) = &form.error {
                ui.colored_label(ui.visuals().error_fg_color, error);
            }
            if save {
                match form.target() {
                    Ok(target) => self.start_save(target, ui.ctx()),
                    Err(error) => form.error = Some(error),
                }
            } else if cancel {
                self.session.form = None;
            }
        }
        if let Some(message) = &self.session.message {
            match message {
                Ok(text) => {
                    ui.weak(text);
                }
                Err(text) => {
                    ui.colored_label(ui.visuals().error_fg_color, text);
                }
            }
        }
    }

    fn start_save(&mut self, target: Target, ctx: &Context) {
        let recipe = Recipe::take(self);
        let edits = AnalysisEdits::take(self);
        let ctx = ctx.clone();
        let (_, receiver) = BackgroundJobs::global().submit(Duration::MAX, move || {
            let result = recipe.write(target);
            ctx.request_repaint();
            result
        });
        self.session.message = None;
        self.session.pending = Some(PendingSave { receiver, edits });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{ffi::OsString, fs, os::unix::ffi::OsStringExt};

    fn logic_fixture(path: &Path) {
        fs::write(
            path,
            "$timescale 1us $end\n$scope module bench $end\n\
             $var wire 1 ! PWM $end\n$upscope $end\n$enddefinitions $end\n\
             #0\n0!\n#1000\n1!\n#1250\n0!\n#2000\n1!\n#2500\n0!\n#3000\n",
        )
        .unwrap();
    }

    #[test]
    fn recipe_dirty_tracks_analysis_changes_and_the_snapshot_accepted_for_save() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("capture.vcd");
        let path = root.path().join("bench.clip-analysis");
        logic_fixture(&source);
        let cancel = AtomicBool::new(false);
        let viewer = Viewer::load(&source, super::super::Format::LogicCapture, &cancel).unwrap();
        Recipe::take(&viewer)
            .write(Target::New(path.clone()))
            .unwrap();

        let mut viewer = Viewer::load_session(&path, &cancel).unwrap();
        viewer.span = 0.001..0.002;
        viewer.probe = Some(0.0015);
        viewer.session.scroll.offset = 12.0;
        viewer.comparison.spectrum = true;
        assert!(!viewer.session_dirty());

        viewer.sources[0].clock = Clock::offset(1.0);
        assert!(viewer.session_dirty());
        let ctx = Context::default();
        let file = viewer.session.file.clone().unwrap();
        viewer.start_save(Target::Replace(file), &ctx);
        viewer.sources[0].clock = Clock::offset(2.0);
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while viewer.session_write_pending() {
            viewer.tick_session(&ctx);
            assert!(std::time::Instant::now() < deadline, "save did not settle");
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(viewer.session.message.as_ref().unwrap().is_ok());
        assert!(viewer.session_dirty());
        viewer.sources[0].clock = Clock::offset(1.0);
        assert!(!viewer.session_dirty());
        let saved = Viewer::load_session(&path, &cancel).unwrap();
        assert_eq!(saved.sources[0].clock, Clock::offset(1.0));
        assert!(!saved.session_dirty());
    }

    #[test]
    fn session_preserves_paths_source_ids_calibration_and_working_interval() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let first = root.join("removed.vcd");
        let second = root.join(OsString::from_vec(b"recording-\xff.vcd".to_vec()));
        let third = root.join("reference.vcd");
        for path in [&first, &second, &third] {
            logic_fixture(path);
        }
        let cancel = AtomicBool::new(false);
        let mut viewer = Viewer::load_sources(vec![first, second.clone(), third], &cancel).unwrap();
        viewer.sources.remove(0);
        let clock = Clock::anchored(0.125, 0.25, 1.001).unwrap();
        assert!(viewer.sources[0].set_clock(clock));
        let alignment = serde_json::json!({
            "recordings": {"reference": 3, "target": 2},
            "sets": [{
                "recordings": {"reference": 3, "target": 2},
                "pairs": [
                    {"native": [0.001, 0.001], "included": true},
                    {"native": [0.002, 0.002], "included": false}
                ]
            }]
        });
        viewer
            .alignment
            .restore(
                serde_json::from_value(alignment.clone()).unwrap(),
                &viewer.sources,
            )
            .unwrap();
        viewer.span = 0.0..0.125;
        viewer.session.scroll.offset = 240.0;
        viewer.reference = Some(Reference {
            source: 3,
            native_time: 0.001,
        });
        viewer.shared.set(Some(super::super::interval::Pin {
            source: 2,
            native: 0.001..0.002,
        }));
        let path = root.join("bench.clip-analysis");
        Recipe::take(&viewer)
            .write(Target::New(path.clone()))
            .unwrap();
        let (saved, _) = Recipe::read(&path).unwrap();
        assert_eq!(
            decode_path(&saved.sources[0].path).unwrap(),
            second.file_name().unwrap()
        );

        let loaded = Viewer::load_session(&path, &cancel).unwrap();
        assert_eq!(
            loaded
                .sources
                .iter()
                .map(|source| source.id)
                .collect::<Vec<_>>(),
            [2, 3]
        );
        assert_eq!(loaded.sources[0].path, second);
        assert_eq!(loaded.sources[0].clock, clock);
        assert_eq!(
            serde_json::to_value(loaded.alignment.save()).unwrap(),
            alignment
        );
        assert_eq!(loaded.span, 0.0..0.125);
        assert_eq!(loaded.session.scroll.restore, Some(240.0));
        assert_eq!(loaded.reference_time(), Some(0.001));
        let pin = loaded.shared.pin.as_ref().unwrap();
        assert_eq!(pin.source, 2);
        assert_eq!(pin.native, 0.001..0.002);
        assert_eq!(
            pin.project(&loaded.sources[0], None),
            Some(clock.workspace_range(&pin.native))
        );
        let workspace = clock.workspace_range(&pin.native);
        assert_eq!(pin.native_for(2, clock, &workspace), pin.native);
        assert_eq!(pin.native_for(3, Clock::default(), &workspace), workspace);
        assert_eq!(loaded.sources[0].inputs()[0].alias, "s2_c1");
    }

    #[test]
    fn opens_version_one_recipes_with_constant_offsets() {
        let directory = tempfile::tempdir().unwrap();
        let recording = directory.path().join("recording.vcd");
        logic_fixture(&recording);
        let cancel = AtomicBool::new(false);
        let viewer = Viewer::load(&recording, super::super::Format::LogicCapture, &cancel).unwrap();
        let mut recipe = serde_json::to_value(Recipe::take(&viewer)).unwrap();
        recipe["version"] = 1.into();
        recipe.as_object_mut().unwrap().remove("alignment");
        let source = recipe["sources"][0].as_object_mut().unwrap();
        source.remove("clock");
        source.insert("offset".into(), 0.125.into());
        let path = directory.path().join("legacy.clip-analysis");
        fs::write(&path, serde_json::to_vec(&recipe).unwrap()).unwrap();
        let loaded = Viewer::load_session(&path, &cancel).unwrap();
        assert_eq!(loaded.sources[0].clock, Clock::offset(0.125));
    }

    #[test]
    fn missing_and_revised_recordings_keep_bindings_until_explicit_relink() {
        let directory = tempfile::tempdir().unwrap();
        let first = directory.path().join("missing.vcd");
        let second = directory.path().join("revised.vcd");
        let third = directory.path().join("intact.vcd");
        for path in [&first, &second, &third] {
            logic_fixture(path);
        }
        let cancel = AtomicBool::new(false);
        let viewer =
            Viewer::load_sources(vec![first.clone(), second.clone(), third], &cancel).unwrap();
        let path = directory.path().join("bench.clip-analysis");
        Recipe::take(&viewer)
            .write(Target::New(path.clone()))
            .unwrap();
        let replacement = directory.path().join("relocated.vcd");
        fs::rename(&first, &replacement).unwrap();
        let mut revised = fs::read(&second).unwrap();
        revised.extend_from_slice(b"#3500\n");
        fs::write(&second, revised).unwrap();

        let loaded = Viewer::load_session(&path, &cancel).unwrap();
        for (index, path) in [&replacement, &second].into_iter().enumerate() {
            let source = &loaded.sources[index];
            let Capture::Unavailable(unavailable) = &source.capture else {
                panic!("the wrong recording was opened");
            };
            let original_inputs = viewer.sources[index].inputs();
            let missing_inputs = source.inputs();
            assert!(missing_inputs[0].unavailable());
            assert_eq!(missing_inputs[0].alias, original_inputs[0].alias);
            let relinked = unavailable.saved.open(path, true, &cancel).unwrap();
            assert_eq!(relinked.id, source.id);
            assert_eq!(relinked.inputs()[0].alias, original_inputs[0].alias);
        }
        assert!(!loaded.sources[2].inputs()[0].unavailable());
        let Capture::Unavailable(revised) = &loaded.sources[1].capture else {
            unreachable!();
        };
        assert!(revised.error.contains("revision changed"));
    }

    #[test]
    fn saving_respects_occupied_destinations_and_external_edits() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("bench.vcd");
        logic_fixture(&source);
        let cancel = AtomicBool::new(false);
        let viewer = Viewer::load(&source, super::super::Format::LogicCapture, &cancel).unwrap();
        let path = directory.path().join("bench.clip-analysis");
        let saved = Recipe::take(&viewer)
            .write(Target::New(path.clone()))
            .unwrap();
        let original = fs::read(&path).unwrap();
        assert!(
            Recipe::take(&viewer)
                .write(Target::New(path.clone()))
                .is_err()
        );
        assert_eq!(fs::read(&path).unwrap(), original);
        let saved = Recipe::take(&viewer).write(Target::Replace(saved)).unwrap();
        fs::write(&path, b"{").unwrap();
        assert!(Recipe::take(&viewer).write(Target::Replace(saved)).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"{");
        assert!(
            Recipe::read(&path)
                .err()
                .unwrap()
                .contains("Invalid analysis session")
        );
        fs::write(&path, br#"{"version":3}"#).unwrap();
        assert!(Recipe::read(&path).err().unwrap().contains("version 3"));
    }
}
