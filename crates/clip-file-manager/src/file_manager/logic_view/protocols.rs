//! Controls and bounded reads for passive bus decoding.
use super::{JOBS, Task, event_list};
use crate::file_manager::signal_view::timeline;
use egui::{Context, Ui};
use little_clip_core::engineering::{
    analysis::Clock,
    logic::{Capture, Lane, Trace},
    logic_decode::{self, Event, Kind, SpiConfig},
    logic_preview::Recording,
    waveforms::display_value,
};
use std::{
    ops::Range,
    sync::{Arc, atomic::AtomicBool},
    time::Duration,
};

pub(in crate::file_manager) const DECODE_EDGES: usize = 8192;

mod i2s;
mod samples;
pub(super) mod wiring;
use i2s::I2sInputs;
pub(in crate::file_manager) use samples::Pins;
use wiring::{Pin, Role, Wiring};

pub(in crate::file_manager) trait TraceInput: Send + Sync {
    fn validate(&self, _cancel: &AtomicBool) -> Result<(), String> {
        Ok(())
    }
    fn time_bounds(&self, index: usize) -> Option<Range<f64>>;
    fn read_edges(
        &self,
        index: usize,
        time: Range<f64>,
        limit: usize,
        cancel: &AtomicBool,
    ) -> Result<Trace, String>;
}

impl TraceInput for Capture {
    fn time_bounds(&self, index: usize) -> Option<Range<f64>> {
        self.time_bounds(index)
    }

    fn read_edges(
        &self,
        index: usize,
        time: Range<f64>,
        limit: usize,
        cancel: &AtomicBool,
    ) -> Result<Trace, String> {
        self.read_edges(index, time, limit, cancel)
    }
}

pub(in crate::file_manager) enum Source {
    Recording(Arc<Recording>),
    Materialized(Arc<dyn TraceInput>),
}

impl Source {
    pub(super) fn checked<T>(
        &self,
        cancel: &AtomicBool,
        read: impl FnOnce(&dyn TraceInput) -> Result<T, String>,
    ) -> Result<T, String> {
        match self {
            Self::Recording(recording) => recording.checked(cancel, |capture| read(capture)),
            Self::Materialized(source) => {
                source.validate(cancel)?;
                let value = read(source.as_ref())?;
                source.validate(cancel)?;
                Ok(value)
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
enum Mode {
    I2c,
    Spi,
    Uart,
    I2s,
    Pwm,
}

impl Mode {
    fn label(self) -> &'static str {
        match self {
            Self::I2c => "I²C",
            Self::Spi => "SPI",
            Self::Uart => "UART",
            Self::I2s => "I²S",
            Self::Pwm => "PWM",
        }
    }
}

#[derive(Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
struct SpiInputs {
    clock: usize,
    select: Option<usize>,
    data: [Option<usize>; 2],
    config: SpiConfig,
}

impl SpiInputs {
    fn channels(self) -> impl Iterator<Item = usize> {
        [Some(self.clock), self.select, self.data[0], self.data[1]]
            .into_iter()
            .flatten()
    }

    fn controls(&mut self, ui: &mut Ui) {
        ui.label("Mode");
        for mode in 0..4 {
            if ui
                .add(egui::Button::new(mode.to_string()).selected(self.config.mode == mode))
                .clicked()
            {
                self.config.mode = mode;
            }
        }
        ui.add(
            egui::DragValue::new(&mut self.config.bits)
                .range(1..=32)
                .suffix(" bits"),
        );
        clip_ui::buttons::toggle(ui, &mut self.config.lsb_first, "LSB first");
        if self.select.is_some() {
            clip_ui::buttons::toggle(ui, &mut self.config.active_high, "Select active high");
        }
    }

    fn validate(self) -> Result<Inputs, &'static str> {
        if self.data.iter().all(Option::is_none) {
            return Err("Choose MOSI or MISO.");
        }
        let mut seen = Vec::new();
        for channel in self.channels() {
            if seen.contains(&channel) {
                return Err("Choose different SPI channels.");
            }
            seen.push(channel);
        }
        Ok(Inputs::Spi(self))
    }
}

#[derive(Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
enum Inputs {
    I2c { clock: usize, data: usize },
    Uart { data: usize, baud: f64 },
    Spi(SpiInputs),
    I2s(I2sInputs),
    Pwm { data: usize, active_high: bool },
}

#[derive(Clone, PartialEq)]
struct Key {
    span: Range<f64>,
    inputs: Inputs,
    source_revision: u64,
}

#[derive(Clone, PartialEq, serde::Serialize, serde::Deserialize)]
struct PinRecipe {
    inputs: Inputs,
    decoded_span: Range<f64>,
    selection: Range<f64>,
}

impl PinRecipe {
    fn read(&self, recording: &Recording, cancel: &AtomicBool) -> Result<Decoded, String> {
        if self.decoded_span.is_empty() || self.selection.is_empty() {
            return Err("Invalid saved decoder pin interval".into());
        }
        let key = Key {
            inputs: self.inputs,
            span: self.decoded_span.clone(),
            source_revision: 0,
        };
        recording.checked(cancel, |capture| decode(capture, &key, cancel))
    }
}

struct Decoded {
    window: Range<f64>,
    inspected_until: f64,
    events: Vec<Event>,
}

pub(in crate::file_manager) enum EventAction {
    Select(Event),
    Inspect(Event),
}

struct Inspection {
    origin: Range<f64>,
    reveal: bool,
}

#[derive(Default)]
pub(in crate::file_manager) struct Panel {
    decoders: Vec<Decoder>,
    selected: Option<usize>,
    pins: Option<Pins>,
    // Keep the decode window available while examining individual pulses.
    inspection: Option<Inspection>,
}

#[derive(Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub(in crate::file_manager) struct SessionSettings {
    decoders: Vec<DecoderSettings>,
    selected: Option<usize>,
    pins: Option<samples::SavedPins>,
}

impl SessionSettings {
    pub(in crate::file_manager) fn analysis_edits(mut self) -> Self {
        self.selected = None;
        for decoder in &mut self.decoders {
            decoder.show_samples = false;
        }
        self
    }
}

impl Panel {
    pub(in crate::file_manager) fn session_settings(&self) -> SessionSettings {
        SessionSettings {
            decoders: self
                .decoders
                .iter()
                .map(|decoder| decoder.settings.clone())
                .collect(),
            selected: self.selected,
            pins: self.pins.as_ref().map(Pins::save),
        }
    }

