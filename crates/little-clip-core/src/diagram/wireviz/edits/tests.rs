use super::*;

const SOURCE: &str = "\
# Keep this harness comment.
connectors:
  J1:
    pins: [\"01\", 2, 3]
    pinlabels: [GND, POWER, DATA] # keep this comment
    loops: [[GND, DATA]]
  J2: {pincount: 3}
cables:
  W1: {wirecount: 3, colors: [BK, RD], wirelabels: [Earth, Supply, Signal]}
connections:
  - - J1: [GND, POWER, DATA] # keep the lane order
    - W1: [Earth, RD, Signal]
    - J2: [3-1]
  - - J1: GND
    - W1: 1
notes: |-
  Preserve this opaque section exactly.
";

fn pin_label(label: &str) -> Edit {
    Edit::PinLabel {
        connector: 0,
        pin: 0,
        label: label.into(),
    }
}
fn color(wire: usize, color: Option<&str>) -> Edit {
    Edit::WireColor {
        cable: 0,
        wire,
        color: color.map(str::to_owned),
    }
}

#[test]
fn changes_labels_and_references_without_changing_the_circuit_or_comments() {
    let editor = Editable::parse(SOURCE).unwrap();
    let updated = editor.apply(SOURCE, pin_label("RETURN")).unwrap();
    let expected = SOURCE
        .replace(
            "[GND, POWER, DATA] # keep this comment",
            "[RETURN, POWER, DATA] # keep this comment",
        )
        .replace("loops: [[GND, DATA]]", "loops: [['01', DATA]]")
        .replace(
            "[GND, POWER, DATA] # keep the lane order",
            "['01', POWER, DATA] # keep the lane order",
        )
        .replace("J1: GND", "J1: '01'");
    assert_eq!(updated, expected);
    let after = super::super::parse(&updated).unwrap();
    assert_eq!(after.links, editor.harness().links);
    assert_eq!(after.connectors[0].pins[0].label.as_deref(), Some("RETURN"));
    assert_eq!(
        editor.harness().connectors[0].pins[0].label.as_deref(),
        Some("GND")
    );
    assert_eq!(editor.apply(SOURCE, pin_label("GND")).unwrap(), SOURCE);
}

#[test]
fn preserves_bom_crlf_quoted_designators_and_yaml11_string_typing() {
    let source = format!(
        "\u{feff}{}",
        SOURCE.replace("J1", "J/1.~").replace('\n', "\r\n")
    );
    let editor = Editable::parse(&source).unwrap();
    let updated = editor.apply(&source, pin_label("NO")).unwrap();
    assert!(updated.starts_with('\u{feff}'));
    assert!(!updated.replace("\r\n", "").contains('\n'));
    let after = super::super::parse(&updated).unwrap();
    assert_eq!(after.connectors[0].pins[0].label.as_deref(), Some("NO"));
    assert_eq!(after.links, editor.harness().links);
    assert!(updated.ends_with("notes: |-\r\n  Preserve this opaque section exactly.\r\n"));
}

#[test]
fn materializes_repeating_colors_before_changing_one_wire() {
    let editor = Editable::parse(SOURCE).unwrap();
    let updated = editor.apply(SOURCE, color(0, Some("GN"))).unwrap();
    let after = super::super::parse(&updated).unwrap();
    assert_eq!(
        after.cables[0]
            .wires
            .iter()
            .map(|wire| wire.color.as_deref())
            .collect::<Vec<_>>(),
        [Some("GN"), Some("RD"), Some("BK")]
    );
    assert!(updated.contains("colors: [GN, RD, BK]"));
    assert_eq!(after.links, editor.harness().links);
    assert!(updated.contains("notes: |-\n  Preserve this opaque section exactly."));
    // A unique authored color reference is replaced by a stable wire number.
    let updated = editor.apply(SOURCE, color(1, Some("BUWH"))).unwrap();
    assert!(updated.contains("W1: [Earth, 2, Signal]"));
    assert_eq!(
        super::super::parse(&updated).unwrap().links,
        editor.harness().links
    );
    assert_eq!(editor.apply(SOURCE, color(0, Some("BK"))).unwrap(), SOURCE);
}

#[test]
fn inserts_missing_or_short_lists_and_clears_a_single_color() {
    let source = SOURCE
        .replace(
            "pinlabels: [GND, POWER, DATA] # keep this comment",
            "pinlabels: [GND] # keep this comment",
        )
        .replace("loops: [[GND, DATA]]", "loops: [['01', 3]]")
        .replace(
            "[GND, POWER, DATA] # keep the lane order",
            "['01', 2, 3] # keep the lane order",
        );
    let source = source.replace("J1: GND", "J1: '01'");
    let editor = Editable::parse(&source).unwrap();
    let edit = Edit::PinLabel {
        connector: 0,
        pin: 2,
        label: "DATA".into(),
    };
    let updated = editor.apply(&source, edit).unwrap();
    let after = super::super::parse(&updated).unwrap();
    assert_eq!(after.connectors[0].pins[1].label, None);
    assert_eq!(after.connectors[0].pins[2].label.as_deref(), Some("DATA"));
    assert!(updated.contains("# keep this comment"));
    let source = source.replace("    pinlabels: [GND] # keep this comment\n", "");
    let source = source
        .replace("colors: [BK, RD], ", "")
        .replace("[Earth, RD, Signal]", "[1, 2, 3]");
    let editor = Editable::parse(&source).unwrap();
    let updated = editor.apply(&source, pin_label("Ground")).unwrap();
    assert_eq!(
        super::super::parse(&updated).unwrap().connectors[0].pins[0]
            .label
            .as_deref(),
        Some("Ground")
    );
    assert_eq!(
        super::super::parse(&updated).unwrap().links,
        editor.harness().links
    );
    let updated = editor.apply(&source, color(1, Some("#112233"))).unwrap();
    assert_eq!(
        super::super::parse(&updated).unwrap().cables[0].wires[1]
            .color
            .as_deref(),
        Some("#112233")
    );
    assert_eq!(
        super::super::parse(&updated).unwrap().cables[0].wires[0].color,
        None
    );
    let original = Editable::parse(SOURCE).unwrap();
    let cleared = original.apply(SOURCE, color(1, None)).unwrap();
    let after = super::super::parse(&cleared).unwrap();
    assert_eq!(after.links, original.harness().links);
    assert_eq!(after.cables[0].wires[1].color, None);
    assert_eq!(after.cables[0].wires[2].color.as_deref(), Some("BK"));
}

#[test]
fn preserves_block_list_comments_while_replacing_and_appending_items() {
    let source = SOURCE.replace("pinlabels: [GND, POWER, DATA] # keep this comment", "pinlabels:\n      - GND # keep this comment\n      - POWER\n      - DATA")
        .replace("W1: {wirecount: 3, colors: [BK, RD], wirelabels: [Earth, Supply, Signal]}", "W1:\n    wirecount: 3\n    colors:\n      - BK # black wire\n      - RD # red wire\n    wirelabels: [Earth, Supply, Signal]");
    let editor = Editable::parse(&source).unwrap();
    let updated = editor.apply(&source, pin_label("RETURN")).unwrap();
    assert!(updated.contains("      - RETURN # keep this comment\n      - POWER\n      - DATA"));
    let updated = editor.apply(&source, color(2, Some("GN"))).unwrap();
    assert!(updated.contains("      - BK # black wire\n      - RD # red wire\n      - GN"));
    assert_eq!(
        super::super::parse(&updated).unwrap().cables[0].wires[2]
            .color
            .as_deref(),
        Some("GN")
    );
    assert_eq!(
        super::super::parse(&updated).unwrap().links,
        editor.harness().links
    );
}

#[test]
fn refuses_alias_and_merge_writes_and_shared_anchor_side_effects() {
    for (case, source) in [
        "connectors:\n  A: &shared {pincount: 1, pinlabels: [OLD]}\n  B: *shared\nconnections: [[A], [B]]\n",
        "base: &shared [OLD]\nconnectors:\n  A: {pincount: 1, pinlabels: *shared}\nconnections: [[A]]\n",
        "base: &shared {pincount: 1, pinlabels: [OLD]}\nconnectors:\n  A: {<<: *shared}\nconnections: [[A]]\n",
        "connectors:\n  A: &shared {pincount: 1, pinlabels: [OLD]}\nopaque_metadata: *shared\nconnections: [[A]]\n",
    ].into_iter().enumerate() {
        let editor = Editable::parse(source).unwrap();
        let error = editor.apply(source, pin_label("NEW")).unwrap_err();
        if matches!(case, 0 | 3) { assert!(error.contains("shared YAML data"), "{error}"); }
        assert_eq!(editor.apply(source, pin_label("OLD")).unwrap(), source);
        assert_eq!(editor.harness(), &super::super::parse(source).unwrap());
    }
}

#[test]
fn permits_local_overrides_and_anchor_edits_without_shared_value_changes() {
    for source in [
        "base: &base {pincount: 1}\nconnectors:\n  A: {<<: *base}\nconnections: [[A]]\n",
        "connectors:\n  A: &base {pincount: 1, pinlabels: [OLD]}\n  B: {<<: *base, pinlabels: [LOCAL]}\nconnections: [[A], [B]]\n",
    ] {
        let editor = Editable::parse(source).unwrap();
        let updated = editor.apply(source, pin_label("NEW")).unwrap();
        let after = super::super::parse(&updated).unwrap();
        assert_eq!(after.connectors[0].pins[0].label.as_deref(), Some("NEW"));
        assert_eq!(after.links, editor.harness().links);
        assert_eq!(after.connectors.get(1), editor.harness().connectors.get(1));
    }
}

#[test]
fn refuses_stale_targets_invalid_colors_and_invalid_field_values() {
    let editor = Editable::parse(SOURCE).unwrap();
    assert!(
        editor
            .apply(&format!("{SOURCE}\n"), pin_label("NEW"))
            .is_err()
    );
    for edit in [
        Edit::PinLabel {
            connector: 99,
            pin: 0,
            label: "NEW".into(),
        },
        Edit::PinLabel {
            connector: 0,
            pin: 99,
            label: "NEW".into(),
        },
        color(99, Some("BK")),
        color(0, Some("bogus")),
        pin_label(&"x".repeat(1025)),
        pin_label("bad\0label"),
    ] {
        assert!(editor.apply(SOURCE, edit).is_err());
    }
    assert_eq!(editor.apply(SOURCE, pin_label("GND")).unwrap(), SOURCE);
}

#[test]
fn respects_range_semantics_and_exact_pin_ids_over_labels() {
    let source = "connectors:\n  A: {pins: [1, 2], pinlabels: ['2', OTHER], loops: [['2', 1]]}\ncables:\n  W: {wirecount: 2}\nconnections:\n  - [{A: [1-2]}, {W: [1-2]}]\n";
    let editor = Editable::parse(source).unwrap();
    // Numeric 2 and string '2' are distinct pin identities; the string selects
    // pin 1's label here, while [1-2] is an actual numeric range.
    let updated = editor.apply(source, pin_label("GROUND")).unwrap();
    assert!(updated.contains("A: [1-2]"));
    assert_eq!(
        super::super::parse(&updated).unwrap().links,
        editor.harness().links
    );
    assert!(updated.contains("loops: [[1, 1]]"));
}

#[test]
fn bounds_reference_transactions_before_returning_any_source() {
    let mut source = String::from(
        "connectors:\n  A: {pincount: 1, pinlabels: [OLD]}\ncables:\n  W: {wirecount: 1}\nconnections:\n",
    );
    let row = "  - [{A: [OLD, OLD, OLD, OLD, OLD]}, {W: [1, 1, 1, 1, 1]}]\n";
    source.push_str(&row.repeat(820));
    // Model link count is within budget; authoring has a smaller operation cap.
    let editor = Editable::parse(&source).unwrap();
    assert!(
        editor
            .apply(&source, pin_label("NEW"))
            .unwrap_err()
            .contains("transaction budget")
    );
    assert_eq!(editor.apply(&source, pin_label("OLD")).unwrap(), source);
}

#[test]
fn bounds_patch_bytes_and_rejects_oversized_source_transactions() {
    let id = "X".repeat(8192);
    for (references, expected) in [
        (100, "transaction budget"),
        (40, "edited document would exceed a limit"),
    ] {
        let source = format!(
            "connectors:\n  A: {{pins: ['{id}'], pinlabels: [OLD]}}\ncables:\n  W: {{wirecount: 1}}\nconnections:\n{}",
            "  - [{A: OLD}, {W: 1}]\n".repeat(references)
        );
        let editor = Editable::parse(&source).unwrap();
        let error = editor.apply(&source, pin_label("NEW")).unwrap_err();
        assert!(error.contains(expected), "{error}");
        assert_eq!(editor.apply(&source, pin_label("OLD")).unwrap(), source);
    }
}

#[test]
fn rewrites_only_references_that_the_edit_would_invalidate_or_make_ambiguous() {
    let editor = Editable::parse(SOURCE).unwrap();
    let updated = editor.apply(SOURCE, pin_label("POWER")).unwrap();
    assert!(updated.contains("['01', 2, DATA] # keep the lane order"));
    assert_eq!(
        super::super::parse(&updated).unwrap().links,
        editor.harness().links
    );
    let updated = editor.apply(SOURCE, color(0, Some("RD"))).unwrap();
    // The other red conductor's symbolic color becomes ambiguous, so pin its
    // reference; stable authored wire labels retain their original spelling.
    assert!(updated.contains("W1: [Earth, 2, Signal]"));
    assert_eq!(
        super::super::parse(&updated).unwrap().links,
        editor.harness().links
    );
}
