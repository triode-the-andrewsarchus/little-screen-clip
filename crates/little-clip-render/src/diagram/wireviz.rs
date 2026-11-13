//! Harness tables and conductor paths laid out by the existing native engine.
use super::graphviz::label;
use little_clip_core::diagram::wireviz::{
    self, Cable, Conductor, Connector, Direction, Endpoint, Harness, Link, LinkKind, Pin, PinId,
};
use std::{collections::HashSet, fmt::Write};

pub(super) fn svg(source: &str) -> Result<(String, Vec<String>), String> {
    let harness = wireviz::parse(source)?;
    let dot = dot(&harness)?;
    let (svg, mut warnings) = super::graphviz::svg(&dot)?;
    warnings.extend(harness.warnings);
    Ok((svg, warnings))
}

fn dot(harness: &Harness) -> Result<String, String> {
    let visible: HashSet<_> = harness
        .links
        .iter()
        .flat_map(|link| link.endpoints)
        .collect();
    let visible_rows: usize = harness
        .connectors
        .iter()
        .enumerate()
        .filter(|(_, c)| c.used)
        .map(|(index, c)| {
            if c.simple {
                1
            } else {
                visible_pins(c, index, &visible).count()
            }
        })
        .sum::<usize>()
        + harness
            .cables
            .iter()
            .filter(|c| c.used)
            .map(|c| c.wires.len() + usize::from(c.shield.is_some()))
            .sum::<usize>();
    if !harness.connectors.iter().any(|c| c.used) && !harness.cables.iter().any(|c| c.used) {
        return Err("No components are included in WireViz connections".into());
    }
    if visible_rows > 1024 || harness.links.len() > 4096 {
        return Err("Harness exceeds the native preview geometry budget".into());
    }
    let mut dot = String::from(
        "digraph harness { graph [rankdir=LR bgcolor=white nodesep=0.45 ranksep=0.7]; node [shape=plain fontname=Helvetica fontsize=12]; edge [dir=none color=\"#607485\" penwidth=2];\n",
    );
    if let Some(background) = harness.background {
        writeln!(dot, "graph [bgcolor=\"{}\"];", rgb(background)).unwrap();
    }
    for (index, connector) in harness
        .connectors
        .iter()
        .enumerate()
        .filter(|(_, c)| c.used)
    {
        let table = connector_table(connector, index, &visible)?;
        writeln!(
            dot,
            "c{index} [id=\"wireviz-connector-{index}\" fontcolor=\"{}\" label=<{table}>];",
            foreground(connector.background.unwrap_or([244, 247, 250]))
        )
        .unwrap();
    }
    for (index, cable) in harness.cables.iter().enumerate().filter(|(_, c)| c.used) {
        let table = cable_table(cable, index)?;
        writeln!(
            dot,
            "w{index} [id=\"wireviz-cable-{index}\" fontcolor=\"{}\" label=<{table}>];",
            foreground(cable.background.unwrap_or([244, 247, 250]))
        )
        .unwrap();
    }
    for (index, link) in harness.links.iter().enumerate() {
        write_link(&mut dot, harness, index, link)?;
    }
    dot.push('}');
    if dot.len() > 1024 * 1024 {
        return Err("Harness exceeds the generated layout budget".into());
    }
    Ok(dot)
}

fn connector_table(
    connector: &Connector,
    index: usize,
    visible: &HashSet<Endpoint>,
) -> Result<String, String> {
    let mut table = header(
        connector.show_name.then_some(connector.name.as_str()),
        connector.description.as_deref(),
        connector
            .show_pincount
            .then_some((connector.pins.len(), "pins")),
        connector.background,
        connector.title_background,
        false,
    );
    if !connector.simple {
        for (pin, value) in visible_pins(connector, index, visible) {
            let id = match &value.id {
                PinId::Number(n) => n.to_string(),
                PinId::Text(s) => s.clone(),
            };
            row(
                &mut table,
                &format!("p{pin}"),
                Some(&id),
                value.label.as_deref(),
                value.color.as_deref(),
                &format!("wireviz-pin-{index}-{pin}"),
            )?;
        }
    }
    footer(&mut table, connector.notes.as_deref());
    if connector.simple {
        Ok(format!(
            "<TABLE BORDER=\"0\" CELLBORDER=\"0\" CELLSPACING=\"0\" CELLPADDING=\"0\"><TR><TD PORT=\"p0\" ID=\"wireviz-pin-{index}-0\" TITLE=\"Terminal\">{table}</TD></TR></TABLE>"
        ))
    } else {
        Ok(table)
    }
}

