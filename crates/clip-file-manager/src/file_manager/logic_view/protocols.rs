//! Controls and bounded reads for passive bus decoding.
use super::{JOBS, Task, event_list};
use egui::{Context, Ui};
use little_clip_core::engineering::{
    logic::{Capture, Lane},
    logic_decode::{self, Event, Kind},
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
    Edges,
    I2c,
    Uart,
}

#[derive(Clone, Copy, PartialEq)]
enum Inputs {
    I2c { clock: usize, data: usize },
    Uart { data: usize, baud: f64 },
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

pub(super) struct Panel {
    mode: Mode,
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

impl Panel {
    pub(super) fn new(lanes: &[Lane]) -> Self {
        let clock = named_lane(lanes, &["SCL"]).unwrap_or(0);
        let data =
            named_lane(lanes, &["SDA"]).unwrap_or(usize::from(lanes.len() > 1 && clock == 0));
        let uart = named_lane(lanes, &["UART", "TX", "RX", "TXD", "RXD"]).unwrap_or(0);
        Self {
            mode: Mode::Edges,
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

    pub(super) fn edges(&self) -> bool {
        self.mode == Mode::Edges
    }

    pub(super) fn data_lane(&self) -> Option<usize> {
        match self.mode {
            Mode::I2c => Some(self.data),
            Mode::Uart => Some(self.uart),
            Mode::Edges => None,
        }
    }

    pub(super) fn channels(&self) -> Option<Vec<usize>> {
        match self.mode {
            Mode::I2c if self.clock != self.data => Some(vec![self.clock, self.data]),
            Mode::I2c => Some(vec![self.clock]),
            Mode::Uart => Some(vec![self.uart]),
            Mode::Edges => None,
        }
    }

    pub(super) fn controls(&mut self, ui: &mut Ui, lanes: &[Lane]) {
        ui.horizontal_wrapped(|ui| {
            ui.selectable_value(&mut self.mode, Mode::Edges, "Edges");
            ui.selectable_value(&mut self.mode, Mode::I2c, "I²C");
            ui.selectable_value(&mut self.mode, Mode::Uart, "UART");
            match self.mode {
                Mode::Edges => {}
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
            }
        });
    }

    fn inputs(&self) -> Result<Inputs, &'static str> {
        match self.mode {
            Mode::Edges => Err("Select a protocol."),
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
                coverage(ui, output, span);
                let selected = active_event(&output.events, cursor);
                event_list(
                    ui,
                    "logic-protocol-rows",
                    cursor,
                    &mut followed,
                    selected,
                    output.events.len(),
                    |ui, rows| {
                        for row in rows {
                            let event = &output.events[row];
                            let time = display_value(event.time.start, "s");
                            let label = format!("{time:>13}    {}", event_label(event.kind));
                            let response = ui.selectable_label(
                                selected == Some(row),
                                egui::RichText::new(label).monospace(),
                            );
                            if response.clicked() {
                                destination = Some(event.clone());
                            }
                        }
                    },
                );
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

fn event_label(kind: Kind) -> String {
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
