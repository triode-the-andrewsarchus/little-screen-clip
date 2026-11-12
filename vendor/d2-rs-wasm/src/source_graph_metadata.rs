// SPDX-License-Identifier: MPL-2.0
//! Compiled source graph metadata, distinct from the rendered target root.
use d2_rs_core::{Graph, LayoutResult};
use serde_json::{Value, json};

/// Formatting Graph.AST populates source-spelling caches on Go's shared AST
/// nodes. The Rust graph projection was serialized earlier, so copy those
/// caches back only to references to the same source range and value.
pub(super) fn apply_formatted_reference_spellings(parts: &mut GraphParts, ast: &Value) {
    fn collect(
        value: &Value,
        spellings: &mut std::collections::BTreeMap<(String, String), String>,
    ) {
        match value {
            Value::Object(object) => {
                if let Some(node) = object
                    .get("single_quoted_string")
                    .and_then(Value::as_object)
                    && let (Some(range), Some(text), Some(raw)) = (
                        node.get("range").and_then(Value::as_str),
                        node.get("value").and_then(Value::as_str),
                        node.get("raw").and_then(Value::as_str),
                    )
                {
                    spellings.insert((range.to_owned(), text.to_owned()), raw.to_owned());
                }
                for child in object.values() {
                    collect(child, spellings);
                }
            }
            Value::Array(array) => {
                for child in array {
                    collect(child, spellings);
                }
            }
            _ => {}
        }
    }
    fn apply(value: &mut Value, spellings: &std::collections::BTreeMap<(String, String), String>) {
        match value {
            Value::Object(object) => {
                if let Some(node) = object
                    .get_mut("single_quoted_string")
                    .and_then(Value::as_object_mut)
                    && let (Some(range), Some(text)) = (
                        node.get("range").and_then(Value::as_str),
                        node.get("value").and_then(Value::as_str),
                    )
                    && let Some(raw) = spellings.get(&(range.to_owned(), text.to_owned()))
                {
                    node.insert("raw".to_owned(), json!(raw));
                }
                for child in object.values_mut() {
                    apply(child, spellings);
                }
            }
            Value::Array(array) => {
                for child in array {
                    apply(child, spellings);
                }
            }
            _ => {}
        }
    }
    let mut spellings = std::collections::BTreeMap::new();
    collect(ast, &mut spellings);
    if let Some(objects) = parts.objects.as_array_mut() {
        for object in objects {
            if let Some(references) = object.get_mut("references") {
                apply(references, &spellings);
            }
        }
    }
}

pub(super) fn root(graph: &Graph, layout: &LayoutResult) -> Value {
    let mut root = graph.source_root.clone().unwrap_or(Value::Null);
    if !graph.nodes.is_empty() {
        // LayoutNested initializes a zero box with a nil TopLeft before the
        // backend. Specialized engines may commit a real root rectangle.
        let state = layout.synthetic_root.unwrap_or_default();
        root["box"] =
            json!({"TopLeft":state.top_left,"Width":state.size.width,"Height":state.size.height});
        if let Some(position) = state.label_position {
            root["labelPosition"] = json!(position.d2_name());
        }
        if let Some(position) = state.icon_position {
            root["iconPosition"] = json!(position.d2_name());
        }
    }
    root
}

pub(super) fn theme(id: i64) -> Option<Value> {
    let theme = d2_rs_core::theme::find(id)?;
    let p = d2_rs_core::theme::palette(id)?;
    let n = p.neutrals;
    let s = p.special_rules;
    Some(json!({"id":id,"name":theme.name,"colors":{"neutrals":{
        "n1":n.n1,"n2":n.n2,"n3":n.n3,"n4":n.n4,"n5":n.n5,"n6":n.n6,"n7":n.n7},
        "b1":p.b1,"b2":p.b2,"b3":p.b3,"b4":p.b4,"b5":p.b5,"b6":p.b6,
        "aa2":p.aa2,"aa4":p.aa4,"aa5":p.aa5,"ab4":p.ab4,"ab5":p.ab5},
        "specialRules":{"mono":s.mono,"noCornerRadius":s.no_corner_radius,
        "outerContainerDoubleBorder":s.outer_container_double_border,"containerDots":s.container_dots,
        "capsLock":s.caps_lock,"c4":s.c4,"allPaper":s.all_paper}}))
}

