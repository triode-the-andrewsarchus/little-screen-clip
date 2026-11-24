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
use little_clip_core::{BackgroundJobs, CommandRequest};
use std::{
    ffi::OsString,
    os::unix::ffi::OsStringExt,
    path::{Path, PathBuf},
    sync::{Arc, LazyLock, atomic::AtomicBool},
    time::Duration,
};

static PICKERS: LazyLock<BackgroundJobs> = LazyLock::new(|| BackgroundJobs::new(1, 1));
const PICKER_TIMEOUT: Duration = Duration::from_secs(600);

const FIELDS: Columns<4> = Columns {
    id: "CAN signal fields",
    headings: ["Signal", "Value", "Raw", "Bits"],
    fractions: [0.37, 0.28, 0.17, 0.18],
    min_height: 46.0,
    selected_fill: 0.52,
    hover_fill: 0.21,
};

#[derive(Debug, PartialEq, Eq)]
pub(super) struct FieldSelection {
    generation: u64,
    selected: Option<(Identifier, usize)>,
}

#[derive(Default)]
pub(super) struct Panel {
    editing: bool,
    autofocus: bool,
    path: String,
    database: Option<(PathBuf, Arc<Database>)>,
    loading: Option<Job<(PathBuf, Database)>>,
    picker: Option<Job<Option<PathBuf>>>,
    nearby_check: Option<Job<Option<PathBuf>>>,
    nearby_checked: bool,
    nearby: Option<PathBuf>,
    error: Option<String>,
    selected: Option<(Identifier, usize)>,
    reveal_field: Option<(Identifier, usize)>,
    generation: u64,
    plot: signal_plot::Plot,
}

impl Panel {
    pub(super) fn pending(&self) -> bool {
        self.loading.is_some() || self.nearby_check.is_some() || self.plot.pending()
    }

    pub(super) fn plot_open(&self) -> bool {
        self.plot.active()
    }

    pub(super) fn attached(&self) -> bool {
        self.database.is_some()
    }

    pub(super) fn toolbar(&mut self, ui: &mut Ui, capture: &Arc<Capture>) {
        if !self.nearby_checked {
            let capture = capture.clone();
            self.nearby_check = Some(start(
                &JOBS,
                ui.ctx(),
                Duration::from_secs(3),
                move |cancel| Ok(capture.nearby_database(cancel)),
            ));
            self.nearby_checked = true;
        }
        let ready = self.loading.is_none() && self.picker.is_none();
        let label = if let Some((path, database)) = self.database.as_ref() {
            let name = path
                .file_name()
                .unwrap_or(path.as_os_str())
                .to_string_lossy();
            ui.label(name).on_hover_text(format!(
                "{} messages · {}",
                database.message_count(),
                path.display()
            ));
            "Change DBC"
        } else {
            "Attach DBC"
        };
        ui.add_enabled_ui(ready, |ui| {
            egui::menu::MenuButton::new(label)
                .config(
                    egui::menu::MenuConfig::new()
                        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside),
                )
                .ui(ui, |ui| self.attachment_menu(ui, capture));
        });
        if self.loading.is_some() {
            ui.weak("Reading DBC…");
        }
        if let Some(error) = self.error.as_ref().filter(|_| !self.editing) {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
    }

    fn attachment_menu(&mut self, ui: &mut Ui, capture: &Arc<Capture>) {
        if let Some(path) = self.nearby.as_ref()
            && ui.button(format!("Use {}", path.display())).clicked()
        {
            self.begin_attach(ui.ctx(), capture, path.clone());
            ui.close();
        }
        let available = clip_studios::is_zenity_available();
        if ui
            .add_enabled(available, egui::Button::new("Browse DBC…"))
            .on_disabled_hover_text("The file chooser needs Zenity. You can enter a DBC path.")
            .clicked()
        {
            let initial = capture.directory().join("");
            self.editing = false;
            self.picker = Some(start(&PICKERS, ui.ctx(), PICKER_TIMEOUT, move |cancel| {
                pick_database(&initial, cancel)
            }));
            ui.ctx().request_repaint_after(Duration::from_millis(250));
            ui.close();
        }
        if !self.editing && ui.button("Enter DBC path").clicked() {
            self.editing = true;
            self.autofocus = true;
        }
        self.attachment(ui, capture);
        if self.attached() && ui.button("Detach DBC").clicked() {
            self.database = None;
            self.plot = signal_plot::Plot::default();
            self.error = None;
            self.editing = false;
            self.selected = None;
            self.reveal_field = None;
            self.generation += 1;
            ui.close();
        }
    }

