//! Parser-backed, source-preserving graphical edits for Mermaid flowcharts.
//!
//! Merman owns grammar, semantic models and original-source spans. The editor
//! changes authored byte ranges or appends ordinary Mermaid statements, then
//! reparses and verifies the complete semantic delta before publishing text.
//! Other Mermaid families and unproved compound removals remain source-only.
use merman_core::{
    EditorRenamePolicy, EditorSemanticCompleteness, EditorSemanticFacts, EditorSemanticRole,
    ParsedEditorFacts, SemanticParseOutcome, SemanticParser, SemanticSnapshotCapture,
};
use merman_model::{
    families::flowchart::FlowchartModel, operation::OperationControl,
    semantic::SemanticDiagramModel,
};
use serde_json::Value;
use std::{collections::HashMap, ops::Range, sync::Arc, time::Duration};

const MAX_SOURCE: usize = 256 * 1024;

/// One source-authored flowchart node, excluding synthetic subgraph anchors.
#[derive(Clone, Debug)]
pub struct Node {
    pub id: String,
    pub label: String,
    pub shape: String,
}

/// One canonical relationship, in the parser's authored order.
#[derive(Clone, Debug)]
pub struct Edge {
    pub id: String,
    pub from: String,
    pub to: String,
    pub label: String,
}

/// Stable semantic selection within one source revision.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    Node(String),
    Edge(String),
}

/// Basic source-authored shapes available to the initial add-node tool.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Shape {
    #[default]
    Box,
    Rounded,
    Decision,
}
impl Shape {
    pub fn name(self) -> &'static str {
        match self {
            Self::Box => "Box",
            Self::Rounded => "Rounded",
            Self::Decision => "Decision",
        }
    }
    fn delimiters(self) -> (&'static str, &'static str) {
        match self {
            Self::Box => ("[", "]"),
            Self::Rounded => ("(", ")"),
            Self::Decision => ("{", "}"),
        }
    }
}

/// A single graphical gesture; it never owns file writes or editor undo.
#[derive(Clone, Debug)]
pub enum Change {
    NodeLabel {
        id: String,
        label: String,
    },
    EdgeLabel {
        id: String,
        label: String,
    },
    AddNode {
        id: String,
        label: String,
        shape: Shape,
    },
    Connect {
        from: String,
        to: String,
        label: String,
    },
    Delete(Target),
}

#[derive(Clone, Debug)]
struct Label {
    text: String,
    span: Range<usize>,
    selection: Range<usize>,
}
#[derive(Clone, Debug)]
struct Occurrence {
    id: String,
    span: Range<usize>,
    label: Option<Label>,
}

/// One immutable source generation and its authoritative native parser facts.
#[derive(Clone, Debug)]
pub struct Editable {
    source: Arc<str>,
    flow: FlowchartModel,
    occurrences: Vec<Occurrence>,
    labels: Vec<(String, Label)>,
    references: HashMap<String, usize>,
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
}
impl Editable {
    /// Parses on a bounded worker; no resources, themes, layout or files resolve.
    pub fn parse(source: &str) -> Result<Self, String> {
        let (flow, facts) = parse(source)?;
        let (occurrences, labels, references) = source_facts(source, &facts)?;
        let nodes = flow
            .nodes
            .iter()
            .filter(|node| {
                !node.is_subgraph_anchor()
                    && !flow.subgraphs.iter().any(|group| group.id == node.id)
            })
            .map(|node| Node {
                id: node.id.clone(),
                label: node.label.clone().unwrap_or_else(|| node.id.clone()),
                shape: node
                    .shape
                    .clone()
                    .or_else(|| node.layout_shape.clone())
                    .unwrap_or_else(|| "box".into()),
            })
            .collect();
        let edges = flow
            .edges
            .iter()
            .map(|edge| Edge {
                id: edge.id.clone(),
                from: edge.from.clone(),
                to: edge.to.clone(),
                label: edge.label.clone().unwrap_or_default(),
            })
            .collect();
        Ok(Self {
            source: Arc::from(source),
            flow,
            occurrences,
            labels,
            references,
            nodes,
            edges,
        })
    }