pub(super) fn objects(
    graph: &Graph,
    layout: &LayoutResult,
    labels: &std::collections::BTreeMap<d2_rs_core::NodeId, String>,
    document_root: bool,
) -> Result<Value, String> {
    use d2_rs_core::{LabelPosition, ShapeKind};
    let mut rows = Vec::new();
    for id in d2_rs_render::source_graph_object_order(graph) {
        let node = graph.node(id).ok_or("missing compiled object")?;
        let mut value = graph
            .source_metadata
            .as_ref()
            .and_then(|m| m.objects.get(&id))
            .ok_or_else(|| {
                format!(
                    "compiled object has no source metadata: {}",
                    node.external_id
                )
            })?
            .to_json();
        if let Some(label) = labels.get(&id) {
            value["attributes"]["label"]["value"] = json!(label);
        }
        let rect = layout.boxes.get(&id);
        let size = rect.map(|r| r.size).unwrap_or(node.size);
        value["box"] =
            json!({"TopLeft":rect.map(|r|r.origin),"Width":size.width,"Height":size.height});
        let state = layout.node_labels.get(&id);
        let span_scalars = d2_rs_render::source_graph_object_has_sequence_span_scalars(graph, id);
        if span_scalars {
            // sequence_diagram.go replaces the whole scalar records, leaving
            // the already measured label dimensions intact.
            value["attributes"]["label"] = json!({"value":""});
            value["attributes"]["shape"] = json!({"value":"Square"});
        } else if d2_rs_render::source_graph_object_has_sequence_note_shape(graph, id) {
            value["attributes"]["shape"] = json!({"value":"Page"});
        }
        let sequence_actor = node.parent.map_or(graph.root_sequence_diagram, |parent| {
            graph.is_sequence_diagram(parent)
        });
        let position = state
            .map(|s| s.position)
            .filter(|p| *p != LabelPosition::Unset)
            .unwrap_or(node.label_position);
        let has_label = !matches!(
            node.shape,
            ShapeKind::Text | ShapeKind::Code | ShapeKind::Class | ShapeKind::SqlTable
        ) && value["attributes"]["label"]["value"]
            .as_str()
            .is_some_and(|v| !v.is_empty());
        // LayoutNested restores the extracted grid root after laying out its
        // placeholder, so the placeholder's TALA font patch is discarded.
        let extracted_grid = (node.grid_rows.is_some() || node.grid_columns.is_some())
            && graph.nodes().any(|(_, child)| child.parent == Some(id));
        if let Some(state) = state.filter(|s| s.font_size_committed && !extracted_grid) {
            if let Some(font) = state.font_size {
                value["attributes"]["style"]["fontSize"] = json!({"value":font.to_string()});
            }
            if has_label {
                if let Some(size) = state.size {
                    value["attributes"]["labelDimensions"] =
                        json!({"width":size.width,"height":size.height});
                }
            }
        }
        // Clearing a span's text does not clear an authored position pointer.
        if (has_label
            || sequence_actor
            || (span_scalars && node.label_position != LabelPosition::Unset))
            && position != LabelPosition::Unset
        {
            value["labelPosition"] = json!(position.d2_name());
        }
        let icon_position = state
            .and_then(|s| s.icon_position)
            .or(node.icon_position)
            .filter(|p| *p != LabelPosition::Unset);
        if value["attributes"].get("icon").is_some() {
            if let Some(position) = icon_position {
                value["iconPosition"] = json!(position.d2_name());
            } else if node.shape == ShapeKind::Image && state.is_some_and(|s| s.font_size_committed)
            {
                // TALA's object patch commits even an unset image icon position.
                value["iconPosition"] = json!("");
            }
        }
        value["zIndex"] = json!(d2_rs_render::source_graph_object_z_index(graph, id));
        rows.push(value);
    }
    Ok(
        if rows.is_empty()
            && (document_root
                || !graph
                    .source_metadata
                    .as_ref()
                    .is_some_and(|m| m.objects_allocated))
        {
            Value::Null
        } else {
            json!(rows)
        },
    )
}

pub(super) fn edges(graph: &Graph, layout: &LayoutResult) -> Result<Value, String> {
    use d2_rs_core::LabelPosition;
    let mut rows = Vec::new();
    for (id, edge) in graph.edges() {
        // TALA's committed graph omits edges consumed by step sequences.
        if layout.removed_edges.contains(&id) {
            continue;
        }
        let mut value = graph
            .source_metadata
            .as_ref()
            .and_then(|m| m.edges.get(&id))
            .ok_or_else(|| format!("compiled edge has no source metadata: {id:?}"))?
            .clone();
        value["index"] = json!(
            graph
                .edge_index(id)
                .ok_or("compiled edge has no source index")?
        );
        let arrows = graph
            .edge_arrows
            .get(id.0 as usize)
            .copied()
            .unwrap_or_default();
        value["src_arrow"] = json!(arrows.source);
        value["dst_arrow"] = json!(arrows.target);
        value["isCurve"] = json!(layout.route_is_curve(graph, id));
        if let Some(route) = layout.routes.get(&id).filter(|r| !r.is_empty()) {
            value["route"] = json!(route);
        }
        let label = layout.edge_labels.get(&id).or_else(|| graph.edge_label(id));
        // SetDimensions mutates Label.Value (ApplyTextTransform). The compiler
        // projection retains the original scalar for its pre-measurement API.
        if let Some(label) = label {
            value["attributes"]["label"]["value"] = json!(label.text);
        }
        if let Some(label) = label.filter(|l| l.position != LabelPosition::Unset) {
            value["labelPosition"] = json!(label.position.d2_name());
        }
        let authored = graph
            .d2_edge_validation
            .get(id.0 as usize)
            .and_then(Option::as_ref)
            .and_then(|v| v.label_percentage);
        let percentage = if layout.edge_label_percentage_commits.contains(&id) {
            label.map(|v| v.percentage)
        } else {
            authored
        };
        if let Some(percentage) = percentage {
            value["labelPercentage"] = json!(percentage);
        }
        let columns = graph
            .edge_table_columns
            .get(id.0 as usize)
            .copied()
            .unwrap_or_default();
        if let Some(column) = columns.source {
            value["srcTableColumnIndex"] = json!(column);
        }
        if let Some(column) = columns.target {
            value["dstTableColumnIndex"] = json!(column);
        }
        value["zIndex"] = json!(d2_rs_render::source_graph_edge_z_index(graph, edge));
        rows.push(value);
    }
    for line in &layout.sequence_lifelines {
        let actor = graph
            .source_metadata
            .as_ref()
            .and_then(|m| m.objects.get(&line.actor))
            .ok_or("lifeline has no source actor")?;
        let mut style = json!({"strokeWidth":{"value":"2"},"strokeDash":{"value":"6"}});
        for name in ["stroke", "strokeDash"] {
            if let Some(value) = actor.value["attributes"]["style"].get(name) {
                style[name] = value.clone();
            }
        }
        rows.push(json!({"index":0,"isCurve":false,"route":line.route,"src_arrow":false,"dst_arrow":false,
            "attributes":{"label":{"value":""},"labelDimensions":{"width":0,"height":0},"style":style,
            "iconStyle":{},"near_key":null,"shape":{"value":""},"direction":{"value":""},"constraint":null},"zIndex":1}));
    }
    Ok(
        if rows.is_empty() && layout.source_edge_storage != d2_rs_core::SourceEdgeStorage::Allocated
        {
            Value::Null
        } else {
            json!(rows)
        },
    )
}

