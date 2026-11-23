use super::{Event, Kind, Line, Trace, earliest, emit, validate_trace};

/// Sampling and framing parameters for one SPI bus.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SpiConfig {
    pub bits: u8,
    pub mode: u8,
    pub lsb_first: bool,
    pub active_high: bool,
}

impl Default for SpiConfig {
    fn default() -> Self {
        Self {
            bits: 8,
            mode: 0,
            lsb_first: false,
            active_high: false,
        }
    }
}

/// Decode complete SPI words in the common inspected interval.
/// With chip select configured, only a frame whose assertion was observed can
/// establish word alignment. Without chip select, alignment begins at the
/// first sampling edge of the input window.
pub fn spi(
    clock: &Trace,
    select: Option<&Trace>,
    data: [Option<&Trace>; 2],
    config: SpiConfig,
) -> Result<Vec<Event>, String> {
    if !(1..=32).contains(&config.bits) || config.mode > 3 || data.iter().all(Option::is_none) {
        return Err("SPI needs a data channel, a mode from 0 to 3, and 1 to 32 bits.".into());
    }
    let mut end = clock.inspected_until;
    for trace in [Some(clock), select, data[0], data[1]]
        .into_iter()
        .flatten()
    {
        validate_trace(trace, None)?;
        end = end.min(trace.inspected_until);
    }
    let mut bus = Bus {
        clock: Line::new(clock),
        select: select.map(Line::new),
        data: data.map(|trace| trace.map(Line::new)),
        armed: select.is_none(),
        config,
    };
    let mut word = Word::default();
    let mut events = Vec::new();
    while let Some(time) =
        next_edge(&bus.clock, bus.select.as_ref(), &bus.data).filter(|&t| t < end)
    {
        let (reset, bits) = bus.advance(time);
        if reset {
            word = Word::default();
        }
        if let Some(kind) = bits.and_then(|levels| word.sample(config, time, levels)) {
            emit(&mut events, word.start..time, kind)?;
            word = Word::default();
        }
    }
    Ok(events)
}

struct Bus<'a> {
    clock: Line<'a>,
    select: Option<Line<'a>>,
    data: [Option<Line<'a>>; 2],
    armed: bool,
    config: SpiConfig,
}

impl Bus<'_> {
    fn advance(&mut self, time: f64) -> (bool, Option<[Option<bool>; 2]>) {
        let mut select_changed = false;
        if let Some(cs) = &mut self.select {
            let before = cs.level;
            cs.advance(time);
            select_changed = before != cs.level;
            if select_changed {
                self.armed = cs.level == Some(self.config.active_high)
                    && before == Some(!self.config.active_high);
            }
        }
        let before = self.clock.level;
        self.clock.advance(time);
        let bits = self.data.each_mut().map(|line| {
            let line = line.as_mut()?;
            let level = line.level;
            line.advance(time);
            (level == line.level).then_some(level).flatten()
        });
        let rising = matches!(self.config.mode, 0 | 3);
        let sampled = (self.armed
            && !select_changed
            && before == Some(!rising)
            && self.clock.level == Some(rising))
        .then_some(bits);
        (select_changed || self.clock.level.is_none(), sampled)
    }
}

fn next_edge(
    clock: &Line<'_>,
    select: Option<&Line<'_>>,
    data: &[Option<Line<'_>>; 2],
) -> Option<f64> {
    [select, data[0].as_ref(), data[1].as_ref()]
        .into_iter()
        .flatten()
        .fold(clock.next_time(), |time, line| {
            earliest(time, line.next_time())
        })
}

#[derive(Default)]
struct Word {
    start: f64,
    count: u8,
    value: [Option<u32>; 2],
}

impl Word {
    fn sample(&mut self, config: SpiConfig, time: f64, bits: [Option<bool>; 2]) -> Option<Kind> {
        if self.count == 0 {
            self.start = time;
            self.value = [Some(0); 2];
        }
        let shift = if config.lsb_first {
            self.count
        } else {
            config.bits - self.count - 1
        };
        for (word, bit) in self.value.iter_mut().zip(bits) {
            *word = word
                .zip(bit)
                .map(|(value, high)| value | (u32::from(high) << shift));
        }
        self.count += 1;
        if self.count == config.bits {
            let [mosi, miso] = self.value;
            // Preserve word framing even when either data line is unknown.
            return Some(Kind::SpiWord {
                mosi,
                miso,
                bits: config.bits,
            });
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engineering::logic::{Edge, Level};

    fn trace(initial: Level, edges: &[(f64, Level)]) -> Trace {
        Trace {
            initial: Some(initial),
            edges: edges
                .iter()
                .map(|&(time_seconds, level)| Edge {
                    time_seconds,
                    level,
                })
                .collect(),
            inspected_until: 30.0,
        }
    }

    #[test]
    fn spi_ignores_partial_chip_select_and_decodes_both_directions() {
        use Level::{High, Low};
        let clock = trace(
            Low,
            &[
                (1., High),
                (2., Low),
                (6., High),
                (7., Low),
                (10., High),
                (11., Low),
                (14., High),
                (15., Low),
                (18., High),
                (19., Low),
            ],
        );
        let select = trace(Low, &[(3., High), (4., Low), (20., High)]);
        let mosi = trace(High, &[(8., Low), (12., High), (16., Low)]);
        let miso = trace(Low, &[(8., High), (12., Low), (16., High)]);
        let config = SpiConfig {
            bits: 4,
            ..SpiConfig::default()
        };
        let words = spi(&clock, Some(&select), [Some(&mosi), Some(&miso)], config).unwrap();
        assert_eq!(
            words,
            vec![Event {
                time: 6.0..18.0,
                kind: Kind::SpiWord {
                    mosi: Some(10),
                    miso: Some(5),
                    bits: 4
                }
            }]
        );
        let reverse = SpiConfig {
            lsb_first: true,
            mode: 1,
            ..config
        };
        let words = spi(&clock, Some(&select), [Some(&mosi), None], reverse).unwrap();
        assert_eq!(
            words[0].kind,
            Kind::SpiWord {
                mosi: Some(5),
                miso: None,
                bits: 4
            }
        );
    }
}