    /// Generates an unused plain identifier accepted by the actual Flowchart grammar.
    pub fn next_id(&self) -> String {
        (1..)
            .map(|index| format!("node_{index}"))
            .find(|id| {
                !self.flow.nodes.iter().any(|node| &node.id == id)
                    && !self.flow.subgraphs.iter().any(|group| &group.id == id)
            })
            .unwrap()
    }

    /// Returns whether the initial inspector can edit this independently sourced label.
    pub fn label_editable(&self, target: &Target) -> bool {
        match target {
            Target::Node(id) => self.node_index(id).ok().is_some_and(|index| {
                self.flow.nodes[index].label_type.as_deref() != Some("markdown")
            }),
            Target::Edge(id) => self.edge_index(id).ok().is_some_and(|index| {
                let edge = &self.flow.edges[index];
                edge.label_type.as_deref() != Some("markdown")
                    && edge.label.as_deref().is_some_and(|label| {
                        self.labels.iter().filter(|(text, _)| text == label).count() == 1
                            && self
                                .flow
                                .edges
                                .iter()
                                .filter(|item| item.label.as_deref() == Some(label))
                                .count()
                                == 1
                    })
            }),
        }
    }

    /// Checks a removal on a worker, including reparse and complete semantic equality.
    pub fn can_delete(&self, target: &Target) -> bool {
        self.apply(&self.source, Change::Delete(target.clone()))
            .is_ok()
    }

    /// Applies a gesture only to the exact captured source, preserving every other byte.
    pub fn apply(&self, current: &str, change: Change) -> Result<String, String> {
        if current != self.source.as_ref() {
            return Err("Mermaid source changed; the graphical edit was not applied".into());
        }
        let next = self.patch(&change)?;
        let (after, _) = parse(&next)?;
        self.verify(&after, &change)?;
        Ok(next)
    }

    fn patch(&self, change: &Change) -> Result<String, String> {
        match change {
            Change::NodeLabel { id, label } => self.node_label(id, label),
            Change::EdgeLabel { id, label } => self.edge_label(id, label),
            Change::AddNode { id, label, shape } => self.add_node(id, label, *shape),
            Change::Connect { from, to, label } => self.connect(from, to, label),
            Change::Delete(target) => {
                let range = self.deletion_range(target)?;
                let mut next = self.source.to_string();
                next.replace_range(range, "");
                Ok(next)
            }
        }
    }

    fn node_label(&self, id: &str, label: &str) -> Result<String, String> {
        let index = self.node_index(id)?;
        let quoted = quote(label)?;
        let occurrence = self.occurrences.iter().rev().find(|item| {
            item.id == id
                && item.label.as_ref().is_some_and(|label| {
                    self.flow.nodes[index].label.as_deref() == Some(label.text.as_str())
                })
        });
        if let Some(found) = occurrence.and_then(|item| item.label.as_ref()) {
            replace_label(&self.source, found, &quoted)
        } else {
            identifier(id)?;
            Ok(self.append(&format!("{id}@{{ label: {quoted} }};")))
        }
    }

    fn edge_label(&self, id: &str, label: &str) -> Result<String, String> {
        let edge = &self.flow.edges[self.edge_index(id)?];
        let old = edge.label.as_deref().ok_or(
            "This edge has no source label; edit it in Source or add a labeled connection",
        )?;
        let matches: Vec<_> = self.labels.iter().filter(|(text, _)| text == old).collect();
        if matches.len() != 1
            || self
                .flow
                .edges
                .iter()
                .filter(|item| item.label.as_deref() == Some(old))
                .count()
                != 1
        {
            return Err("This edge label is shared or ambiguous; edit it in Source".into());
        }
        replace_label(&self.source, &matches[0].1, &quote(label)?)
    }

    fn add_node(&self, id: &str, label: &str, shape: Shape) -> Result<String, String> {
        identifier(id)?;
        if self.flow.nodes.iter().any(|node| node.id == id)
            || self.flow.subgraphs.iter().any(|group| group.id == id)
        {
            return Err("A Mermaid node or subgraph already uses that ID".into());
        }
        let (open, close) = shape.delimiters();
        Ok(self.append(&format!("{id}{open}{}{close};", quote(label)?)))
    }