fn legend(graph: &Graph) -> Result<Option<Value>, String> {
    let Some(legend) = &graph.legend else {
        return Ok(None);
    };
    let Some(source) = &legend.source_graph else {
        return Err("compiled legend has no source graph".into());
    };
    // compileLegend never invokes SetDimensions or layout: source objects
    // use fixed 100x100 boxes and all edges use a two-point horizontal route.
    let mut layout = LayoutResult {
        removed_edges: Default::default(),
        source_edge_storage: Default::default(),
        synthetic_root: None,
        route_curves: Default::default(),
        sequence_lifelines: Vec::new(),
        boxes: Default::default(),
        routes: Default::default(),
        edge_labels: Default::default(),
        edge_label_percentage_commits: Default::default(),
        node_labels: Default::default(),
        report: d2_rs_core::LayoutReport {
            selected_seed: 0,
            selected_pass: 0,
            score: Default::default(),
            improvement_passes: 0,
            candidates: Vec::new(),
            warnings: Vec::new(),
        },
    };
    for (id, _) in source.graph.nodes() {
        layout.boxes.insert(
            id,
            d2_rs_core::Rect {
                origin: d2_rs_core::Point { x: 10.0, y: 10.0 },
                size: d2_rs_core::Size {
                    width: 100.0,
                    height: 100.0,
                },
            },
        );
    }
    for (id, _) in source.graph.edges() {
        layout.routes.insert(
            id,
            vec![
                d2_rs_core::Point { x: 10.0, y: 10.0 },
                d2_rs_core::Point { x: 110.0, y: 10.0 },
            ],
        );
    }
    let visible: std::collections::BTreeSet<_> =
        legend.shapes.iter().filter_map(|s| s.source_node).collect();
    let ids = d2_rs_render::source_graph_object_order(&source.graph);
    let all_objects = objects(&source.graph, &layout, &source.labels, false)?;
    let objects: Vec<_> = all_objects
        .as_array()
        .into_iter()
        .flatten()
        .zip(ids)
        .filter(|(_, id)| visible.contains(id))
        .map(|(v, _)| {
            let mut v = v.clone();
            v["zIndex"] = json!(0);
            v
        })
        .collect();
    let all_edges = edges(&source.graph, &layout)?;
    let edges: Vec<_> = all_edges
        .as_array()
        .into_iter()
        .flatten()
        .map(|v| {
            let mut v = v.clone();
            v["zIndex"] = json!(0);
            v
        })
        .collect();
    let mut value = json!({});
    if !legend.label.is_empty() {
        value["label"] = json!(legend.label);
    }
    if !objects.is_empty() {
        value["objects"] = json!(objects);
    }
    if !edges.is_empty() {
        value["edges"] = json!(edges);
    }
    Ok(Some(value))
}

/// The computed parts of `Graph`'s source projection. The caller serializes
/// them between its shared name, AST, root, theme and data fields.
pub(super) struct GraphParts {
    pub(super) objects: Value,
    pub(super) edges: Value,
    pub(super) legend: Option<Value>,
}

pub(super) fn graph(
    graph: &Graph,
    layout: &LayoutResult,
    labels: &std::collections::BTreeMap<d2_rs_core::NodeId, String>,
    document_root: bool,
) -> Result<GraphParts, String> {
    Ok(GraphParts {
        objects: objects(graph, layout, labels, document_root)?,
        edges: edges(graph, layout)?,
        legend: legend(graph)?,
    })
}
