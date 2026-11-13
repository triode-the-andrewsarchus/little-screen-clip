use oneyaml::Patch;
use oneyaml::{Document, Limits, Settings};
use std::str::FromStr;

fn patch(path: &str) -> Patch {
    Patch::from_json(&format!(
        r#"[{{"op":"replace","path":"{path}","value":"POWER"}}]"#
    ))
    .unwrap()
}

fn preserving_literals() {
    let settings = Settings::new().limits(Limits::untrusted()).merge_keys(true);
    for (name, source) in [
        (
            "flow",
            "# Harness\nconnectors:\n  X1:\n    pins: [\"01\", 2]\n    pinlabels: [GND, VCC] # labels\nnotes: |\n  keep me\n",
        ),
        (
            "block",
            "# Harness\nconnectors:\n  X1:\n    pins: [\"01\", 2]\n    pinlabels:\n      - GND # ground\n      - VCC\nnotes: untouched\n",
        ),
        (
            "bom_crlf",
            "\u{feff}# Harness\r\nconnectors:\r\n  X1:\r\n    pinlabels: [GND, VCC] # labels\r\nnotes: untouched\r\n",
        ),
        (
            "escaped_identity",
            "connectors:\n  'X/1.~':\n    pinlabels: [GND, VCC]\nnotes: untouched\n",
        ),
    ] {
        let key = if name == "escaped_identity" {
            "X/1.~"
        } else {
            "X1"
        };
        let pointer = format!(
            "/connectors/{}/pinlabels/1",
            key.replace('~', "~0").replace('/', "~1")
        );
        let mut doc = Document::parse_with(source, &settings).unwrap();
        let exact = doc.to_string() == source;
        let edited = doc.apply_patch(&patch(&pointer));
        println!(
            "oneyaml {name}: roundtrip={exact}, edit={edited:?}, exact_replacement={}",
            doc.to_string() == source.replace("VCC", "POWER")
        );
        assert!(exact && edited.is_ok());
        assert_eq!(doc.to_string(), source.replace("VCC", "POWER"));

        match yaml_edit::YamlFile::from_str(source) {
            Ok(yaml) => {
                let roundtrip = yaml.to_string() == source;
                let changed = yaml
                    .document()
                    .and_then(|d| d.get_mapping("connectors"))
                    .and_then(|m| m.get_mapping(key))
                    .and_then(|m| m.get_sequence("pinlabels"))
                    .is_some_and(|s| s.set(1, "POWER"));
                println!(
                    "yaml-edit {name}: roundtrip={roundtrip}, edit={changed}, exact_replacement={}",
                    yaml.to_string() == source.replace("VCC", "POWER")
                );
                assert!(roundtrip && changed);
                assert_eq!(yaml.to_string(), source.replace("VCC", "POWER"));
            }
            Err(e) => panic!("yaml-edit {name}: unexpected parse error: {e:?}"),
        }
    }
}

fn alias_and_merge_writes() {
    let settings = Settings::new().limits(Limits::untrusted()).merge_keys(true);
    for (name, source, pointer) in [
        (
            "alias",
            "base: &pins [GND, VCC]\nconnectors:\n  X1:\n    pinlabels: *pins\n",
            "/connectors/X1/pinlabels/1",
        ),
        (
            "merge",
            "base: &base {pinlabels: [GND, VCC]}\nconnectors:\n  X1:\n    <<: *base\n",
            "/connectors/X1/pinlabels/1",
        ),
    ] {
        let mut doc = Document::parse_with(source, &settings).unwrap();
        println!(
            "oneyaml {name}: refused={}, unchanged={}",
            doc.apply_patch(&patch(pointer)).is_err(),
            doc.to_string() == source
        );
        assert_eq!(doc.to_string(), source);
        assert!(doc.apply_patch(&patch(pointer)).is_err());
    }
}