    fn connect(&self, from: &str, to: &str, label: &str) -> Result<String, String> {
        self.node_index(from)?;
        self.node_index(to)?;
        identifier(from)?;
        identifier(to)?;
        if self
            .edges
            .iter()
            .any(|edge| edge.from == from && edge.to == to && edge.label == label)
        {
            return Err("That connection already exists".into());
        }
        let label = if label.is_empty() {
            String::new()
        } else {
            format!("|{}|", quote(label)?)
        };
        Ok(self.append(&format!("{from} -->{label} {to};")))
    }

    fn append(&self, statement: &str) -> String {
        let newline = if self.source.contains("\r\n") {
            "\r\n"
        } else {
            "\n"
        };
        let separator = if self.source.ends_with(['\n', '\r']) {
            ""
        } else {
            newline
        };
        format!("{}{separator}{statement}{newline}", self.source)
    }

    fn node_index(&self, id: &str) -> Result<usize, String> {
        self.flow
            .nodes
            .iter()
            .position(|node| {
                node.id == id
                    && !node.is_subgraph_anchor()
                    && !self.flow.subgraphs.iter().any(|group| group.id == node.id)
            })
            .ok_or_else(|| format!("Unknown Mermaid node: {id}"))
    }
    fn edge_index(&self, id: &str) -> Result<usize, String> {
        self.flow
            .edges
            .iter()
            .position(|edge| edge.id == id)
            .ok_or_else(|| format!("Unknown Mermaid edge: {id}"))
    }

    fn deletion_range(&self, target: &Target) -> Result<Range<usize>, String> {
        match target {
            Target::Node(id) => {
                self.node_index(id)?;
                if self
                    .edges
                    .iter()
                    .any(|edge| edge.from == *id || edge.to == *id)
                    || self.references.get(id) != Some(&1)
                {
                    return Err(
                        "Connected or multiply referenced nodes must be removed in Source".into(),
                    );
                }
                let occurrence = self
                    .occurrences
                    .iter()
                    .find(|item| item.id == *id)
                    .ok_or("Node has no editable source occurrence")?;
                self.standalone_range(
                    occurrence.span.start,
                    occurrence
                        .label
                        .as_ref()
                        .map_or(occurrence.span.end, |label| label.span.end),
                )
            }
            Target::Edge(id) => self.edge_deletion_range(id),
        }
    }

    fn edge_deletion_range(&self, id: &str) -> Result<Range<usize>, String> {
        let edge = &self.flow.edges[self.edge_index(id)?];
        let signatures = self
            .edges
            .iter()
            .filter(|item| {
                item.from == edge.from
                    && item.to == edge.to
                    && item.label == edge.label.as_deref().unwrap_or_default()
            })
            .count();
        if signatures != 1 {
            return Err("Parallel ambiguous edges must be removed in Source".into());
        }
        for pair in self.occurrences.windows(2) {
            let [from, to] = pair else { continue };
            if from.id != edge.from
                || to.id != edge.to
                || from.label.is_some()
                || to.label.is_some()
            {
                continue;
            }
            let Ok(range) = self.standalone_range(from.span.start, to.span.end) else {
                continue;
            };
            let fragment = format!("flowchart TD\n{}\n", &self.source[range.clone()]);
            let (parsed, _) = parse(&fragment)?;
            if parsed.edges.len() == 1
                && parsed.nodes.len() == 2
                && parsed.edges[0].from == edge.from
                && parsed.edges[0].to == edge.to
            {
                return Ok(range);
            }
        }
        Err("Compound edges or inline node definitions must be removed in Source".into())
    }

    fn standalone_range(&self, start: usize, end: usize) -> Result<Range<usize>, String> {
        if self.source[start..end].contains(['\n', '\r']) {
            return Err("Multiline declarations must be removed in Source".into());
        }
        let line_start = self.source[..start]
            .rfind(['\n', '\r'])
            .map_or(0, |index| index + 1);
        let line_end = self.source[end..]
            .find(['\n', '\r'])
            .map_or(self.source.len(), |index| end + index);
        if !self.source[line_start..start].trim().is_empty() {
            return Err("This declaration shares a statement; edit it in Source".into());
        }
        let suffix = &self.source[end..line_end];
        let code = suffix.split_once("%%").map_or(suffix, |(code, _)| code);
        if !matches!(code.trim(), "" | ";") {
            return Err("This declaration shares a statement; edit it in Source".into());
        }
        Ok(start..end + code.len())
    }

