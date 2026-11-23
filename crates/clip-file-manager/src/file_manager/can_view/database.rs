use super::{Capture, JOBS, signal_plot, table};
use crate::{
    file_manager::{
        capture_browse::{
            Job, poll, start,
            table::{Columns, Page},
        },
        preview_copy::Copies,
    },
    ui_appearance::WorkbenchColors,
};
use egui::{Context, Ui};
use little_clip_core::engineering::can::{
    Frame, Identifier, Index,
    dbc::{Database, Field, RawValue},
};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

const FIELDS: Columns<4> = Columns {
    id: "CAN signal fields",
    headings: ["Signal", "Value", "Raw", "Bits"],
    fractions: [0.37, 0.28, 0.17, 0.18],
    min_height: 46.0,
    selected_fill: 0.52,
    hover_fill: 0.21,
};

#[derive(Default)]
pub(super) struct Panel {
    dialog: bool,
    autofocus: bool,
    path: String,
    database: Option<(PathBuf, Arc<Database>)>,
    loading: Option<Job<(PathBuf, Database)>>,
    error: Option<String>,
    selected: Option<(Identifier, usize)>,
    generation: u64,
    plot: signal_plot::Plot,
}

impl Panel {
    pub(super) fn pending(&self) -> bool {
        self.loading.is_some() || self.plot.pending()
    }

    pub(super) fn plot_open(&self) -> bool {
        self.plot.signal.is_some()
    }

    pub(super) fn attached(&self) -> bool {
        self.database.is_some()
    }

    pub(super) fn toolbar(&mut self, ui: &mut Ui) {
        if ui
            .button(if self.attached() {
                "Change DBC"
            } else {
                "Load DBC"
            })
            .clicked()
        {
            self.dialog = true;
            self.autofocus = true;
        }
        if let Some((path, database)) = self.database.as_ref() {
            let name = path
                .file_name()
                .unwrap_or(path.as_os_str())
                .to_string_lossy();
            ui.label(name).on_hover_text(format!(
                "{} messages · {}",
                database.message_count(),
                path.display()
            ));
            if ui.small_button("Detach DBC").clicked() {
                self.database = None;
                self.plot = signal_plot::Plot::default();
                self.loading = None;
                self.error = None;
                self.selected = None;
                self.generation += 1;
            }
        }
        if self.loading.is_some() {
            ui.weak("Reading DBC…");
        }
        if let Some(error) = &self.error {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
    }

    pub(super) fn dialog(&mut self, ctx: &Context, capture: &Arc<Capture>) {
        if !self.dialog {
            return;
        }
        let dialog = egui::Modal::new(egui::Id::unique("attach-can-database")).show(ctx, |ui| {
            ui.heading("Load a CAN database");
            let field = ui.add(
                egui::TextEdit::singleline(&mut self.path)
                    .hint_text("Path to a .dbc file")
                    .desired_width(460.0),
            );
            if std::mem::take(&mut self.autofocus) {
                field.request_focus();
            }
            ui.weak("Relative paths use the capture's folder.");
            ui.horizontal(|ui| {
                let submitted =
                    field.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
                let load = ui
                    .add_enabled(!self.path.trim().is_empty(), egui::Button::new("Load DBC"))
                    .clicked()
                    || submitted;
                if load && !self.path.trim().is_empty() {
                    let source = Path::new(self.path.trim()).to_owned();
                    let capture = capture.clone();
                    self.loading =
                        Some(start(&JOBS, ctx, Duration::from_secs(15), move |cancel| {
                            capture.database(&source, cancel)
                        }));
                    self.error = None;
                    self.dialog = false;
                }
                if ui.button("Cancel").clicked() {
                    self.dialog = false;
                }
            });
        });
        if dialog.should_close() {
            self.dialog = false;
        }
    }

    pub(super) fn poll(&mut self) {
        if let Some(result) = poll(&mut self.loading) {
            match result {
                Ok((path, database)) => {
                    self.database = Some((path, Arc::new(database)));
                    self.plot = signal_plot::Plot::default();
                    self.selected = None;
                    self.generation += 1;
                }
                Err(error) => self.error = Some(error),
            }
        }
    }

    pub(super) fn plot(
        &mut self,
        ui: &mut Ui,
        capture: &Arc<Capture>,
        index: Option<&Arc<Index>>,
        viewport: signal_plot::Viewport<'_>,
        colors: WorkbenchColors,
        copies: &mut Copies,
    ) -> Option<u64> {
        let (_, database) = self.database.as_ref()?;
        self.plot
            .show(ui, capture, database, index, viewport, colors, copies)
    }

    pub(super) fn inspect(
        &mut self,
        ui: &mut Ui,
        frame: &Frame,
        origin: i128,
        height: f32,
        colors: WorkbenchColors,
    ) {
        let message = frame
            .id
            .and_then(|id| self.database.as_ref()?.1.message(id));
        if message.is_none() {
            table::details(ui, frame, origin, height - 49.0, None);
            return;
        }
        if ui.available_width() < 690.0 {
            table::details(ui, frame, origin, 45.0, self.field(frame));
            self.fields(ui, frame, height - 90.0, colors);
            return;
        }
        let width = ui.available_width();
        ui.horizontal_top(|ui| {
            ui.allocate_ui_with_layout(
                egui::vec2(width * 0.47, height),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.set_width(width * 0.47);
                    table::details(ui, frame, origin, height - 49.0, self.field(frame));
                },
            );
            ui.separator();
            ui.vertical(|ui| {
                self.fields(ui, frame, height - 24.0, colors);
            });
        });
    }

