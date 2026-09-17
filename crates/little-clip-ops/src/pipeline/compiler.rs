//! Pipeline graph compilation and structural validation.
//!
//! Compilation validates serialized graph data before execution can prompt the user,
//! launch a process, or write an artifact.

use super::catalog::NODE_CATALOG;
use super::executor::PipelineError;
use super::types::{Connection, NodeGraph, NodeInstance, NodeKind, Pin, PortType};
use std::collections::{HashMap, VecDeque};
use uuid::Uuid;

/// Determines whether interactive nodes may run after compilation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionPolicy {
    /// Allows desktop prompts and other interactive nodes.
    AllowInteractive,
    /// Rejects graphs that require a desktop prompt.
    NonInteractive,
}

/// A validated, topologically ordered view of a graph.
pub struct CompiledPipeline<'a> {
    nodes: Vec<&'a NodeInstance>,
    inputs: HashMap<(Uuid, String), &'a Connection>,
}

impl<'a> CompiledPipeline<'a> {
    /// Compiles a graph using the default interactive execution policy.
    ///
    /// # Errors
    ///
    /// Returns [`PipelineError`] if the graph contains cycles, duplicate node IDs,
    /// dangling connections, multiple connections to the same input pin, incompatible port types,
    /// missing required inputs, or unknown node kinds.
    pub fn compile(graph: &'a NodeGraph) -> Result<Self, PipelineError> {
        Self::compile_with_policy(graph, ExecutionPolicy::AllowInteractive)
    }

    /// Validates graph structure and produces an execution plan.
    ///
    /// # Errors
    ///
    /// Returns [`PipelineError`] if the graph fails structural validation or violates
    /// the supplied [`ExecutionPolicy`] (e.g. non-interactive policy rejecting prompt nodes).
    pub fn compile_with_policy(
        graph: &'a NodeGraph,
        policy: ExecutionPolicy,
    ) -> Result<Self, PipelineError> {
        let nodes = index_and_validate_nodes(graph, policy)?;
        let inputs = validate_connections(graph, &nodes)?;
        validate_required_inputs(&nodes, &inputs)?;
        let ordered_ids = topological_order(graph, &nodes)?;
        let nodes = ordered_ids.into_iter().map(|id| nodes[&id]).collect();
        Ok(Self { nodes, inputs })
    }

    /// Returns nodes in the order they may execute.
    pub fn nodes(&self) -> &[&'a NodeInstance] {
        &self.nodes
    }

    /// Returns the sole connection that supplies an input pin.
    pub fn input_for(&self, node_id: Uuid, pin: &str) -> Option<&'a Connection> {
        self.inputs.get(&(node_id, pin.to_owned())).copied()
    }
}

fn index_and_validate_nodes<'a>(
    graph: &'a NodeGraph,
    policy: ExecutionPolicy,
) -> Result<HashMap<Uuid, &'a NodeInstance>, PipelineError> {
    let mut nodes = HashMap::new();
    for node in &graph.nodes {
        if nodes.insert(node.id, node).is_some() {
            return Err(PipelineError::InvalidGraph(format!(
                "duplicate node id {}",
                node.id
            )));
        }
        if policy == ExecutionPolicy::NonInteractive
            && matches!(node.kind, NodeKind::PromptOrHandText)
        {
            return Err(PipelineError::InvalidGraph(format!(
                "interactive node '{}' is not allowed by this execution policy",
                node.title
            )));
        }
        let template = NODE_CATALOG
            .iter()
            .find(|template| template.kind == node.kind)
            .ok_or_else(|| {
                PipelineError::InvalidGraph(format!("unknown node kind: {:?}", node.kind))
            })?;
        if !matches_schema(&node.inputs, template.inputs)
            || !matches_schema(&node.outputs, template.outputs)
        {
            return Err(PipelineError::InvalidGraph(format!(
                "node '{}' does not match catalog pin schema",
                node.title
            )));
        }
    }
    Ok(nodes)
}

fn matches_schema(pins: &[Pin], expected: &[(&str, PortType)]) -> bool {
    pins.len() == expected.len()
        && pins
            .iter()
            .zip(expected)
            .all(|(pin, &(name, port_type))| pin.name == name && pin.port_type == port_type)
}

