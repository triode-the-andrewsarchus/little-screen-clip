use super::*;

enum Step {
    Endpoints(Vec<Endpoint>),
    Arrows(Vec<Direction>),
    Mating(Direction),
}

pub(super) fn read(root: &Mapping, harness: &mut Harness) -> Result<(), String> {
    let sets = root
        .get_str("connections")
        .ok_or("WireViz needs a connections list")?;
    let sets = sequence(sets, "connections")?;
    if sets.len() > 1024 {
        return Err("Too many WireViz connection sets".into());
    }
    for (set, items) in sets.iter().enumerate() {
        let items = sequence(items, "Connection set")?;
        if items.is_empty() || items.len() > 256 {
            return Err(format!("Connection set {} needs 1–256 steps", set + 1));
        }
        // A sole cable designator includes an unconnected component without
        // selecting, or inventing connections to, any of its conductors.
        if let [value] = items
            && let Some(name) = value.as_str()
            && let Some(cable) = harness.cables.iter_mut().find(|c| c.name == name)
        {
            cable.used = true;
            continue;
        }
        let steps = items
            .iter()
            .map(|item| resolve(item, harness))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Connection set {}: {e}", set + 1))?;
        mark_used(&steps, harness);
        link_steps(&steps, set, harness)?;
    }
    loops(root, harness)?;
    let unused = harness.connectors.iter().filter(|c| !c.used).count()
        + harness.cables.iter().filter(|c| !c.used).count();
    if unused > 0 {
        harness.warnings.push(format!(
            "{unused} component definitions are not referenced by connections"
        ));
    }
    Ok(())
}

fn resolve(item: &Value, harness: &Harness) -> Result<Step, String> {
    if let Some(name) = item.as_str() {
        if let Some(direction) = arrow(name, true) {
            return Ok(Step::Mating(direction));
        }
        return component(name, None, harness).map(Step::Endpoints);
    }
    if let Some(values) = item.as_sequence() {
        if values.is_empty() || values.len() > MAX_PINS {
            return Err("A parallel connection list needs 1–512 items".into());
        }
        let arrows = values
            .iter()
            .map(|value| value.as_str().and_then(|s| arrow(s, false)))
            .collect::<Option<Vec<_>>>();
        if let Some(arrows) = arrows {
            return Ok(Step::Arrows(arrows));
        }
        let mut pins = Vec::new();
        for value in values {
            let name = value
                .as_str()
                .ok_or("Parallel components must be single-pin designators")?;
            let endpoints = component(name, None, harness)?;
            if !matches!(endpoints.as_slice(), [Endpoint::Pin { .. }]) {
                return Err("Parallel components must be single-pin connectors".into());
            }
            pins.extend(endpoints);
        }
        return Ok(Step::Endpoints(pins));
    }
    let map = mapping(item, "Connection step")?;
    if map.len() != 1 {
        return Err("A connection step needs exactly one component".into());
    }
    let (name, selector) = map.iter().next().ok_or("Missing connection component")?;
    let name = name.as_str().ok_or("Component designator must be text")?;
    component(name, Some(selector), harness).map(Step::Endpoints)
}

fn component(
    name: &str,
    selector: Option<&Value>,
    harness: &Harness,
) -> Result<Vec<Endpoint>, String> {
    if let Some(index) = harness.connectors.iter().position(|c| c.name == name) {
        let connector = &harness.connectors[index];
        return match selector {
            Some(value) => selections(value)?
                .iter()
                .map(|pin| {
                    connector.pin(pin).map(|pin| Endpoint::Pin {
                        connector: index,
                        pin,
                    })
                })
                .collect(),
            None if connector.pins.len() == 1 => Ok(vec![Endpoint::Pin {
                connector: index,
                pin: 0,
            }]),
            None => Ok(vec![Endpoint::Connector(index)]),
        };
    }
    if let Some(index) = harness.cables.iter().position(|c| c.name == name) {
        let cable = &harness.cables[index];
        return selections(selector.ok_or("Cable connections need explicit wire numbers")?)?
            .iter()
            .map(|wire| {
                cable.wire(wire).map(|conductor| Endpoint::Cable {
                    cable: index,
                    conductor,
                })
            })
            .collect();
    }
    Err(format!(
        "Unknown component {name:?}; template instance expansion is not implemented yet"
    ))
}

fn selections(value: &Value) -> Result<Vec<Value>, String> {
    let Some(items) = value.as_sequence() else {
        return Ok(vec![value.clone()]);
    };
    let mut result = Vec::new();
    for value in items {
        let range = value
            .as_str()
            .and_then(|s| s.split_once('-'))
            .and_then(|(a, b)| Some((a.parse::<u32>().ok()?, b.parse::<u32>().ok()?)));
        if let Some((start, end)) = range {
            if start.abs_diff(end) as usize >= MAX_PINS {
                return Err("Pin/wire range exceeds the expansion budget".into());
            }
            let values = start.min(end)..=start.max(end);
            if start <= end {
                result.extend(values.map(Value::from))
            } else {
                result.extend(values.rev().map(Value::from))
            }
        } else {
            result.push(value.clone());
        }
        if result.len() > MAX_PINS {
            return Err("Parallel connection exceeds the lane budget".into());
        }
    }
    if result.is_empty() {
        return Err("Empty pin/wire selection".into());
    }
    Ok(result)
}

fn arrow(text: &str, body: bool) -> Option<Direction> {
    match (body, text) {
        (false, "--") | (true, "==") => Some(Direction::None),
        (false, "-->") | (true, "==>") => Some(Direction::Forward),
        (false, "<--") | (true, "<==") => Some(Direction::Backward),
        (false, "<-->") | (true, "<==>") => Some(Direction::Both),
        _ => None,
    }
}

