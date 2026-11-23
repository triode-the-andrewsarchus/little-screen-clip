//! Read-only views of tag memory, captured frames, and measured samples.

mod memory;
mod plot;
mod signal;
pub(super) mod strip;
mod trace;

use super::{preview_copy::Copies, preview_job::Task};
use crate::{
    file_source::{FileSource, ReadCheck},
    ui_appearance::WorkbenchColors,
};
use egui::{Context, Ui};
use little_clip_core::{
    BackgroundJobs,
    engineering::tags::{self, Document},
    palette::AppPalette,
};
use std::{
    fs::File,
    io::BufReader,
    path::{Path, PathBuf},
    sync::{
        Arc, LazyLock,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

static JOBS: LazyLock<BackgroundJobs> = LazyLock::new(|| BackgroundJobs::new(2, 5));
type Job<T> = Task<Result<T, String>>;

enum Mode {
    Memory(memory::View),
    Trace(Box<trace::View>),
    Signal(Box<signal::View>),
}

pub(super) struct Viewer {
    source: Arc<Source>,
    title: &'static str,
    mode: Mode,
}

impl Viewer {
    pub fn load(path: &Path, cancel: &AtomicBool) -> Result<Self, String> {
        let (source, doc) = Source::open(path, cancel)?;
        let title = doc.format().title();
        let mode = match doc {
            Document::Memory(memory) => Mode::Memory(memory::View::new(memory)),
            Document::Trace(trace) => Mode::Trace(Box::new(trace::View::new(trace))),
            Document::Signal(signal) => Mode::Signal(Box::new(signal::View::new(signal))),
        };
        Ok(Self {
            source: Arc::new(source),
            title,
            mode,
        })
    }

    pub fn set_check(&mut self, check: ReadCheck) {
        if let Some(source) = Arc::get_mut(&mut self.source) {
            source.check = check;
        }
    }

    pub fn focus_record(&mut self, ordinal: u64) {
        match &mut self.mode {
            Mode::Memory(view) => view.focus(ordinal),
            Mode::Trace(view) => view.focus(ordinal),
            Mode::Signal(view) => view.focus(ordinal),
        }
    }

    pub fn ui(&mut self, ui: &mut Ui, close: &mut bool, palette: &AppPalette, copies: &mut Copies) {
        let mut shell = crate::shell::StudioShell::new(
            self.title,
            crate::icons::Icon::Crosshair,
            palette,
            close,
        );
        shell.render_header(ui, |_| {});
        let colors = WorkbenchColors::from_palette(palette);
        match &mut self.mode {
            Mode::Memory(view) => view.ui(ui, colors),
            Mode::Trace(view) => view.ui(ui, colors, &self.source, copies),
            Mode::Signal(view) => view.ui(ui, colors, &self.source, copies),
        }
    }
}

struct Source {
    path: PathBuf,
    revision: FileSource,
    check: ReadCheck,
}

impl Source {
    fn open(path: &Path, cancel: &AtomicBool) -> Result<(Self, Document), String> {
        let check = ReadCheck::default();
        let (input, revision) = check.open(path, None, cancel)?;
        let doc = tags::preview(path, BufReader::new(&input), cancel).map_err(error)?;
        let source = Self {
            path: path.to_owned(),
            revision,
            check,
        };
        source.validate(&input, cancel)?;
        Ok((source, doc))
    }

    fn validate(&self, input: &File, cancel: &AtomicBool) -> Result<(), String> {
        if cancel.load(Ordering::Relaxed) {
            return Err("Tag recording read cancelled".into());
        }
        self.revision.check_file(input)?;
        if FileSource::capture(&self.path)? != self.revision {
            return Err("Tag recording changed; reopen it before reading".into());
        }
        self.check.validate(cancel)
    }

    fn read<T>(
        &self,
        cancel: &AtomicBool,
        read: impl FnOnce(BufReader<&File>) -> Result<T, tags::Error>,
    ) -> Result<T, String> {
        let (input, _) = self.check.open(&self.path, Some(&self.revision), cancel)?;
        let value = read(BufReader::new(&input)).map_err(error)?;
        self.validate(&input, cancel)?;
        Ok(value)
    }
}

fn launch<T: Send + 'static>(
    context: &Context,
    timeout: Duration,
    run: impl FnOnce(&AtomicBool) -> Result<T, String> + Send + 'static,
) -> Job<T> {
    let context = context.clone();
    Task::start(&JOBS, timeout, move |cancel| {
        let result = run(cancel);
        context.request_repaint();
        result
    })
}

fn poll<T: Send + 'static>(job: &mut Option<Job<T>>) -> Option<Result<T, String>> {
    let result = job.as_ref()?.poll()?;
    *job = None;
    Some(result.map_err(error).and_then(|result| result))
}

fn keyed_poll<K, T: Send + 'static>(
    job: &mut Option<(K, Job<T>)>,
) -> Option<(K, Result<T, String>)> {
    let result = job.as_ref()?.1.poll()?;
    let (key, _) = job.take()?;
    Some((key, result.map_err(error).and_then(|result| result)))
}

fn error(error: impl std::fmt::Display) -> String {
    error.to_string()
}

fn hex(bytes: impl IntoIterator<Item = Option<u8>>) -> String {
    use std::fmt::Write as _;
    let mut text = String::new();
    for byte in bytes {
        if !text.is_empty() {
            text.push(' ');
        }
        if let Some(byte) = byte {
            let _ = write!(text, "{byte:02X}");
        } else {
            text.push_str("··");
        }
    }
    text
}
