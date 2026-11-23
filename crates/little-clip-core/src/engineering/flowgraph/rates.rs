//! Static nominal clocks for saved scalar stream connections.

mod numbers;
mod rules;

use super::{Direction, Graph, State};
use numbers::Numbers;
use rules::Rule;
use std::collections::VecDeque;

/// How a block's saved setting describes a rate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RateMeaning {
    Signal,
    Hardware,
    Pacing,
    Display,
}

/// One resolved value from a saved block setting.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RateSetting {
    pub rate: f64,
    pub key: &'static str,
    pub meaning: RateMeaning,
}

impl RateSetting {
    pub fn differs(self, nominal: NominalRate) -> bool {
        !same_rate(self.rate, nominal.rate)
    }
}

/// Provenance of a nominal rate at its original source or throttle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RateOrigin {
    pub block: usize,
    pub meaning: RateMeaning,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NominalRate {
    pub rate: f64,
    /// Multiple upstream clocks can agree without one unique origin.
    pub origin: Option<RateOrigin>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum StreamRate {
    #[default]
    Unknown,
    Nominal(NominalRate),
    Conflicting,
}

impl StreamRate {
    fn anchored(block: usize, setting: RateSetting) -> Self {
        Self::Nominal(NominalRate {
            rate: setting.rate,
            origin: Some(RateOrigin {
                block,
                meaning: setting.meaning,
            }),
        })
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct BlockRates {
    pub input: StreamRate,
    pub output: StreamRate,
    pub setting: Option<RateSetting>,
    pub ratio: Option<f64>,
}

/// Configured-rate observations indexed by original block and wire ordinals.
pub struct Rates {
    blocks: Vec<BlockRates>,
    wires: Vec<Option<StreamRate>>,
}

impl Rates {
    pub fn inspect(graph: &Graph) -> Self {
        let numbers = Numbers::from_graph(graph);
        let mut remaining_inputs = vec![0; graph.blocks.len()];
        let wires = graph
            .connections
            .iter()
            .map(|link| {
                let source = &graph.blocks[link.source.block];
                let sink = &graph.blocks[link.sink.block];
                if source.state != State::Enabled
                    || sink.state != State::Enabled
                    || !numbered_port(&link.source.port)
                    || !numbered_port(&link.sink.port)
                {
                    return None;
                }
                remaining_inputs[link.sink.block] += 1;
                Some(StreamRate::Unknown)
            })
            .collect();
        let mut ready: VecDeque<_> = remaining_inputs
            .iter()
            .enumerate()
            .filter_map(|(index, &remaining)| (remaining == 0).then_some(index))
            .collect();
        let rules = graph
            .blocks
            .iter()
            .map(|block| Rule::read(&numbers, block))
            .collect::<Vec<_>>();
        let mut rates = Self {
            blocks: rules.iter().map(|rule| rule.initial()).collect(),
            wires,
        };
        while let Some(index) = ready.pop_front() {
            let incoming = graph
                .adjacent(index, Direction::Upstream)
                .iter()
                .filter_map(|&edge| rates.wire(edge));
            let input = agreed_input(incoming);
            let output = rules[index].output(index, input);
            rates.blocks[index].input = input;
            rates.blocks[index].output = output;
            for &edge in graph.adjacent(index, Direction::Downstream) {
                if let Some(wire) = &mut rates.wires[edge] {
                    *wire = output;
                    let sink = graph.connections[edge].sink.block;
                    remaining_inputs[sink] -= 1;
                    if remaining_inputs[sink] == 0 {
                        ready.push_back(sink);
                    }
                }
            }
        }
        rates
    }

    pub fn block(&self, ordinal: usize) -> Option<BlockRates> {
        self.blocks.get(ordinal).copied()
    }

    pub fn wire(&self, ordinal: usize) -> Option<StreamRate> {
        self.wires.get(ordinal).copied().flatten()
    }
}

fn numbered_port(port: &str) -> bool {
    !port.is_empty() && port.bytes().all(|byte| byte.is_ascii_digit())
}

fn agreed_input(inputs: impl Iterator<Item = StreamRate>) -> StreamRate {
    let mut nominal: Option<NominalRate> = None;
    let mut unknown = false;
    for input in inputs {
        match input {
            StreamRate::Conflicting => return StreamRate::Conflicting,
            StreamRate::Unknown => unknown = true,
            StreamRate::Nominal(next) => {
                if let Some(prior) = &mut nominal {
                    if !same_rate(prior.rate, next.rate) {
                        return StreamRate::Conflicting;
                    }
                    if prior.origin != next.origin {
                        prior.origin = None;
                    }
                } else {
                    nominal = Some(next);
                }
            }
        }
    }
    match (unknown, nominal) {
        (false, Some(rate)) => StreamRate::Nominal(rate),
        _ => StreamRate::Unknown,
    }
}

fn same_rate(a: f64, b: f64) -> bool {
    (a - b).abs() <= 32.0 * f64::EPSILON * a.max(b)
}
