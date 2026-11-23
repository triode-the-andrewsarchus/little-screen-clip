//! Compile and evaluate named equations over mixed-signal observations.

use super::{Binding, Dimension, Expression, SignalType, Value};
use std::{
    collections::{BTreeSet, VecDeque},
    fmt,
};

const MAX_LANES: usize = 32;

/// One line of a graph the runtime could not compile.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GraphError {
    /// One-based line number in the equation editor.
    pub line: usize,
    /// Description of the invalid definition.
    pub message: String,
}

impl fmt::Display for GraphError {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(output, "Line {}. {}", self.line, self.message)
    }
}

impl std::error::Error for GraphError {}

/// A named output and its physical or digital result type.
pub struct DerivedLane {
    /// Variable name usable by subsequent definitions.
    pub name: String,
    /// Inferred result type.
    pub signal_type: SignalType,
    expression: Expression,
    initial: Option<Value>,
}

/// A graph of equations in dependency order.
///
/// Expressions can see recorded signals and preceding derived lanes. Keeping
/// definitions in that order makes the data flow visible in the editor.
pub struct SignalGraph {
    inputs: usize,
    used_inputs: Vec<usize>,
    lanes: Vec<DerivedLane>,
    history_slots: Vec<(usize, u16)>,
}

impl SignalGraph {
    /// Compiles one `name = expression` assignment per nonblank line.
    ///
    /// Use `name[n-1]` to reference the previous sample. A lane that uses its
    /// own history declares its unit, for example `filtered: V = 0.9 *
    /// filtered[n-1] + 0.1 * input`. Declared lanes start at zero or low before
    /// the calculation interval. Recorded and uninitialized inputs have gaps
    /// before that interval.
    ///
    /// # Errors
    /// Rejects duplicate or reserved names, expressions referring to
    /// unavailable lanes, and assignments with incompatible units.
    pub fn compile(
        document: &str,
        inputs: &[Binding<'_>],
        active: usize,
    ) -> Result<Self, GraphError> {
        let mut bindings = inputs.to_vec();
        let mut names: BTreeSet<_> = bindings
            .iter()
            .map(|entry| entry.name.to_lowercase())
            .collect();
        let mut used_inputs = BTreeSet::new();
        let mut lanes = Vec::new();
        let mut depths = vec![0; inputs.len()];
        for (index, text) in document.lines().enumerate() {
            let line = index + 1;
            let text = text.trim();
            if text.is_empty() {
                continue;
            }
            if lanes.len() >= MAX_LANES {
                return Err(problem(
                    line,
                    format!("Choose at most {MAX_LANES} equations"),
                ));
            }
            let (name, declared, source) = assignment(text, line, &mut names)?;
            let output = bindings.len();
            depths.push(0);
            if let Some(signal_type) = declared {
                bindings.push(Binding { name, signal_type });
            }
            let expression = Expression::parse_history(source, &bindings, active)
                .map_err(|error| problem(line, error.to_string()))?;
            let signal_type = expression.signal_type();
            if declared.is_some_and(|declared| declared != signal_type) {
                return Err(problem(
                    line,
                    "The equation does not produce the declared unit or logic type",
                ));
            }
            let mut instant_feedback = false;
            expression.visit_inputs(|index, delay| {
                depths[index] = depths[index].max(delay);
                instant_feedback |= index == output && delay == 0;
                if index < inputs.len() {
                    used_inputs.insert(index);
                }
            });
            if instant_feedback {
                return Err(problem(
                    line,
                    "Use a past sample such as [n-1] for feedback",
                ));
            }
            lanes.push(DerivedLane {
                name: name.to_owned(),
                signal_type,
                expression,
                initial: declared.map(initial_value),
            });
            if declared.is_none() {
                bindings.push(Binding { name, signal_type });
            }
        }
        Ok(Self {
            inputs: inputs.len(),
            used_inputs: used_inputs.into_iter().collect(),
            lanes,
            history_slots: depths
                .into_iter()
                .enumerate()
                .filter(|(_, depth)| *depth > 0)
                .collect(),
        })
    }

    /// Returns the ordered output names and their types.
    pub fn lanes(&self) -> &[DerivedLane] {
        &self.lanes
    }

    /// Returns the recorded inputs required by the expressions.
    pub fn used_inputs(&self) -> &[usize] {
        &self.used_inputs
    }

    /// Returns whether evaluating a lane depends on past samples.
    pub fn requires_history(&self) -> bool {
        !self.history_slots.is_empty()
    }

    /// Creates reusable storage for evaluating one row at a time.
    pub fn evaluator(&self) -> Evaluator<'_> {
        let count = self.inputs + self.lanes.len();
        let history = (0..count).map(|_| VecDeque::new()).collect();
        Evaluator {
            graph: self,
            values: vec![None; count],
            history,
        }
    }
}

/// Reuses input and result storage across successive sample instants.
pub struct Evaluator<'a> {
    graph: &'a SignalGraph,
    values: Vec<Option<Value>>,
    history: Vec<VecDeque<Option<Value>>>,
}

impl Evaluator<'_> {
    /// Evaluates all derived lanes from one aligned instant.
    ///
    /// Supply rows in chronological order for equations with sample history.
    /// The returned slice is ordered like [`SignalGraph::lanes`].
    pub fn evaluate(&mut self, input: impl Fn(usize) -> Option<Value>) -> &[Option<Value>] {
        for &index in &self.graph.used_inputs {
            self.values[index] = input(index);
        }
        for (index, lane) in self.graph.lanes.iter().enumerate() {
            let value = lane.expression.evaluate_history(|index, delay| {
                if delay == 0 {
                    return self.values[index];
                }
                self.history[index]
                    .get(usize::from(delay) - 1)
                    .copied()
                    .unwrap_or_else(|| {
                        index
                            .checked_sub(self.graph.inputs)
                            .and_then(|lane| self.graph.lanes[lane].initial)
                    })
            });
            self.values[self.graph.inputs + index] = value;
        }
        for &(index, depth) in &self.graph.history_slots {
            let history = &mut self.history[index];
            if history.len() == usize::from(depth) {
                history.pop_back();
            }
            history.push_front(self.values[index]);
        }
        &self.values[self.graph.inputs..]
    }
}