    pub(in crate::file_manager) fn restore_session(
        &mut self,
        state: SessionSettings,
        recording: Option<&Recording>,
        cancel: &AtomicBool,
    ) -> Result<(), String> {
        if state.decoders.len() > 12
            || state
                .selected
                .is_some_and(|index| index >= state.decoders.len())
            || state.decoders.iter().any(|decoder| decoder.baud.len() > 64)
        {
            return Err("Invalid saved decoder settings".into());
        }
        let pins = match state.pins {
            Some(saved) => {
                let source = recording.ok_or("Decoder pins require a recorded logic capture")?;
                Some(Pins::restore(saved, source, cancel)?)
            }
            None => recording.map(|_| Pins::default()),
        };
        self.decoders = state
            .decoders
            .into_iter()
            .map(Decoder::from_settings)
            .collect();
        self.selected = state.selected;
        self.pins = pins;
        self.inspection = None;
        Ok(())
    }

    pub fn remap_channels(&mut self, lanes: &[Option<usize>]) {
        self.inspection = None;
        for decoder in &mut self.decoders {
            for pin in &mut decoder.settings.wiring() {
                pin.remap(lanes);
            }
            decoder.wiring.close();
            decoder.lane_count = None;
            decoder.task = None;
            decoder.request = None;
            decoder.result = None;
            decoder.follow = None;
            decoder.picked = None;
        }
    }

    pub(super) fn recorded() -> Self {
        Self {
            pins: Some(Pins::default()),
            ..Self::default()
        }
    }

    pub(super) fn pins(&self) -> Option<&Pins> {
        self.pins.as_ref()
    }

    fn current(&self) -> Option<&Decoder> {
        self.selected.and_then(|index| self.decoders.get(index))
    }

    fn current_mut(&mut self) -> Option<&mut Decoder> {
        self.selected.and_then(|index| self.decoders.get_mut(index))
    }

    pub fn decoder_buttons(&mut self, ui: &mut Ui, lanes: &[Lane]) -> bool {
        self.decoder_buttons_with(ui, lanes, |_| {})
    }

    pub(in crate::file_manager) fn decoder_buttons_with(
        &mut self,
        ui: &mut Ui,
        lanes: &[Lane],
        extra: impl FnOnce(&mut Ui),
    ) -> bool {
        let mut added = None;
        ui.add_enabled_ui(!lanes.is_empty(), |ui| {
            ui.menu_button("Add decoder", |ui| {
                ui.add_enabled_ui(self.decoders.len() < 12, |ui| {
                    for mode in [Mode::Spi, Mode::I2c, Mode::Uart, Mode::I2s, Mode::Pwm] {
                        if ui.button(mode.label()).clicked() {
                            added = Some(mode);
                            ui.close();
                        }
                    }
                });
                extra(ui);
            });
        });
        let Some(mode) = added else { return false };
        self.cancel_wiring();
        self.close();
        self.inspection = None;
        self.decoders.push(Decoder::new(mode, lanes));
        self.selected = Some(self.decoders.len() - 1);
        true
    }

    pub(in crate::file_manager) fn wiring_active(&self) -> bool {
        self.current()
            .is_some_and(|decoder| decoder.wiring.active())
    }

    pub(in crate::file_manager) fn wire_lane(&mut self, lane: usize) {
        if let Some(decoder) = self.current_mut() {
            decoder.wiring.choose(&mut decoder.settings.wiring(), lane);
            self.inspection = None;
        }
    }

    pub(super) fn cancel_wiring(&mut self) {
        if let Some(decoder) = self.current_mut() {
            decoder.wiring.close();
        }
    }

    pub fn edges(&self) -> bool {
        self.selected.is_none()
    }

    pub(super) fn decoding(&self) -> bool {
        self.current().is_some_and(|decoder| decoder.task.is_some())
    }

    pub fn data_lane(&self) -> Option<usize> {
        self.current().and_then(Decoder::data_lane)
    }