fn visible_pins<'a>(
    connector: &'a Connector,
    index: usize,
    visible: &'a HashSet<Endpoint>,
) -> impl Iterator<Item = (usize, &'a Pin)> + 'a {
    connector.pins.iter().enumerate().filter(move |(pin, _)| {
        !connector.hide_disconnected_pins
            || visible.contains(&Endpoint::Pin {
                connector: index,
                pin: *pin,
            })
    })
}

fn cable_table(cable: &Cable, index: usize) -> Result<String, String> {
    let kind = if cable.bundle {
        "bundle wires"
    } else {
        "wires"
    };
    let mut table = header(
        cable.show_name.then_some(cable.name.as_str()),
        cable.description.as_deref(),
        cable.show_wirecount.then_some((cable.wires.len(), kind)),
        cable.background,
        cable.title_background,
        cable.bundle,
    );
    for (wire, value) in cable.wires.iter().enumerate() {
        let number = (wire + 1).to_string();
        row(
            &mut table,
            &format!("p{wire}"),
            cable.show_wirenumbers.then_some(number.as_str()),
            value.label.as_deref(),
            value.color.as_deref(),
            &format!("wireviz-wire-{index}-{wire}"),
        )?;
    }
    if let Some(color) = &cable.shield {
        row(
            &mut table,
            "s",
            Some("s"),
            Some("Shield"),
            Some(color),
            &format!("wireviz-shield-{index}"),
        )?;
    }
    footer(&mut table, cable.notes.as_deref());
    Ok(table)
}

fn write_link(
    dot: &mut String,
    harness: &Harness,
    index: usize,
    link: &Link,
) -> Result<(), String> {
    let from = port(link.endpoints[0], "e");
    let to = port(link.endpoints[1], "w");
    let direction = match link.kind {
        LinkKind::PinMating(d) | LinkKind::ConnectorMating(d) => match d {
            Direction::None => "none",
            Direction::Forward => "forward",
            Direction::Backward => "back",
            Direction::Both => "both",
        },
        _ => "none",
    };
    let color = link_color(harness, &link.endpoints)?;
    let style = if link.kind == LinkKind::Loop {
        "dashed"
    } else {
        "solid"
    };
    writeln!(dot, "{from} -> {to} [id=\"wireviz-link-{index}\" dir={direction} style={style} color=\"{color}\"];").unwrap();
    Ok(())
}

fn header(
    name: Option<&str>,
    description: Option<&str>,
    count: Option<(usize, &str)>,
    background: Option<[u8; 3]>,
    title_background: Option<[u8; 3]>,
    dashed: bool,
) -> String {
    let body = rgb(background.unwrap_or([244, 247, 250]));
    let title_color = title_background.unwrap_or([34, 56, 77]);
    let title = rgb(title_color);
    let mut table = format!(
        "<TABLE BORDER=\"1\" CELLBORDER=\"0\" CELLSPACING=\"0\" CELLPADDING=\"6\" COLOR=\"#748899\" BGCOLOR=\"{body}\" STYLE=\"{}\">",
        if dashed { "DASHED" } else { "SOLID" }
    );
    let mut parts = Vec::new();
    if let Some(name) = name {
        parts.push(label(name));
    }
    if let Some((count, kind)) = count {
        parts.push(format!("{count} {kind}"));
    }
    if !parts.is_empty() {
        write!(table, "<TR><TD BGCOLOR=\"{title}\" ALIGN=\"LEFT\"><FONT COLOR=\"{}\"><B>{}</B></FONT></TD></TR>", foreground(title_color), parts.join(" | ")).unwrap();
    }
    if let Some(description) = description {
        write!(
            table,
            "<TR><TD ALIGN=\"LEFT\">{}</TD></TR>",
            label(description)
        )
        .unwrap();
    }
    table
}

fn foreground([r, g, b]: [u8; 3]) -> &'static str {
    if little_clip_core::palette::contrast_ratio((r, g, b), (0, 0, 0))
        >= little_clip_core::palette::contrast_ratio((r, g, b), (255, 255, 255))
    {
        "#000000"
    } else {
        "#ffffff"
    }
}

