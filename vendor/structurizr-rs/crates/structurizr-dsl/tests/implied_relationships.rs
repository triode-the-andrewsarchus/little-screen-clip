use structurizr_dsl::parse;

const BODY: &str = r#"
    model {
        user = person "Reader"
        system = softwareSystem "Library" {
            api = container "API" {
                reader = component "Reader service"
            }
            store = container "Store"
        }
        user -> reader "Reads" "HTTPS"
        reader -> store "Loads" "SQL"
    }
"#;

#[test]
fn default_and_explicit_true_lift_endpoints_without_graph_transitivity() {
    for directive in ["", "!impliedRelationships true"] {
        let workspace = parse(&format!("workspace {{\n{directive}\n{BODY}\n}}")).unwrap();
        let model = workspace.model();
        let user = model.people[0].id();
        let system = model.software_systems[0].id();
        let api = model.software_systems[0].containers[0].id();
        let store = model.software_systems[0].containers[1].id();
        let reader = model.software_systems[0].containers[0].components[0].id();
        let pairs: std::collections::HashSet<_> = model
            .relationships
            .iter()
            .map(|r| (r.source_id, r.destination_id))
            .collect();
        assert_eq!(
            pairs.len(),
            model.relationships.len(),
            "ancestor pairs are deduplicated"
        );
        assert_eq!(
            pairs,
            [
                (user, reader),
                (user, api),
                (user, system),
                (reader, store),
                (api, store)
            ]
            .into_iter()
            .collect()
        );
        for relationship in &model.relationships {
            if relationship.source_id == user {
                assert_eq!(relationship.description.as_deref(), Some("Reads"));
                assert_eq!(relationship.technology.as_deref(), Some("HTTPS"));
            }
        }
        assert!(
            !pairs.contains(&(user, store)),
            "an indirect dependency must not invent a direct link"
        );
    }
}

#[test]
fn false_keeps_only_authored_endpoints_and_explicit_pairs_win() {
    let workspace = parse(&format!(
        "workspace {{\n!impliedRelationships false\n{BODY}\n}}"
    ))
    .unwrap();
    assert_eq!(workspace.model().relationships.len(), 2);
    for (is_system, additional) in [
        (true, "user -> system \"Explicit\" \"Mail\""),
        (false, "user -> api \"Explicit\" \"Mail\""),
    ] {
        for before in [true, false] {
            let original = "user -> reader \"Reads\" \"HTTPS\"";
            let replacement = if before {
                format!("{additional}\n        {original}")
            } else {
                format!("{original}\n        {additional}")
            };
            let body = BODY.replace(original, &replacement);
            let workspace = parse(&format!("workspace {{ {body} }}")).unwrap();
            let model = workspace.model();
            let source = model.people[0].id();
            let destination = if is_system {
                model.software_systems[0].id()
            } else {
                model.software_systems[0].containers[0].id()
            };
            let explicit: Vec<_> = model
                .relationships
                .iter()
                .filter(|r| r.source_id == source && r.destination_id == destination)
                .collect();
            assert_eq!(explicit.len(), 1);
            assert_eq!(explicit[0].description.as_deref(), Some("Explicit"));
            assert_eq!(explicit[0].technology.as_deref(), Some("Mail"));
        }
    }
}

#[test]
fn unsupported_java_strategy_is_diagnosed_instead_of_treated_as_true() {
    let source = "workspace { !impliedRelationships com.example.Strategy }";
    assert!(parse(source)
        .unwrap_err()
        .to_string()
        .contains("Unsupported implied relationship strategy"));
}