    fn verify(&self, after: &FlowchartModel, change: &Change) -> Result<(), String> {
        let mut before = meaning(&self.flow)?;
        let mut next = meaning(after)?;
        match change {
            Change::NodeLabel { .. } | Change::EdgeLabel { .. } => {
                self.verify_label(after, change, &mut before, &mut next)?
            }
            Change::AddNode { .. } | Change::Connect { .. } => {
                self.verify_addition(after, change, &mut next)?
            }
            Change::Delete(Target::Node(id)) => {
                before["nodes"]
                    .as_array_mut()
                    .ok_or("Missing node model")?
                    .remove(self.node_index(id)?);
            }
            Change::Delete(Target::Edge(id)) => {
                before["edges"]
                    .as_array_mut()
                    .ok_or("Missing edge model")?
                    .remove(self.edge_index(id)?);
            }
        }
        if before != next {
            return Err("This gesture would change other labels, styles, groups or relationships; edit it in Source".into());
        }
        Ok(())
    }

    fn verify_label(
        &self,
        after: &FlowchartModel,
        change: &Change,
        before: &mut Value,
        next: &mut Value,
    ) -> Result<(), String> {
        match change {
            Change::NodeLabel { id, label } => {
                let old = self.node_index(id)?;
                let new = after
                    .nodes
                    .iter()
                    .position(|node| node.id == *id)
                    .ok_or("Edited node disappeared")?;
                if after.nodes[new].label != canonical_label(label, false)? {
                    return Err(
                        "A later declaration overrides this label; edit it in Source".into(),
                    );
                }
                remove_label(&mut before["nodes"][old]);
                remove_label(&mut next["nodes"][new]);
            }
            Change::EdgeLabel { id, label } => {
                let index = self.edge_index(id)?;
                if after.edges.get(index).and_then(|edge| edge.label.clone())
                    != canonical_label(label, true)?
                {
                    return Err("The parser could not apply this edge label independently".into());
                }
                remove_label(&mut before["edges"][index]);
                remove_label(&mut next["edges"][index]);
            }
            _ => return Err("Expected a Mermaid label gesture".into()),
        }
        Ok(())
    }

    fn verify_addition(
        &self,
        after: &FlowchartModel,
        change: &Change,
        next: &mut Value,
    ) -> Result<(), String> {
        match change {
            Change::AddNode { id, .. } => {
                if after.nodes.len() != self.flow.nodes.len() + 1 {
                    return Err("Adding this node changed other flowchart declarations".into());
                }
                let index = after
                    .nodes
                    .iter()
                    .position(|node| node.id == *id)
                    .ok_or("New node was not parsed")?;
                next["nodes"]
                    .as_array_mut()
                    .ok_or("Missing node model")?
                    .remove(index);
            }
            Change::Connect { from, to, .. } => {
                if after.edges.len() != self.flow.edges.len() + 1
                    || after
                        .edges
                        .last()
                        .is_none_or(|edge| edge.from != *from || edge.to != *to)
                {
                    return Err("The new connection changed other flowchart relationships".into());
                }
                next["edges"]
                    .as_array_mut()
                    .ok_or("Missing edge model")?
                    .pop();
            }
            _ => return Err("Expected a Mermaid addition gesture".into()),
        }
        Ok(())
    }
}