    pub(super) fn active_pwm(&self) -> Option<(usize, bool)> {
        match self.current()?.inputs().ok()? {
            Inputs::Pwm { data, active_high } => Some((data, active_high)),
            _ => None,
        }
    }

    pub fn channels(&self) -> Option<Vec<usize>> {
        self.current().map(Decoder::channels)
    }

    pub fn controls(&mut self, ui: &mut Ui, lanes: &[Lane]) -> bool {
        if self.decoders.is_empty() {
            return false;
        }
        let mut selected = self.selected;
        let mut remove = false;
        ui.horizontal_wrapped(|ui| {
            if ui
                .add(egui::Button::new("Raw edges").selected(selected.is_none()))
                .clicked()
            {
                selected = None;
            }
            for (index, decoder) in self.decoders.iter().enumerate() {
                let label = format!("{} {}", decoder.settings.mode.label(), index + 1);
                if ui
                    .add(egui::Button::new(label).selected(selected == Some(index)))
                    .clicked()
                {
                    selected = Some(index);
                }
            }
            if selected.is_some() {
                remove = ui.small_button("Remove decoder").clicked();
            }
        });
        let changed = selected != self.selected || remove;
        self.select_decoder(selected);
        if remove && let Some(index) = self.selected.take() {
            self.decoders.remove(index);
            self.inspection = None;
        }
        let wiring_changed = self.current_mut().is_some_and(|decoder| {
            let previous = decoder.inputs().ok();
            decoder.controls(ui, lanes);
            previous != decoder.inputs().ok()
        });
        if wiring_changed {
            self.inspection = None;
        }
        changed || wiring_changed
    }

    pub(in crate::file_manager) fn select_decoder(&mut self, selected: Option<usize>) {
        if self.selected != selected {
            self.cancel_wiring();
            self.close();
            self.inspection = None;
            self.selected = selected;
        }
    }

    pub fn inspecting(&self) -> bool {
        self.inspection.is_some()
    }

    fn inspect(&mut self, event: &Event, visible: &mut Range<f64>, domain: &Range<f64>, step: f64) {
        let inspection = self.inspection.get_or_insert_with(|| Inspection {
            origin: visible.clone(),
            reveal: false,
        });
        let origin = &inspection.origin;
        let padding = ((event.time.end - event.time.start) * 0.18)
            .max((origin.end - origin.start) * 0.002)
            .max(2.0 * step);
        *visible = timeline::fit(
            domain,
            event.time.start - padding..event.time.end + padding,
            step,
        );
        inspection.reveal = true;
    }

    pub fn follow_event(
        &mut self,
        action: EventAction,
        visible: &mut Range<f64>,
        domain: &Range<f64>,
        step: f64,
    ) -> f64 {
        let event = match &action {
            EventAction::Select(event) | EventAction::Inspect(event) => event,
        };
        let picked = self.current().and_then(|decoder| {
            let output = decoder
                .current(self.decode_window(visible))?
                .as_ref()
                .ok()?;
            output
                .events
                .iter()
                .position(|candidate| candidate == event)
        });
        if let Some(decoder) = self.current_mut() {
            decoder.picked = picked;
        }
        if matches!(action, EventAction::Inspect(_)) {
            self.inspect(event, visible, domain, step);
        }
        event.time.start
    }

    pub fn return_controls(&mut self, ui: &mut Ui, visible: &mut Range<f64>) {
        let Some(inspection) = &mut self.inspection else {
            return;
        };
        let back = ui.button("Back to decoded window");
        if std::mem::take(&mut inspection.reveal) {
            back.scroll_to_me(None);
        }
        if back.clicked() {
            self.exit_inspection(visible);
        }
    }

    pub fn exit_inspection(&mut self, visible: &mut Range<f64>) {
        if let Some(inspection) = self.inspection.take() {
            *visible = inspection.origin;
        }
    }

    fn decode_window<'a>(&'a self, visible: &'a Range<f64>) -> &'a Range<f64> {
        self.inspection
            .as_ref()
            .map_or(visible, |inspection| &inspection.origin)
    }

    pub fn update(
        &mut self,
        ctx: &Context,
        span: Range<f64>,
        refined: bool,
        recording: &Arc<Recording>,
    ) {
        let span = self.decode_window(&span).clone();
        if let Some(decoder) = self.current_mut() {
            decoder.update(
                ctx,
                span,
                u64::from(refined),
                Source::Recording(recording.clone()),
            );
        }
    }

    pub fn update_materialized(
        &mut self,
        ctx: &Context,
        span: Range<f64>,
        revision: u64,
        source: Arc<dyn TraceInput>,
    ) {
        let span = self.decode_window(&span).clone();
        if let Some(decoder) = self.current_mut() {
            decoder.update(ctx, span, revision, Source::Materialized(source));
        }
    }

    pub(in crate::file_manager) fn update_all_materialized(
        &mut self,
        ctx: &Context,
        span: &Range<f64>,
        revision: u64,
        source: Arc<dyn TraceInput>,
    ) {
        let pinned = self.decode_window(span).clone();
        for (index, decoder) in self.decoders.iter_mut().enumerate() {
            let window = if Some(index) == self.selected {
                &pinned
            } else {
                span
            };
            decoder.update(
                ctx,
                window.clone(),
                revision,
                Source::Materialized(source.clone()),
            );
        }
    }

