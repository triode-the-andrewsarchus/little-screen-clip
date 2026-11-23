use super::super::{Block, Graph, State};
use crate::engineering::analysis::{Binding, Dimension, Expression, SignalType, Value};
use std::collections::VecDeque;

const MAX_VARIABLES: usize = 1024;

pub(super) struct Numbers<'a> {
    bindings: Vec<Binding<'a>>,
    values: Vec<Option<f64>>,
}

impl<'a> Numbers<'a> {
    pub(super) fn from_graph(graph: &'a Graph) -> Self {
        let candidates: Vec<_> = graph
            .blocks
            .iter()
            .filter(|block| {
                numeric_variable(block)
                    && block.state == State::Enabled
                    && !graph.ambiguous_names.contains(&block.name)
            })
            .take(MAX_VARIABLES)
            .collect();
        let bindings: Vec<_> = candidates
            .iter()
            .map(|block| Binding {
                name: &block.name,
                signal_type: SignalType::Analog(Dimension::SCALAR),
            })
            .collect();
        let expressions = candidates
            .into_iter()
            .map(|block| compile(block, "value", &bindings))
            .collect::<Vec<_>>();
        let values = resolve(&expressions);
        Self { bindings, values }
    }

    pub(super) fn setting(&self, block: &Block, key: &str) -> Option<f64> {
        let expression = compile(block, key, &self.bindings)?;
        evaluate(&expression, &self.values)
    }
}

fn numeric_variable(block: &Block) -> bool {
    match block.kind.as_str() {
        "variable" | "variable_qtgui_range" => true,
        "variable_qtgui_label" => {
            matches!(block.setting("type"), None | Some("int" | "real" | "raw"))
        }
        _ => false,
    }
}

fn compile(block: &Block, key: &str, bindings: &[Binding<'_>]) -> Option<Expression> {
    Expression::parse_scalar(block.setting(key)?, bindings).ok()
}

fn evaluate(expression: &Expression, values: &[Option<f64>]) -> Option<f64> {
    let Value::Numeric(value) = expression.evaluate(|index| values[index])? else {
        return None;
    };
    value.is_finite().then_some(value)
}

fn resolve(expressions: &[Option<Expression>]) -> Vec<Option<f64>> {
    let mut values = vec![None; expressions.len()];
    let mut dependants = vec![Vec::<usize>::new(); expressions.len()];
    let mut unmet = vec![0; expressions.len()];
    let mut inputs = Vec::new();
    for (index, expression) in expressions.iter().enumerate() {
        if let Some(expression) = expression {
            expression.visit_inputs(|input, _| inputs.push(input));
            inputs.sort_unstable();
            inputs.dedup();
            unmet[index] = inputs.len();
            for input in inputs.drain(..) {
                dependants[input].push(index);
            }
        }
    }
    let mut ready: VecDeque<_> = unmet
        .iter()
        .enumerate()
        .filter_map(|(index, &count)| (count == 0).then_some(index))
        .collect();
    while let Some(index) = ready.pop_front() {
        values[index] = expressions[index]
            .as_ref()
            .and_then(|node| evaluate(node, &values));
        for &next in &dependants[index] {
            unmet[next] -= 1;
            if unmet[next] == 0 {
                ready.push_back(next);
            }
        }
    }
    values
}