fn parse(source: &str) -> Result<(FlowchartModel, EditorSemanticFacts), String> {
    if source.len() > MAX_SOURCE || source.contains('\0') {
        return Err("Mermaid editing source exceeds 256 KiB or contains NUL".into());
    }
    let control = OperationControl::new().with_deadline(Duration::from_secs(1));
    let capture = SemanticParser::new()
        .capture_diagram_snapshot_controlled_sync(source, &control)
        .map_err(|error| error.to_string())?;
    let SemanticSnapshotCapture::Snapshot(snapshot) = capture else {
        return Err("Mermaid source could not be parsed for editing".into());
    };
    let (metadata, outcome, facts) = snapshot.into_parts();
    if !matches!(metadata.diagram_type.as_str(), "flowchart-v2" | "flowchart") {
        return Err(
            "Graphical editing currently supports Mermaid flowcharts; edit this family in Source"
                .into(),
        );
    }
    let flow = match outcome {
        SemanticParseOutcome::Parsed(SemanticDiagramModel::Flowchart(flow)) => flow,
        SemanticParseOutcome::Failed(error) => return Err(error.to_string()),
        _ => return Err("Mermaid flowchart could not be parsed for editing".into()),
    };
    let ParsedEditorFacts::Available(facts) = facts else {
        return Err("Flowchart source correspondence is unavailable".into());
    };
    if facts.completeness != EditorSemanticCompleteness::Complete
        || flow.nodes.len() > 512
        || flow.edges.len() > 1024
        || flow.nodes.iter().any(|node| {
            node.id.len() > 256 || node.label.as_ref().is_some_and(|label| label.len() > 4096)
        })
        || flow.edges.iter().any(|edge| {
            edge.id.len() > 512 || edge.label.as_ref().is_some_and(|label| label.len() > 4096)
        })
        || facts.symbols.len() > 16384
    {
        return Err("Flowchart is incomplete or exceeds the graphical editing budget".into());
    }
    Ok((flow, facts))
}

type SourceFacts = (
    Vec<Occurrence>,
    Vec<(String, Label)>,
    HashMap<String, usize>,
);
fn source_facts(source: &str, facts: &EditorSemanticFacts) -> Result<SourceFacts, String> {
    let mut occurrences: Vec<Occurrence> = Vec::new();
    let mut labels = Vec::new();
    let mut references = HashMap::new();
    let mut last_node = None;
    for fact in &facts.symbols {
        let span = fact.span.start..fact.span.end;
        let selection = fact.selection.start..fact.selection.end;
        if source.get(span.clone()).is_none()
            || source.get(selection.clone()).is_none()
            || span.start > selection.start
            || selection.end > span.end
        {
            return Err("Invalid Mermaid parser source correspondence".into());
        }
        if matches!(
            fact.role,
            EditorSemanticRole::Entity | EditorSemanticRole::Reference
        ) && fact.rename_policy == EditorRenamePolicy::FlowchartNodeId
        {
            *references.entry(fact.name.clone()).or_insert(0) += 1;
        }
        match fact.detail.as_deref() {
            Some("flowchart node") => {
                if source.get(selection.clone()) == Some(fact.name.as_str()) {
                    occurrences.push(Occurrence {
                        id: fact.name.clone(),
                        span: selection,
                        label: None,
                    });
                    last_node = Some(occurrences.len() - 1);
                }
            }
            Some("flowchart node label") => {
                if let Some(index) = last_node.take() {
                    occurrences[index].label = Some(Label {
                        text: fact.name.clone(),
                        span,
                        selection,
                    });
                }
            }
            Some("flowchart edge label") => {
                labels.push((
                    fact.name.clone(),
                    Label {
                        text: fact.name.clone(),
                        span,
                        selection,
                    },
                ));
                last_node = None;
            }
            _ => last_node = None,
        }
    }
    Ok((occurrences, labels, references))
}

