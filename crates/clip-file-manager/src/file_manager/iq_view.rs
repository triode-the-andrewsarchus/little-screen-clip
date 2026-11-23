//! Read-only I/Q capture activity, linked spectral views, and sample selection.
mod panels;
mod plot;
mod raw;
pub(super) mod strip;
mod waterfall;

use super::{preview_copy, preview_copy::Copies, preview_job::Task};
use crate::file_source::{FileSource, ReadCheck};
use egui::{Context, TextureHandle, TextureOptions, Ui};
use little_clip_core::{
    BackgroundJobs,
    engineering::iq::{PowerOverview, RawInterpretation, Recording, Spectrum, Waterfall},
    palette::AppPalette,
};
use std::{
    io::Write,
    ops::Range,
    path::{Path, PathBuf},
    sync::{Arc, LazyLock, atomic::AtomicBool},
    time::Duration,
};

const MAX_EXPORT_SAMPLES: u64 = 262_144;
const WATERFALL_ROWS: usize = 192;
const FFT_SIZES: [usize; 5] = [128, 256, 512, 1024, 4096];
static JOBS: LazyLock<BackgroundJobs> = LazyLock::new(|| BackgroundJobs::new(3, 5));
type ReadJob<T> = Task<Result<T, String>>;

struct Loaded {
    recording: Arc<Recording>,
    power: PowerOverview,
    scale: Range<f32>,
}

impl Loaded {
    fn read(recording: Recording, cancel: &AtomicBool) -> Result<Self, String> {
        let power = recording.checked(cancel, |capture| {
            capture.overview(0..capture.sample_count(), 256, cancel)
        })?;
        let scale = plot::power_scale(&power);
        Ok(Self {
            recording: Arc::new(recording),
            power,
            scale,
        })
    }
}

#[derive(Clone, PartialEq, Eq)]
struct WaterfallKey {
    span: Range<u64>,
    fft: usize,
}

struct WaterfallFrame {
    key: WaterfallKey,
    data: Waterfall,
    texture: TextureHandle,
    color_span: (f32, f32),
}

#[derive(Default)]
struct Displays {
    waterfall: Option<WaterfallFrame>,
    waterfall_key: Option<WaterfallKey>,
    waterfall_job: Option<ReadJob<waterfall::Image>>,
    spectrum: Option<Spectrum>,
    spectrum_range: Option<Range<u64>>,
    spectrum_job: Option<ReadJob<Spectrum>>,
}

pub(super) struct Viewer {
    path: PathBuf,
    check: ReadCheck,
    raw: Option<raw::Fields>,
    raw_source: Option<Arc<FileSource>>,
    applying: Option<ReadJob<Loaded>>,
    loaded: Option<Loaded>,
    view: Range<u64>,
    selection: Option<Range<u64>>,
    cursor: Option<u64>,
    frequency: Option<f32>,
    fft: usize,
    threshold_dbfs: f32,
    show_density: bool,
    error: Option<String>,
    request_after: f64,
    displays: Displays,
}

impl Viewer {
    pub fn load(path: &Path, cancel: &AtomicBool) -> Result<Self, String> {
        let recording = Recording::open_sigmf(path, None, ReadCheck::default(), cancel)?;
        let mut viewer = Self::new(path);
        viewer.install(Loaded::read(recording, cancel)?);
        Ok(viewer)
    }

    pub fn load_raw(path: &Path, cancel: &AtomicBool) -> Result<Self, String> {
        let (_, source) = ReadCheck::default().open(path, None, cancel)?;
        let mut viewer = Self::new(path);
        viewer.raw = Some(raw::Fields::for_path(path));
        viewer.raw_source = Some(Arc::new(source));
        Ok(viewer)
    }

    fn new(path: &Path) -> Self {
        Self {
            path: path.to_path_buf(),
            check: ReadCheck::default(),
            raw: None,
            raw_source: None,
            applying: None,
            loaded: None,
            view: 0..0,
            selection: None,
            cursor: None,
            frequency: None,
            fft: 512,
            threshold_dbfs: -45.0,
            show_density: true,
            error: None,
            request_after: 0.0,
            displays: Displays::default(),
        }
    }

    pub fn set_check(&mut self, check: ReadCheck) {
        self.check = check.clone();
        if let Some(loaded) = &mut self.loaded
            && let Some(recording) = Arc::get_mut(&mut loaded.recording)
        {
            recording.set_read_check(check);
        }
    }

