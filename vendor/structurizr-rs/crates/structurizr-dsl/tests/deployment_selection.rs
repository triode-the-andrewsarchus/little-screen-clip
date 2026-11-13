use std::collections::HashSet;
use structurizr_core::Workspace;
use structurizr_dsl::parse;

const MODEL: &str = r#"
model {
    user = person "Reader"
    app = softwareSystem "Library" { api = container "API" }
    external = softwareSystem "Storage" { database = container "Database" }
    prod = deploymentEnvironment "Production" {
        region = deploymentNode "Region" {
            host = deploymentNode "Host" {
                one = containerInstance api
                two = containerInstance api
                balancer = infrastructureNode "Balancer"
            }
        }
        externalHost = deploymentNode "Storage host" { storage = containerInstance database }
    }
    deploymentEnvironment "Testing" { testHost = deploymentNode "Test host" { test = containerInstance api } }
    deploymentEnvironment "Empty" {}
    one -> two "Replicates"
    two -> test "Checks"
}
"#;

fn workspace(views: &str) -> Workspace {
    parse(&format!("workspace {{ {MODEL} views {{ {views} }} }}")).unwrap()
}

fn names(workspace: &Workspace, index: usize) -> HashSet<String> {
    let ids: HashSet<_> = workspace.views().deployment_views[index]
        .properties
        .elements
        .iter()
        .map(|element| element.id)
        .collect();
    workspace
        .model()
        .deployment_elements()
        .into_iter()
        .filter(|entry| ids.contains(&entry.element.id()))
        .map(|entry| match entry.element {
            structurizr_core::deployment::DeploymentElement::Node(node) => node.name().to_owned(),
            structurizr_core::deployment::DeploymentElement::Infrastructure(node) => {
                node.properties.name.clone()
            }
            _ => workspace
                .model()
                .find_element(entry.element.base_id().unwrap())
                .unwrap()
                .name()
                .to_owned(),
        })
        .collect()
}

#[test]
fn deployment_wildcards_follow_environment_scope_and_real_placement_containment() {
    let workspace = workspace(
        r#"
        deployment * prod "All" { include * }
        deployment app "Production" "Scoped" { include * }
        deployment * "Empty" "Empty" { include * }
    "#,
    );
    let views = &workspace.views().deployment_views;
    assert_eq!(views[0].environment, "Production");
    assert_eq!(views[0].properties.elements.len(), 7);
    assert_eq!(
        views[0].properties.relationships.len(),
        1,
        "cross-environment link is excluded"
    );
    assert_eq!(views[1].properties.elements.len(), 5);
    assert_eq!(
        names(&workspace, 1),
        ["Region", "Host", "API", "Balancer"]
            .into_iter()
            .map(String::from)
            .collect()
    );
    assert!(views[2].properties.elements.is_empty());
}

#[test]
fn explicit_leaf_selection_adds_only_its_parents_and_preserves_annotations() {
    let workspace = workspace(
        r#"
        deployment app prod "Chosen" {
            include one
            title "Inspect worker"
            description "One placement"
            properties { "purpose" "review" }
            autoLayout lr
            default
        }
    "#,
    );
    let view = &workspace.views().deployment_views[0];
    assert_eq!(view.properties.elements.len(), 3);
    assert_eq!(
        names(&workspace, 0),
        ["Region", "Host", "API"]
            .into_iter()
            .map(String::from)
            .collect()
    );
    assert!(view.properties.relationships.is_empty());
    assert_eq!(view.properties.title.as_deref(), Some("Inspect worker"));
    assert_eq!(
        view.properties.description.as_deref(),
        Some("One placement")
    );
    assert_eq!(
        view.properties
            .properties
            .get("purpose")
            .map(String::as_str),
        Some("review")
    );
    assert!(view.properties.auto_layout.is_some());
    assert!(view.properties.default);
}