    pub(in crate::file_manager) fn annotation_rows<'a>(
        &'a self,
        span: &'a Range<f64>,
    ) -> impl Iterator<Item = (usize, &'static str, &'a [Event])> {
        self.decoders
            .iter()
            .enumerate()
            .map(move |(index, decoder)| {
                let window = if Some(index) == self.selected {
                    self.decode_window(span)
                } else {
                    span
                };
                let events = decoder
                    .current(window)
                    .and_then(|result| result.as_ref().ok())
                    .map_or(&[][..], |result| result.events.as_slice());
                (index, decoder.settings.mode.label(), events)
            })
    }

    pub(in crate::file_manager) fn suspend_all(&mut self) {
        for decoder in &mut self.decoders {
            decoder.close();
        }
    }

    pub fn close(&mut self) {
        if let Some(decoder) = self.current_mut() {
            decoder.close();
        }
    }

    pub fn cursor_event(&self, span: &Range<f64>, cursor: Option<f64>) -> Option<&Event> {
        self.current()?
            .cursor_event(self.decode_window(span), cursor)
    }

    pub(super) fn clear_event_pick(&mut self) {
        if let Some(decoder) = self.current_mut() {
            decoder.picked = None;
        }
    }

    pub(in crate::file_manager) fn selected_index(&self) -> Option<usize> {
        self.selected
    }

    pub(in crate::file_manager) fn picked_event(&self, span: &Range<f64>) -> Option<&Event> {
        self.events(span).get(self.current()?.picked?)
    }

    pub fn table(
        &mut self,
        ui: &mut Ui,
        span: &Range<f64>,
        cursor: Option<f64>,
        selection: Option<&Range<f64>>,
        clock: Clock,
    ) -> Option<EventAction> {
        let span = self
            .inspection
            .as_ref()
            .map_or(span, |inspection| &inspection.origin);
        let decoder = self.decoders.get_mut(self.selected?)?;
        decoder.table(ui, span, cursor, self.pins.as_mut(), selection, clock)
    }

    pub fn events(&self, span: &Range<f64>) -> &[Event] {
        self.current()
            .and_then(|decoder| decoder.current(self.decode_window(span)))
            .and_then(|result| result.as_ref().ok())
            .map_or(&[], |output| output.events.as_slice())
    }
}

#[derive(Clone, PartialEq, serde::Serialize, serde::Deserialize)]
struct DecoderSettings {
    mode: Mode,
    spi: SpiInputs,
    i2s: I2sInputs,
    show_samples: bool,
    clock: usize,
    data: usize,
    uart: usize,
    pwm: usize,
    pwm_active_high: bool,
    baud: String,
}

impl DecoderSettings {
    fn wiring(&mut self) -> Vec<Pin<'_>> {
        match self.mode {
            Mode::I2c => vec![
                Pin::required(Role::Clock, &mut self.clock),
                Pin::required(Role::Data, &mut self.data),
            ],
            Mode::Uart => vec![Pin::required(Role::Data, &mut self.uart)],
            Mode::Pwm => vec![Pin::required(Role::Signal, &mut self.pwm)],
            Mode::Spi => {
                let [mosi, miso] = &mut self.spi.data;
                vec![
                    Pin::required(Role::Clock, &mut self.spi.clock),
                    Pin::optional(Role::Mosi, mosi),
                    Pin::optional(Role::Miso, miso),
                    Pin::optional(Role::ChipSelect, &mut self.spi.select),
                ]
            }
            Mode::I2s => {
                let [clock, select, data] = &mut self.i2s.channels;
                vec![
                    Pin::required(Role::Clock, clock),
                    Pin::required(Role::WordSelect, select),
                    Pin::required(Role::Data, data),
                ]
            }
        }
    }
}

struct Decoder {
    settings: DecoderSettings,
    wiring: Wiring,
    lane_count: Option<usize>,
    request: Option<Key>,
    task: Option<Task<Result<Decoded, String>>>,
    result: Option<Result<Decoded, String>>,
    follow: Option<usize>,
    picked: Option<usize>,
    next_request: f64,
}

impl Decoder {
    fn new(mode: Mode, lanes: &[Lane]) -> Self {
        let clock = named_lane(lanes, &["SCL"]).unwrap_or(0);
        let data =
            named_lane(lanes, &["SDA"]).unwrap_or(usize::from(lanes.len() > 1 && clock == 0));
        let uart = named_lane(lanes, &["UART", "TX", "RX", "TXD", "RXD"]).unwrap_or(0);
        Self::from_settings(DecoderSettings {
            mode,
            i2s: I2sInputs::new(lanes),
            show_samples: true,
            spi: SpiInputs {
                clock: named_lane(lanes, &["SCK", "CLK", "SCLK", "SPI_CLK"]).unwrap_or(0),
                select: named_lane(lanes, &["CS", "CS_N", "SS", "SS_N", "CE"]),
                data: [
                    named_lane(lanes, &["MOSI", "SDI", "DATA", "SI"]).or_else(|| {
                        if lanes.len() > 1 && named_lane(lanes, &["MISO", "SDO", "SO"]).is_none() {
                            Some(1)
                        } else {
                            None
                        }
                    }),
                    named_lane(lanes, &["MISO", "SDO", "SO"]),
                ],
                config: SpiConfig::default(),
            },
            clock,
            data,
            uart,
            pwm: named_lane(lanes, &["PWM", "DUTY"]).unwrap_or(0),
            pwm_active_high: true,
            baud: String::new(),
        })
    }