fn assignment<'a>(
    text: &'a str,
    line: usize,
    names: &mut BTreeSet<String>,
) -> Result<(&'a str, Option<SignalType>, &'a str), GraphError> {
    let (label, source) = text
        .split_once('=')
        .ok_or_else(|| problem(line, "Expected name = expression"))?;
    let (name, declared) = declaration(label, line)?;
    if !valid_name(name) || !names.insert(name.to_lowercase()) {
        return Err(problem(
            line,
            format!("Invalid or duplicate lane name {name}"),
        ));
    }
    Ok((name, declared, source))
}

fn declaration(label: &str, line: usize) -> Result<(&str, Option<SignalType>), GraphError> {
    let (name, unit) = match label.split_once(':') {
        Some((name, unit)) => {
            let unit = unit.trim();
            let signal_type = if unit.eq_ignore_ascii_case("logic") {
                SignalType::Digital
            } else {
                Dimension::from_unit(unit)
                    .map(SignalType::Analog)
                    .ok_or_else(|| problem(line, format!("Unknown output unit {unit}")))?
            };
            (name.trim(), Some(signal_type))
        }
        None => (label.trim(), None),
    };
    Ok((name.strip_suffix("[n]").unwrap_or(name).trim(), unit))
}

fn initial_value(signal_type: SignalType) -> Value {
    match signal_type {
        SignalType::Analog(_) => Value::Numeric(0.0),
        SignalType::Digital => Value::Predicate(false),
    }
}

fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_alphabetic() || c == '_')
        && chars.all(|c| c.is_alphanumeric() || c == '_')
        && !["x", "and", "or", "not"]
            .iter()
            .any(|reserved| reserved.eq_ignore_ascii_case(name))
}

fn problem(line: usize, message: impl Into<String>) -> GraphError {
    GraphError {
        line,
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn can_compute_new_lanes_from_prior_derived_values_and_digital_inputs() {
        let inputs = [
            Binding {
                name: "voltage",
                signal_type: SignalType::Analog(Dimension::VOLTAGE),
            },
            Binding {
                name: "current",
                signal_type: SignalType::Analog(Dimension::CURRENT),
            },
            Binding {
                name: "gate",
                signal_type: SignalType::Digital,
            },
            Binding {
                name: "unused",
                signal_type: SignalType::Digital,
            },
        ];
        let graph = SignalGraph::compile(
            "power = voltage * current\nactive = gate && power > 1W",
            &inputs,
            0,
        )
        .unwrap();
        assert_eq!(graph.used_inputs(), &[0, 1, 2]);
        let mut evaluator = graph.evaluator();
        let input = [
            Some(Value::Numeric(5.0)),
            Some(Value::Numeric(0.25)),
            Some(Value::Predicate(true)),
        ];
        assert_eq!(
            evaluator.evaluate(|index| input[index]),
            &[Some(Value::Numeric(1.25)), Some(Value::Predicate(true))]
        );
        assert_eq!(
            evaluator.evaluate(|index| input[index].filter(|_| index != 1)),
            &[None, None]
        );
    }

    #[test]
    fn causal_feedback_uses_an_initialized_previous_sample() {
        let inputs = [Binding {
            name: "input",
            signal_type: SignalType::Analog(Dimension::VOLTAGE),
        }];
        let graph = SignalGraph::compile(
            "smooth[n]: V = 0.5 * smooth[n-1] + 0.5 * input[n]\nprevious = smooth[n-2]",
            &inputs,
            0,
        )
        .unwrap();
        assert!(graph.requires_history());
        let mut evaluator = graph.evaluator();
        for (current, previous) in [(1.0, 0.0), (1.5, 0.0), (1.75, 1.0)] {
            assert_eq!(
                evaluator.evaluate(|_| Some(Value::Numeric(2.0))),
                &[
                    Some(Value::Numeric(current)),
                    Some(Value::Numeric(previous))
                ]
            );
        }
    }

    #[test]
    fn delays_preserve_missing_input_history_and_forbid_future_reads() {
        let inputs = [Binding {
            name: "input",
            signal_type: SignalType::Analog(Dimension::VOLTAGE),
        }];
        let graph = SignalGraph::compile("delta = input - input[n-1]", &inputs, 0).unwrap();
        let mut evaluator = graph.evaluator();
        assert_eq!(evaluator.evaluate(|_| Some(Value::Numeric(4.0))), &[None]);
        assert_eq!(
            evaluator.evaluate(|_| Some(Value::Numeric(6.0))),
            &[Some(Value::Numeric(2.0))]
        );
        for invalid in [
            "value = input[n+1]",
            "value: V = value[n]",
            "value: A = input",
        ] {
            assert!(SignalGraph::compile(invalid, &inputs, 0).is_err());
        }
    }

    #[test]
    fn rejects_duplicate_and_forward_dependencies_at_their_actual_line() {
        let inputs = [Binding {
            name: "source",
            signal_type: SignalType::Digital,
        }];
        assert_eq!(
            SignalGraph::compile("a = source\n\na = a", &inputs, 0)
                .err()
                .unwrap()
                .line,
            3
        );
        assert_eq!(
            SignalGraph::compile("a = b\nb = source", &inputs, 0)
                .err()
                .unwrap()
                .line,
            1
        );
    }
}
