//! Wiring transactions preserve authored selectors and prove a link multiset.
use super::*;
use connections::Step;
use oneyaml::{NodeId, NodeKind, NodeRef, Sequence};
use std::collections::HashMap;

const SOURCE_EDIT: &str = "Cannot preserve this wiring edit; use the source editor";

pub(super) fn apply(snapshot: &Editable, current: &str, edit: Edit) -> Result<String, String> {
    let (mut yaml, value) = document(current)?;
    if yaml.to_string() != current {
        return Err(SOURCE_EDIT.into());
    }
    let before_comments = comments(&yaml)?;
    let mut expected = snapshot.harness.clone();
    let mut plan = Plan {
        value,
        changes: Changes {
            ops: Vec::new(),
            bytes: 0,
        },
    };
    let split = match edit {
        Edit::Connect { from, to } => {
            if !connect(&mut plan, &mut expected, from, to)? {
                return Ok(current.to_owned());
            }
            None
        }
        Edit::Disconnect { link } => disconnect(&mut plan, &mut expected, link)?,
        _ => return Err("Expected a wiring transaction".into()),
    };
    yaml.apply_patch(&Patch::new(plan.changes.ops))
        .map_err(failure)?;
    if let Some((set, cut, length)) = split {
        move_suffix(&mut yaml, &mut plan.value, set, cut, length)?;
    }
    prove(&yaml, &plan.value, &expected, &before_comments)
}

/// The complete YAML value protects opaque/shared data; the circuit proof
/// compares all model fields and a multiset of links, ignoring changed origins.
fn prove(
    yaml: &Document,
    value: &Value,
    expected: &Harness,
    before_comments: &[String],
) -> Result<String, String> {
    let output = yaml.to_string();
    if output.len() > MAX_SOURCE || comments(yaml)?.as_slice() != before_comments {
        return Err(
            "Wiring edit would discard comments or exceed the source budget; use the source editor"
                .into(),
        );
    }
    let (_, actual) = document(&output)?;
    if actual != *value {
        return Err("Wiring edit would change shared YAML data outside the selection; use the source editor".into());
    }
    let mut actual = model(&actual)?;
    if links(&actual) != links(expected) {
        return Err(
            "Wiring edit would change unaffected circuit semantics; use the source editor".into(),
        );
    }
    // Origins necessarily change when lanes are split. All other harness
    // fields, including future presentation fields, remain covered by Eq.
    actual.links.clone_from(&expected.links);
    if actual != *expected {
        return Err("Wiring edit would change unrelated components or their visibility; use the source editor".into());
    }
    Ok(output)
}

fn connect(
    plan: &mut Plan,
    expected: &mut Harness,
    from: Endpoint,
    to: Endpoint,
) -> Result<bool, String> {
    if !matches!(
        (from, to),
        (Endpoint::Pin { .. }, Endpoint::Cable { .. })
            | (Endpoint::Cable { .. }, Endpoint::Pin { .. })
    ) {
        return Err(
            "Drag between a connector pin and a cable conductor; mating annotations are separate"
                .into(),
        );
    }
    let from_value = endpoint(expected, from)?;
    let to_value = endpoint(expected, to)?;
    if expected.links.iter().any(|link| {
        link.kind == LinkKind::Cable
            && (link.endpoints == [from, to] || link.endpoints == [to, from])
    }) {
        return Ok(false);
    }
    append(plan, vec![from_value, to_value])?;
    expected.links.push(Link {
        endpoints: [from, to],
        kind: LinkKind::Cable,
        origin: Origin::Connection {
            set: 0,
            step: 0,
            lane: 0,
        },
    });
    Ok(true)
}