    fn from_settings(settings: DecoderSettings) -> Self {
        Self {
            settings,
            wiring: Wiring::default(),
            lane_count: None,
            request: None,
            task: None,
            result: None,
            follow: None,
            picked: None,
            next_request: 0.0,
        }
    }

    fn data_lane(&self) -> Option<usize> {
        match self.settings.mode {
            Mode::I2c => Some(self.settings.data),
            Mode::Uart => Some(self.settings.uart),
            Mode::Spi => self.settings.spi.data.into_iter().flatten().next(),
            Mode::I2s => Some(self.settings.i2s.channels[2]),
            Mode::Pwm => Some(self.settings.pwm),
        }
    }

    fn channels(&self) -> Vec<usize> {
        let mut channels = match self.settings.mode {
            Mode::I2c => vec![self.settings.clock, self.settings.data],
            Mode::Uart => vec![self.settings.uart],
            Mode::Spi => self.settings.spi.channels().collect(),
            Mode::I2s => self.settings.i2s.channels.to_vec(),
            Mode::Pwm => vec![self.settings.pwm],
        };
        channels.sort_unstable();
        channels.dedup();
        channels
    }

    pub(super) fn controls(&mut self, ui: &mut Ui, lanes: &[Lane]) {
        self.lane_count = Some(lanes.len());
        ui.horizontal_wrapped(|ui| {
            for pin in &self.settings.wiring() {
                self.wiring.button(ui, pin, lanes);
            }
            self.mode_controls(ui);
        });
        self.wiring.choices(ui, &mut self.settings.wiring(), lanes);
        if let Err(message) = self.inputs() {
            ui.colored_label(ui.visuals().error_fg_color, message);
        }
    }