    pub fn focus_samples(&mut self, first: u64, end: u64) {
        let count = self.count();
        let range = first.min(count)..end.min(count);
        if range.is_empty() {
            return;
        }
        let padding = ((range.end - range.start) / 12).max(1);
        self.cursor = Some(midpoint(&range));
        self.view =
            range.start.saturating_sub(padding)..range.end.saturating_add(padding).min(count);
        self.selection = Some(range);
    }

    pub fn ui(&mut self, ui: &mut Ui, close: &mut bool, palette: &AppPalette, copies: &mut Copies) {
        self.poll(ui.ctx());
        let mut shell = crate::shell::StudioShell::new(
            "I/Q recording",
            crate::icons::Icon::Waves,
            palette,
            close,
        );
        shell.render_header(ui, |_| {});
        self.raw_controls(ui);
        if let Some(error) = &self.error {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
        if self.loaded.is_none() {
            if self.applying.is_some() {
                ui.ctx().request_repaint_after(Duration::from_millis(30));
            }
            return;
        }
        self.toolbar(ui, copies);
        self.activity(ui);
        self.annotation_panel(ui);
        self.spectral_views(ui);
        self.request_displays(ui);
        if self.applying.is_some()
            || self.displays.waterfall_job.is_some()
            || self.displays.spectrum_job.is_some()
        {
            ui.ctx().request_repaint_after(Duration::from_millis(30));
        }
    }

    fn raw_controls(&mut self, ui: &mut Ui) {
        let Some(fields) = self.raw.as_mut() else {
            return;
        };
        let applying = self.applying.is_some();
        let result = if self.loaded.is_some() {
            ui.collapsing("Raw interpretation", |ui| fields.show(ui, applying))
                .body_returned
                .flatten()
        } else {
            fields.show(ui, applying)
        };
        if let Some(result) = result {
            match result {
                Ok(raw) => self.start_raw(ui.ctx(), raw),
                Err(error) => self.error = Some(error),
            }
        }
    }

    fn start_raw(&mut self, ctx: &Context, raw: RawInterpretation) {
        let path = self.path.clone();
        let check = self.check.clone();
        let expected = self.raw_source.clone();
        self.error = None;
        self.applying = Some(start_job(ctx, move |cancel| {
            let recording = Recording::open_raw(&path, raw, expected.as_deref(), check, cancel)?;
            Loaded::read(recording, cancel)
        }));
    }

    fn install(&mut self, loaded: Loaded) {
        let count = loaded.power.samples.end;
        self.view = 0..count;
        self.selection = None;
        self.frequency = None;
        self.cursor = loaded
            .power
            .bins
            .iter()
            .filter_map(|bin| Some((bin.peak_dbfs?, midpoint(&bin.samples))))
            .max_by(|(a, _), (b, _)| a.total_cmp(b))
            .map(|(_, sample)| sample);
        self.threshold_dbfs = (loaded.scale.start + 12.0).min(loaded.scale.end - 6.0);
        self.loaded = Some(loaded);
        self.displays = Displays::default();
        self.request_after = 0.0;
    }

    fn poll(&mut self, ctx: &Context) {
        if let Some(result) = take_result(&mut self.applying) {
            match result {
                Ok(state) => self.install(state),
                Err(error) => self.error = Some(error),
            }
        }
        if let Some(result) = take_result(&mut self.displays.waterfall_job) {
            match result {
                Ok(image) => self.install_waterfall(ctx, image),
                Err(error) => self.error = Some(error),
            }
        }
        if let Some(result) = take_result(&mut self.displays.spectrum_job) {
            match result {
                Ok(frame) => self.displays.spectrum = Some(frame),
                Err(error) => self.error = Some(error),
            }
        }
    }

    fn install_waterfall(&mut self, ctx: &Context, frame: waterfall::Image) {
        let Some(key) = self.displays.waterfall_key.clone() else {
            return;
        };
        let previous = self.displays.waterfall.take();
        let texture = if let Some(mut previous) = previous {
            previous.texture.set(frame.pixels, TextureOptions::NEAREST);
            previous.texture
        } else {
            ctx.load_texture("saved-iq-waterfall", frame.pixels, TextureOptions::NEAREST)
        };
        self.displays.waterfall = Some(WaterfallFrame {
            key,
            data: frame.data,
            texture,
            color_span: (frame.low_dbfs, frame.high_dbfs),
        });
    }

    fn request_displays(&mut self, ui: &Ui) {
        if self.applying.is_some() || self.count() < 2 {
            return;
        }
        let now = ui.input(|input| input.time);
        if self.request_after > now {
            ui.ctx()
                .request_repaint_after(Duration::from_secs_f64(self.request_after - now));
            return;
        }
        if self.view.end - self.view.start < 2 {
            return;
        }
        self.request_waterfall(ui.ctx());
        self.request_spectrum(ui.ctx());
    }

    fn request_waterfall(&mut self, ctx: &Context) {
        let key = WaterfallKey {
            span: self.view.clone(),
            fft: self.fft,
        };
        if self.displays.waterfall_key.as_ref() == Some(&key) {
            return;
        }
        let Some(recording) = self.recording() else {
            return;
        };
        let read = key.clone();
        self.displays.waterfall_key = Some(key);
        self.displays.waterfall_job = Some(start_job(ctx, move |cancel| {
            recording
                .checked(cancel, |capture| {
                    capture.waterfall(read.span, read.fft, WATERFALL_ROWS, cancel)
                })
                .map(waterfall::Image::prepare)
        }));
    }

    fn request_spectrum(&mut self, ctx: &Context) {
        let range = self.spectrum_range();
        if self.displays.spectrum_range.as_ref() == Some(&range) {
            return;
        }
        let Some(recording) = self.recording() else {
            return;
        };
        self.displays.spectrum_range = Some(range.clone());
        self.displays.spectrum_job = Some(start_job(ctx, move |cancel| {
            recording.checked(cancel, |capture| capture.spectrum(range, cancel))
        }));
    }

    fn spectrum_range(&self) -> Range<u64> {
        let count = self.count();
        let range = self
            .selection
            .as_ref()
            .filter(|selection| selection.end - selection.start >= 2)
            .unwrap_or(&self.view);
        let width = (range.end - range.start).min(self.fft as u64).max(2);
        let center = self
            .selection
            .as_ref()
            .map_or_else(|| self.cursor.unwrap_or_else(|| midpoint(range)), midpoint);
        let left = center
            .saturating_sub(width / 2)
            .max(range.start)
            .min(range.end.saturating_sub(width));
        left..left.saturating_add(width).min(count)
    }

    fn copy(&self, ui: &Ui, copies: &mut Copies) {
        let (Some(recording), Some(range)) = (self.recording(), self.selection.clone()) else {
            return;
        };
        copies.start(ui.ctx(), move |cancel| export(&recording, range, cancel));
    }

    fn count(&self) -> u64 {
        self.loaded
            .as_ref()
            .map_or(0, |loaded| loaded.power.samples.end)
    }

    fn rate(&self) -> Option<f64> {
        self.loaded
            .as_ref()?
            .recording
            .capture()
            .metadata()
            .sample_rate_hz
    }

    fn recording(&self) -> Option<Arc<Recording>> {
        self.loaded
            .as_ref()
            .map(|loaded| Arc::clone(&loaded.recording))
    }

    fn delay(&mut self, ui: &Ui) {
        self.request_after = ui.input(|input| input.time) + 0.055;
    }
}

fn start_job<T: Send + 'static>(
    ctx: &Context,
    run: impl FnOnce(&AtomicBool) -> Result<T, String> + Send + 'static,
) -> ReadJob<T> {
    let ctx = ctx.clone();
    Task::start(&JOBS, Duration::from_secs(12), move |cancel| {
        let result = run(cancel);
        ctx.request_repaint();
        result
    })
    .with_response_deadline(Duration::from_secs(15))
}

