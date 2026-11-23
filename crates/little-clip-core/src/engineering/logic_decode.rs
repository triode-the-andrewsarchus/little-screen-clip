//! Passive decoding of already bounded digital timing windows.

use super::logic::{Edge, Level, Trace};
use std::ops::Range;

mod i2s;
mod pcm;
mod spi;
pub use i2s::{I2sChannel, I2sConfig, I2sFormat, i2s};
pub use pcm::PcmLane;
pub use spi::{SpiConfig, spi};

const MAX_EVENTS: usize = 16_384;
const MAX_INPUT_EDGES: usize = 1 << 20;

/// A decoded protocol symbol with its position on the capture time axis.
#[derive(Clone, Debug, PartialEq)]
pub struct Event {
    /// START and STOP markers have equal start and end coordinates.
    pub time: Range<f64>,
    pub kind: Kind,
}

/// Supported serial bus symbols.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    I2cStart {
        repeated: bool,
    },
    I2cStop,
    I2cAddress {
        address: u16,
        ten_bit: bool,
        read: bool,
        ack: bool,
    },
    I2cData {
        byte: u8,
        ack: bool,
    },
    SpiWord {
        mosi: Option<u32>,
        miso: Option<u32>,
        bits: u8,
    },
    UartByte {
        byte: u8,
    },
    I2sSample {
        channel: I2sChannel,
        sample: Option<i32>,
        bits: u8,
    },
    I2sShortWord {
        channel: I2sChannel,
        received: u8,
        bits: u8,
    },
}

/// Decodes I²C START, STOP, address, and byte events.
///
/// Both traces must start at the same time on the capture axis. Only their
/// common inspected prefix is used. A transaction needs an observed START.
/// Ambiguous changes at the sampling edge and undriven levels retire the
/// active transaction.
///
/// # Errors
/// Returns an error for malformed edge order or an oversized event window.
pub fn i2c(clock: &Trace, data: &Trace) -> Result<Vec<Event>, String> {
    validate_trace(clock, None)?;
    validate_trace(data, None)?;
    let end = clock.inspected_until.min(data.inspected_until);
    let mut clock = Line::new(clock);
    let mut data = Line::new(data);
    let mut bus = I2c::default();
    let mut events = Vec::new();

    while let Some(step) = WireStep::next(&mut clock, &mut data).filter(|step| step.time < end) {
        bus.observe(step, &mut events)?;
    }
    Ok(events)
}

#[derive(Default)]
struct I2c {
    phase: Phase,
    address: Option<u16>,
    byte: u8,
    bits: u8,
    start: f64,
}

#[derive(Clone, Copy, Default)]
enum Phase {
    #[default]
    Idle,
    Address,
    AddressLow {
        high: u16,
        start: f64,
    },
    Data,
    Wait,
}

impl I2c {
    fn observe(&mut self, step: WireStep, events: &mut Vec<Event>) -> Result<(), String> {
        let (Some(clock), Some(data)) = step.after else {
            self.reset();
            return Ok(());
        };
        let stable_clock = step.before.0 == Some(clock);
        let stable_data = step.before.1 == Some(data);
        match (clock, stable_clock, stable_data) {
            (true, false, false) => self.reset(),
            (true, true, false) => return self.condition(step.before.1, data, step.time, events),
            (true, false, true) if step.before.0 == Some(false) => {
                return self.sample(data, step.time, events);
            }
            _ => {}
        }
        Ok(())
    }

    fn reset(&mut self) {
        *self = Self::default();
    }

    fn condition(
        &mut self,
        previous: Option<bool>,
        high: bool,
        at: f64,
        events: &mut Vec<Event>,
    ) -> Result<(), String> {
        let active = !matches!(self.phase, Phase::Idle);
        match (previous, high) {
            (Some(true), false) => {
                self.phase = Phase::Address;
                self.bits = 0;
                self.byte = 0;
                emit(events, at..at, Kind::I2cStart { repeated: active })
            }
            (Some(false), true) if active => {
                self.reset();
                emit(events, at..at, Kind::I2cStop)
            }
            _ => Ok(()),
        }
    }

    fn sample(&mut self, high: bool, at: f64, events: &mut Vec<Event>) -> Result<(), String> {
        if matches!(self.phase, Phase::Idle | Phase::Wait) {
            return Ok(());
        }
        if self.bits == 0 {
            self.start = at;
        }
        if self.bits < 8 {
            self.byte = (self.byte << 1) | u8::from(high);
            self.bits += 1;
            return Ok(());
        }
        let ack = !high;
        let byte = self.byte;
        let time = self.start..at;
        self.bits = 0;
        self.byte = 0;
        self.complete_byte(byte, ack, time, events)
    }