    fn mode_controls(&mut self, ui: &mut Ui) {
        match self.settings.mode {
            Mode::Spi => self.settings.spi.controls(ui),
            Mode::I2s => self.settings.i2s.controls(ui),
            Mode::Pwm => {
                egui::ComboBox::from_id_salt("pwm-active-level")
                    .selected_text(if self.settings.pwm_active_high {
                        "Active high"
                    } else {
                        "Active low"
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut self.settings.pwm_active_high,
                            true,
                            "Active high",
                        );
                        ui.selectable_value(
                            &mut self.settings.pwm_active_high,
                            false,
                            "Active low",
                        );
                    });
            }
            Mode::I2c => {}
            Mode::Uart => {
                ui.label("Baud");
                ui.add(
                    egui::TextEdit::singleline(&mut self.settings.baud)
                        .id_salt("uart-baud")
                        .desired_width(80.0)
                        .char_limit(16)
                        .hint_text("115200"),
                );
                ui.weak("8N1");
            }
        }
    }

    fn inputs(&self) -> Result<Inputs, &'static str> {
        if self
            .lane_count
            .is_some_and(|count| self.channels().iter().any(|lane| *lane >= count))
        {
            return Err("Assign a channel to each unavailable input.");
        }
        match self.settings.mode {
            Mode::Spi => self.settings.spi.validate(),
            Mode::I2s => self.settings.i2s.validate(),
            Mode::Pwm => Ok(Inputs::Pwm {
                data: self.settings.pwm,
                active_high: self.settings.pwm_active_high,
            }),
            Mode::I2c if self.settings.clock != self.settings.data => Ok(Inputs::I2c {
                clock: self.settings.clock,
                data: self.settings.data,
            }),
            Mode::I2c => Err("Choose different clock and data channels."),
            Mode::Uart if self.settings.baud.trim().is_empty() => {
                Err("Enter the recorded UART baud rate.")
            }
            Mode::Uart => {
                let baud = self
                    .settings
                    .baud
                    .trim()
                    .parse::<f64>()
                    .ok()
                    .filter(|baud| baud.is_finite() && *baud > 0.0)
                    .ok_or("Enter a positive baud rate.")?;
                Ok(Inputs::Uart {
                    data: self.settings.uart,
                    baud,
                })
            }
        }
    }

    pub(super) fn close(&mut self) {
        if self.task.take().is_some() {
            self.request = None;
        }
    }

    fn update(&mut self, ctx: &Context, span: Range<f64>, source_revision: u64, source: Source) {
        let Ok(inputs) = self.inputs() else {
            self.close();
            return;
        };
        let key = Key {
            span,
            inputs,
            source_revision,
        };
        if self.request.as_ref() != Some(&key) {
            self.start(ctx, key, source);
        }
        if let Some(result) = self.task.as_ref().and_then(Task::poll) {
            self.task = None;
            self.result = Some(
                result
                    .map_err(|error| error.to_string())
                    .and_then(|result| result),
            );
            self.follow = None;
        }
        if self.task.is_some() {
            ctx.request_repaint_after(Duration::from_millis(20));
        }
    }

    fn start(&mut self, ctx: &Context, key: Key, source: Source) {
        self.task = None;
        self.request = None;
        self.result = None;
        self.picked = None;
        let now = ctx.input(|input| input.time);
        if now < self.next_request {
            ctx.request_repaint_after(Duration::from_secs_f64(self.next_request - now));
            return;
        }
        self.next_request = now + 0.075;
        self.request = Some(key.clone());
        let repaint = ctx.clone();
        self.task = Some(Task::start(&JOBS, Duration::from_secs(25), move |cancel| {
            let result = match source {
                Source::Recording(recording) => {
                    recording.checked(cancel, |capture| decode(capture, &key, cancel))
                }
                Source::Materialized(source) => decode(source.as_ref(), &key, cancel),
            };
            repaint.request_repaint();
            result
        }));
    }

    fn current(&self, span: &Range<f64>) -> Option<&Result<Decoded, String>> {
        let key = self.request.as_ref()?;
        (key.span == *span && self.inputs().ok() == Some(key.inputs)).then_some(())?;
        self.result.as_ref()
    }

    pub(super) fn cursor_event(&self, span: &Range<f64>, cursor: Option<f64>) -> Option<&Event> {
        let events = &self.current(span)?.as_ref().ok()?.events;
        self.event_index(events, cursor).map(|index| &events[index])
    }

    fn event_index(&self, events: &[Event], cursor: Option<f64>) -> Option<usize> {
        self.picked
            .filter(|&index| {
                events
                    .get(index)
                    .is_some_and(|event| cursor == Some(event.time.start))
            })
            .or_else(|| active_event(events, cursor))
    }

    pub(super) fn table(
        &mut self,
        ui: &mut Ui,
        span: &Range<f64>,
        cursor: Option<f64>,
        pins: Option<&mut Pins>,
        selection: Option<&Range<f64>>,
        clock: Clock,
    ) -> Option<EventAction> {
        self.inputs().ok()?;
        // Move the scroll-follow state out while the decoded result is borrowed.
        let mut followed = self.follow;
        let mut show_samples = self.settings.show_samples;
        let mut destination = None;
        match self.current(span) {
            Some(Ok(output)) => {
                let events = &output.events;
                let selected = self.event_index(events, cursor);
                let inspect = decoded_controls(
                    ui,
                    events,
                    self.settings.mode,
                    &mut show_samples,
                    pins,
                    selection.unwrap_or(span),
                    self.request.as_ref(),
                    selected,
                );
                coverage(ui, output, span);
                let plot = enabled_plot(self.settings.mode);
                let picked = if let Some(plot) = plot.filter(|_| show_samples && !events.is_empty())
                {
                    samples::plot(ui, span, cursor, events, plot)
                } else {
                    None
                };
                let selected = picked.or(selected);
                let clicked = event_list(
                    ui,
                    ("logic-protocol-rows", "Decoded event"),
                    &mut followed,
                    selected,
                    events.len(),
                    |index| {
                        let event = &events[index];
                        (
                            display_value(clock.to_workspace(event.time.start), "s"),
                            event_label(event.kind),
                        )
                    },
                )
                .or(picked);
                destination = clicked
                    .map(|index| EventAction::Select(events[index].clone()))
                    .or(inspect.map(EventAction::Inspect));
            }
            Some(Err(error)) => {
                ui.colored_label(ui.visuals().error_fg_color, error);
            }
            None => {
                ui.weak("Decoding the visible window…");
            }
        }
        self.follow = followed;
        self.settings.show_samples = show_samples;
        destination
    }
}

fn decoded_controls(
    ui: &mut Ui,
    events: &[Event],
    mode: Mode,
    show_samples: &mut bool,
    pins: Option<&mut Pins>,
    selection: &Range<f64>,
    request: Option<&Key>,
    selected: Option<usize>,
) -> Option<Event> {
    if events.is_empty() {
        return None;
    }
    ui.horizontal_wrapped(|ui| {
        let inspect = ui
            .add_enabled(selected.is_some(), egui::Button::new("Show pulses"))
            .clicked();
        if ui.button("Copy decoded").clicked() {
            use std::fmt::Write;
            let mut text = String::from("Start (s)\tEnd (s)\tEvent\n");
            for event in events {
                let _ = writeln!(
                    text,
                    "{}\t{}\t{}",
                    event.time.start,
                    event.time.end,
                    event_label(event.kind)
                );
            }
            ui.ctx().copy_text(text);
        }
        if let Some(plot) = enabled_plot(mode) {
            if ui.button(plot.copy_label()).clicked() {
                ui.ctx().copy_text(plot.tsv(events));
            }
            clip_ui::buttons::toggle(ui, show_samples, plot.label());
            if let Some((pins, key)) = pins.zip(request) {
                let recipe = PinRecipe {
                    inputs: key.inputs,
                    decoded_span: key.span.clone(),
                    selection: selection.clone(),
                };
                pins.controls(ui, plot, events, &recipe);
            }
        }
        inspect
            .then_some(selected)
            .flatten()
            .map(|index| events[index].clone())
    })
    .inner
}

