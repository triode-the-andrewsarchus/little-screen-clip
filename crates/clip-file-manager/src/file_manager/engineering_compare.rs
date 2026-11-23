//! Read-only comparison of guarded file snapshots and captured waveform summaries.
mod facts;
use super::{preview_job::Task, scope_view, shelf::preview};
use crate::file_source::{FileSource, ReadCheck};
use clip_studios::binary_diff_view::BinaryDiffViewState;
use egui::{FontId, Rect, Sense, Ui, pos2, vec2};
use egui_extras::{Column, TableBuilder};
use facts::Facts;
use little_clip_core::{
    BackgroundJobs,
    engineering::waveforms::{Peak, display_value, time_span},
    file_format::{Format, recognize_engineering},
    palette::AppPalette,
};
use std::{
    collections::BTreeSet,
    fs::File,
    io::{Read, Seek, SeekFrom},
    ops::Range,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

pub(super) const MAX_SOURCES: usize = 8;
const WINDOW: u64 = 64 * 1024;
const FIND_BYTES: u64 = 32 * 1024 * 1024;

struct Input {
    title: String,
    detail: String,
    size: u64,
    file: FileSource,
    waveform: Option<Arc<scope_view::strip::Preview>>,
    facts: Facts,
}

pub(super) struct Viewer {
    sources: Arc<Vec<Input>>,
    other: usize,
    offset: u64,
    scanned_to: u64,
    note: Option<String>,
    task: Option<Task<Result<Action, String>>>,
    diff: Option<BinaryDiffViewState>,
    waveform: Range<f64>,
    waveform_end: f64,
    channel: usize,
    show_fields: bool,
    show_matches: bool,
    keys: Vec<String>,
}

enum Action {
    Bytes {
        offset: u64,
        pair: Box<[Vec<u8>; 2]>,
    },
    Found(Option<u64>, u64),
}

impl Viewer {
    pub fn load(paths: Vec<PathBuf>, cancel: &AtomicBool) -> Result<Self, String> {
        if !(2..=MAX_SOURCES).contains(&paths.len()) {
            return Err(format!(
                "Select between two and {MAX_SOURCES} ordinary files to compare."
            ));
        }
        let sources = paths
            .into_iter()
            .map(|path| Input::load(path, cancel))
            .collect::<Result<Vec<_>, String>>()?;
        let waveform_end = sources
            .iter()
            .filter_map(|input| input.waveform.as_ref())
            .map(|wave| {
                let span = time_span(&wave.overview().channels);
                span.end - span.start
            })
            .fold(0.0_f64, f64::max);
        let keys = diff_keys(&sources[0].facts, &sources[1].facts, false);
        Ok(Self {
            sources: Arc::new(sources),
            other: 1,
            offset: 0,
            scanned_to: 0,
            note: None,
            task: None,
            diff: None,
            waveform: 0.0..waveform_end,
            waveform_end,
            channel: 0,
            show_fields: false,
            show_matches: false,
            keys,
        })
    }

    pub fn ui(&mut self, ui: &mut Ui, close: &mut bool, palette: &AppPalette) {
        self.poll(ui);
        let mut shell = crate::shell::StudioShell::new(
            "Compare files",
            crate::icons::Icon::BinaryDiff,
            palette,
            close,
        );
        shell.render_header(ui, |_| {});
        if let Some(diff) = &mut self.diff {
            let mut back = ui.button("Back to comparison").clicked();
            if !back {
                let ctx = ui.ctx().clone();
                diff.tick(&ctx);
                diff.render_ui(ui, &ctx, palette, &mut back);
            }
            if back {
                self.diff = None;
            }
            return;
        }
        self.toolbar(ui);
        if let Some(note) = &self.note {
            ui.label(note);
        }
        ui.separator();
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.show_fields, false, "Sources");
            ui.selectable_value(&mut self.show_fields, true, "Decoded fields");
            if self.show_fields
                && ui
                    .checkbox(&mut self.show_matches, "Show matching fields")
                    .changed()
            {
                self.refresh_fields();
            }
        });
        if self.show_fields {
            self.fields(ui);
        } else {
            self.summaries(ui);
        }
    }

    fn toolbar(&mut self, ui: &mut Ui) {
        ui.horizontal_wrapped(|ui| {
            ui.strong("Compare files");
            if self.sources.len() > 2 && self.task.is_none() {
                let before = self.other;
                egui::ComboBox::from_id_salt("comparison pair")
                    .selected_text(&self.sources[self.other].title)
                    .show_ui(ui, |ui| {
                        for (index, source) in self.sources.iter().enumerate().skip(1) {
                            ui.selectable_value(&mut self.other, index, &source.title);
                        }
                    });
                if self.other != before {
                    self.offset = 0;
                    self.scanned_to = 0;
                    self.note = None;
                    self.refresh_fields();
                }
            }
            let max = self.sources[0].size.max(self.sources[self.other].size);
            ui.label("Byte offset");
            if ui
                .add_enabled(
                    self.task.is_none(),
                    egui::DragValue::new(&mut self.offset).range(0..=max.saturating_sub(1)),
                )
                .changed()
            {
                self.scanned_to = self.offset;
            }
            if ui
                .add_enabled(self.task.is_none(), egui::Button::new("Compare bytes"))
                .clicked()
            {
                self.start(ui, false);
            }
            if ui
                .add_enabled(
                    self.task.is_none(),
                    egui::Button::new("Find next difference"),
                )
                .clicked()
            {
                self.start(ui, true);
            }
            if ui
                .add_enabled(
                    self.task.is_none(),
                    egui::Button::new("Next window").small(),
                )
                .clicked()
            {
                self.offset = (self.offset + WINDOW).min(max.saturating_sub(1));
                self.scanned_to = self.offset;
            }
            if self.task.is_some() {
                ui.spinner();
            }
        });
    }

    fn poll(&mut self, ui: &Ui) {
        if let Some(result) = self.task.as_ref().and_then(Task::poll) {
            self.task = None;
            match result
                .map_err(|err| err.to_string())
                .and_then(|value| value)
            {
                Ok(Action::Bytes { offset, pair }) => {
                    let labels = [0, self.other]
                        .map(|index| format!("{} · from 0x{offset:X}", self.sources[index].title));
                    let [a, b] = *pair;
                    let [left, right] = labels;
                    self.diff = Some(BinaryDiffViewState::new(left, a, right, b));
                }
                Ok(Action::Found(Some(offset), _)) => {
                    self.offset = offset;
                    self.scanned_to = offset.saturating_add(1);
                    self.note = Some(format!("Difference at byte {offset} (0x{offset:X})"));
                }
                Ok(Action::Found(None, end)) => {
                    let start = self.scanned_to.max(self.offset);
                    self.scanned_to = end;
                    self.note = Some(format!("Bytes {start} through {end} match · end exclusive"));
                }
                Err(error) => self.note = Some(error),
            }
        }
        if self.task.is_some() {
            ui.ctx().request_repaint_after(Duration::from_millis(33));
        }
    }

    fn start(&mut self, ui: &Ui, find: bool) {
        let sources = self.sources.clone();
        let other = self.other;
        let start = if find {
            self.scanned_to.max(self.offset)
        } else {
            self.offset
        };
        let repaint = ui.ctx().clone();
        self.task = Some(Task::start(
            BackgroundJobs::global(),
            Duration::from_secs(45),
            move |cancel| {
                let result = compare(&sources[0], &sources[other], start, find, cancel);
                repaint.request_repaint();
                result
            },
        ));
    }

    fn refresh_fields(&mut self) {
        self.keys = diff_keys(
            &self.sources[0].facts,
            &self.sources[self.other].facts,
            self.show_matches,
        );
    }

    fn fields(&self, ui: &mut Ui) {
        if self.keys.is_empty() {
            ui.weak("No differences in the decoded fields shown here.");
            return;
        }
        let a = &self.sources[0];
        let b = &self.sources[self.other];
        let row_height = ui.text_style_height(&egui::TextStyle::Body) + 4.0;
        let width = ui.available_width();
        TableBuilder::new(ui)
            .id_salt("compared fields")
            .striped(true)
            .auto_shrink([false, false])
            .columns(Column::exact(width * 0.32).clip(true), 2)
            .column(Column::remainder().clip(true))
            .header(row_height, |mut row| {
                for title in ["Field", &a.title, &b.title] {
                    row.col(|ui| {
                        ui.add(egui::Label::new(egui::RichText::new(title).strong()).truncate());
                    });
                }
            })
            .body(|body| {
                body.rows(row_height, self.keys.len(), |mut row| {
                    let key = &self.keys[row.index()];
                    let left = a.facts.0.get(key);
                    let right = b.facts.0.get(key);
                    row.col(|ui| {
                        ui.add(egui::Label::new(key).truncate()).on_hover_text(key);
                    });
                    for value in [left, right] {
                        let text = value.map_or("—", String::as_str);
                        row.col(|ui| {
                            ui.add(egui::Label::new(text).truncate())
                                .on_hover_text(text);
                        });
                    }
                });
            });
    }

    fn summaries(&mut self, ui: &mut Ui) {
        let channels = self
            .sources
            .iter()
            .filter_map(|source| source.waveform.as_ref())
            .map(|wave| wave.overview().channels.len())
            .max()
            .unwrap_or(0);
        if channels > 0 {
            ui.horizontal(|ui| {
                ui.label("Channel");
                ui.add(
                    egui::DragValue::new(&mut self.channel)
                        .range(0..=channels - 1)
                        .custom_formatter(|n, _| format!("{}", n as usize + 1)),
                );
                if ui.button("Fit waveforms").clicked() {
                    self.waveform = 0.0..self.waveform_end;
                }
                ui.weak("Shared time from the start of each recording");
            });
        }
        let mut zoom = None;
        egui::ScrollArea::vertical()
            .id_salt("compared files")
            .show(ui, |ui| {
                let mut previous = None;
                for (index, source) in self.sources.iter().enumerate() {
                    if previous.is_some() {
                        ui.separator();
                    }
                    previous = Some(index);
                    ui.horizontal_wrapped(|ui| {
                        ui.strong(&source.title);
                        ui.weak(format!("{} bytes", source.size));
                        ui.label(&source.detail);
                    });
                    if let Some(wave) = &source.waveform {
                        let overview = wave.overview();
                        if let Some(channel) = overview.channels.get(self.channel) {
                            let (area, response) = ui.allocate_exact_size(
                                vec2(ui.available_width(), 92.0),
                                Sense::hover(),
                            );
                            let plot = Rect::from_min_max(
                                pos2(area.left() + 48.0, area.top() + 4.0),
                                pos2(area.right() - 4.0, area.bottom() - 16.0),
                            );
                            let painter = ui.painter_at(area);
                            painter.rect_filled(plot, 3.0, ui.visuals().extreme_bg_color);
                            let start = time_span(&overview.channels).start;
                            let span = start + self.waveform.start..start + self.waveform.end;
                            let bounds = self.common_scale(&channel.unit);
                            let envelope = &overview.envelopes[self.channel];
                            scope_view::plot::draw_envelope(
                                &painter.with_clip_rect(plot),
                                plot,
                                &span,
                                channel,
                                envelope,
                                bounds,
                                scope_view::plot::color(index),
                            );
                            scope_view::plot::axes(
                                &painter,
                                plot,
                                &self.waveform,
                                ui.visuals().weak_text_color(),
                            );
                            for (point, value) in [
                                (plot.left_top(), bounds.max),
                                (plot.left_bottom(), bounds.min),
                            ] {
                                painter.text(
                                    point - vec2(3.0, 0.0),
                                    egui::Align2::RIGHT_CENTER,
                                    display_value(f64::from(value), &channel.unit),
                                    FontId::monospace(9.0),
                                    ui.visuals().text_color(),
                                );
                            }
                            let scroll = if response.hovered() {
                                ui.input_mut(|input| {
                                    std::mem::take(&mut input.smooth_scroll_delta.y)
                                })
                            } else {
                                0.0
                            };
                            if scroll != 0.0
                                && let Some(point) = response.hover_pos()
                            {
                                let at = scope_view::plot::at(plot, &self.waveform, point.x);
                                let width = ((self.waveform.end - self.waveform.start)
                                    * f64::from((-0.01 * scroll).exp()))
                                .clamp(
                                    channel.interval_seconds.min(self.waveform_end),
                                    self.waveform_end,
                                );
                                let relative = ((point.x - plot.left()) / plot.width().max(1.0))
                                    .clamp(0.0, 1.0);
                                let start = (at - width * f64::from(relative))
                                    .clamp(0.0, self.waveform_end - width);
                                zoom = Some(start..start + width);
                            }
                        }
                    }
                }
            });
        if let Some(span) = zoom {
            self.waveform = span;
        }
    }

    fn common_scale(&self, unit: &str) -> Peak {
        let mut bounds = Peak::default();
        for source in &*self.sources {
            if let Some(wave) = &source.waveform {
                let overview = wave.overview();
                if overview
                    .channels
                    .get(self.channel)
                    .is_some_and(|channel| channel.unit == unit)
                {
                    bounds.merge(overview.envelopes[self.channel].extrema());
                }
            }
        }
        bounds
    }
}

