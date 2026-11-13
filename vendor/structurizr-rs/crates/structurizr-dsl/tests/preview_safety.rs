use structurizr_dsl::parse;

#[test]
fn memory_parser_rejects_unfinished_extra_and_excessively_nested_workspaces() {
    for source in [
        "workspace { model { person \"Reader\" }",
        "workspace {} }",
        "workspace {} workspace {}",
        "workspace {} unexpected",
    ] {
        assert!(parse(source).is_err(), "{source}");
    }
    let nested = format!(
        "workspace {{ {} {} }}",
        "group \"nested\" {".repeat(65),
        "}".repeat(65)
    );
    assert!(parse(&nested)
        .unwrap_err()
        .to_string()
        .contains("depth budget"));
    assert!(parse(&" ".repeat(256 * 1024 + 1))
        .unwrap_err()
        .to_string()
        .contains("byte budget"));
    assert!(parse(&"\n".repeat(16_385))
        .unwrap_err()
        .to_string()
        .contains("token budget"));
}

#[test]
fn quoted_braces_and_comments_do_not_affect_structure_validation() {
    let source = "workspace \"Brace { test }\" {\n// { unmatched comment\nmodel { reader = person \"Reader }\" }\n}";
    assert_eq!(parse(source).unwrap().model().people.len(), 1);
}

#[test]
fn includes_extension_and_execution_cannot_be_silently_ignored() {
    for source in [
        "workspace extends \"/etc/passwd\" {}",
        "workspace { !include \"/etc/passwd\" }",
        "workspace { !extend \"https://example.com/workspace.dsl\" }",
        "workspace { !plugin \"com.example.Plugin\" }",
        "workspace { !script \"script.lua\" }",
    ] {
        let error = parse(source).unwrap_err().to_string();
        assert!(error.contains("self-contained"), "{source}: {error}");
    }
    let source = "workspace { model { reader = person \"!include is a label\" } }";
    assert_eq!(parse(source).unwrap().model().people.len(), 1);
}