// A pending split identifies existing source steps, not a serialized subtree.
type Split = (usize, usize, usize);
fn disconnect(
    plan: &mut Plan,
    expected: &mut Harness,
    index: usize,
) -> Result<Option<Split>, String> {
    let selected = expected
        .links
        .get(index)
        .cloned()
        .ok_or("Unknown selected connection")?;
    let split =
        match selected.origin {
            Origin::Loop { connector, pair } if selected.kind == LinkKind::Loop => {
                let name = &expected.connectors[connector].name;
                let pairs = component_mut(&mut plan.value, "connectors", name)?
                    .get_mut(&Value::from("loops"))
                    .and_then(Value::as_sequence_mut)
                    .ok_or(SOURCE_EDIT)?;
                pairs.remove_at(pair).ok_or(SOURCE_EDIT)?;
                remove(
                    &mut plan.changes,
                    format!("/connectors/{}/loops/{pair}", token(name)),
                )?;
                None
            }
            Origin::Connection { set, step, lane } if selected.kind == LinkKind::Cable => {
                disconnect_lane(plan, expected, set, step, lane)?
            }
            _ => return Err(
                "Mating annotations are distinct from electrical connections; edit them in source"
                    .into(),
            ),
        };
    expected.links.remove(index);
    Ok(split)
}

fn disconnect_lane(
    plan: &mut Plan,
    expected: &Harness,
    set: usize,
    step: usize,
    lane: usize,
) -> Result<Option<Split>, String> {
    let items = sets(&plan.value)?
        .get(set)
        .and_then(Value::as_sequence)
        .ok_or(SOURCE_EDIT)?;
    let steps = connections::resolved(items.as_slice(), expected)?;
    let width = lane_width(&steps)?;
    let (prefix, suffix) = steps
        .split_at_checked(step.saturating_add(1))
        .ok_or("Selected connection no longer matches its source step")?;
    if width == 1 {
        // Move actual source nodes, including their comments. Never
        // reserialize a previously authored connection path.
        return Ok(Some((set, step + 1, items.len())));
    }
    let mut remaining = items.clone();
    for (column, item) in remaining.iter_mut().enumerate() {
        remove_lane(
            item,
            lane,
            format!("/connections/{set}/{column}"),
            &mut plan.changes,
        )?;
    }
    *sets_mut(&mut plan.value)?.get_mut(set).ok_or(SOURCE_EDIT)? = Value::Sequence(remaining);
    append(plan, lane_path(prefix, lane, expected)?)?;
    append(plan, lane_path(suffix, lane, expected)?)?;
    Ok(None)
}

fn failure(error: impl std::fmt::Display) -> String {
    format!("{SOURCE_EDIT}: {error}")
}

fn comments(yaml: &Document) -> Result<Vec<String>, String> {
    let mut result: Vec<_> = yaml
        .explain_comments()
        .map_err(failure)?
        .iter()
        .map(|comment| comment.text().to_owned())
        .collect();
    result.sort();
    Ok(result)
}

fn links(harness: &Harness) -> HashMap<([Endpoint; 2], LinkKind), usize> {
    let mut result = HashMap::new();
    for link in &harness.links {
        // Cable edges have no authored arrow direction. Normalizing their
        // orientation also lets reversed duplicate connections be detected.
        let endpoints = if link.kind == LinkKind::Cable
            && matches!(link.endpoints[0], Endpoint::Cable { .. })
        {
            [link.endpoints[1], link.endpoints[0]]
        } else {
            link.endpoints
        };
        *result.entry((endpoints, link.kind)).or_default() += 1;
    }
    result
}

fn endpoint(harness: &Harness, endpoint: Endpoint) -> Result<Value, String> {
    let (name, selector) = match endpoint {
        Endpoint::Pin { connector, pin } => {
            let connector = harness
                .connectors
                .get(connector)
                .ok_or("Unknown connector")?;
            if !connector.used {
                return Err("Connectors must be visible before graphical wiring".into());
            }
            let pin = connector.pins.get(pin).ok_or("Unknown pin")?;
            let selector = match &pin.id {
                PinId::Number(n) => Value::from(*n),
                PinId::Text(s) => Value::from(s.clone()),
            };
            (&connector.name, selector)
        }
        Endpoint::Cable { cable, conductor } => {
            let cable = harness.cables.get(cable).ok_or("Unknown cable")?;
            if !cable.used {
                return Err("Cables must be visible before graphical wiring".into());
            }
            let selector = match conductor {
                Conductor::Wire(wire) if wire < cable.wires.len() => Value::from((wire + 1) as u64),
                Conductor::Shield if cable.shield.is_some() => Value::from("s"),
                _ => return Err("Unknown cable conductor".into()),
            };
            (&cable.name, selector)
        }
        Endpoint::Connector(_) => return Err("Select an individual connector pin".into()),
    };
    let mut result = Mapping::new();
    result.insert(Value::from(name.clone()), selector);
    Ok(Value::from(result))
}

