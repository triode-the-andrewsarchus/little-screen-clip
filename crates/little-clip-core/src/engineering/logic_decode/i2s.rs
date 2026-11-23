use super::{Event, Kind, Line, Trace, earliest, emit, validate_trace};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum I2sFormat {
    /// Word select changes one bit clock before the new word's MSB.
    Philips,
    /// The MSB is sampled on the first rising clock after word select changes.
    LeftJustified,
}

impl I2sFormat {
    pub fn label(self) -> &'static str {
        match self {
            Self::Philips => "I²S",
            Self::LeftJustified => "Left-justified",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum I2sChannel {
    Left,
    Right,
}

impl I2sChannel {
    pub fn label(self) -> &'static str {
        match self {
            Self::Left => "Left",
            Self::Right => "Right",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct I2sConfig {
    pub bits: u8,
    pub format: I2sFormat,
    pub left_high: bool,
}

impl Default for I2sConfig {
    fn default() -> Self {
        Self {
            bits: 16,
            format: I2sFormat::Philips,
            left_high: false,
        }
    }
}

/// Decode signed stereo serial samples on rising bit-clock edges.
///
/// The three traces share a start time. Decoding starts after the first
/// observed word-select transition and ends at their common inspected limit.
/// Word select can change on the sampling edge. Unknown or changing data at
/// that edge yields a sample gap. No value is produced for the leading
/// partial word or a word cut off by the inspected limit.
///
/// # Errors
///
/// Returns an error for invalid word width, malformed traces, or an oversized
/// result.
pub fn i2s(
    clock: &Trace,
    select: &Trace,
    data: &Trace,
    config: I2sConfig,
) -> Result<Vec<Event>, String> {
    if !(8..=32).contains(&config.bits) {
        return Err("I²S needs a word width from 8 to 32 bits.".into());
    }
    let mut end = clock.inspected_until;
    for trace in [clock, select, data] {
        validate_trace(trace, None)?;
        end = end.min(trace.inspected_until);
    }
    let mut clock = Line::new(clock);
    let mut select = Line::new(select);
    let mut data = Line::new(data);
    let mut bus = Bus {
        side: select
            .level
            .filter(|_| clock.level.is_some())
            .map(|level| level != config.left_high),
        word: None,
        config,
    };
    let mut events = Vec::new();
    while let Some(at) = earliest(
        earliest(clock.next_time(), select.next_time()),
        data.next_time(),
    )
    .filter(|&time| time < end)
    {
        let (prior_clock, prior_data) = (clock.level, data.level);
        clock.advance(at);
        select.advance(at);
        data.advance(at);
        let Some(side) = select.level.filter(|_| clock.level.is_some()) else {
            bus.retire();
            continue;
        };
        if prior_clock == Some(false) && clock.level == Some(true) {
            let bit = prior_data.filter(|_| prior_data == data.level);
            bus.sample(at, side != config.left_high, bit, &mut events)?;
        }
    }
    Ok(events)
}

struct Bus {
    side: Option<bool>,
    word: Option<Word>,
    config: I2sConfig,
}

impl Bus {
    fn retire(&mut self) {
        self.side = None;
        self.word = None;
    }

    fn sample(
        &mut self,
        at: f64,
        right: bool,
        bit: Option<bool>,
        events: &mut Vec<Event>,
    ) -> Result<(), String> {
        let previous = self.side.replace(right);
        if previous == Some(!right) {
            let delayed = self.config.format == I2sFormat::Philips;
            if let Some(mut word) = self.word.take() {
                if delayed {
                    // The edge that observes new WS can still carry the previous LSB.
                    word.sample(at, bit, self.config.bits, events)?;
                }
                word.close(at, self.config.bits, events)?;
            }
            self.word = Some(Word::new(at, right));
            if delayed {
                return Ok(());
            }
        }
        if let Some(word) = &mut self.word {
            word.sample(at, bit, self.config.bits, events)?;
        }
        Ok(())
    }
}

struct Word {
    channel: I2sChannel,
    start: f64,
    count: u8,
    raw: Option<u32>,
}

impl Word {
    fn new(at: f64, right: bool) -> Self {
        Self {
            channel: if right {
                I2sChannel::Right
            } else {
                I2sChannel::Left
            },
            start: at,
            count: 0,
            raw: Some(0),
        }
    }

    fn sample(
        &mut self,
        at: f64,
        bit: Option<bool>,
        bits: u8,
        events: &mut Vec<Event>,
    ) -> Result<(), String> {
        if self.count == bits {
            return Ok(());
        }
        if self.count == 0 {
            self.start = at;
        }
        self.raw = self
            .raw
            .zip(bit)
            .map(|(value, high)| (value << 1) | u32::from(high));
        self.count += 1;
        if self.count == bits {
            let shift = 32 - bits;
            let sample = self.raw.map(|raw| ((raw << shift) as i32) >> shift);
            emit(
                events,
                self.start..at,
                Kind::I2sSample {
                    channel: self.channel,
                    sample,
                    bits,
                },
            )?;
        }
        Ok(())
    }

    fn close(self, at: f64, bits: u8, events: &mut Vec<Event>) -> Result<(), String> {
        if self.count < bits {
            emit(
                events,
                self.start..at,
                Kind::I2sShortWord {
                    channel: self.channel,
                    received: self.count,
                    bits,
                },
            )?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engineering::logic::{Edge, Level};

    #[test]
    fn philips_carries_the_previous_lsb_on_the_word_select_edge() {
        let mut cells = vec![(true, Level::Low); 3];
        cells.push((false, Level::Low));
        cells.extend(word(false, 0x81, 8));
        cells.last_mut().unwrap().0 = true;
        cells.extend(word(true, 0x43, 8));
        cells.last_mut().unwrap().0 = false;
        let [clock, select, data] = capture(&cells, true);
        let events = i2s(&clock, &select, &data, config()).unwrap();
        assert_eq!(
            events,
            [
                Event {
                    time: 20.0..48.0,
                    kind: sample(I2sChannel::Left, Some(-127), 8),
                },
                Event {
                    time: 52.0..80.0,
                    kind: sample(I2sChannel::Right, Some(67), 8),
                },
            ]
        );
    }

    #[test]
    fn left_justified_uses_the_boundary_bit_and_ignores_slot_padding() {
        let mut cells = vec![(false, Level::Low); 3];
        cells.extend(word(true, 0xffff_fffd, 32));
        cells.extend([(true, Level::Unknown); 2]);
        cells.extend(word(false, 0x4000_0001, 32));
        let [clock, select, data] = capture(&cells, false);
        let cfg = I2sConfig {
            bits: 32,
            format: I2sFormat::LeftJustified,
            left_high: true,
        };
        let events = i2s(&clock, &select, &data, cfg).unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].time, 16.0..140.0);
        assert_eq!(events[0].kind, sample(I2sChannel::Left, Some(-3), 32));
        assert_eq!(
            events[1].kind,
            sample(I2sChannel::Right, Some(0x4000_0001), 32)
        );
    }

    #[test]
    fn short_and_unknown_words_are_gaps_and_recover_at_the_next_boundary() {
        let mut cells = vec![(true, Level::Low), (false, Level::Low)];
        cells.extend([(false, Level::High); 3]);
        cells.push((true, Level::Low));
        cells.extend(word(true, 0xfe, 8));
        cells[8].1 = Level::Unknown;
        cells.last_mut().unwrap().0 = false;
        cells.extend(word(false, 0x25, 8));
        cells.extend([(true, Level::Low); 5]);
        let [clock, select, data] = capture(&cells, false);
        let events = i2s(&clock, &select, &data, config()).unwrap();
        assert_eq!(
            events.iter().map(|event| event.kind).collect::<Vec<_>>(),
            [
                Kind::I2sShortWord {
                    channel: I2sChannel::Left,
                    received: 4,
                    bits: 8,
                },
                sample(I2sChannel::Right, None, 8),
                sample(I2sChannel::Left, Some(37), 8),
            ]
        );
    }

    fn sample(channel: I2sChannel, sample: Option<i32>, bits: u8) -> Kind {
        Kind::I2sSample {
            channel,
            sample,
            bits,
        }
    }

    fn config() -> I2sConfig {
        I2sConfig {
            bits: 8,
            ..I2sConfig::default()
        }
    }

    fn word(ws: bool, value: u32, width: u8) -> impl Iterator<Item = (bool, Level)> {
        (0..width)
            .rev()
            .map(move |bit| (ws, level(value & (1 << bit) != 0)))
    }

    fn level(high: bool) -> Level {
        if high { Level::High } else { Level::Low }
    }

    fn capture(cells: &[(bool, Level)], synchronous_ws: bool) -> [Trace; 3] {
        let end = cells.len() as f64 * 4.0 + 2.0;
        let [mut clock, mut select, mut data] =
            [Level::Low, level(cells[0].0), Level::Low].map(|initial| Trace {
                inspected_until: end,
                initial: Some(initial),
                edges: Vec::new(),
            });
        for (index, &(ws, bit)) in cells.iter().enumerate() {
            let time = (index + 1) as f64 * 4.0;
            drive(&mut clock, time, Level::High);
            drive(&mut clock, time + 1.0, Level::Low);
            drive(&mut select, time - f64::from(!synchronous_ws), level(ws));
            drive(&mut data, time - 1.0, bit);
        }
        [clock, select, data]
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
}
