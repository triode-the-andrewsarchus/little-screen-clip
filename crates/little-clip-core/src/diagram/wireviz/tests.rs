use super::*;

const HARNESS: &str = "\
connectors:
  X1:
    pins: [\"01\", 1, 3]
    pinlabels: [GND, POWER, DATA]
    pincolors: [BK]
  X2:
    pincount: 3
  UNUSED:
    style: simple
cables:
  W1:
    wirecount: 3
    colors: [BK, RD]
    wirelabels: [RETURN, SUPPLY, SIGNAL]
    shield: true
connections:
  - - X1: [\"01\", 1, DATA]
    - W1: [1-3]
    - X2: [3-1]
  - - X1: POWER
    - W1: s
";

#[test]
fn classifies_explicit_paths_and_only_distinctive_complete_yaml() {
    use crate::diagram::Format;
    use std::path::Path;
    for path in ["test.wireviz", "test.wire.yaml", "test.WIRE.YML"] {
        assert_eq!(Format::from_path(Path::new(path)), Some(Format::Wireviz));
        assert_eq!(
            Format::source_extension_hint(Path::new(path)),
            Some("wireviz")
        );
    }
    for extension in ["yaml", "yml"] {
        assert_eq!(Format::from_extension(extension), None);
        assert_eq!(
            Format::from_source_extension(extension, HARNESS),
            Some(Format::Wireviz)
        );
        for source in [
            "connectors: {}\nconnections: []",
            "connectors: [other]\nconnections: database",
            "title: Notes\ndraft: false",
            "connectors: {A: {pincount: 2}}\nconnections: [",
        ] {
            assert_eq!(Format::from_source_extension(extension, source), None);
        }
    }
    assert_eq!(Format::from_language("wireviz"), Some(Format::Wireviz));
    assert_eq!(Format::from_snippet(HARNESS), Some(Format::Wireviz));
    let commented = format!("# Harness comment\n{HARNESS}");
    assert!(
        matches!(crate::ClipItem::from_text(&commented).payload, crate::ClipPayload::Text(text) if text == commented)
    );
    assert_eq!(crate::syntax::syntect_fallback("wireviz"), "yaml");
    assert_eq!(
        crate::syntax::detect_language(HARNESS, Some("wireviz")),
        "wireviz"
    );
    assert!(!signature(&format!("{}{}", " ".repeat(32 * 1024), HARNESS)));
}

#[test]
fn normalizes_parallel_paths_ranges_and_one_ended_shields() {
    let harness = parse(HARNESS).unwrap();
    assert_eq!(harness.links.len(), 7);
    assert_eq!(harness.connectors[0].pins[0].id, PinId::Text("01".into()));
    assert_eq!(harness.connectors[0].pins[1].id, PinId::Number(1));
    assert_eq!(harness.cables[0].wires[2].color.as_deref(), Some("BK"));
    assert_eq!(harness.connectors[0].pins[2].color, None);
    assert_eq!(
        harness.links[3].endpoints,
        [
            Endpoint::Cable {
                cable: 0,
                conductor: Conductor::Wire(0)
            },
            Endpoint::Pin {
                connector: 1,
                pin: 2
            },
        ]
    );
    assert_eq!(
        harness.links[5].origin,
        Origin::Connection {
            set: 0,
            step: 1,
            lane: 2
        }
    );
    assert_eq!(
        harness.links[6].endpoints,
        [
            Endpoint::Pin {
                connector: 0,
                pin: 1
            },
            Endpoint::Cable {
                cable: 0,
                conductor: Conductor::Shield
            },
        ]
    );
    assert!(!harness.connectors[2].used);
    assert_eq!(harness.warnings.len(), 1);
}

#[test]
fn resolves_label_and_unique_color_references() {
    let source = HARNESS.replace("W1: [1-3]", "W1: [RETURN, RD, SIGNAL]");
    assert_eq!(parse(&source).unwrap().links, parse(HARNESS).unwrap().links);
    let error = parse(&HARNESS.replace("W1: [1-3]", "W1: [BK, RD, SIGNAL]")).unwrap_err();
    assert!(error.contains("Ambiguous wire"), "{error}");
    let source = HARNESS
        .replace("[RETURN, SUPPLY, SIGNAL]", "[RD, SUPPLY, SIGNAL]")
        .replace("W1: [1-3]", "W1: [RD, SUPPLY, SIGNAL]");
    assert_eq!(parse(&source).unwrap().links, parse(HARNESS).unwrap().links);
}

#[test]
fn includes_unconnected_components_without_inventing_wires() {
    let source = "connectors:\n  A: {pincount: 4}\ncables:\n  W: {wirecount: 2}\nconnections:\n  - [A]\n  - [W]\n";
    let harness = parse(source).unwrap();
    assert!(harness.connectors[0].used && harness.cables[0].used);
    assert!(harness.links.is_empty() && harness.warnings.is_empty());
    let source = source.replace("  - [A]\n  - [W]", "  - [A, W]");
    assert!(parse(&source).is_err());
}

#[test]
fn duplicate_labels_require_an_unambiguous_pin_identity() {
    let source = HARNESS.replace("[GND, POWER, DATA]", "[GND, POWER, POWER]");
    let error = parse(&source).unwrap_err();
    assert!(error.contains("Unknown pin"), "{error}");
    let source = source.replace("DATA]", "3]");
    let error = parse(&source).unwrap_err();
    assert!(error.contains("Ambiguous pin"), "{error}");
}

#[test]
fn validates_path_types_lane_counts_and_references() {
    for (from, to, expected) in [
        ("X2: [3-1]", "X2: [1, 2]", "counts must match"),
        ("W1: [1-3]", "X2: [1-3]", "alternate"),
        ("W1: [1-3]", "W1: [1, 2, 4]", "Unknown wire"),
        ("X1: POWER", "X1: MISSING", "Unknown pin"),
        ("shield: true", "shield: false", "has no shield"),
        ("W1: [1-3]", "W1: []", "Empty pin/wire"),
        ("W1: [1-3]", "W1: [0-999999999]", "budget"),
        ("X2: [3-1]", "X2.OTHER: [3-1]", "template instance"),
    ] {
        let error = parse(&HARNESS.replace(from, to)).unwrap_err();
        assert!(error.contains(expected), "{from} -> {to}: {error}");
    }
}

#[test]
fn normalizes_connector_loops_and_mating_arrows() {
    let source = "\
connectors:
  A: {pincount: 2, loops: [[1, 2]]}
  B: {pincount: 2}
connections:
  - - A: [1, 2]
    - [-->, <--]
    - B: [2, 1]
  - [A, <==>, B]
";
    let harness = parse(source).unwrap();
    assert_eq!(harness.links.len(), 4);
    assert_eq!(
        harness.links[0].kind,
        LinkKind::PinMating(Direction::Forward)
    );
    assert_eq!(
        harness.links[1].kind,
        LinkKind::PinMating(Direction::Backward)
    );
    assert_eq!(
        harness.links[2].kind,
        LinkKind::ConnectorMating(Direction::Both)
    );
    assert_eq!(
        harness.links[2].endpoints,
        [Endpoint::Connector(0), Endpoint::Connector(1)]
    );
    assert_eq!(harness.links[3].kind, LinkKind::Loop);
    assert_eq!(
        harness.links[3].origin,
        Origin::Loop {
            connector: 0,
            pair: 0
        }
    );
    assert!(parse(&source.replace("[1, 2]]", "[1, 3]]")).is_err());
    assert!(parse(&source.replace("[-->, <--]", "[-->]")).is_err());
    assert!(parse(&source.replace("[A, <==>, B]", "[A, <==>]")).is_err());
}

#[test]
fn maps_parallel_single_pin_components_and_named_splices() {
    let source = "\
connectors:
  A: {style: simple}
  B: {style: simple}
  C: {pincount: 2}
cables:
  W: {colors: [BU, WH]}
connections:
  - - [A, B]
    - W: [1-2]
    - C: [2-1]
";
    let harness = parse(source).unwrap();
    assert_eq!(harness.links.len(), 4);
    assert_eq!(
        harness.links[1].endpoints[0],
        Endpoint::Pin {
            connector: 1,
            pin: 0
        }
    );
    assert!(parse(&source.replace("[A, B]", "[A, C]")).is_err());
}

#[test]
fn merges_yaml_data_but_preserves_quoted_scalar_identities() {
    let source = "\
base: &base {pincount: 2}
connectors:
  A:
    <<: *base
    pins: [\"01\", 01]
  B: *base
cables:
  W: {wirecount: 2}
connections:
  - - A: [\"01\", 01]
    - W: [1, 2]
    - B: [1, 2]
";
    let harness = parse(source).unwrap();
    assert_eq!(harness.connectors[0].pins[0].id, PinId::Text("01".into()));
    assert_eq!(harness.connectors[0].pins[1].id, PinId::Number(1));
    assert_eq!(harness.links.len(), 4);
    assert_eq!(
        parse(&format!("\u{feff}{}", source.replace('\n', "\r\n"))).unwrap(),
        harness
    );
    assert!(
        parse(&source.replace("[\"01\", 01]", "[NO, ON]"))
            .unwrap_err()
            .contains("quote")
    );
}

#[test]
fn rejects_ambiguous_definitions_and_inconsistent_terminal_lists() {
    for (from, to) in [
        ("pins: [\"01\", 1, 3]", "pins: [1, 1, 3]"),
        ("pincount: 3", "pincount: 0"),
        ("pincount: 3", "pincount: 513"),
        ("pincount: 3", "pincount: 2\n    pins: [1]"),
        ("wirecount: 3", "wirecount: true"),
        ("colors: [BK, RD]", "colors: []\n    color_code: UNKNOWN"),
        ("W1:\n", "X1:\n"),
    ] {
        assert!(parse(&HARNESS.replace(from, to)).is_err(), "{from} -> {to}");
    }
    assert!(parse(&format!("{HARNESS}\nconnectors: {{}}\n")).is_err());
    assert!(parse(&format!("{HARNESS}\n---\nnotes: second document\n")).is_err());
}

#[test]
fn never_reads_component_images_or_executes_graphviz_tweaks() {
    let source = HARNESS.replace("pincount: 3", "pincount: 3\n    image: {src: /etc/passwd}");
    let source = format!("{source}\ntweak: {{append: [arbitrary DOT]}}\n");
    let harness = parse(&source).unwrap();
    assert_eq!(harness.links, parse(HARNESS).unwrap().links);
    assert!(harness.warnings.iter().any(|w| w.contains("images")));
    assert!(harness.warnings.iter().any(|w| w.contains("tweaks")));
    assert!(parse(&HARNESS.replace("X2:\n", "X2: !custom\n")).is_err());
}

#[test]
fn bounds_source_nesting_alias_expansion_and_total_links() {
    assert!(parse(&" ".repeat(MAX_SOURCE + 1)).is_err());
    assert!(parse(&format!("{HARNESS}\nnotes: '\0'\n")).is_err());
    let deep = format!("notes: {}0{}\n{HARNESS}", "[".repeat(65), "]".repeat(65));
    assert!(parse(&deep).is_err());
    let mut aliases = "l0: &l0 [x]\n".to_owned();
    for i in 1..7 {
        aliases.push_str(&format!(
            "l{i}: &l{i} [{}]\n",
            vec![format!("*l{}", i - 1); 10].join(", ")
        ));
    }
    assert!(parse(&format!("{aliases}{HARNESS}")).is_err());
    let source =
        "connectors:\n  A: {pincount: 512}\ncables:\n  W: {wirecount: 512}\nconnections:\n";
    let source = format!(
        "{source}{}",
        "  - [{A: [1-512]}, {W: [1-512]}]\n".repeat(17)
    );
    assert!(parse(&source).unwrap_err().contains("link budget"));
    let definitions: String = (0..17)
        .map(|i| format!("  A{i}: {{pincount: 512}}\n"))
        .collect();
    assert!(
        parse(&format!("connectors:\n{definitions}connections: []\n"))
            .unwrap_err()
            .contains("terminal budget")
    );
    let definitions: String = (0..257)
        .map(|i| format!("  A{i}: {{pincount: 1}}\n"))
        .collect();
    assert!(
        parse(&format!("connectors:\n{definitions}connections: []\n"))
            .unwrap_err()
            .contains("component budget")
    );
}

#[test]
fn infers_pincolors_and_resolves_selected_palettes_without_changing_source() {
    let source = "connectors:\n  C: {pincolors: [RD, BU]}\ncables:\n  W: {wirecount: 3, color_code: IEC}\nconnections:\n  - [{C: [1, 2]}, {W: [1, 3]}]\n";
    let before = source.to_owned();
    let harness = parse(source).unwrap();
    assert_eq!(harness.connectors[0].pins.len(), 2);
    assert_eq!(harness.connectors[0].pins[1].color.as_deref(), Some("BU"));
    assert_eq!(
        harness.cables[0]
            .wires
            .iter()
            .map(|wire| wire.color.as_deref())
            .collect::<Vec<_>>(),
        [Some("BN"), Some("RD"), Some("OG")]
    );
    assert_eq!(source, before);
    let explicit =
        parse(&source.replace("color_code: IEC", "color_code: UNKNOWN, colors: [GN]")).unwrap();
    assert!(
        explicit.cables[0]
            .wires
            .iter()
            .all(|wire| wire.color.as_deref() == Some("GN"))
    );
    assert!(
        parse(&source.replace("color_code: IEC", "color_code: UNKNOWN"))
            .unwrap_err()
            .contains("color_code")
    );
}

#[test]
fn presentation_defaults_overrides_and_omissions_are_explicit() {
    let source = "connectors:\n  S: {style: simple}\n  C: {pincount: 2, show_name: false, hide_disconnected_pins: true, bgcolor_title: '#123456'}\ncables:\n  B: {category: bundle, colors: [RD, BU]}\noptions: {bgcolor: BK, bgcolor_node: GY, bgcolor_bundle: WH}\nconnections:\n  - [S]\n  - [C]\n  - [B]\n";
    let h = parse(source).unwrap();
    assert!(!h.connectors[0].show_name && !h.connectors[0].show_pincount);
    assert!(!h.connectors[1].show_name && h.connectors[1].hide_disconnected_pins);
    assert_eq!(h.connectors[1].title_background, Some([0x12, 0x34, 0x56]));
    assert_eq!(h.connectors[1].background, Some([153; 3]));
    assert!(!h.cables[0].show_wirenumbers && h.cables[0].show_wirecount);
    assert_eq!(h.cables[0].background, Some([255; 3]));
    assert_eq!(h.background, Some([0; 3]));
    assert!(h.warnings.is_empty());
    for (from, to, message) in [
        ("show_name: false", "show_name: 'false'", "boolean"),
        ("bgcolor: BK", "bgcolor: GNYE", "solid"),
        ("bgcolor: BK", "bgcolor: 'red; image=x'", "color"),
    ] {
        assert!(
            parse(&source.replace(from, to))
                .unwrap_err()
                .to_lowercase()
                .contains(message)
        );
    }
    let omitted = parse(&source.replace(
        "bgcolor: BK",
        "fontname: Unavailable, color_mode: HEX, bgcolor: BK",
    ))
    .unwrap();
    assert!(
        omitted
            .warnings
            .iter()
            .any(|warning| warning.contains("font"))
    );
    assert!(
        omitted
            .warnings
            .iter()
            .any(|warning| warning.contains("color-name"))
    );
}