fn sets(value: &Value) -> Result<&Sequence, String> {
    value
        .as_mapping()
        .and_then(|map| map.get_str("connections"))
        .and_then(Value::as_sequence)
        .ok_or_else(|| SOURCE_EDIT.into())
}
fn sets_mut(value: &mut Value) -> Result<&mut Sequence, String> {
    value
        .as_mapping_mut()
        .and_then(|map| map.get_mut(&Value::from("connections")))
        .and_then(Value::as_sequence_mut)
        .ok_or_else(|| SOURCE_EDIT.into())
}
fn append(plan: &mut Plan, items: Vec<Value>) -> Result<(), String> {
    let sets = sets_mut(&mut plan.value)?;
    if sets.len() >= 1024 {
        return Err("Wiring edit exceeds the connection-set budget".into());
    }
    let value = Value::from(items);
    sets.push(value.clone());
    plan.changes.push("/connections/-".into(), value, true)
}
fn remove(changes: &mut Changes, path: String) -> Result<(), String> {
    let bytes = path.len();
    changes.record(PatchOp::remove(path), bytes)
}

fn lane_width(steps: &[Step]) -> Result<usize, String> {
    let width = steps
        .iter()
        .find_map(|step| match step {
            Step::Endpoints(items) => Some(items.len()),
            _ => None,
        })
        .ok_or(SOURCE_EDIT)?;
    for step in steps {
        let count = match step {
            Step::Endpoints(items) => items.len(),
            Step::Arrows(items) => items.len(),
            Step::Mating(_) => {
                return Err(
                    "Connection sets containing whole-connector mating need source editing".into(),
                );
            }
        };
        if count != width {
            return Err("Connection lane widths differ; use the source editor".into());
        }
    }
    Ok(width)
}

fn lane_path(steps: &[Step], lane: usize, harness: &Harness) -> Result<Vec<Value>, String> {
    steps
        .iter()
        .map(|step| match step {
            Step::Endpoints(items) => endpoint(harness, *items.get(lane).ok_or(SOURCE_EDIT)?),
            Step::Arrows(items) => Ok(Value::from(vec![Value::from(
                match items.get(lane).ok_or(SOURCE_EDIT)? {
                    Direction::None => "--",
                    Direction::Forward => "-->",
                    Direction::Backward => "<--",
                    Direction::Both => "<-->",
                },
            )])),
            Step::Mating(_) => Err(SOURCE_EDIT.into()),
        })
        .collect()
}

fn remove_lane(
    item: &mut Value,
    lane: usize,
    path: String,
    changes: &mut Changes,
) -> Result<(), String> {
    if let Some(map) = item.as_mapping_mut() {
        let (name, selector) = map.iter_mut().next().ok_or(SOURCE_EDIT)?;
        let name = name.as_str().ok_or(SOURCE_EDIT)?;
        let path = format!("{path}/{}", token(name));
        remove_selector(selector, lane, path, changes)
    } else {
        let items = item.as_sequence_mut().ok_or(SOURCE_EDIT)?;
        items.remove_at(lane).ok_or(SOURCE_EDIT)?;
        remove(changes, format!("{path}/{lane}"))
    }
}

