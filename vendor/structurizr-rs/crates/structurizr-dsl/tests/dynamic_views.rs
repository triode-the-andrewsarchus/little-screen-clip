use structurizr_core::Workspace;
use structurizr_dsl::parse;
use structurizr_render::SvgRenderer;

const MODEL: &str = r#"
    model {
        user = person "Reader"
        app = softwareSystem "Library" {
            web = container "Browser"
            api = container "API" { engine = component "Engine" }
        }
        store = softwareSystem "Store"
        user -> app "Uses" "HTTP"
        user -> web "Browses" "HTTP"
        user -> engine "Invokes" "RPC"
        app -> store "SQL read" "SQL"
        app -> store "REST read" "HTTPS" { tags "RPC" }
        store -> store "Refreshes" "Internal"
    }
"#;

fn workspace(views: &str) -> Workspace {
    parse(&format!("workspace {{\n{MODEL}\nviews {{\n{views}\n}}\n}}")).unwrap()
}

#[test]
fn parallel_groups_retain_every_step_and_use_the_documented_order() {
    let workspace = workspace(
        r#"
        dynamic * "Runtime" {
            user -> app "Request"
            {
                { app -> store "Fetch" "SQL" }
                { app -> store "Query" "HTTPS" }
            }
            app -> user "Reply"
            store -> store
        }
        "#,
    );
    let view = &workspace.views().dynamic_views[0];
    assert_eq!(
        view.steps
            .iter()
            .map(|step| step.order.as_str())
            .collect::<Vec<_>>(),
        ["1", "2", "2", "3", "4"]
    );
    assert_eq!(view.steps.len(), 5);
    assert!(view.steps[3].response);
    assert!(!view.steps[0].response);
    let svg = SvgRenderer::default()
        .render_dynamic(&workspace, view)
        .unwrap();
    assert_eq!(svg.matches("data-source-id=").count(), 5);
    for label in [
        "1. Request",
        "2. Fetch",
        "2. Query",
        "3. Reply",
        "4. Refreshes",
    ] {
        assert!(svg.contains(label), "{label}");
    }
    let response = svg
        .lines()
        .find(|line| line.contains("data-step-index=\"3\"") && line.contains("data-source-id="))
        .unwrap();
    assert!(response.contains("stroke-dasharray="));
    let self_call = svg
        .lines()
        .find(|line| line.contains("data-step-index=\"4\"") && line.contains("data-source-id="))
        .unwrap();
    assert!(
        self_call.contains("<path"),
        "a self-call needs a loop, not a zero-length line"
    );
    assert_eq!(
        svg.matches(">HTTP</text>").count(),
        2,
        "reply uses its request's technology"
    );
}

#[test]
fn explicit_orders_and_technology_bind_the_right_model_relationship() {
    let workspace = workspace(
        r##"
        dynamic * "Ordered" {
            9.3: app -> store "Fetch" "HTTPS"
            app -> store "" "SQL"
        }
        styles { relationship "RPC" { color "#123abc" thickness 3 } }
        "##,
    );
    let view = &workspace.views().dynamic_views[0];
    assert_eq!(view.steps[0].order, "9.3");
    assert_eq!(view.steps[1].order, "2");
    let relationships = &workspace.model().relationships;
    let selected = relationships
        .iter()
        .find(|relationship| Some(relationship.id) == view.steps[0].relationship_id)
        .unwrap();
    assert_eq!(selected.technology.as_deref(), Some("HTTPS"));
    assert_eq!(view.steps[1].description.as_deref(), Some("SQL read"));
    let svg = SvgRenderer::default()
        .render_dynamic(&workspace, view)
        .unwrap();
    let arrow = svg
        .lines()
        .find(|line| line.contains("data-step-index=\"0\"") && line.contains("data-source-id="))
        .unwrap();
    assert!(arrow.contains("#123abc"));
    assert!(arrow.contains("stroke-width=\"3\""));
    assert!(svg.contains("9.3. Fetch"));
    assert!(svg.contains(">HTTPS</text>"));
}

#[test]
fn scoped_dynamic_views_retain_annotations_and_component_endpoints() {
    let workspace = workspace(
        r#"
        dynamic app "Containers" {
            title "Browsing"
            description "Runtime"
            properties { "purpose" "Review" }
            default
            user -> web
        }
        dynamic api "Components" { user -> engine }
        "#,
    );
    let views = &workspace.views().dynamic_views;
    assert_eq!(views.len(), 2);
    assert_eq!(views[0].properties.title.as_deref(), Some("Browsing"));
    assert_eq!(views[0].properties.description.as_deref(), Some("Runtime"));
    assert_eq!(
        views[0]
            .properties
            .properties
            .get("purpose")
            .map(String::as_str),
        Some("Review")
    );
    assert!(views[0].properties.default);
    for view in views {
        assert_eq!(view.steps.len(), 1);
        let svg = SvgRenderer::default()
            .render_dynamic(&workspace, view)
            .unwrap();
        assert_eq!(svg.matches("data-source-id=").count(), 1);
    }
}

#[test]
fn invalid_scopes_endpoints_relations_and_overflow_are_errors_instead_of_partial_views() {
    for view in [
        "dynamic missing \"Bad\" { user -> app }",
        "dynamic user \"Bad\" { user -> app }",
        "dynamic * \"Bad\" { missing -> app }",
        "dynamic * \"Bad\" { user -> missing }",
        "dynamic * \"Bad\" { user -> store }",
        "dynamic * \"Bad\" { app -> user }",
        "dynamic * \"Bad\" { user -> app \"FTP\" \"FTP\" }",
        "dynamic * \"Bad\" { { { user -> missing } } }",
        "dynamic * \"Bad\" { user -> web }",
        "dynamic app \"Bad\" { user -> engine }",
        "dynamic app \"Bad\" { app -> store }",
        "dynamic api \"Bad\" { api -> engine }",
        "dynamic api \"Bad\" { app -> store }",
        "dynamic * \"Bad\" { include * }",
        "dynamic * \"Bad\" { 9223372036854775808: user -> app }",
    ] {
        let source = format!("workspace {{\n{MODEL}\nviews {{ {view} }} }}");
        assert!(parse(&source).is_err(), "{view}");
    }
}

#[test]
fn corrupted_runtime_bindings_do_not_render_unknown_or_mislabelled_interactions() {
    let workspace = workspace("dynamic * \"Runtime\" { user -> app }");
    let original = &workspace.views().dynamic_views[0];
    let renderer = SvgRenderer::default();
    let mut view = original.clone();
    view.steps[0].relationship_id = None;
    assert!(renderer.render_dynamic(&workspace, &view).is_err());
    let mut view = original.clone();
    view.steps[0].source_id = workspace.model().software_systems[1].id();
    assert!(renderer.render_dynamic(&workspace, &view).is_err());
}