fn mark_used(steps: &[Step], harness: &mut Harness) {
    for step in steps {
        if let Step::Endpoints(endpoints) = step {
            for endpoint in endpoints {
                match *endpoint {
                    Endpoint::Pin { connector, .. } | Endpoint::Connector(connector) => {
                        harness.connectors[connector].used = true
                    }
                    Endpoint::Cable { cable, .. } => harness.cables[cable].used = true,
                }
            }
        }
    }
}

fn link_steps(steps: &[Step], set: usize, harness: &mut Harness) -> Result<(), String> {
    let mut step = 0;
    while step < steps.len() {
        let Step::Endpoints(from) = &steps[step] else {
            return Err("An arrow needs a connector on both sides".into());
        };
        match steps.get(step + 1) {
            None => (),
            Some(Step::Endpoints(to)) => link_pairs(from, to, LinkKind::Cable, set, step, harness)?,
            Some(arrow) => {
                let Some(Step::Endpoints(to)) = steps.get(step + 2) else {
                    return Err("An arrow needs a connector on both sides".into());
                };
                match arrow {
                    Step::Arrows(arrows) => {
                        if arrows.len() != from.len() || to.len() != from.len() {
                            return Err("Parallel arrow and pin counts must match".into());
                        }
                        for (lane, ((from, to), direction)) in
                            from.iter().zip(to).zip(arrows).enumerate()
                        {
                            mating_pin(*from)?;
                            mating_pin(*to)?;
                            push(
                                harness,
                                Link {
                                    endpoints: [*from, *to],
                                    kind: LinkKind::PinMating(*direction),
                                    origin: Origin::Connection { set, step, lane },
                                },
                            )?;
                        }
                    }
                    Step::Mating(direction) => {
                        let from = body(from)?;
                        let to = body(to)?;
                        push(
                            harness,
                            Link {
                                endpoints: [from, to],
                                kind: LinkKind::ConnectorMating(*direction),
                                origin: Origin::Connection { set, step, lane: 0 },
                            },
                        )?;
                    }
                    Step::Endpoints(_) => unreachable!(),
                }
                step += 1;
            }
        }
        step += 1;
    }
    Ok(())
}

fn link_pairs(
    from: &[Endpoint],
    to: &[Endpoint],
    kind: LinkKind,
    set: usize,
    step: usize,
    harness: &mut Harness,
) -> Result<(), String> {
    if from.len() != to.len() {
        return Err("Parallel pin and wire counts must match".into());
    }
    for (lane, (from, to)) in from.iter().zip(to).enumerate() {
        if !matches!(
            (from, to),
            (Endpoint::Pin { .. }, Endpoint::Cable { .. })
                | (Endpoint::Cable { .. }, Endpoint::Pin { .. })
        ) {
            return Err(
                "Connection paths must alternate connector pins and cable conductors".into(),
            );
        }
        push(
            harness,
            Link {
                endpoints: [*from, *to],
                kind,
                origin: Origin::Connection { set, step, lane },
            },
        )?;
    }
    Ok(())
}

fn mating_pin(endpoint: Endpoint) -> Result<(), String> {
    if matches!(endpoint, Endpoint::Pin { .. }) {
        Ok(())
    } else {
        Err("Single-line mating arrows require explicit connector pins".into())
    }
}

fn body(endpoints: &[Endpoint]) -> Result<Endpoint, String> {
    let mut connector = None;
    for endpoint in endpoints {
        let (Endpoint::Pin {
            connector: index, ..
        }
        | Endpoint::Connector(index)) = *endpoint
        else {
            return Err("Whole-connector mating arrows cannot attach to cables".into());
        };
        if connector.is_some_and(|old| old != index) {
            return Err("A mating arrow must reference one connector per side".into());
        }
        connector = Some(index);
    }
    connector
        .map(Endpoint::Connector)
        .ok_or("Missing mating connector".into())
}

fn loops(root: &Mapping, harness: &mut Harness) -> Result<(), String> {
    let Some(connectors) = root.get_str("connectors").and_then(Value::as_mapping) else {
        return Ok(());
    };
    for connector in 0..harness.connectors.len() {
        let name = &harness.connectors[connector].name;
        let Some(pairs) = connectors
            .get_str(name)
            .and_then(Value::as_mapping)
            .and_then(|attrs| attrs.get_str("loops"))
        else {
            continue;
        };
        let pairs = sequence(pairs, "Connector loops")?;
        if pairs.len() > MAX_PINS {
            return Err("Too many connector loops".into());
        }
        for (pair, pins) in pairs.iter().enumerate() {
            let pins = sequence(pins, "Connector loop")?;
            let [from, to] = pins else {
                return Err("A connector loop needs exactly two pins".into());
            };
            let from = harness.connectors[connector].pin(from)?;
            let to = harness.connectors[connector].pin(to)?;
            if harness.connectors[connector].used {
                push(
                    harness,
                    Link {
                        endpoints: [
                            Endpoint::Pin {
                                connector,
                                pin: from,
                            },
                            Endpoint::Pin { connector, pin: to },
                        ],
                        kind: LinkKind::Loop,
                        origin: Origin::Loop { connector, pair },
                    },
                )?;
            }
        }
    }
    Ok(())
}

fn push(harness: &mut Harness, link: Link) -> Result<(), String> {
    if harness.links.len() >= MAX_CONNECTIONS {
        return Err("WireViz exceeds the normalized link budget".into());
    }
    harness.links.push(link);
    Ok(())
}