impl Input {
    fn load(path: PathBuf, cancel: &AtomicBool) -> Result<Self, String> {
        let (handle, file) = ReadCheck::default().open(&path, None, cancel)?;
        let size = handle.metadata().map_err(error)?.len();
        let format = recognize_engineering(&path, cancel)
            .or_else(|| Format::for_path(&path))
            .filter(|format| format.is_engineering());
        let data = format.map(|format| preview::load_engineering(format, &path, cancel));
        let mut facts = data
            .as_ref()
            .map(|data| Facts::prepare(data, &path, cancel))
            .unwrap_or_default();
        facts
            .0
            .insert("File size (bytes)".to_owned(), size.to_string());
        let (detail, waveform) =
            data.map_or_else(|| (String::new(), None), |data| (data.detail, data.scope));
        file.check_file(&handle)?;
        file.check()?;
        Ok(Self {
            title: super::display_path(&path),
            detail,
            size,
            file,
            waveform,
            facts,
        })
    }

    fn open_at(&self, offset: u64, cancel: &AtomicBool) -> Result<File, String> {
        let (mut handle, _) =
            ReadCheck::default().open(self.file.path(), Some(&self.file), cancel)?;
        handle
            .seek(SeekFrom::Start(offset.min(self.size)))
            .map_err(error)?;
        Ok(handle)
    }

