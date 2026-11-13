//! Typed, data-only harness presentation; external and report features stay inert.
use super::*;

#[derive(Default)]
struct Backgrounds {
    graph: Option<[u8; 3]>,
    connector: Option<[u8; 3]>,
    cable: Option<[u8; 3]>,
    bundle: Option<[u8; 3]>,
}

/// Applies presentation defaults after physical component normalization.
pub(super) fn apply(root: &Mapping, harness: &mut Harness) -> Result<(), String> {
    let backgrounds = read(root, &mut harness.warnings)?;
    harness.background = backgrounds.graph;
    for connector in &mut harness.connectors {
        connector.background = connector.background.or(backgrounds.connector);
    }
    for cable in &mut harness.cables {
        cable.background = cable.background.or(if cable.bundle {
            backgrounds.bundle
        } else {
            backgrounds.cable
        });
    }
    omissions(root, &mut harness.warnings);
    Ok(())
}

pub(super) fn flag(attrs: &Mapping, name: &str, default: bool) -> Result<bool, String> {
    match attrs.get_str(name) {
        None | Some(Value::Null) => Ok(default),
        Some(Value::Bool(value)) => Ok(*value),
        Some(_) => Err(format!("{name} must be a boolean")),
    }
}

pub(super) fn color(attrs: &Mapping, name: &str) -> Result<Option<[u8; 3]>, String> {
    match attrs.get_str(name) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => {
            let value = value
                .as_str()
                .ok_or_else(|| format!("{name} must be a color string"))?;
            let bands = colors::bands(value)?;
            if let [color] = bands.as_slice() {
                Ok(Some(*color))
            } else {
                Err(format!("{name} requires one solid color"))
            }
        }
    }
}

fn read(root: &Mapping, warnings: &mut Vec<String>) -> Result<Backgrounds, String> {
    let Some(options) = root.get_str("options") else {
        return Ok(Backgrounds::default());
    };
    let options = mapping(options, "WireViz options")?;
    let graph = color(options, "bgcolor")?;
    let node = color(options, "bgcolor_node")?.or_else(|| {
        matches!(options.get_str("bgcolor_node"), Some(Value::Null))
            .then_some(graph)
            .flatten()
    });
    let cable = color(options, "bgcolor_cable")?.or(node);
    for (key, _) in options.iter() {
        let key = key.as_str().ok_or("WireViz option names must be text")?;
        if [
            "bgcolor",
            "bgcolor_node",
            "bgcolor_connector",
            "bgcolor_cable",
            "bgcolor_bundle",
        ]
        .contains(&key)
        {
            continue;
        }
        let message = match key {
            "fontname" => {
                "Requested font settings are not applied; the preview uses its portable font set"
            }
            "color_mode" => {
                "Requested color-name text formatting is not applied; conductor color bands are shown"
            }
            "mini_bom_mode" => {
                "Additional-component and bill-of-materials formatting is not displayed"
            }
            "template_separator" => {
                "Template separator settings are retained, but generated component instances remain unsupported"
            }
            _ => "Unrecognized harness presentation options are not applied",
        };
        if !warnings.iter().any(|warning| warning == message) {
            warnings.push(message.into());
        }
    }
    Ok(Backgrounds {
        graph,
        connector: color(options, "bgcolor_connector")?.or(node),
        cable,
        bundle: color(options, "bgcolor_bundle")?.or(cable),
    })
}

fn omissions(root: &Mapping, warnings: &mut Vec<String>) {
    let omitted = [
        "subtype",
        "color",
        "gauge",
        "gauge_unit",
        "show_equiv",
        "length",
        "length_unit",
        "pn",
        "manufacturer",
        "mpn",
        "supplier",
        "spn",
        "additional_components",
    ];
    let components = ["connectors", "cables"]
        .into_iter()
        .filter_map(|section| root.get_str(section).and_then(Value::as_mapping))
        .flat_map(Mapping::values)
        .filter_map(Value::as_mapping)
        .collect::<Vec<_>>();
    if components
        .iter()
        .any(|attrs| omitted.iter().any(|key| attrs.get_str(key).is_some()))
    {
        warnings.push("Some component metadata and bill-of-materials rows are retained in source but not displayed".into());
    }
    if components.iter().any(|attrs| {
        attrs.iter().any(|(key, _)| {
            key.as_str().is_some_and(|key| {
                (key.starts_with("show_") || key.starts_with("hide_"))
                    && ![
                        "show_name",
                        "show_pincount",
                        "hide_disconnected_pins",
                        "show_wirecount",
                        "show_wirenumbers",
                        "show_equiv",
                    ]
                    .contains(&key)
            })
        })
    }) {
        warnings.push("Unrecognized component visibility options are not applied".into());
    }
    if root.get_str("tweak").is_some() {
        warnings.push("Graphviz tweaks are not executed by the native harness preview".into());
    }
    if components
        .iter()
        .any(|attrs| attrs.get_str("image").is_some())
    {
        warnings
            .push("Linked component images are omitted; no external resources were read".into());
    }
}