fn identifier(id: &str) -> Result<(), String> {
    if EditorRenamePolicy::FlowchartNodeId.accepts(id) {
        Ok(())
    } else {
        Err("This ID needs source editing; use a plain Mermaid node identifier".into())
    }
}
fn quote(text: &str) -> Result<String, String> {
    if text.trim().is_empty() || text.len() > 4096 || text.contains('\0') {
        return Err("Labels require between one and 4096 UTF-8 bytes without NUL".into());
    }
    let mut encoded = String::from("\"");
    for ch in text.chars() {
        match ch {
            '"' | '#' | '<' | '>' | '&' | '`' => encoded.push_str(&format!("#{};", u32::from(ch))),
            _ => encoded.push(ch),
        }
    }
    encoded.push('"');
    Ok(encoded)
}
fn canonical_label(text: &str, edge: bool) -> Result<Option<String>, String> {
    let label = quote(text)?;
    let statement = if edge {
        format!("a -->|{label}| b")
    } else {
        format!("a[{label}]")
    };
    let (flow, _) = parse(&format!("flowchart TD\n{statement}\n"))?;
    Ok(if edge {
        flow.edges.first().and_then(|edge| edge.label.clone())
    } else {
        flow.nodes.first().and_then(|node| node.label.clone())
    })
}
fn replace_label(source: &str, label: &Label, quoted: &str) -> Result<String, String> {
    let prefix = &source[label.span.start..label.selection.start];
    let suffix = &source[label.selection.end..label.span.end];
    let (range, replacement) = if prefix.ends_with('"') && suffix.starts_with('"') {
        (label.selection.clone(), &quoted[1..quoted.len() - 1])
    } else {
        let leading = prefix.len() - prefix.trim_end().len();
        let trailing = suffix.len() - suffix.trim_start().len();
        (
            label.selection.start - leading..label.selection.end + trailing,
            quoted,
        )
    };
    let mut next = source.to_owned();
    next.replace_range(range, replacement);
    Ok(next)
}
fn meaning(flow: &FlowchartModel) -> Result<Value, String> {
    let mut value = serde_json::to_value(flow).map_err(|error| error.to_string())?;
    if let Some(object) = value.as_object_mut() {
        object.remove("warningFacts");
        object.remove("vertexCalls");
    }
    if let Some(edges) = value["edges"].as_array_mut() {
        for edge in edges {
            if edge["isUserDefinedId"] == false
                && let Some(object) = edge.as_object_mut()
            {
                object.remove("id");
            }
        }
    }
    Ok(value)
}
fn remove_label(value: &mut Value) {
    if let Some(object) = value.as_object_mut() {
        object.remove("label");
        object.remove("labelType");
    }
}