fn invalid_inputs() {
    let settings = Settings::new().limits(Limits::untrusted()).merge_keys(true);
    for (name, source) in [
        ("duplicate", "connectors: {}\nconnectors: {}\n"),
        ("malformed", "connectors: [broken\n"),
        (
            "deep",
            &format!("x: {}0{}", "[".repeat(1000), "]".repeat(1000)),
        ),
    ] {
        println!(
            "oneyaml {name}: refused={}",
            Document::parse_with(source, &settings)
                .and_then(|d| d.to_value())
                .is_err()
        );
        assert!(
            Document::parse_with(source, &settings)
                .and_then(|d| d.to_value())
                .is_err()
        );
        let refused = yaml_edit::YamlFile::from_str(source).is_err();
        println!("yaml-edit {name}: refused={refused}");
        assert_eq!(refused, name != "duplicate");
    }
}

fn rollback_and_expansion() {
    let settings = Settings::new().limits(Limits::untrusted()).merge_keys(true);
    let source = "connectors:\n  X1:\n    pinlabels: [GND, VCC]\n";
    let mut doc = Document::parse_with(source, &settings).unwrap();
    let multi = Patch::from_json(r#"[{"op":"replace","path":"/connectors/X1/pinlabels/1","value":"POWER"},{"op":"replace","path":"/missing/0","value":0}]"#).unwrap();
    let refused = doc.apply_patch(&multi).is_err();
    println!(
        "oneyaml rollback: refused={refused}, unchanged={}",
        doc.to_string() == source
    );
    assert!(refused);
    assert_eq!(doc.to_string(), source);
    let mut laughs = "l0: &l0 [lol]\n".to_owned();
    for i in 1..10 {
        let refs = vec![format!("*l{}", i - 1); 10].join(", ");
        laughs.push_str(&format!("l{i}: &l{i} [{refs}]\n"));
    }
    let refused = Document::parse_with(laughs, &settings)
        .and_then(|d| d.to_value())
        .is_err();
    println!("oneyaml alias_expansion: refused={refused}");
    assert!(refused);
}

fn scalar_compatibility() {
    let yaml11 = oneyaml::Compat::yaml_1_1().limits(Limits::untrusted());
    for (name, text) in [
        ("identity", "pins: [\"01\", 01, NO, ON, \"NO\"]\n"),
        ("multiple_documents", "connectors: {}\n---\ncables: {}\n"),
        ("tagged", "connectors: !custom {}\n"),
    ] {
        let result = Document::parse_with(text, &yaml11).and_then(|d| d.to_value());
        println!("oneyaml yaml11 {name}: {result:?}");
        match name {
            "identity" => {
                let value = result.unwrap();
                assert_eq!(value["pins"][0].as_str(), Some("01"));
                assert_eq!(value["pins"][1].as_i64(), Some(1));
                assert_eq!(value["pins"][2].as_bool(), Some(false));
                assert_eq!(value["pins"][3].as_bool(), Some(true));
                assert_eq!(value["pins"][4].as_str(), Some("NO"));
            }
            "multiple_documents" => assert!(result.is_err()),
            "tagged" => assert!(result.unwrap()["connectors"].as_mapping().is_none()),
            _ => unreachable!(),
        }
    }
}

fn shared_anchor_definition() {
    let settings = Settings::new().limits(Limits::untrusted()).merge_keys(true);
    let mut anchor = Document::parse_with(
        "connectors:\n  X1: &shared {pinlabels: [GND, VCC]}\n  X2: *shared\n",
        &settings,
    )
    .unwrap();
    let before = anchor.to_value().unwrap();
    let edited = anchor.apply_patch(&patch("/connectors/X1/pinlabels/1"));
    println!(
        "oneyaml anchor_definition: edit={edited:?}, other_component_changed={}",
        before["connectors"]["X2"] != anchor.to_value().unwrap()["connectors"]["X2"]
    );
    assert!(edited.is_ok());
    assert_ne!(
        before["connectors"]["X2"],
        anchor.to_value().unwrap()["connectors"]["X2"]
    );
}

fn main() {
    preserving_literals();
    alias_and_merge_writes();
    invalid_inputs();
    rollback_and_expansion();
    scalar_compatibility();
    shared_anchor_definition();
}
