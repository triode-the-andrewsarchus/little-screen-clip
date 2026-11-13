use std::collections::HashSet;
use structurizr_dsl::parse;

#[test]
fn repeated_instances_have_distinct_placements_and_resolved_relationship_endpoints() {
    let workspace = parse(
        r#"workspace {
        model {
            early = deploymentNode "Early" { first = containerInstance api }
            app = softwareSystem "Library" { api = container "API" "Serves requests" "Rust" }
            deploymentEnvironment "Production" {
                host1 = deploymentNode "Host 1" {
                    second = containerInstance api
                    service = softwareSystemInstance app
                }
                host2 = deploymentNode "Host 2" { third = containerInstance api }
            }
            first -> second "Synchronizes"
            second -> third "Replicates"
        }
    }"#,
    )
    .unwrap();
    let model = workspace.model();
    let instances: Vec<_> = model
        .deployment_nodes
        .iter()
        .flat_map(|node| &node.container_instances)
        .collect();
    assert_eq!(instances.len(), 3);
    assert_eq!(
        instances
            .iter()
            .map(|instance| instance.id)
            .collect::<HashSet<_>>()
            .len(),
        3
    );
    let api = model.software_systems[0].containers[0].id();
    assert!(
        instances
            .iter()
            .all(|instance| instance.container_id == api)
    );
    assert_eq!(model.relationships.len(), 2);
    assert_eq!(
        (
            model.relationships[0].source_id,
            model.relationships[0].destination_id
        ),
        (instances[0].id, instances[1].id)
    );
    assert_eq!(
        (
            model.relationships[1].source_id,
            model.relationships[1].destination_id
        ),
        (instances[1].id, instances[2].id)
    );
    let system = &model.deployment_nodes[1].software_system_instances[0];
    assert_eq!(system.software_system_id, model.software_systems[0].id());
    assert!(!instances.iter().any(|instance| instance.id == system.id));
}

#[test]
fn instance_and_infrastructure_metadata_survives_without_running_health_checks() {
    let workspace = parse(
        r#"workspace {
        model {
            app = softwareSystem "Library" { api = container "API" }
            deploymentEnvironment "Production" {
                deploymentNode "Host" {
                    containerInstance api "" "Blue" {
                        description "Worker process"
                        properties { "shard" "1" }
                        healthCheck "Alive" "https://example.invalid/health" 30 500
                    }
                    softwareSystemInstance app "" "Green" { description "Whole service" }
                    infrastructureNode "Balancer" "Routes traffic" "Proxy" "Edge" {
                        properties { "region" "west" }
                    }
                }
            }
        }
    }"#,
    )
    .unwrap();
    let host = &workspace.model().deployment_nodes[0];
    let api = &host.container_instances[0];
    for tag in ["Element", "Container", "Container Instance", "Blue"] {
        assert!(api.tags.iter().any(|actual| actual == tag), "{tag}");
    }
    assert_eq!(api.description.as_deref(), Some("Worker process"));
    assert_eq!(api.properties.get("shard").map(String::as_str), Some("1"));
    assert_eq!(api.health_checks.len(), 1);
    assert_eq!(api.health_checks[0].interval, 30);
    assert_eq!(api.health_checks[0].timeout, 500);
    let system = &host.software_system_instances[0];
    assert_eq!(system.description.as_deref(), Some("Whole service"));
    assert!(system.tags.iter().any(|tag| tag == "Green"));
    assert!(
        system
            .tags
            .iter()
            .any(|tag| tag == "Software System Instance")
    );
    let balancer = &host.infrastructure_nodes[0];
    assert_eq!(balancer.technology.as_deref(), Some("Proxy"));
    assert!(balancer.properties.tags.iter().any(|tag| tag == "Edge"));
    assert_eq!(
        balancer
            .properties
            .properties
            .get("region")
            .map(String::as_str),
        Some("west")
    );
}

#[test]
fn missing_and_wrong_kind_instance_targets_are_errors_instead_of_placeholders() {
    for definition in [
        "containerInstance missing",
        "softwareSystemInstance missing",
        "containerInstance app",
        "softwareSystemInstance api",
        "containerInstance user",
        "softwareSystemInstance user",
    ] {
        let source = format!(
            r#"workspace {{ model {{
            user = person "Reader"
            app = softwareSystem "Library" {{ api = container "API" }}
            deploymentEnvironment "Production" {{ deploymentNode "Host" {{ {definition} }} }}
        }} }}"#
        );
        assert!(parse(&source).is_err(), "{definition}");
    }
}

#[test]
fn unsupported_group_constraints_fail_instead_of_becoming_descriptions_or_disappearing() {
    for definition in [
        "deploymentGroup \"Blue\"",
        "blue = deploymentGroup \"Blue\"",
        "deploymentNode \"Host\" { containerInstance api \"blue\" }",
        "deploymentNode \"Host\" { softwareSystemInstance app \"blue\" }",
    ] {
        let source = format!(
            r#"workspace {{ model {{
            app = softwareSystem "Library" {{ api = container "API" }}
            deploymentEnvironment "Production" {{ {definition} }}
        }} }}"#
        );
        let error = parse(&source).unwrap_err().to_string();
        assert!(error.contains("Deployment groups"), "{definition}: {error}");
    }
}
