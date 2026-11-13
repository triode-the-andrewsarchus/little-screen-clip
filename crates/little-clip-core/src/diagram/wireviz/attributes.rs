use super::*;

pub(super) fn read(root: &Mapping) -> Result<Harness, String> {
    let connector_definitions = components(root, "connectors")?;
    let cable_definitions = components(root, "cables")?;
    if connector_definitions.len() + cable_definitions.len() > MAX_COMPONENTS {
        return Err("WireViz exceeds the component budget".into());
    }
    let mut terminals = 0;
    let mut connectors = Vec::new();
    for (name, attributes) in connector_definitions {
        let connector = connector(name, attributes)?;
        terminals += connector.pins.len();
        if terminals > MAX_TERMINALS {
            return Err("WireViz exceeds the terminal budget".into());
        }
        connectors.push(connector);
    }
    let mut cables = Vec::new();
    for (name, attributes) in cable_definitions {
        if connectors.iter().any(|connector| connector.name == name) {
            return Err(format!(
                "Designator {name:?} is both a connector and a cable"
            ));
        }
        let cable = cable(name, attributes)?;
        terminals += cable.wires.len();
        if terminals > MAX_TERMINALS {
            return Err("WireViz exceeds the terminal budget".into());
        }
        cables.push(cable);
    }
    if connectors.is_empty() && cables.is_empty() {
        return Err("WireViz needs connector or cable definitions".into());
    }
    let mut warnings = Vec::new();
    if root.get_str("tweak").is_some() {
        warnings.push("Graphviz tweaks are not executed by the native harness preview".into());
    }
    if ["connectors", "cables"]
        .iter()
        .filter_map(|key| root.get_str(key))
        .filter_map(Value::as_mapping)
        .flat_map(Mapping::values)
        .filter_map(Value::as_mapping)
        .any(|attrs| attrs.get_str("image").is_some())
    {
        warnings
            .push("Linked component images are omitted; no external resources were read".into());
    }
    Ok(Harness {
        connectors,
        cables,
        links: Vec::new(),
        warnings,
    })
}

fn components<'a>(root: &'a Mapping, key: &str) -> Result<Vec<(&'a str, &'a Mapping)>, String> {
    let Some(value) = root.get_str(key) else {
        return Ok(Vec::new());
    };
    let map = mapping(value, key)?;
    if map.len() > MAX_COMPONENTS {
        return Err("WireViz exceeds the component budget".into());
    }
    map.iter()
        .map(|(name, attrs)| {
            let name = name
                .as_str()
                .filter(|s| !s.is_empty())
                .ok_or("Component designators must be nonempty strings")?;
            Ok((name, mapping(attrs, name)?))
        })
        .collect()
}

fn connector(name: &str, attrs: &Mapping) -> Result<Connector, String> {
    let simple = match text(attrs, "style")?.as_deref() {
        None => false,
        Some("simple") => true,
        Some(style) => return Err(format!("Unsupported connector style {style:?}")),
    };
    Ok(Connector {
        name: name.to_owned(),
        description: text(attrs, "type")?,
        notes: text(attrs, "notes")?,
        pins: connector_pins(name, attrs, simple)?,
        simple,
        used: false,
    })
}

fn connector_pins(name: &str, attrs: &Mapping, simple: bool) -> Result<Vec<Pin>, String> {
    let ids = list(attrs, "pins")?;
    let labels = list(attrs, "pinlabels")?;
    let inferred = if !ids.is_empty() {
        ids.len()
    } else {
        labels.len().max(usize::from(simple))
    };
    let count = count(attrs, "pincount", inferred)?;
    if (!ids.is_empty() && ids.len() != count) || labels.len() > count || (simple && count != 1) {
        return Err(format!("Inconsistent pin count on {name}"));
    }
    let colors = list(attrs, "pincolors")?;
    if colors.len() > count {
        return Err(format!("Too many pin colors on {name}"));
    }
    numbered_pins(name, ids, labels, colors, count)
}

fn numbered_pins(
    name: &str,
    ids: &[Value],
    labels: &[Value],
    colors: &[Value],
    count: usize,
) -> Result<Vec<Pin>, String> {
    let mut pins = Vec::with_capacity(count);
    let mut identities = HashSet::new();
    for index in 0..count {
        let id = match ids.get(index) {
            Some(id) => PinId::read(id)?,
            None => PinId::Number((index + 1) as i64),
        };
        if !identities.insert(id.clone()) {
            return Err(format!("Duplicate pin identity {id:?} on {name}"));
        }
        pins.push(Pin {
            id,
            label: optional_text(labels.get(index), "pin label")?,
            color: optional_text(colors.get(index), "pin color")?,
        });
    }
    Ok(pins)
}

fn cable(name: &str, attrs: &Mapping) -> Result<Cable, String> {
    if attrs.get_str("color_code").is_some() {
        return Err(format!(
            "Automatic color_code palettes are not implemented yet ({name}); use explicit colors"
        ));
    }
    let colors = list(attrs, "colors")?;
    let count = count(attrs, "wirecount", colors.len())?;
    let labels = list(attrs, "wirelabels")?;
    if labels.len() > count {
        return Err(format!("Too many wire labels on {name}"));
    }
    let mut wires = Vec::with_capacity(count);
    for index in 0..count {
        let color = if colors.is_empty() {
            None
        } else {
            colors.get(index % colors.len())
        };
        wires.push(Wire {
            label: optional_text(labels.get(index), "wire label")?,
            color: optional_text(color, "wire color")?,
        });
    }
    let bundle = match text(attrs, "category")?.as_deref() {
        None => false,
        Some("bundle") => true,
        Some(category) => return Err(format!("Unsupported cable category {category:?}")),
    };
    Ok(Cable {
        name: name.to_owned(),
        description: text(attrs, "type")?,
        notes: text(attrs, "notes")?,
        wires,
        shield: shield(attrs)?,
        bundle,
        used: false,
    })
}

fn shield(attrs: &Mapping) -> Result<Option<String>, String> {
    match attrs.get_str("shield") {
        None | Some(Value::Bool(false)) => Ok(None),
        Some(Value::Bool(true)) => Ok(Some("BK".into())),
        Some(value) => value
            .as_str()
            .map(|s| Some(s.to_owned()))
            .ok_or("Cable shield must be a boolean or color".into()),
    }
}

fn list<'a>(attrs: &'a Mapping, key: &str) -> Result<&'a [Value], String> {
    attrs
        .get_str(key)
        .map_or(Ok(&[]), |value| sequence(value, key))
}

fn text(attrs: &Mapping, key: &str) -> Result<Option<String>, String> {
    optional_text(attrs.get_str(key), key)
}

fn optional_text(value: Option<&Value>, context: &str) -> Result<Option<String>, String> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_str()
            .map(|s| Some(s.to_owned()))
            .ok_or_else(|| format!("{context} must be text; quote numeric or boolean-like labels")),
    }
}

fn count(attrs: &Mapping, key: &str, inferred: usize) -> Result<usize, String> {
    let count = match attrs.get_str(key) {
        None => inferred,
        Some(value) => value
            .as_u64()
            .and_then(|n| usize::try_from(n).ok())
            .ok_or_else(|| format!("{key} must be a positive integer"))?,
    };
    if count == 0 || count > MAX_PINS {
        return Err(format!("{key} must be between 1 and {MAX_PINS}"));
    }
    Ok(count)
}