    fn complete_byte(
        &mut self,
        byte: u8,
        ack: bool,
        time: Range<f64>,
        events: &mut Vec<Event>,
    ) -> Result<(), String> {
        let phase = self.phase;
        self.phase = if ack { Phase::Data } else { Phase::Wait };
        let event = match phase {
            Phase::Address => match self.address_byte(byte, ack, time.start) {
                Some(kind) => kind,
                None => return Ok(()),
            },
            Phase::AddressLow { high, start } => {
                let address = high | u16::from(byte);
                self.address = ack.then_some(address);
                return emit(
                    events,
                    start..time.end,
                    Kind::I2cAddress {
                        address,
                        ten_bit: true,
                        read: false,
                        ack,
                    },
                );
            }
            Phase::Data => Kind::I2cData { byte, ack },
            Phase::Idle | Phase::Wait => return Ok(()),
        };
        emit(events, time, event)
    }

    fn address_byte(&mut self, byte: u8, ack: bool, start: f64) -> Option<Kind> {
        let read = byte & 1 != 0;
        if byte & 0xf8 != 0xf0 {
            self.address = None;
            return Some(Kind::I2cAddress {
                address: u16::from(byte >> 1),
                ten_bit: false,
                read,
                ack,
            });
        }
        let high = u16::from(byte & 6) << 7;
        if !read {
            self.address = None;
            if ack {
                self.phase = Phase::AddressLow { high, start };
            }
            return None;
        }
        match self.address.filter(|address| address & 0x300 == high) {
            Some(address) => Some(Kind::I2cAddress {
                address,
                ten_bit: true,
                read,
                ack,
            }),
            None => {
                self.phase = Phase::Wait;
                None
            }
        }
    }
}

/// Decodes UART 8N1 bytes at an explicit baud rate.
///
/// `window_start` is the clipped left edge used with `Trace::initial`.
/// A start bit needs a preceding high interval and a stable low center.
/// Bytes with unknown, unstable, missing, or invalid stop bits are skipped.
///
/// # Errors
/// Returns an error for invalid timing, malformed edge order, or an oversized
/// result.
pub fn uart(data: &Trace, window_start: f64, baud: f64) -> Result<Vec<Event>, String> {
    validate_trace(data, Some(window_start))?;
    let bit = 1.0 / baud;
    if !bit.is_finite() || bit <= 0.0 || !(10.0 * bit).is_finite() {
        return Err("UART requires a finite positive baud rate.".into());
    }
    let mut events = Vec::new();
    let mut high_since = (data.initial == Some(Level::High)).then_some(window_start);
    let mut next_candidate = window_start;
    for edge in &data.edges {
        let at = edge.time_seconds;
        let prior_idle = high_since.take();
        if edge.level == Level::High {
            high_since = prior_idle.or(Some(at));
        }
        if edge.level != Level::Low || at < next_candidate {
            continue;
        }
        if prior_idle.is_none_or(|start| at - start < 0.8 * bit) {
            continue;
        }
        let end = at + 10.0 * bit;
        if end > data.inspected_until || end <= at || stable_bit(data, at, bit) != Some(false) {
            continue;
        }
        next_candidate = end - 0.2 * bit;
        if let Some(byte) = uart_byte(data, at, end, bit) {
            emit(&mut events, at..end, Kind::UartByte { byte })?;
        }
    }
    Ok(events)
}

fn uart_byte(trace: &Trace, at: f64, end: f64, bit: f64) -> Option<u8> {
    let first = trace.edges.partition_point(|edge| edge.time_seconds <= at);
    let last = trace.edges.partition_point(|edge| edge.time_seconds < end);
    if trace.edges[first..last]
        .iter()
        .any(|edge| driven(edge.level).is_none())
    {
        return None;
    }
    let mut byte = 0;
    for shift in 0..8 {
        let high = stable_bit(trace, at + f64::from(shift + 1) * bit, bit)?;
        byte |= u8::from(high) << shift;
    }
    (stable_bit(trace, at + 9.0 * bit, bit) == Some(true)).then_some(byte)
}

fn stable_bit(trace: &Trace, start: f64, period: f64) -> Option<bool> {
    let left = start + 0.35 * period;
    let right = start + 0.65 * period;
    if right <= left {
        return None;
    }
    let first = trace
        .edges
        .partition_point(|edge| edge.time_seconds <= left);
    if trace
        .edges
        .get(first)
        .is_some_and(|edge| edge.time_seconds < right)
    {
        return None;
    }
    let level = first
        .checked_sub(1)
        .map_or(trace.initial, |index| Some(trace.edges[index].level));
    level.and_then(driven)
}