fn row(
    table: &mut String,
    port: &str,
    number: Option<&str>,
    name: Option<&str>,
    color: Option<&str>,
    id: &str,
) -> Result<(), String> {
    write!(table, "<TR><TD PORT=\"{port}\" ID=\"{id}\" TITLE=\"Terminal\" ALIGN=\"LEFT\"><TABLE BORDER=\"0\" CELLBORDER=\"0\" CELLSPACING=\"0\" CELLPADDING=\"3\"><TR>").unwrap();
    if let Some(number) = number {
        write!(
            table,
            "<TD WIDTH=\"24\" ALIGN=\"LEFT\"><B>{}</B></TD>",
            label(number)
        )
        .unwrap();
    }
    if let Some(color) = color {
        for band in wireviz::colors::bands(color)? {
            write!(table, "<TD WIDTH=\"12\" BGCOLOR=\"{}\"></TD>", rgb(band)).unwrap();
        }
    }
    write!(
        table,
        "<TD ALIGN=\"LEFT\">{}</TD></TR></TABLE></TD></TR>",
        label(name.unwrap_or(""))
    )
    .unwrap();
    Ok(())
}

fn footer(table: &mut String, notes: Option<&str>) {
    if let Some(notes) = notes {
        write!(
            table,
            "<TR><TD ALIGN=\"LEFT\"><FONT POINT-SIZE=\"10\">{}</FONT></TD></TR>",
            label(notes)
        )
        .unwrap();
    }
    if !table.contains("<TR>") {
        table.push_str("<TR><TD> </TD></TR>");
    }
    table.push_str("</TABLE>");
}

fn port(endpoint: Endpoint, side: &str) -> String {
    match endpoint {
        Endpoint::Pin { connector, pin } => format!("c{connector}:p{pin}:{side}"),
        Endpoint::Cable {
            cable,
            conductor: Conductor::Wire(wire),
        } => format!("w{cable}:p{wire}:{side}"),
        Endpoint::Cable {
            cable,
            conductor: Conductor::Shield,
        } => format!("w{cable}:s:{side}"),
        Endpoint::Connector(connector) => format!("c{connector}"),
    }
}

fn link_color(harness: &Harness, endpoints: &[Endpoint; 2]) -> Result<String, String> {
    for endpoint in endpoints {
        if let Endpoint::Cable { cable, conductor } = *endpoint {
            let cable = &harness.cables[cable];
            let color = match conductor {
                Conductor::Wire(index) => cable.wires[index].color.as_deref(),
                Conductor::Shield => cable.shield.as_deref(),
            };
            if let Some(color) = color {
                // Parallel gray border bands keep even a white wire visible
                // against the paper background without changing its actual color.
                let colors = wireviz::colors::bands(color)?
                    .into_iter()
                    .map(rgb)
                    .collect::<Vec<_>>();
                return Ok(format!("#748899:{}:#748899", colors.join(":")));
            }
        }
    }
    Ok("#607485".into())
}

fn rgb([r, g, b]: [u8; 3]) -> String {
    format!("#{r:02x}{g:02x}{b:02x}")
}

#[cfg(test)]
mod tests {
    use super::*;
    const SOURCE: &str = "connectors:\n  J1: {pins: [\"01\", 2], pinlabels: [RETURN, DATA]}\n  J2: {pincount: 2}\ncables:\n  W1: {colors: [WH, GNYE], shield: true}\nconnections:\n  - [{J1: [\"01\", 2]}, {W1: [1-2]}, {J2: [2-1]}]\n  - [{J1: RETURN}, {W1: s}]\n";
    #[test]
    fn renders_port_tables_striped_wires_and_shields_as_outlined_vectors() {
        let (svg, warnings) = svg(SOURCE).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        assert!(svg.contains("wireviz-connector-0"));
        assert!(svg.contains("wireviz-link-4"));
        assert!(!svg.contains("<text"));
        let prepared = super::super::retained::prepare(&svg).unwrap();
        let pin = prepared
            .tree
            .node_by_id("graph-0-a_wireviz-pin-0-0")
            .expect("semantic pin group survives outlining");
        assert!(pin.abs_bounding_box().width() > 0.0);
        assert!(
            prepared
                .tree
                .node_by_id("graph-0-a_wireviz-shield-0")
                .is_some()
        );
        for color in ["#ffffff", "#00ff00", "#ffff00"] {
            assert!(svg.contains(color), "{color}");
        }
        let rendered =
            super::super::render_inline(little_clip_core::diagram::Format::Wireviz, SOURCE, None)
                .unwrap();
        assert!(rendered.width > 0 && rendered.height > 0 && !rendered.png.is_empty());
    }
    #[test]
    fn escapes_authored_text_and_never_accepts_color_as_dot_source() {
        let source = SOURCE
            .replace("RETURN, DATA", "'<IMG SRC=\"/etc/passwd\"/>', DATA")
            .replace("J1: RETURN", "J1: '01'");
        let source = source.replace(
            "J2: {pincount: 2}",
            "J2: {pincount: 2, notes: '<&>\" malicious; shape=box'}",
        );
        let harness = wireviz::parse(&source).unwrap();
        let dot = dot(&harness).unwrap();
        assert!(!dot.contains("<IMG"));
        assert!(dot.contains("&lt;IMG"));
        assert!(svg(&source).is_ok());
        assert!(svg(&SOURCE.replace("[WH, GNYE]", "['#000000\"; image=x', GN]")).is_err());
        assert!(svg(&SOURCE.replace("connections:", "unused_connections:")).is_err());
    }
}