    fn field(&self, frame: &Frame) -> Option<&Field> {
        let (id, field) = self.selected.filter(|(id, _)| Some(*id) == frame.id)?;
        self.database.as_ref()?.1.message(id)?.signals.get(field)
    }

    fn fields(&mut self, ui: &mut Ui, frame: &Frame, height: f32, colors: WorkbenchColors) {
        let Some(id) = frame.id else { return };
        let Some(message) = self.database.as_ref().and_then(|entry| entry.1.message(id)) else {
            return;
        };
        let selected = self
            .selected
            .filter(|(active, _)| *active == id)
            .map(|(_, field)| field as u64);
        ui.horizontal(|ui| {
            ui.strong(&message.name);
            if let Some(field) = selected.and_then(|index| message.signals.get(index as usize))
                && ui
                    .add_enabled(!field.bits().is_empty(), egui::Button::new("Plot signal"))
                    .on_hover_text(format!("Plot {} on {}", field.name, frame.channel))
                    .clicked()
            {
                self.plot.set(signal_plot::Signal {
                    channel: frame.channel.clone(),
                    id,
                    field: selected.unwrap_or_default() as usize,
                });
            }
        });
        let page = Page {
            count: message.signals.len(),
            offset: 0,
            selected,
            selection: None,
            scroll_to: None,
            generation: self.generation,
            height,
        };
        let result = FIELDS.show(
            ui,
            page,
            colors,
            |ordinal| {
                message
                    .signals
                    .get(ordinal as usize)
                    .map(|field| (ordinal, field))
            },
            |(ordinal, _)| ordinal,
            |ui, rect, (index, field)| {
                let (value, raw) = message.decode(index as usize, frame).map_or_else(
                    || ("—".to_owned(), "—".to_owned()),
                    |value| (physical(value.value, &field.unit), raw_value(value.raw)),
                );
                let bits = field.bits().len().to_string();
                FIELDS.cells(
                    ui,
                    rect,
                    [&field.name, &value, &raw, &bits],
                    colors.text_primary,
                );
            },
        );
        if let Some((field, _)) = result.clicked {
            self.selected = Some((id, field as usize));
            ui.ctx().request_repaint();
        }
    }
}

pub(super) fn physical(value: f64, unit: &str) -> String {
    let magnitude = value.abs();
    let value = if magnitude >= 1e8 || (magnitude > 0.0 && magnitude < 1e-6) {
        format!("{value:.5e}")
    } else {
        let digits = format!("{value:.6}");
        digits
            .trim_end_matches('0')
            .trim_end_matches('.')
            .to_owned()
    };
    if unit.is_empty() {
        value
    } else {
        format!("{value} {unit}")
    }
}

pub(super) fn raw_value(value: RawValue) -> String {
    match value {
        RawValue::Signed(value) => value.to_string(),
        RawValue::Unsigned(value) => format!("0x{value:X}"),
        RawValue::Float(value) => value.to_string(),
    }
}