fn enabled_plot(mode: Mode) -> Option<samples::Measure> {
    match mode {
        Mode::I2s => Some(samples::Measure::Pcm),
        Mode::Pwm => Some(samples::Measure::Duty),
        _ => None,
    }
}

fn decode(capture: &dyn TraceInput, request: &Key, cancel: &AtomicBool) -> Result<Decoded, String> {
    match request.inputs {
        Inputs::Spi(inputs) => read_spi(capture, request.span.clone(), inputs, cancel),
        Inputs::I2s(inputs) => inputs.read(capture, request.span.clone(), cancel),
        Inputs::I2c { clock, data } => {
            let window = lane_window(
                capture,
                data,
                lane_window(capture, clock, request.span.clone())?,
            )?;
            let scl = capture.read_edges(clock, window.clone(), DECODE_EDGES, cancel)?;
            let sda = capture.read_edges(data, window.clone(), DECODE_EDGES, cancel)?;
            Ok(Decoded {
                window,
                inspected_until: scl.inspected_until.min(sda.inspected_until),
                events: logic_decode::i2c(&scl, &sda)?,
            })
        }
        Inputs::Uart { data, baud } => read_single(
            capture,
            request.span.clone(),
            data,
            cancel,
            |trace, start| logic_decode::uart(trace, start, baud),
        ),
        Inputs::Pwm { data, active_high } => {
            read_single(capture, request.span.clone(), data, cancel, |trace, _| {
                logic_decode::pwm(trace, active_high)
            })
        }
    }
}

fn read_single(
    capture: &dyn TraceInput,
    span: Range<f64>,
    lane: usize,
    cancel: &AtomicBool,
    decode: impl FnOnce(&Trace, f64) -> Result<Vec<Event>, String>,
) -> Result<Decoded, String> {
    let window = lane_window(capture, lane, span)?;
    let trace = capture.read_edges(lane, window.clone(), DECODE_EDGES, cancel)?;
    let events = decode(&trace, window.start)?;
    Ok(Decoded {
        window,
        inspected_until: trace.inspected_until,
        events,
    })
}

fn read_spi(
    capture: &dyn TraceInput,
    span: Range<f64>,
    inputs: SpiInputs,
    cancel: &AtomicBool,
) -> Result<Decoded, String> {
    let window = inputs
        .channels()
        .try_fold(span, |span, lane| lane_window(capture, lane, span))?;
    let read = |lane| capture.read_edges(lane, window.clone(), DECODE_EDGES, cancel);
    let clock = read(inputs.clock)?;
    let select = inputs.select.map(read).transpose()?;
    let [mosi, miso] = inputs.data;
    let mosi = mosi.map(read).transpose()?;
    let miso = miso.map(read).transpose()?;
    let inspected_until = [select.as_ref(), mosi.as_ref(), miso.as_ref()]
        .into_iter()
        .flatten()
        .fold(clock.inspected_until, |end, trace| {
            end.min(trace.inspected_until)
        });
    let events = logic_decode::spi(
        &clock,
        select.as_ref(),
        [mosi.as_ref(), miso.as_ref()],
        inputs.config,
    )?;
    Ok(Decoded {
        window,
        inspected_until,
        events,
    })
}

fn lane_window(
    capture: &dyn TraceInput,
    index: usize,
    requested: Range<f64>,
) -> Result<Range<f64>, String> {
    let lane = capture
        .time_bounds(index)
        .ok_or("Selected channel is unavailable.")?;
    let span = requested.start.max(lane.start)..requested.end.min(lane.end);
    if span.is_empty() {
        return Err("The selected channels have no common samples in this window.".into());
    }
    Ok(span)
}

fn named_lane(lanes: &[Lane], aliases: &[&str]) -> Option<usize> {
    lanes.iter().position(|lane| {
        let label = lane.name.rsplit('.').next().unwrap_or_default();
        aliases
            .iter()
            .any(|alias| label.eq_ignore_ascii_case(alias))
    })
}

fn active_event(events: &[Event], time: Option<f64>) -> Option<usize> {
    let time = time?;
    let index = events
        .partition_point(|event| event.time.start <= time)
        .checked_sub(1)?;
    (time <= events[index].time.end).then_some(index)
}

fn coverage(ui: &mut Ui, output: &Decoded, span: &Range<f64>) {
    if output.inspected_until < output.window.end {
        ui.weak(format!(
            "Exact prefix {} to {} · Zoom for more",
            display_value(output.window.start, "s"),
            display_value(output.inspected_until, "s"),
        ));
    } else if *span != output.window {
        ui.weak(format!(
            "Channel overlap {} to {}",
            display_value(output.window.start, "s"),
            display_value(output.window.end, "s"),
        ));
    }
    if output.events.is_empty() {
        ui.weak("No complete symbols decoded in this window.");
    }
}