#[cfg(test)]
mod presentation_tests {
    use super::*;

    #[test]
    fn hidden_pins_keep_connected_and_looped_ports_and_simple_rows_stay_selectable() {
        let source = "connectors:\n  C: {pincount: 4, hide_disconnected_pins: true, loops: [[3,4]]}\n  S: {style: simple, type: Terminal}\ncables:\n  HIDE_BUNDLE_NAME: {wirecount: 1, category: bundle, color_code: BW, show_name: false, show_wirecount: false}\nconnections:\n  - [{C: 2}, {HIDE_BUNDLE_NAME: 1}, S]\n";
        let harness = wireviz::parse(source).unwrap();
        let dot = dot(&harness).unwrap();
        assert!(!dot.contains("wireviz-pin-0-0"));
        for id in [
            "wireviz-pin-0-1",
            "wireviz-pin-0-2",
            "wireviz-pin-0-3",
            "wireviz-pin-1-0",
        ] {
            assert!(dot.contains(id), "{id}");
        }
        assert!(!dot.contains("HIDE_BUNDLE_NAME"));
        assert!(dot.contains("STYLE=\"DASHED\""));
        let (svg, warnings) = svg(source).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        assert!(svg.contains("stroke-dasharray"));
        let prepared = super::super::retained::prepare(&svg).unwrap();
        assert!(
            prepared
                .tree
                .node_by_id("graph-0-a_wireviz-pin-0-0")
                .is_none()
        );
        assert!(
            prepared
                .tree
                .node_by_id("graph-0-a_wireviz-pin-1-0")
                .is_some()
        );
        assert_eq!(
            harness.links,
            wireviz::parse(&source.replace(
                "hide_disconnected_pins: true",
                "hide_disconnected_pins: false"
            ))
            .unwrap()
            .links
        );
    }

    #[test]
    fn authored_backgrounds_are_safe_real_colors_and_all_hidden_pins_keep_a_component() {
        let source = "connectors:\n  C: {pincount: 2, show_name: false, show_pincount: false, hide_disconnected_pins: true, bgcolor_title: YE}\noptions: {bgcolor: '#123456', bgcolor_connector: '#654321'}\nconnections:\n  - [C]\n";
        let (svg, warnings) = svg(source).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        assert!(svg.contains("#123456") && svg.contains("#654321"));
        assert!(!svg.contains("wireviz-pin-0-0"));
        assert!(svg.contains("wireviz-connector-0"));
    }
}

#[cfg(test)]
mod wiring_tests {
    use super::*;
    use wireviz::edits::{Edit, Editable};

    #[test]
    fn source_wiring_transactions_keep_native_terminal_hit_identities() {
        let source = include_str!("../../../../docs/examples/diagrams/harness-wiring.wireviz");
        let disconnected = Editable::parse(source)
            .unwrap()
            .apply(source, Edit::Disconnect { link: 4 })
            .unwrap();
        let connected = Editable::parse(&disconnected)
            .unwrap()
            .apply(
                &disconnected,
                Edit::Connect {
                    from: Endpoint::Pin {
                        connector: 2,
                        pin: 0,
                    },
                    to: Endpoint::Cable {
                        cable: 0,
                        conductor: Conductor::Shield,
                    },
                },
            )
            .unwrap();
        let (svg, warnings) = svg(&connected).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        let prepared = super::super::retained::prepare(&svg).unwrap();
        for id in [
            "graph-0-a_wireviz-pin-0-0",
            "graph-0-a_wireviz-pin-1-1",
            "graph-0-a_wireviz-pin-2-0",
            "graph-0-a_wireviz-wire-0-1",
            "graph-0-a_wireviz-shield-0",
        ] {
            let node = prepared.tree.node_by_id(id).expect(id);
            assert!(node.abs_bounding_box().width() > 0.0, "{id}");
        }
        assert_eq!(wireviz::parse(&connected).unwrap().links.len(), 6);
        assert!(connected.contains("# three distinct parallel lanes"));
    }
}