    fn attachment(&mut self, ui: &mut Ui, capture: &Arc<Capture>) {
        if !self.editing {
            return;
        }
        ui.horizontal(|ui| {
            ui.label("DBC path");
            let field = ui.add(
                egui::TextEdit::singleline(&mut self.path)
                    .id_salt("attach-can-database")
                    .hint_text("Path to a .dbc file")
                    .desired_width(250.0),
            );
            field
                .clone()
                .on_hover_text("Relative paths use the capture's folder.");
            if std::mem::take(&mut self.autofocus) {
                field.request_focus();
            }
            let submitted =
                field.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
            let load = ui
                .add_enabled(!self.path.trim().is_empty(), egui::Button::new("Attach"))
                .clicked()
                || submitted;
            if load {
                match little_clip_core::path_address::decode_path(self.path.trim()) {
                    Ok(path) if !path.as_os_str().is_empty() => {
                        self.begin_attach(ui.ctx(), capture, path);
                        ui.close();
                    }
                    Ok(_) => self.error = Some("Enter a DBC path.".into()),
                    Err(error) => self.error = Some(error),
                }
            }
            if ui.button("Cancel").clicked() {
                self.editing = false;
                self.error = None;
            }
        });
        if self.editing
            && let Some(error) = &self.error
        {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
    }

    fn begin_attach(&mut self, ctx: &Context, capture: &Arc<Capture>, path: PathBuf) {
        let capture = capture.clone();
        self.loading = Some(start(&JOBS, ctx, Duration::from_secs(15), move |cancel| {
            capture.database(&path, cancel)
        }));
        self.error = None;
        self.editing = false;
    }

    pub(super) fn poll(&mut self, ctx: &Context, capture: &Arc<Capture>) {
        if let Some(result) = poll(&mut self.nearby_check) {
            self.nearby = result.ok().flatten();
        }
        if let Some(result) = poll(&mut self.picker) {
            match result {
                Ok(Some(path)) => self.begin_attach(ctx, capture, path),
                Ok(None) => {}
                Err(error) => self.error = Some(error),
            }
        } else if self.picker.is_some() {
            ctx.request_repaint_after(Duration::from_millis(250));
        }
        if let Some(result) = poll(&mut self.loading) {
            match result {
                Ok((path, database)) => {
                    self.database = Some((path, Arc::new(database)));
                    self.plot = signal_plot::Plot::default();
                    self.selected = None;
                    self.reveal_field = None;
                    self.generation += 1;
                }
                Err(error) => self.error = Some(error),
            }
        }
    }

    pub(super) fn field_selection(&self) -> FieldSelection {
        FieldSelection {
            generation: self.generation,
            selected: self.selected,
        }
    }

    pub(super) fn reveal_plot_field(&mut self, signal: &signal_plot::Signal) {
        self.selected = Some((signal.id, signal.field));
        self.reveal_field = self.selected;
    }

    pub(super) fn restore_field_selection(&mut self, selection: FieldSelection) {
        if self.generation == selection.generation {
            self.selected = selection.selected;
            self.reveal_field = self.selected;
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
    ) -> Option<signal_plot::Hit> {
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
        plot_action: bool,
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
            self.fields(ui, frame, height - 90.0, colors, plot_action);
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
                self.fields(ui, frame, height - 24.0, colors, plot_action);
            });
        });
    }

    fn field(&self, frame: &Frame) -> Option<&Field> {
        let (id, field) = self.selected.filter(|(id, _)| Some(*id) == frame.id)?;
        let message = self.database.as_ref()?.1.message(id)?;
        message.decode(field, frame)?;
        message.signals.get(field)
    }

    fn fields(
        &mut self,
        ui: &mut Ui,
        frame: &Frame,
        height: f32,
        colors: WorkbenchColors,
        plot_action: bool,
    ) {
        let Some(id) = frame.id else { return };
        let Some(message) = self.database.as_ref().and_then(|entry| entry.1.message(id)) else {
            return;
        };
        let selected = self
            .selected
            .filter(|(active, _)| *active == id)
            .map(|(_, field)| field as u64);
        ui.horizontal_wrapped(|ui| {
            ui.strong(&message.name);
            if plot_action
                && let Some(index) = selected
                && let Some(field) = message.signals.get(index as usize)
                && !field.bits().is_empty()
            {
                let signal = signal_plot::Signal {
                    channel: frame.channel.clone(),
                    id,
                    field: index as usize,
                };
                self.plot.field_actions(ui, signal);
            }
        });
        let page = Page {
            count: message.signals.len(),
            offset: 0,
            selected,
            selection: None,
            scroll_to: self
                .reveal_field
                .take_if(|(message, _)| *message == id)
                .map(|(_, field)| field),
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

fn pick_database(initial: &Path, cancel: &AtomicBool) -> Result<Option<PathBuf>, String> {
    let mut filename = OsString::from("--filename=");
    filename.push(initial);
    let output = CommandRequest::new("zenity")
        .args([
            "--file-selection",
            "--title=Attach CAN database",
            "--file-filter=CAN database | *.dbc *.DBC",
            "--file-filter=All files | *",
        ])
        .arg(filename)
        .timeout(PICKER_TIMEOUT)
        .max_output_bytes(16 * 1024)
        .die_with_parent()
        .run_cancellable(cancel)
        .map_err(|error| error.to_string())?;
    if output.status.code() == Some(1) {
        return Ok(None);
    }
    if !output.status.success() || output.stdout_truncated {
        return Err("The DBC file chooser did not return a complete path.".into());
    }
    let mut path = output.stdout;
    if path.last() == Some(&b'\n') {
        path.pop();
    }
    Ok((!path.is_empty()).then(|| PathBuf::from(OsString::from_vec(path))))
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
