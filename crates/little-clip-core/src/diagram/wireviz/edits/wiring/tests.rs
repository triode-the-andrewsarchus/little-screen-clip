use super::*;

const SOURCE: &str = "# retained heading\nconnectors:\n  J: {pins: ['01', 2, 3], pinlabels: [GND, POWER, DATA]}\n  K: {pincount: 3}\ncables:\n  W: {wirecount: 3, shield: true, colors: [RD, BK, WH]}\nconnections:\n  - - J: ['01', 2, 3] # keep selector note\n    - W: [1-3]\n    - K: [3-1]\n  - [{J: '01'}, {W: 1}]\nnotes: |-\n  An opaque section is not rewritten.\n";

fn pin(connector: usize, pin: usize) -> Endpoint {
    Endpoint::Pin { connector, pin }
}
fn wire(wire: usize) -> Endpoint {
    Endpoint::Cable {
        cable: 0,
        conductor: Conductor::Wire(wire),
    }
}
fn disconnect(source: &str, index: usize) -> String {
    Editable::parse(source)
        .unwrap()
        .apply(source, Edit::Disconnect { link: index })
        .unwrap()
}
fn assert_removed(before: &str, after: &str, index: usize) {
    let mut expected = super::super::super::parse(before).unwrap();
    expected.links.remove(index);
    let actual = super::super::super::parse(after).unwrap();
    assert_eq!(links(&actual), links(&expected));
    assert_eq!(actual.connectors, expected.connectors);
    assert_eq!(actual.cables, expected.cables);
    assert_eq!(actual.warnings, expected.warnings);
    assert_eq!(
        comments(&document(before).unwrap().0).unwrap(),
        comments(&document(after).unwrap().0).unwrap()
    );
}

#[test]
fn connects_stable_typed_id_and_shield_without_rewriting_existing_source() {
    let editor = Editable::parse(SOURCE).unwrap();
    let shield = Endpoint::Cable {
        cable: 0,
        conductor: Conductor::Shield,
    };
    let output = editor
        .apply(
            SOURCE,
            Edit::Connect {
                from: pin(0, 0),
                to: shield,
            },
        )
        .unwrap();
    assert!(output.starts_with(SOURCE.split("notes:").next().unwrap()));
    assert!(output.ends_with("notes: |-\n  An opaque section is not rewritten.\n"));
    assert!(output.contains("'01'"));
    let actual = super::super::super::parse(&output).unwrap();
    assert_eq!(actual.links.len(), editor.harness.links.len() + 1);
    assert_eq!(actual.links.last().unwrap().endpoints, [pin(0, 0), shield]);
    let repeated = Editable::parse(&output)
        .unwrap()
        .apply(
            &output,
            Edit::Connect {
                from: shield,
                to: pin(0, 0),
            },
        )
        .unwrap();
    assert_eq!(repeated, output);
}

#[test]
fn duplicate_connect_does_not_create_an_undo_change() {
    let output = Editable::parse(SOURCE)
        .unwrap()
        .apply(
            SOURCE,
            Edit::Connect {
                from: wire(0),
                to: pin(0, 0),
            },
        )
        .unwrap();
    assert_eq!(output, SOURCE);
}

#[test]
fn removes_exact_duplicate_origin_and_preserves_its_other_lane_links() {
    let output = disconnect(SOURCE, 0);
    assert_removed(SOURCE, &output, 0);
    assert!(output.contains("- [{J: '01'}, {W: 1}]"));
    assert!(output.contains("# keep selector note"));
    assert!(output.contains("An opaque section is not rewritten."));
    // The other source occurrence is still present once, rather than both
    // duplicate links disappearing through endpoint-set comparison.
    let actual = super::super::super::parse(&output).unwrap();
    assert_eq!(
        actual
            .links
            .iter()
            .filter(|link| link.endpoints == [pin(0, 0), wire(0)])
            .count(),
        1
    );
}

#[test]
fn splits_middle_of_ascending_and_descending_ranges_without_replacing_the_set() {
    let output = disconnect(SOURCE, 4);
    assert_removed(SOURCE, &output, 4);
    assert!(output.contains("J: ['01', 3] # keep selector note"));
    assert!(output.contains("W: [1, 3]"));
    assert!(output.contains("K: [3, 1]"));
}

#[test]
fn preserves_every_other_lane_for_all_links_in_a_multicolumn_path() {
    for index in 0..7 {
        let output = disconnect(SOURCE, index);
        assert_removed(SOURCE, &output, index);
    }
}

#[test]
fn single_lane_split_moves_authored_steps_and_their_comments() {
    let source = "connectors:\n  J: {pincount: 1}\n  K: {pincount: 1}\ncables:\n  W: {wirecount: 1}\nconnections:\n  - - J: 1 # keep prefix\n    - W: 1 # keep suffix wire\n    - K: 1 # keep suffix pin\nnotes: opaque\n";
    let output = disconnect(source, 0);
    assert_removed(source, &output, 0);
    assert!(output.contains("J: 1 # keep prefix"));
    assert!(output.contains("W: 1 # keep suffix wire"));
    assert!(output.contains("K: 1 # keep suffix pin"));
    assert!(output.ends_with("notes: opaque\n"));
}

#[test]
fn flow_path_split_preserves_its_authored_scalar_spelling() {
    let source = "connectors: {J: {pins: ['01']}, K: {pincount: 1}}\ncables: {W: {wirecount: 1}}\nconnections: [[{J: \"01\"}, {W: 1}, {K: 1}]]\n";
    let output = disconnect(source, 0);
    assert_removed(source, &output, 0);
    assert!(output.contains("{J: \"01\"}"));
}

