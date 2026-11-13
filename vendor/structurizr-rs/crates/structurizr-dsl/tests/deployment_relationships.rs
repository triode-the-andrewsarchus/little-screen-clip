use std::collections::{HashMap, HashSet};
use structurizr_dsl::parse;

const MODEL: &str = r#"
    app = softwareSystem "Library" {
        api = container "API"
        db = container "Database"
    }
    api -> db "Reads" "SQL" { tags "Data" properties { "mode" "read" } }
    deploymentEnvironment "Production" {
        deploymentNode "Host" {
            a = containerInstance api
            b = containerInstance api
            c = containerInstance db
            d = containerInstance db
        }
    }
    deploymentEnvironment "Testing" {
        deploymentNode "Test host" { e = containerInstance api f = containerInstance db }
    }
"#;

#[test]
fn model_links_replicate_between_all_same_environment_instances_with_metadata() {
    for directive in ["", "!impliedRelationships false"] {
        let workspace = parse(&format!(
            r#"workspace {{ {directive} model {{ {MODEL} }}
            views {{ deployment * "Production" "Runtime" {{ include * }} }}
        }}"#
        ))
        .unwrap();
        let model = workspace.model();
        let inventory = model.deployment_elements();
        let environments: HashMap<_, _> = inventory
            .iter()
            .map(|entry| (entry.element.id(), entry.environment))
            .collect();
        let replicated: Vec<_> = model
            .relationships
            .iter()
            .filter(|relationship| environments.contains_key(&relationship.source_id))
            .collect();
        assert_eq!(
            replicated.len(),
            5,
            "four production pairs and one testing pair"
        );
        assert_eq!(
            replicated
                .iter()
                .map(|relationship| relationship.id)
                .collect::<HashSet<_>>()
                .len(),
            5
        );
        for relationship in replicated {
            assert_eq!(
                environments[&relationship.source_id],
                environments[&relationship.destination_id]
            );
            assert_eq!(relationship.description.as_deref(), Some("Reads"));
            assert_eq!(relationship.technology.as_deref(), Some("SQL"));
            assert!(relationship.tags.iter().any(|tag| tag == "Data"));
            assert_eq!(
                relationship.properties.get("mode").map(String::as_str),
                Some("read")
            );
        }
        assert_eq!(
            workspace.views().deployment_views[0]
                .properties
                .relationships
                .len(),
            4
        );
    }
}

#[test]
fn explicit_instance_links_take_precedence_and_environment_nested_links_survive() {
    let source = format!(
        r#"workspace {{ model {{ {MODEL}
        a -> c "Authored override" "Custom"
        deploymentEnvironment "More" {{
            node = deploymentNode "More host" {{
                x = containerInstance api
                y = containerInstance db
                x -> y "Inside node"
            }}
            y -> x "Inside environment"
        }}
    }} }}"#
    );
    let workspace = parse(&source).unwrap();
    let descriptions: Vec<_> = workspace
        .model()
        .relationships
        .iter()
        .filter_map(|relationship| relationship.description.as_deref())
        .collect();
    assert_eq!(
        descriptions
            .iter()
            .filter(|description| **description == "Reads")
            .count(),
        5,
        "one model link plus four generated links; explicit pairs are preserved"
    );
    for expected in ["Authored override", "Inside node", "Inside environment"] {
        assert_eq!(
            descriptions
                .iter()
                .filter(|description| **description == expected)
                .count(),
            1,
            "{expected}"
        );
    }
}

#[test]
fn nested_unknown_endpoints_are_errors_and_grouped_authored_links_are_retained() {
    for body in [
        "user = person \"Reader\" { user -> missing }",
        "deploymentEnvironment \"Prod\" { n = deploymentNode \"Host\" { n -> missing } }",
        "deploymentEnvironment \"Prod\" { n = deploymentNode \"Host\" n -> missing }",
    ] {
        assert!(
            parse(&format!("workspace {{ model {{ {body} }} }}")).is_err(),
            "{body}"
        );
    }
    let workspace = parse(
        r#"workspace { model {
        app = softwareSystem "Library"
        group "People" { user = person "Reader" { user -> app "Uses" } }
    } }"#,
    )
    .unwrap();
    assert_eq!(workspace.model().relationships.len(), 1);
    assert_eq!(
        workspace.model().relationships[0].description.as_deref(),
        Some("Uses")
    );
}

#[test]
fn relationship_expansion_has_an_explicit_budget() {
    let mut source = String::from(
        r#"workspace { model {
        app = softwareSystem "Library" { api = container "API" db = container "Database" }
        api -> db "Reads"
        deploymentEnvironment "Production" { deploymentNode "Host" {
    "#,
    );
    for _ in 0..129 {
        source.push_str("containerInstance api\ncontainerInstance db\n");
    }
    source.push_str("} } } }");
    let error = parse(&source).unwrap_err().to_string();
    assert!(error.contains("relationship budget"), "{error}");
}

#[test]
fn replicated_links_keep_multiple_declarations_and_self_relationships() {
    let workspace = parse(
        r#"workspace { model {
        app = softwareSystem "Library" { api = container "API" db = container "Database" }
        api -> db "SQL query" "SQL"
        api -> db "REST query" "HTTP"
        api -> api "Refresh"
        deploymentEnvironment "Production" { deploymentNode "Host" {
            containerInstance api containerInstance db
        } }
    } }"#,
    )
    .unwrap();
    let inventory = workspace.model().deployment_elements();
    let ids: HashSet<_> = inventory.iter().map(|entry| entry.element.id()).collect();
    let links: Vec<_> = workspace
        .model()
        .relationships
        .iter()
        .filter(|relationship| ids.contains(&relationship.source_id))
        .collect();
    assert_eq!(links.len(), 3);
    assert_eq!(
        links
            .iter()
            .filter(|relationship| relationship.source_id == relationship.destination_id)
            .count(),
        1
    );
    assert!(links
        .iter()
        .any(|relationship| relationship.technology.as_deref() == Some("SQL")));
    assert!(links
        .iter()
        .any(|relationship| relationship.technology.as_deref() == Some("HTTP")));
}