fn take_result<T: Send + 'static>(job: &mut Option<ReadJob<T>>) -> Option<Result<T, String>> {
    let result = job.as_ref()?.poll()?;
    *job = None;
    Some(
        result
            .map_err(|error| error.to_string())
            .and_then(|result| result),
    )
}

fn midpoint(range: &Range<u64>) -> u64 {
    range.start + (range.end - range.start) / 2
}

fn export(recording: &Recording, range: Range<u64>, cancel: &AtomicBool) -> Result<String, String> {
    let count = range.end - range.start;
    if count > MAX_EXPORT_SAMPLES {
        return Err("Select up to 262,144 sample pairs to copy.".into());
    }
    let samples = recording.checked(cancel, |capture| {
        capture.read_samples(range.clone(), cancel)
    })?;
    preview_copy::copy_file(recording.source().path(), "iq", "csv", |output| {
        let mut output = std::io::BufWriter::new(output);
        writeln!(output, "Sample,I,Q").map_err(|error| error.to_string())?;
        for (index, sample) in samples.iter().enumerate() {
            writeln!(
                output,
                "{},{},{}",
                range.start + index as u64,
                sample.re,
                sample.im
            )
            .map_err(|error| error.to_string())?;
        }
        output.flush().map_err(|error| error.to_string())
    })?;
    Ok(format!("Copied {count} I/Q sample pairs"))
}
