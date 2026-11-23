//! Compile and evaluate named equations over mixed-signal observations.

use super::{Binding, Expression, SignalType, Value};
use std::{collections::BTreeSet, fmt};

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
}

/// A graph of equations in dependency order.
///
/// Expressions can see recorded signals and preceding derived lanes. Keeping
/// definitions in that order makes the data flow visible in the editor.
pub struct SignalGraph {
    inputs: usize,
    used_inputs: Vec<usize>,
    lanes: Vec<DerivedLane>,
}

impl SignalGraph {
    /// Compiles one `name = expression` assignment per nonblank line.
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
            let (name, expression) = text
                .split_once('=')
                .ok_or_else(|| problem(line, "Expected name = expression"))?;
            let name = name.trim();
            if !valid_name(name) || !names.insert(name.to_lowercase()) {
                return Err(problem(
                    line,
                    format!("Invalid or duplicate lane name {name}"),
                ));
            }
            let expression = Expression::parse_bound(expression, &bindings, active)
                .map_err(|error| problem(line, error.to_string()))?;
            expression.visit_inputs(|index| {
                if index < inputs.len() {
                    used_inputs.insert(index);
                }
            });
            let signal_type = expression.signal_type();
            lanes.push(DerivedLane {
                name: name.to_owned(),
                signal_type,
                expression,
            });
            bindings.push(Binding { name, signal_type });
        }
        Ok(Self {
            inputs: inputs.len(),
            used_inputs: used_inputs.into_iter().collect(),
            lanes,
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

    /// Creates reusable storage for evaluating one row at a time.
    pub fn evaluator(&self) -> Evaluator<'_> {
        Evaluator {
            graph: self,
            values: vec![None; self.inputs + self.lanes.len()],
        }
    }
}

/// Reuses input and result storage across successive sample instants.
pub struct Evaluator<'a> {
    graph: &'a SignalGraph,
    values: Vec<Option<Value>>,
}

impl Evaluator<'_> {
    /// Evaluates all derived lanes from one aligned instant.
    ///
    /// The returned slice is ordered like [`SignalGraph::lanes`].
    pub fn evaluate(&mut self, input: impl Fn(usize) -> Option<Value>) -> &[Option<Value>] {
        for &index in &self.graph.used_inputs {
            self.values[index] = input(index);
        }
        for (index, lane) in self.graph.lanes.iter().enumerate() {
            let value = lane.expression.evaluate_bound(|index| self.values[index]);
            self.values[self.graph.inputs + index] = value;
        }
        &self.values[self.graph.inputs..]
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
    use crate::engineering::analysis::Dimension;

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