fn validate_connections<'a>(
    graph: &'a NodeGraph,
    nodes: &HashMap<Uuid, &'a NodeInstance>,
) -> Result<HashMap<(Uuid, String), &'a Connection>, PipelineError> {
    let mut destinations = HashMap::new();
    for connection in &graph.connections {
        let source = nodes.get(&connection.from_node).ok_or_else(|| {
            PipelineError::InvalidGraph(format!(
                "connection source node {} does not exist",
                connection.from_node
            ))
        })?;
        let target = nodes.get(&connection.to_node).ok_or_else(|| {
            PipelineError::InvalidGraph(format!(
                "connection target node {} does not exist",
                connection.to_node
            ))
        })?;
        let source_pin = source
            .outputs
            .iter()
            .find(|pin| pin.name == connection.from_pin)
            .ok_or_else(|| {
                PipelineError::InvalidGraph(format!(
                    "source pin '{}' does not exist on '{}'",
                    connection.from_pin, source.title
                ))
            })?;
        let target_pin = target
            .inputs
            .iter()
            .find(|pin| pin.name == connection.to_pin)
            .ok_or_else(|| {
                PipelineError::InvalidGraph(format!(
                    "target pin '{}' does not exist on '{}'",
                    connection.to_pin, target.title
                ))
            })?;
        if !source_pin
            .port_type
            .is_compatible_with(&target_pin.port_type)
        {
            return Err(PipelineError::InvalidGraph(format!(
                "cannot connect {} output '{}' to {} input '{}'",
                source.title, connection.from_pin, target.title, connection.to_pin
            )));
        }
        if destinations
            .insert((connection.to_node, connection.to_pin.clone()), connection)
            .is_some()
        {
            return Err(PipelineError::InvalidGraph(format!(
                "multiple connections target {} input '{}'",
                target.title, connection.to_pin
            )));
        }
    }
    Ok(destinations)
}

fn validate_required_inputs(
    nodes: &HashMap<Uuid, &NodeInstance>,
    connections: &HashMap<(Uuid, String), &Connection>,
) -> Result<(), PipelineError> {
    for node in nodes.values() {
        for pin in &node.inputs {
            if !connections.contains_key(&(node.id, pin.name.clone())) {
                return Err(PipelineError::MissingInput(format!(
                    "{} input '{}' is not connected",
                    node.title, pin.name
                )));
            }
        }
    }
    Ok(())
}

fn topological_order(
    graph: &NodeGraph,
    nodes: &HashMap<Uuid, &NodeInstance>,
) -> Result<Vec<Uuid>, PipelineError> {
    let mut in_degree: HashMap<Uuid, usize> = nodes.keys().map(|id| (*id, 0)).collect();
    let mut adjacent: HashMap<Uuid, Vec<Uuid>> = nodes.keys().map(|id| (*id, Vec::new())).collect();
    for connection in &graph.connections {
        *in_degree
            .get_mut(&connection.to_node)
            .expect("connections were validated") += 1;
        adjacent
            .get_mut(&connection.from_node)
            .expect("connections were validated")
            .push(connection.to_node);
    }
    let mut queue: VecDeque<Uuid> = in_degree
        .iter()
        .filter_map(|(&id, &degree)| (degree == 0).then_some(id))
        .collect();
    let mut ordered = Vec::with_capacity(nodes.len());
    while let Some(node_id) = queue.pop_front() {
        ordered.push(node_id);
        for neighbor in &adjacent[&node_id] {
            let degree = in_degree
                .get_mut(neighbor)
                .expect("adjacency references a node");
            *degree -= 1;
            if *degree == 0 {
                queue.push_back(*neighbor);
            }
        }
    }
    if ordered.len() != nodes.len() {
        return Err(PipelineError::CycleDetected);
    }
    Ok(ordered)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::catalog::{find_template, instantiate_template};

    #[test]
    fn compilation_produces_the_input_lookup_before_execution() {
        let source = instantiate_template(find_template("source_const_text").unwrap(), [0.0, 0.0]);
        let target = instantiate_template(find_template("text_trim").unwrap(), [100.0, 0.0]);
        let graph = NodeGraph {
            nodes: vec![source.clone(), target.clone()],
            connections: vec![Connection {
                from_node: source.id,
                from_pin: "Text".to_string(),
                to_node: target.id,
                to_pin: "In".to_string(),
            }],
            ..NodeGraph::default()
        };

        let compiled = CompiledPipeline::compile(&graph).unwrap();

        assert_eq!(compiled.nodes()[0].id, source.id);
        assert_eq!(
            compiled.input_for(target.id, "In").unwrap().from_node,
            source.id
        );
    }

    #[test]
    fn non_interactive_policy_rejects_prompt_nodes() {
        let prompt =
            instantiate_template(find_template("source_prompt_or_hand").unwrap(), [0.0, 0.0]);
        let graph = NodeGraph {
            nodes: vec![prompt],
            ..NodeGraph::default()
        };

        assert!(matches!(
            CompiledPipeline::compile_with_policy(&graph, ExecutionPolicy::NonInteractive),
            Err(PipelineError::InvalidGraph(_))
        ));
    }

    #[test]
    fn compilation_rejects_missing_required_inputs() {
        let trim = instantiate_template(find_template("text_trim").unwrap(), [0.0, 0.0]);
        let graph = NodeGraph {
            nodes: vec![trim],
            ..NodeGraph::default()
        };

        assert!(matches!(
            CompiledPipeline::compile(&graph),
            Err(PipelineError::MissingInput(_))
        ));
    }
}