struct Line<'a> {
    edges: &'a [Edge],
    level: Option<bool>,
}

impl<'a> Line<'a> {
    fn new(trace: &'a Trace) -> Self {
        Self {
            edges: &trace.edges,
            level: trace.initial.and_then(driven),
        }
    }

    fn next_time(&self) -> Option<f64> {
        self.edges.first().map(|edge| edge.time_seconds)
    }

    fn advance(&mut self, at: f64) {
        if let Some((edge, rest)) = self.edges.split_first()
            && edge.time_seconds == at
        {
            self.edges = rest;
            self.level = driven(edge.level);
        }
    }
}

struct WireStep {
    time: f64,
    before: (Option<bool>, Option<bool>),
    after: (Option<bool>, Option<bool>),
}

impl WireStep {
    fn next(clock: &mut Line<'_>, data: &mut Line<'_>) -> Option<Self> {
        let time = earliest(clock.next_time(), data.next_time())?;
        let before = (clock.level, data.level);
        clock.advance(time);
        data.advance(time);
        Some(Self {
            time,
            before,
            after: (clock.level, data.level),
        })
    }
}

fn earliest(left: Option<f64>, right: Option<f64>) -> Option<f64> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.min(right)),
        (left, right) => left.or(right),
    }
}

fn driven(level: Level) -> Option<bool> {
    match level {
        Level::Low => Some(false),
        Level::High => Some(true),
        Level::Unknown | Level::HighImpedance => None,
    }
}

fn emit(events: &mut Vec<Event>, time: Range<f64>, kind: Kind) -> Result<(), String> {
    if events.len() == MAX_EVENTS {
        return Err("Too many protocol events in this window. Zoom in to decode it.".into());
    }
    events.push(Event { time, kind });
    Ok(())
}