#[test]
fn loop_removal_keeps_other_loops_and_external_connections() {
    let source = SOURCE.replace(
        "pinlabels: [GND, POWER, DATA]",
        "pinlabels: [GND, POWER, DATA], loops: [[GND, DATA], [GND, POWER]]",
    );
    let editor = Editable::parse(&source).unwrap();
    let selected = editor
        .harness
        .links
        .iter()
        .position(|link| link.kind == LinkKind::Loop)
        .unwrap();
    let output = editor
        .apply(&source, Edit::Disconnect { link: selected })
        .unwrap();
    assert_removed(&source, &output, selected);
    assert!(output.contains("loops: [[GND, POWER]]"));
}

#[test]
fn pin_mating_directions_survive_other_electrical_link_removal() {
    let source = "connectors: {J: {pincount: 2}, K: {pincount: 2}}\ncables: {W: {wirecount: 2}}\nconnections: [[{J: [1, 2]}, ['-->', '<--'], {K: [1, 2]}, {W: [1, 2]}]]\n";
    let output = disconnect(source, 2);
    assert_removed(source, &output, 2);
    let editor = Editable::parse(source).unwrap();
    assert!(
        editor
            .apply(source, Edit::Disconnect { link: 0 })
            .unwrap_err()
            .contains("Mating annotations")
    );
}

#[test]
fn refuses_stale_invalid_or_non_electrical_targets() {
    let editor = Editable::parse(SOURCE).unwrap();
    assert!(
        editor
            .apply(&format!("{SOURCE}\n"), Edit::Disconnect { link: 0 })
            .unwrap_err()
            .contains("changed")
    );
    assert!(
        editor
            .apply(SOURCE, Edit::Disconnect { link: usize::MAX })
            .is_err()
    );
    assert!(
        editor
            .apply(
                SOURCE,
                Edit::Connect {
                    from: pin(0, 0),
                    to: pin(1, 0)
                }
            )
            .is_err()
    );
    assert!(
        editor
            .apply(
                SOURCE,
                Edit::Connect {
                    from: pin(0, usize::MAX),
                    to: wire(0)
                }
            )
            .is_err()
    );
}

#[test]
fn refuses_alias_mutation_without_changing_an_unrelated_reference() {
    let source = "connectors: {J: {pincount: 2}, K: {pincount: 2}}\ncables: {W: {wirecount: 2}}\nconnections:\n  - - J: &pins [1, 2]\n    - W: *pins\n  - - K: *pins\n    - W: [1, 2]\n";
    let editor = Editable::parse(source).unwrap();
    assert!(editor.apply(source, Edit::Disconnect { link: 0 }).is_err());
    assert_eq!(editor.source, source);
}

#[test]
fn refuses_removal_when_owned_comment_would_be_discarded() {
    let source = "connectors: {J: {pincount: 2}}\ncables: {W: {wirecount: 2}}\nconnections:\n  - - J:\n      - 1 # cannot discard this note\n      - 2\n    - W: [1, 2]\n";
    let editor = Editable::parse(source).unwrap();
    assert!(editor.apply(source, Edit::Disconnect { link: 0 }).is_err());
    assert_eq!(editor.source, source);
}

#[test]
fn preserves_bom_crlf_and_quoted_designators_during_single_lane_move() {
    let source = "\u{feff}connectors:\r\n  'J/1.~': {pins: ['01']}\r\ncables:\r\n  'W/1.~': {wirecount: 1}\r\nconnections:\r\n  - - 'J/1.~': \"01\" # retained\r\n    - 'W/1.~': 1\r\n";
    let output = disconnect(source, 0);
    assert_removed(source, &output, 0);
    assert!(output.starts_with('\u{feff}'));
    assert!(!output.replace("\r\n", "").contains('\n'));
    assert!(output.contains("'J/1.~': \"01\" # retained"));
}

#[test]
fn rejects_merged_connection_ownership_and_set_budget_overflow() {
    let source = "connectors: {J: {pincount: 1}}\ncables: {W: {wirecount: 1}}\nbase: &base {connections: [[{J: 1}, {W: 1}]]}\n<<: *base\n";
    let editor = Editable::parse(source).unwrap();
    assert!(editor.apply(source, Edit::Disconnect { link: 0 }).is_err());
    let source = format!(
        "connectors: {{J: {{pincount: 1}}}}\ncables: {{W: {{wirecount: 1}}}}\nconnections:\n{}",
        "  - [J]\n  - [W]\n".repeat(512)
    );
    let editor = Editable::parse(&source).unwrap();
    assert!(
        editor
            .apply(
                &source,
                Edit::Connect {
                    from: pin(0, 0),
                    to: wire(0)
                }
            )
            .unwrap_err()
            .contains("budget")
    );
}

#[test]
fn independent_yaml_proof_rejects_anchor_definition_side_effects() {
    let source = "connectors: {J: {pincount: 2}}\ncables: {W: {wirecount: 2}}\nconnections:\n  - - J: &pins [1, 2]\n    - W: [1, 2]\nopaque_reference: *pins\n";
    let editor = Editable::parse(source).unwrap();
    let error = editor
        .apply(source, Edit::Disconnect { link: 0 })
        .unwrap_err();
    assert!(
        error.contains("shared YAML") || error.to_ascii_lowercase().contains("alias"),
        "{error}"
    );
    assert_eq!(editor.source, source);
}
