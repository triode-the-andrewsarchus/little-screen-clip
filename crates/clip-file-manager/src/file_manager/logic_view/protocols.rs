//! Controls and bounded reads for passive bus decoding.
use super::{JOBS, Task, event_list};
use egui::{Context, Ui};
use little_clip_core::engineering::{
    logic::{Capture, Lane},
    logic_decode::{self, Event, Kind, SpiConfig},
    logic_preview::Recording,
    waveforms::display_value,
};
use std::{
    ops::Range,
    sync::{Arc, atomic::AtomicBool},
    time::Duration,
};

const DECODE_EDGES: usize = 8192;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    I2c,
    Spi,
    Uart,
}

impl Mode {
    fn label(self) -> &'static str {
        match self {
            Self::I2c => "I²C",
            Self::Spi => "SPI",
            Self::Uart => "UART",
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
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

    fn controls(&mut self, ui: &mut Ui, lanes: &[Lane]) {
        lane_picker(ui, "spi-clock", "Clock", &mut self.clock, lanes);
        optional_lane(ui, "spi-mosi", "MOSI", &mut self.data[0], lanes);
        optional_lane(ui, "spi-miso", "MISO", &mut self.data[1], lanes);
        optional_lane(ui, "spi-cs", "Select", &mut self.select, lanes);
        egui::ComboBox::from_id_salt("spi-mode")
            .selected_text(format!("Mode {}", self.config.mode))
            .show_ui(ui, |ui| {
                for mode in 0..4 {
                    ui.selectable_value(&mut self.config.mode, mode, format!("Mode {mode}"));
                }
            });
        ui.add(
            egui::DragValue::new(&mut self.config.bits)
                .range(1..=32)
                .suffix(" bits"),
        );
        ui.toggle_value(&mut self.config.lsb_first, "LSB first");
        if self.select.is_some() {
            ui.toggle_value(&mut self.config.active_high, "Select active high");
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

#[derive(Clone, Copy, PartialEq)]
enum Inputs {
    I2c { clock: usize, data: usize },
    Uart { data: usize, baud: f64 },
    Spi(SpiInputs),
}

#[derive(Clone, PartialEq)]
struct Key {
    span: Range<f64>,
    inputs: Inputs,
    refined: bool,
}

struct Decoded {
    window: Range<f64>,
    inspected_until: f64,
    events: Vec<Event>,
}

#[derive(Default)]
pub(super) struct Panel {
    decoders: Vec<Decoder>,
    selected: Option<usize>,
}

impl Panel {
    fn current(&self) -> Option<&Decoder> {
        self.selected.and_then(|index| self.decoders.get(index))
    }

    fn current_mut(&mut self) -> Option<&mut Decoder> {
        self.selected.and_then(|index| self.decoders.get_mut(index))
    }

    pub fn add_menu(&mut self, ui: &mut Ui, lanes: &[Lane]) -> bool {
        let mut added = false;
        ui.menu_button("Add decoder", |ui| {
            for mode in [Mode::Spi, Mode::I2c, Mode::Uart] {
                if ui
                    .add_enabled(self.decoders.len() < 12, egui::Button::new(mode.label()))
                    .clicked()
                {
                    self.close();
                    self.decoders.push(Decoder::new(mode, lanes));
                    self.selected = Some(self.decoders.len() - 1);
                    added = true;
                    ui.close();
                }
            }
        });
        added
    }

    pub fn edges(&self) -> bool {
        self.selected.is_none()
    }

    pub fn data_lane(&self) -> Option<usize> {
        self.current().and_then(Decoder::data_lane)
    }

    pub fn channels(&self) -> Option<Vec<usize>> {
        self.current().map(Decoder::channels)
    }

    pub fn controls(&mut self, ui: &mut Ui, lanes: &[Lane]) {
        if self.decoders.is_empty() {
            return;
        }
        let mut selected = self.selected;
        let mut remove = false;
        ui.horizontal_wrapped(|ui| {
            ui.selectable_value(&mut selected, None, "Edges");
            for (index, decoder) in self.decoders.iter().enumerate() {
                ui.selectable_value(
                    &mut selected,
                    Some(index),
                    format!("{} {}", decoder.mode.label(), index + 1),
                );
            }
            if selected.is_some() {
                remove = ui.button("Remove decoder").clicked();
            }
        });
        if self.selected != selected {
            self.close();
            self.selected = selected;
        }
        if remove && let Some(index) = self.selected.take() {
            self.decoders.remove(index);
        }
        if let Some(decoder) = self.current_mut() {
            decoder.controls(ui, lanes);
        }
    }

    pub fn update(
        &mut self,
        ctx: &Context,
        span: Range<f64>,
        refined: bool,
        recording: &Arc<Recording>,
    ) {
        if let Some(decoder) = self.current_mut() {
            decoder.update(ctx, span, refined, recording);
        }
    }

    pub fn close(&mut self) {
        if let Some(decoder) = self.current_mut() {
            decoder.close();
        }
    }

    pub fn cursor_event(&self, span: &Range<f64>, cursor: Option<f64>) -> Option<&Event> {
        self.current()?.cursor_event(span, cursor)
    }

    pub fn table(&mut self, ui: &mut Ui, span: &Range<f64>, cursor: Option<f64>) -> Option<Event> {
        self.current_mut()?.table(ui, span, cursor)
    }

    pub fn events(&self, span: &Range<f64>) -> &[Event] {
        self.current()
            .and_then(|decoder| decoder.current(span))
            .and_then(|result| result.as_ref().ok())
            .map_or(&[], |output| output.events.as_slice())
    }
}

struct Decoder {
    mode: Mode,
    spi: SpiInputs,
    clock: usize,
    data: usize,
    uart: usize,
    baud: String,
    request: Option<Key>,
    task: Option<Task<Result<Decoded, String>>>,
    result: Option<Result<Decoded, String>>,
    follow: Option<f64>,
    next_request: f64,
}

impl Decoder {
    fn new(mode: Mode, lanes: &[Lane]) -> Self {
        let clock = named_lane(lanes, &["SCL"]).unwrap_or(0);
        let data =
            named_lane(lanes, &["SDA"]).unwrap_or(usize::from(lanes.len() > 1 && clock == 0));
        let uart = named_lane(lanes, &["UART", "TX", "RX", "TXD", "RXD"]).unwrap_or(0);
        Self {
            mode,
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
            baud: String::new(),
            request: None,
            task: None,
            result: None,
            follow: None,
            next_request: 0.0,
        }
    }

    fn data_lane(&self) -> Option<usize> {
        match self.mode {
            Mode::I2c => Some(self.data),
            Mode::Uart => Some(self.uart),
            Mode::Spi => self.spi.data.into_iter().flatten().next(),
        }
    }

    fn channels(&self) -> Vec<usize> {
        let mut channels = match self.mode {
            Mode::I2c => vec![self.clock, self.data],
            Mode::Uart => vec![self.uart],
            Mode::Spi => self.spi.channels().collect(),
        };
        channels.sort_unstable();
        channels.dedup();
        channels
    }

    pub(super) fn controls(&mut self, ui: &mut Ui, lanes: &[Lane]) {
        ui.horizontal_wrapped(|ui| match self.mode {
            Mode::Spi => self.spi.controls(ui, lanes),
            Mode::I2c => {
                lane_picker(ui, "i2c-scl", "Clock", &mut self.clock, lanes);
                lane_picker(ui, "i2c-sda", "Data", &mut self.data, lanes);
            }
            Mode::Uart => {
                lane_picker(ui, "uart-data", "Data", &mut self.uart, lanes);
                ui.label("Baud");
                ui.add(
                    egui::TextEdit::singleline(&mut self.baud)
                        .id_salt("uart-baud")
                        .desired_width(80.0)
                        .char_limit(16)
                        .hint_text("115200"),
                );
                ui.weak("8N1");
            }
        });
    }

    fn inputs(&self) -> Result<Inputs, &'static str> {
        match self.mode {
            Mode::Spi => self.spi.validate(),
            Mode::I2c if self.clock != self.data => Ok(Inputs::I2c {
                clock: self.clock,
                data: self.data,
            }),
            Mode::I2c => Err("Choose different clock and data channels."),
            Mode::Uart if self.baud.trim().is_empty() => Err("Enter the recorded UART baud rate."),
            Mode::Uart => {
                let baud = self
                    .baud
                    .trim()
                    .parse::<f64>()
                    .ok()
                    .filter(|baud| baud.is_finite() && *baud > 0.0)
                    .ok_or("Enter a positive baud rate.")?;
                Ok(Inputs::Uart {
                    data: self.uart,
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

    pub(super) fn update(
        &mut self,
        ctx: &Context,
        span: Range<f64>,
        refined: bool,
        recording: &Arc<Recording>,
    ) {
        let Ok(inputs) = self.inputs() else {
            self.close();
            return;
        };
        let key = Key {
            span,
            inputs,
            refined,
        };
        if self.request.as_ref() != Some(&key) {
            self.start(ctx, key, recording);
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

    fn start(&mut self, ctx: &Context, key: Key, recording: &Arc<Recording>) {
        self.task = None;
        self.request = None;
        self.result = None;
        let now = ctx.input(|input| input.time);
        if now < self.next_request {
            ctx.request_repaint_after(Duration::from_secs_f64(self.next_request - now));
            return;
        }
        self.next_request = now + 0.075;
        self.request = Some(key.clone());
        let recording = Arc::clone(recording);
        let repaint = ctx.clone();
        self.task = Some(Task::start(&JOBS, Duration::from_secs(25), move |cancel| {
            let result = recording.checked(cancel, |capture| decode(capture, &key, cancel));
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
        active_event(events, cursor).map(|index| &events[index])
    }

    pub(super) fn table(
        &mut self,
        ui: &mut Ui,
        span: &Range<f64>,
        cursor: Option<f64>,
    ) -> Option<Event> {
        if let Err(message) = self.inputs() {
            ui.weak(message);
            return None;
        }
        // Move the scroll-follow state out while the decoded result is borrowed.
        let mut followed = self.follow;
        let mut destination = None;
        match self.current(span) {
            Some(Ok(output)) => {
                if !output.events.is_empty() && ui.button("Copy decoded").clicked() {
                    use std::fmt::Write;
                    let mut text = String::from("Start (s)\tEnd (s)\tEvent\n");
                    for event in &output.events {
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
                coverage(ui, output, span);
                let selected = active_event(&output.events, cursor);
                destination = event_list(
                    ui,
                    ("logic-protocol-rows", "Decoded event"),
                    cursor,
                    &mut followed,
                    selected,
                    output.events.len(),
                    |index| {
                        let event = &output.events[index];
                        (
                            display_value(event.time.start, "s"),
                            event_label(event.kind),
                        )
                    },
                )
                .map(|index| output.events[index].clone());
            }
            Some(Err(error)) => {
                ui.colored_label(ui.visuals().error_fg_color, error);
            }
            None => {
                ui.weak("Decoding the visible window…");
            }
        }
        self.follow = followed;
        destination
    }
}

fn decode(capture: &Capture, request: &Key, cancel: &AtomicBool) -> Result<Decoded, String> {
    let (window, inspected_until, events) = match request.inputs {
        Inputs::Spi(inputs) => return read_spi(capture, request.span.clone(), inputs, cancel),
        Inputs::I2c { clock, data } => {
            let window = lane_window(
                capture,
                data,
                lane_window(capture, clock, request.span.clone())?,
            )?;
            let scl = capture.read_edges(clock, window.clone(), DECODE_EDGES, cancel)?;
            let sda = capture.read_edges(data, window.clone(), DECODE_EDGES, cancel)?;
            let inspected = scl.inspected_until.min(sda.inspected_until);
            (window, inspected, logic_decode::i2c(&scl, &sda)?)
        }
        Inputs::Uart { data, baud } => {
            let window = lane_window(capture, data, request.span.clone())?;
            let trace = capture.read_edges(data, window.clone(), DECODE_EDGES, cancel)?;
            let events = logic_decode::uart(&trace, window.start, baud)?;
            (window, trace.inspected_until, events)
        }
    };
    Ok(Decoded {
        window,
        inspected_until,
        events,
    })
}

fn read_spi(
    capture: &Capture,
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
    capture: &Capture,
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

fn lane_picker(ui: &mut Ui, id: &str, label: &str, index: &mut usize, lanes: &[Lane]) {
    ui.label(label);
    egui::ComboBox::from_id_salt(id)
        .selected_text(&lanes[*index].name)
        .show_ui(ui, |ui| {
            for (lane, signal) in lanes.iter().enumerate() {
                ui.selectable_value(index, lane, &signal.name);
            }
        });
}

fn optional_lane(ui: &mut Ui, id: &str, label: &str, index: &mut Option<usize>, lanes: &[Lane]) {
    ui.label(label);
    let title = index.map_or("None", |index| lanes[index].name.as_str());
    egui::ComboBox::from_id_salt(id)
        .selected_text(title)
        .show_ui(ui, |ui| {
            ui.selectable_value(index, None, "None");
            for (lane, signal) in lanes.iter().enumerate() {
                ui.selectable_value(index, Some(lane), &signal.name);
            }
        });
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

pub(super) fn event_label(kind: Kind) -> String {
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