#[test]
fn ordered_node_exclusion_removes_descendants_and_reinclusion_restores_the_branch() {
    let workspace = workspace(
        r#"
        deployment * prod "Removed" { include * exclude host }
        deployment * prod "Restored" { include * exclude * include host }
        deployment * prod "Empty" { include * exclude * }
        deployment * prod "Unselected" {}
    "#,
    );
    assert_eq!(
        names(&workspace, 0),
        ["Region", "Storage host", "Database"]
            .into_iter()
            .map(String::from)
            .collect()
    );
    assert_eq!(
        workspace.views().deployment_views[1]
            .properties
            .elements
            .len(),
        5
    );
    assert!(workspace.views().deployment_views[2]
        .properties
        .elements
        .is_empty());
    assert!(workspace.views().deployment_views[3]
        .properties
        .elements
        .is_empty());
}

#[test]
fn wrong_scopes_environments_and_members_are_errors_instead_of_fallbacks() {
    for view in [
        "deployment missing prod \"Bad\" { include * }",
        "deployment user prod \"Bad\" { include * }",
        "deployment api prod \"Bad\" { include * }",
        "deployment * \"Missing\" \"Bad\" { include * }",
        "deployment * prod \"Bad\" { include test }",
        "deployment * prod \"Bad\" { include app }",
        "deployment * prod \"Bad\" { include missing }",
        "deployment * prod \"Bad\" { include element.tag==Unknown }",
    ] {
        assert!(
            parse(&format!("workspace {{ {MODEL} views {{ {view} }} }}")).is_err(),
            "{view}"
        );
    }
}

#[test]
fn instance_abstraction_is_consistent_and_scoped_system_placements_are_rejected() {
    let model = r#"model {
        app = softwareSystem "Library" { api = container "API" }
        deploymentEnvironment "Production" {
            host = deploymentNode "Host" {
                system = softwareSystemInstance app
                process = containerInstance api
            }
        }
    }"#;
    let workspace = parse(&format!(
        r#"workspace {{ {model} views {{
        deployment * "Production" "All" {{ include * }}
        deployment app "Production" "Scoped" {{ include * }}
        deployment * "Production" "Chosen first" {{ include process include * }}
    }} }}"#
    ))
    .unwrap();
    let entries = workspace.model().deployment_elements();
    let all = &workspace.views().deployment_views[0].properties.elements;
    assert_eq!(all.len(), 2);
    assert!(
        all.iter().any(|entry| entry.id == entries[2].element.id()),
        "wildcard prefers the system placement"
    );
    let scoped = &workspace.views().deployment_views[1].properties.elements;
    assert_eq!(scoped.len(), 2);
    assert!(
        scoped
            .iter()
            .any(|entry| entry.id == entries[1].element.id()),
        "scoped wildcard selects its container placement"
    );
    assert_eq!(
        workspace.views().deployment_views[2]
            .properties
            .elements
            .len(),
        2
    );
    assert!(
        workspace.views().deployment_views[2]
            .properties
            .elements
            .iter()
            .any(|entry| entry.id == entries[1].element.id()),
        "wildcard retains the previously chosen container abstraction"
    );
    for view in [
        r#"deployment * "Production" "Mixed" { include system process }"#,
        r#"deployment app "Production" "Own" { include system }"#,
    ] {
        assert!(
            parse(&format!("workspace {{ {model} views {{ {view} }} }}")).is_err(),
            "{view}"
        );
    }
}

#[test]
fn including_one_host_does_not_select_or_inherit_abstraction_from_another_host() {
    let workspace = parse(
        r#"workspace { model {
        app = softwareSystem "Library" { api = container "API" }
        deploymentEnvironment "Production" {
            whole = deploymentNode "System host" { softwareSystemInstance app }
            worker = deploymentNode "Worker host" { containerInstance api }
        }
    } views { deployment * "Production" "Worker" { include worker } } }"#,
    )
    .unwrap();
    assert_eq!(
        names(&workspace, 0),
        ["Worker host", "API"]
            .into_iter()
            .map(String::from)
            .collect()
    );
}