/// Renderer/editor correspondence without treating authored IDs as SVG identifiers.
pub fn selection_id(kind: &str, id: &str) -> String {
    use std::fmt::Write;
    let mut result = format!("clip-mermaid-{kind}-");
    for byte in id.bytes() {
        let _ = write!(result, "{byte:02x}");
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parser_spans_preserve_bom_crlf_comments_styles_ids_and_other_shapes() {
        let source = "\u{feff}---\r\ntitle: Kept title\r\n---\r\nflowchart LR\r\n  %% A[Old] is only a comment\r\n  A[Old] -->|keep| B{Decide}\r\n  %% keep this comment\r\n  classDef accent fill:#f0f0f0\r\n  class A accent\r\n";
        let model = Editable::parse(source).unwrap();
        let edited = model
            .apply(
                source,
                Change::NodeLabel {
                    id: "A".into(),
                    label: "新しい label".into(),
                },
            )
            .unwrap();
        let expected = source.replace("  A[Old] -->", "  A[\"新しい label\"] -->");
        assert_eq!(edited, expected);
        assert_eq!(Editable::parse(&edited).unwrap().edges[0].label, "keep");
    }

    #[test]
    fn quoted_unicode_labels_and_generated_connections_stay_data() {
        let source = "flowchart TD\nA[Start]\nB[Finish]\n";
        let model = Editable::parse(source).unwrap();
        let edited = model
            .apply(
                source,
                Change::NodeLabel {
                    id: "A".into(),
                    label: "Use \"quotes\", [brackets] & <tags> #1".into(),
                },
            )
            .unwrap();
        assert!(edited.contains("#34;quotes#34;"));
        assert!(edited.contains("#38; #60;tags#62; #35;1"));
        let model = Editable::parse(&edited).unwrap();
        let connected = model
            .apply(
                &edited,
                Change::Connect {
                    from: "A".into(),
                    to: "B".into(),
                    label: "ready?".into(),
                },
            )
            .unwrap();
        assert!(connected.starts_with(&edited));
        let model = Editable::parse(&connected).unwrap();
        assert_eq!(model.edges.len(), 1);
        assert_eq!(model.edges[0].label, "ready?");
        let changed = model
            .apply(
                &connected,
                Change::EdgeLabel {
                    id: model.edges[0].id.clone(),
                    label: "now".into(),
                },
            )
            .unwrap();
        assert_eq!(Editable::parse(&changed).unwrap().edges[0].label, "now");
    }

    #[test]
    fn add_node_retains_existing_source_and_uses_actual_identifier_admission() {
        let source = "flowchart LR\r\n%% keep\r\nA[Old]\r\n";
        let model = Editable::parse(source).unwrap();
        let added = model
            .apply(
                source,
                Change::AddNode {
                    id: model.next_id(),
                    label: "Decide".into(),
                    shape: Shape::Decision,
                },
            )
            .unwrap();
        assert_eq!(added, format!("{source}node_1{{\"Decide\"}};\r\n"));
        assert_eq!(Editable::parse(&added).unwrap().nodes.len(), 2);
        assert!(
            model
                .apply(
                    source,
                    Change::AddNode {
                        id: "A".into(),
                        label: "duplicate".into(),
                        shape: Shape::Box
                    }
                )
                .is_err()
        );
        assert!(
            model
                .apply(
                    source,
                    Change::Connect {
                        from: "A".into(),
                        to: "missing".into(),
                        label: String::new()
                    }
                )
                .is_err()
        );
    }

    #[test]
    fn safe_removal_preserves_comments_and_rejects_connected_or_compound_nodes() {
        let source = "flowchart LR\n  A[Keep]\n  B[Remove]\n  %% keep comment\n";
        let model = Editable::parse(source).unwrap();
        let target = Target::Node("B".into());
        assert!(model.can_delete(&target));
        assert_eq!(
            model.apply(source, Change::Delete(target)).unwrap(),
            "flowchart LR\n  A[Keep]\n  \n  %% keep comment\n"
        );
        for source in [
            "flowchart LR\nA[Keep] --> B[Remove]\n",
            "flowchart LR\nA[Keep]; B[Remove]\n",
            "flowchart LR\nB[Remove]\nstyle B fill:red\n",
        ] {
            let model = Editable::parse(source).unwrap();
            assert!(!model.can_delete(&Target::Node("B".into())), "{source}");
        }
    }

    #[test]
    fn edge_removal_keeps_declared_nodes_and_cannot_retarget_indexed_styles() {
        let source = "flowchart LR\nA[One]\nB[Two]\nA --> B\n%% preserve\n";
        let model = Editable::parse(source).unwrap();
        let target = Target::Edge(model.edges[0].id.clone());
        assert!(model.can_delete(&target));
        let removed = model.apply(source, Change::Delete(target)).unwrap();
        let after = Editable::parse(&removed).unwrap();
        assert_eq!(after.nodes.len(), 2);
        assert!(after.edges.is_empty());
        assert!(removed.contains("%% preserve"));
        for source in [
            "flowchart LR\nA --> B --> C\n",
            "flowchart LR\nA[One]\nB[Two]\nC[Three]\nA --> B\nB --> C\nlinkStyle 1 stroke:red\n",
        ] {
            let model = Editable::parse(source).unwrap();
            assert!(
                !model.can_delete(&Target::Edge(model.edges[0].id.clone())),
                "{source}"
            );
        }
    }

    #[test]
    fn unsupported_families_stale_sources_and_incomplete_parses_never_authorize_edits() {
        assert!(
            Editable::parse("sequenceDiagram\nA->>B: Hello\n")
                .unwrap_err()
                .contains("flowcharts")
        );
        assert!(Editable::parse("flowchart TD\nA[unfinished").is_err());
        let source = "flowchart LR\nA[Label]\n";
        let model = Editable::parse(source).unwrap();
        assert!(
            model
                .apply(
                    &format!("{source}%% newer\n"),
                    Change::NodeLabel {
                        id: "A".into(),
                        label: "Lost".into()
                    }
                )
                .unwrap_err()
                .contains("source changed")
        );
        assert_eq!(selection_id("node", "A<&"), "clip-mermaid-node-413c26");
    }
}