fn remove_selector(
    value: &mut Value,
    lane: usize,
    path: String,
    changes: &mut Changes,
) -> Result<(), String> {
    let items = value.as_sequence_mut().ok_or(SOURCE_EDIT)?;
    let mut offset = 0;
    for index in 0..items.len() {
        let range = connections::numeric_range(items.get(index).ok_or(SOURCE_EDIT)?);
        let length = range.map_or(1, |(a, b)| a.abs_diff(b) as usize + 1);
        if lane >= offset + length {
            offset += length;
            continue;
        }
        let Some((start, end)) = range else {
            items.remove_at(index).ok_or(SOURCE_EDIT)?;
            return remove(changes, format!("{path}/{index}"));
        };
        let parts = remaining_range(start, end, lane - offset);
        if parts.is_empty() {
            items.remove_at(index).ok_or(SOURCE_EDIT)?;
            return remove(changes, format!("{path}/{index}"));
        }
        *items.get_mut(index).ok_or(SOURCE_EDIT)? = parts[0].clone();
        changes.push(format!("{path}/{index}"), parts[0].clone(), false)?;
        if let Some(second) = parts.get(1) {
            items
                .insert_at(index + 1, second.clone())
                .map_err(|_| SOURCE_EDIT)?;
            changes.push(format!("{path}/{}", index + 1), second.clone(), true)?;
        }
        return Ok(());
    }
    Err("Selected lane is outside its source selector".into())
}
fn remaining_range(start: u32, end: u32, selected: usize) -> Vec<Value> {
    let length = start.abs_diff(end) as usize + 1;
    let at = |position: usize| {
        if start <= end {
            start + position as u32
        } else {
            start - position as u32
        }
    };
    let mut parts = Vec::new();
    if selected > 0 {
        parts.push(range_value(start, at(selected - 1)));
    }
    if selected + 1 < length {
        parts.push(range_value(at(selected + 1), end));
    }
    parts
}

fn range_value(start: u32, end: u32) -> Value {
    if start == end {
        Value::from(start)
    } else {
        Value::from(format!("{start}-{end}"))
    }
}

// Raw entries only: normal YAML lookup follows aliases and merge keys. A
// transaction must never move an inherited/shared collection by that route.
fn own_key<'a>(node: NodeRef<'a>, key: &str) -> Result<NodeRef<'a>, String> {
    let map = node.as_mapping().ok_or(SOURCE_EDIT)?;
    for (name, value) in map.iter() {
        if name
            .as_scalar()
            .and_then(|s| s.resolved().ok())
            .and_then(|v| v.as_str().map(str::to_owned))
            .as_deref()
            == Some(key)
        {
            if value.kind() == NodeKind::Alias {
                return Err("Aliased wiring requires source editing".into());
            }
            return Ok(value);
        }
    }
    Err("Inherited wiring requires source editing".into())
}
fn connections_id(yaml: &Document) -> Result<NodeId, String> {
    Ok(own_key(yaml.root().ok_or(SOURCE_EDIT)?, "connections")?.id())
}
fn step_id(yaml: &Document, set: usize, step: usize) -> Result<NodeId, String> {
    let sets = own_key(yaml.root().ok_or(SOURCE_EDIT)?, "connections")?;
    let set = sets
        .as_sequence()
        .and_then(|items| items.get(set))
        .ok_or(SOURCE_EDIT)?;
    if set.kind() == NodeKind::Alias {
        return Err("Aliased wiring requires source editing".into());
    }
    let node = set
        .as_sequence()
        .and_then(|items| items.get(step))
        .ok_or(SOURCE_EDIT)?;
    Ok(node.id())
}
fn move_suffix(
    yaml: &mut Document,
    value: &mut Value,
    set: usize,
    cut: usize,
    length: usize,
) -> Result<(), String> {
    let count = sets(value)?.len();
    if count >= 1024 {
        return Err("Wiring edit exceeds the connection-set budget".into());
    }
    let connections = connections_id(yaml)?;
    // A new container is serialized; every authored step is moved as source.
    let destination = yaml
        .node_mut(connections)
        .map_err(failure)?
        .push(vec![Value::Null])
        .map_err(failure)?
        .id();
    for _ in cut..length {
        let id = step_id(yaml, set, cut)?;
        let removed = yaml.remove_node(id).map_err(failure)?;
        yaml.node_mut(destination)
            .map_err(failure)?
            .insert_removed(removed)
            .map_err(failure)?;
    }
    let _placeholder = yaml
        .node_mut(destination)
        .map_err(failure)?
        .remove_at(0)
        .map_err(failure)?;
    split_value(value, set, cut)
}

fn split_value(value: &mut Value, set: usize, cut: usize) -> Result<(), String> {
    let sets = sets_mut(value)?;
    let items = sets
        .get_mut(set)
        .and_then(Value::as_sequence_mut)
        .ok_or(SOURCE_EDIT)?;
    let mut suffix = Vec::new();
    while items.len() > cut {
        suffix.push(items.remove_at(cut).ok_or(SOURCE_EDIT)?);
    }
    sets.push(Value::from(suffix));
    Ok(())
}

#[cfg(test)]
mod tests;