fn validate_trace(trace: &Trace, start: Option<f64>) -> Result<(), String> {
    if !trace.inspected_until.is_finite()
        || start.is_some_and(|start| !start.is_finite() || start > trace.inspected_until)
    {
        return Err("Protocol decoder needs a finite inspected interval.".into());
    }
    if trace.edges.len() > MAX_INPUT_EDGES {
        return Err("Protocol decoder input exceeds the edge budget.".into());
    }
    let mut previous = f64::NEG_INFINITY;
    for edge in &trace.edges {
        let time = edge.time_seconds;
        if !time.is_finite()
            || time <= previous
            || time >= trace.inspected_until
            || start.is_some_and(|start| time < start)
        {
            return Err("Protocol decoder needs ordered edges in the inspected window.".into());
        }
        previous = time;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trace() -> Trace {
        Trace {
            initial: Some(Level::High),
            edges: Vec::new(),
            inspected_until: 0.0,
        }
    }

    fn drive(trace: &mut Trace, time_seconds: f64, level: Level) {
        let previous = trace.edges.last().map(|edge| edge.level).or(trace.initial);
        if previous != Some(level) {
            trace.edges.push(Edge {
                time_seconds,
                level,
            });
        }
    }

    fn level(high: bool) -> Level {
        if high { Level::High } else { Level::Low }
    }

    struct TestBus {
        clock: Trace,
        data: Trace,
        time: f64,
    }

    impl TestBus {
        fn new() -> Self {
            Self {
                clock: trace(),
                data: trace(),
                time: 0.0,
            }
        }

        fn clock(&mut self, high: bool) {
            self.time += 1e-6;
            drive(&mut self.clock, self.time, level(high));
        }

        fn data(&mut self, high: bool) {
            self.time += 1e-6;
            drive(&mut self.data, self.time, level(high));
        }

        fn start(&mut self) {
            self.clock(false);
            self.data(true);
            self.clock(true);
            self.data(false);
        }

        fn stop(&mut self) {
            self.clock(false);
            self.data(false);
            self.clock(true);
            self.data(true);
        }

        fn byte(&mut self, byte: u8, ack: bool) {
            let bits = (0..8).rev().map(|bit| byte & (1 << bit) != 0);
            for high in bits.chain(std::iter::once(!ack)) {
                self.clock(false);
                self.data(high);
                self.clock(true);
            }
        }

        fn decode(mut self) -> Vec<Kind> {
            self.clock.inspected_until = self.time + 1e-6;
            self.data.inspected_until = self.time + 1e-6;
            i2c(&self.clock, &self.data)
                .unwrap()
                .into_iter()
                .map(|event| event.kind)
                .collect()
        }
    }

    #[test]
    fn i2c_reports_start_seven_bit_address_ack_and_repeated_start() {
        let mut bus = TestBus::new();
        bus.byte(0xa0, true); // No START yet.
        bus.start();
        bus.byte(0xa0, true);
        bus.byte(0x36, true);
        bus.start();
        bus.byte(0xa1, true);
        bus.byte(0xc4, false);
        bus.stop();
        assert_eq!(
            bus.decode(),
            [
                Kind::I2cStart { repeated: false },
                Kind::I2cAddress {
                    address: 0x50,
                    ten_bit: false,
                    read: false,
                    ack: true
                },
                Kind::I2cData {
                    byte: 0x36,
                    ack: true
                },
                Kind::I2cStart { repeated: true },
                Kind::I2cAddress {
                    address: 0x50,
                    ten_bit: false,
                    read: true,
                    ack: true
                },
                Kind::I2cData {
                    byte: 0xc4,
                    ack: false
                },
                Kind::I2cStop,
            ]
        );
    }

    #[test]
    fn i2c_reuses_a_complete_ten_bit_address_for_repeated_reads() {
        let mut bus = TestBus::new();
        bus.start();
        bus.byte(0xf4, true);
        bus.byte(0x23, true);
        bus.start();
        bus.byte(0xf5, true);
        bus.stop();
        assert_eq!(
            bus.decode(),
            [
                Kind::I2cStart { repeated: false },
                Kind::I2cAddress {
                    address: 0x223,
                    ten_bit: true,
                    read: false,
                    ack: true
                },
                Kind::I2cStart { repeated: true },
                Kind::I2cAddress {
                    address: 0x223,
                    ten_bit: true,
                    read: true,
                    ack: true
                },
                Kind::I2cStop,
            ]
        );
    }

    #[test]
    fn i2c_does_not_mistake_unknown_states_or_partial_bits_for_bytes() {
        let mut bus = TestBus::new();
        bus.start();
        bus.clock(false);
        bus.time += 1e-6;
        drive(&mut bus.data, bus.time, Level::Unknown);
        bus.clock(true);
        bus.byte(0xac, true);
        assert_eq!(bus.decode(), [Kind::I2cStart { repeated: false }]);
    }

    fn uart_frame(line: &mut Trace, start: f64, bit: f64, byte: u8, stop: bool) {
        let body = (0..8).map(|position| byte & (1 << position) != 0);
        for (position, high) in std::iter::once(false)
            .chain(body)
            .chain(std::iter::once(stop))
            .enumerate()
        {
            drive(line, start + position as f64 * bit, level(high));
        }
    }

    #[test]
    fn uart_reads_lsb_first_and_keeps_back_to_back_frames() {
        let bit = 1.0 / 115_200.0;
        let mut line = trace();
        let first = 2.0 * bit;
        let second = first + 10.0 * bit;
        uart_frame(&mut line, first, bit, 0xa5, true);
        uart_frame(&mut line, second, bit, 0x13, true);
        line.inspected_until = second + 11.0 * bit;
        let events = uart(&line, 0.0, 115_200.0).unwrap();
        assert_eq!(events[0].time.start, first);
        assert_eq!(events[1].time.start, second);
        assert_eq!(events[0].kind, Kind::UartByte { byte: 0xa5 });
        assert_eq!(events[1].kind, Kind::UartByte { byte: 0x13 });
        assert_eq!(events.len(), 2);
    }

    #[test]
    fn uart_skips_incomplete_frames_invalid_stop_and_unverified_idle() {
        let mut line = trace();
        let bit = 0.001;
        uart_frame(&mut line, 2.0 * bit, bit, 0x01, false);
        line.inspected_until = 20.0 * bit;
        assert!(uart(&line, 0.0, 1000.0).unwrap().is_empty());

        let mut partial = trace();
        uart_frame(&mut partial, 2.0 * bit, bit, 0x01, true);
        partial.inspected_until = 11.5 * bit;
        assert!(uart(&partial, 0.0, 1000.0).unwrap().is_empty());

        let mut missing_idle = trace();
        uart_frame(&mut missing_idle, 2.0 * bit, bit, 0x00, true);
        missing_idle.inspected_until = 13.0 * bit;
        assert!(uart(&missing_idle, 1.5 * bit, 1000.0).unwrap().is_empty());
        assert!(uart(&missing_idle, 0.0, f64::INFINITY).is_err());
    }
}