pub(in crate::file_manager) fn event_label(kind: Kind) -> String {
    let ack_text = |ack| if ack { "ACK" } else { "NACK" };
    match kind {
        Kind::I2cStart { repeated: false } => "START".into(),
        Kind::I2cStart { repeated: true } => "Repeated START".into(),
        Kind::I2cStop => "STOP".into(),
        Kind::I2cAddress {
            address,
            ten_bit,
            read,
            ack,
        } => {
            let width = if ten_bit { 3 } else { 2 };
            let direction = if read { "read" } else { "write" };
            format!("0x{address:0width$X}  {direction}  {}", ack_text(ack))
        }
        Kind::I2cData { byte, ack } => format!("0x{byte:02X}  {}", ack_text(ack)),
        Kind::SpiWord { mosi, miso, bits } => {
            let width = (bits as usize).div_ceil(4);
            [("MOSI", mosi), ("MISO", miso)]
                .into_iter()
                .filter_map(|(label, value)| {
                    value.map(|value| format!("{label} 0x{value:0width$X}"))
                })
                .reduce(|left, right| format!("{left}   {right}"))
                .unwrap_or_else(|| "Unknown SPI word".into())
        }
        Kind::UartByte { byte } if byte.is_ascii_graphic() || byte == b' ' => {
            format!("0x{byte:02X}  '{}'", char::from(byte))
        }
        Kind::UartByte { byte } => format!("0x{byte:02X}"),
        Kind::I2sSample {
            channel,
            sample: Some(sample),
            ..
        } => format!("{}  {sample}", channel.label()),
        Kind::I2sSample {
            channel,
            sample: None,
            ..
        } => format!("{}  Unknown sample", channel.label()),
        Kind::I2sShortWord {
            channel,
            received,
            bits,
        } => format!("{}  Short word ({received}/{bits} bits)", channel.label()),
        Kind::PwmCycle(cycle) => format!(
            "{:.2}% active · {} · {} pulse",
            cycle.duty() * 100.0,
            display_value(cycle.frequency(), "Hz"),
            display_value(cycle.active_seconds, "s"),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remap_moves_surviving_decoder_inputs_and_marks_missing_inputs() {
        let mut decoder = Decoder::new(Mode::Spi, &[]);
        decoder.settings.spi.clock = 1;
        decoder.settings.spi.select = Some(0);
        decoder.settings.spi.data = [Some(2), None];
        let mut panel = Panel {
            decoders: vec![decoder],
            selected: Some(0),
            pins: None,
            ..Default::default()
        };
        panel.remap_channels(&[None, Some(0), Some(1)]);
        let spi = panel.current().unwrap().settings.spi;
        assert_eq!(spi.clock, 0);
        assert_eq!(spi.select, Some(usize::MAX));
        assert_eq!(spi.data, [Some(1), None]);
    }

    #[test]
    fn pulse_inspection_keeps_the_decoded_window_and_exact_event_selection() {
        let original = 0.0..0.025;
        let event = Event {
            time: 0.012..0.01209,
            kind: Kind::I2cAddress {
                address: 0x68,
                ten_bit: false,
                read: false,
                ack: true,
            },
        };
        let coincident = Event {
            time: event.time.clone(),
            kind: Kind::I2cStart { repeated: false },
        };
        let mut decoder = Decoder::new(Mode::I2c, &[]);
        decoder.settings.data = 1;
        decoder.request = Some(Key {
            span: original.clone(),
            inputs: decoder.inputs().unwrap(),
            source_revision: 1,
        });
        decoder.result = Some(Ok(Decoded {
            window: original.clone(),
            inspected_until: original.end,
            events: vec![event.clone(), coincident],
        }));
        let mut panel = Panel {
            decoders: vec![decoder],
            selected: Some(0),
            ..Default::default()
        };

        let mut visible = original.clone();
        assert!(panel.picked_event(&visible).is_none());
        let cursor = panel.follow_event(
            EventAction::Inspect(event.clone()),
            &mut visible,
            &original,
            1e-6,
        );
        assert!(visible.start < event.time.start && visible.end > event.time.end);
        assert!(visible.end - visible.start < 0.001);
        assert_eq!(panel.events(&visible).len(), 2);
        assert_eq!(
            panel.cursor_event(&visible, Some(cursor)).unwrap().kind,
            event.kind
        );
        assert_eq!(panel.picked_event(&visible), Some(&event));
        panel.exit_inspection(&mut visible);
        assert_eq!(visible, original);
        assert!(!panel.inspecting());
    }

    #[test]
    fn unavailable_decoder_inputs_can_be_rewired_before_decoding() {
        let mut decoder = Decoder::new(Mode::I2c, &[]);
        decoder.lane_count = Some(3);
        decoder.settings.data = usize::MAX;
        assert_eq!(
            decoder.inputs().err(),
            Some("Assign a channel to each unavailable input.")
        );
        decoder.settings.data = 0;
        assert_eq!(
            decoder.inputs().err(),
            Some("Choose different clock and data channels.")
        );
        decoder.settings.data = 2;
        assert!(matches!(
            decoder.inputs(),
            Ok(Inputs::I2c { clock: 0, data: 2 })
        ));
    }

    #[test]
    fn address_labels_use_hex_and_distinguish_address_width() {
        assert_eq!(
            event_label(Kind::I2cAddress {
                address: 0x68,
                ten_bit: false,
                read: false,
                ack: true,
            }),
            "0x68  write  ACK"
        );
        assert_eq!(
            event_label(Kind::I2cAddress {
                address: 0x0a,
                ten_bit: true,
                read: true,
                ack: false,
            }),
            "0x00A  read  NACK"
        );
    }
}