    fn check(&self, handle: &File) -> Result<(), String> {
        self.file.check_file(handle)?;
        self.file.check()
    }
}

fn compare(
    left: &Input,
    right: &Input,
    offset: u64,
    find: bool,
    cancel: &AtomicBool,
) -> Result<Action, String> {
    let mut a = left.open_at(offset, cancel)?;
    let mut b = right.open_at(offset, cancel)?;
    let result = if find {
        let limit = left
            .size
            .max(right.size)
            .saturating_sub(offset)
            .min(FIND_BYTES);
        find_difference(&mut a, &mut b, offset, limit, cancel)?
    } else {
        let read = |file: &mut File| {
            let mut bytes = Vec::new();
            file.take(WINDOW).read_to_end(&mut bytes).map_err(error)?;
            Ok::<_, String>(bytes)
        };
        Action::Bytes {
            offset,
            pair: Box::new([read(&mut a)?, read(&mut b)?]),
        }
    };
    left.check(&a)?;
    right.check(&b)?;
    Ok(result)
}

fn find_difference(
    a: &mut File,
    b: &mut File,
    offset: u64,
    limit: u64,
    cancel: &AtomicBool,
) -> Result<Action, String> {
    let mut a_bytes = Vec::with_capacity(WINDOW as usize);
    let mut b_bytes = Vec::with_capacity(WINDOW as usize);
    let mut scanned = 0;
    while scanned < limit {
        if cancel.load(Ordering::Relaxed) {
            return Err("Comparison cancelled".into());
        }
        a_bytes.clear();
        b_bytes.clear();
        let count = (limit - scanned).min(WINDOW);
        a.take(count).read_to_end(&mut a_bytes).map_err(error)?;
        b.take(count).read_to_end(&mut b_bytes).map_err(error)?;
        if a_bytes != b_bytes {
            let index = a_bytes
                .iter()
                .zip(&b_bytes)
                .position(|(a, b)| a != b)
                .unwrap_or(a_bytes.len().min(b_bytes.len()));
            return Ok(Action::Found(
                Some(offset + scanned + index as u64),
                offset + scanned + count,
            ));
        }
        scanned += count;
    }
    Ok(Action::Found(None, offset + scanned))
}
fn error(err: impl std::fmt::Display) -> String {
    err.to_string()
}

fn diff_keys(a: &Facts, b: &Facts, same: bool) -> Vec<String> {
    a.0.keys()
        .chain(b.0.keys())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter(|key| same || a.0.get(*key) != b.0.get(*key))
        .cloned()
        .collect()
}
