use std::collections::BTreeSet;
use structurizr_core::{model::Model, view::ViewProperties, Workspace};
use structurizr_dsl::parse;
use structurizr_render::SvgRenderer;

const MODEL: &str = r#"
    model {
        user = person "Reader"
        outside = person "Outside"
        alone = person "Alone"
        system = softwareSystem "Library" {
            api = container "API" {
                engine = component "Engine"
                spare = component "Spare"
            }
            store = container "Store"
            unused = container "Unused"
        }
        external = softwareSystem "External"
        unrelated = softwareSystem "Unrelated" {
            remote = container "Remote"
        }
        user -> engine "Uses"
        engine -> store "Loads"
        engine -> external "Calls"
        outside -> unrelated "Other"
    }
"#;

fn workspace(views: &str, directive: &str) -> Workspace {
    parse(&format!(
        "workspace {{\n{directive}\n{MODEL}\nviews {{\n{views}\n}}\n}}"
    ))
    .unwrap()
}

fn members(model: &Model, properties: &ViewProperties) -> BTreeSet<String> {
    properties
        .elements
        .iter()
        .map(|element| model.find_element(element.id).unwrap().name().to_owned())
        .collect()
}

fn painted(model: &Model, svg: &str) -> BTreeSet<String> {
    model
        .architecture_elements()
        .filter(|element| svg.contains(&format!("data-element-id=\"{}\"", element.id())))
        .map(|element| element.name().to_owned())
        .collect()
}

#[test]
fn wildcard_scope_selects_direct_neighbours_and_keeps_isolated_local_elements() {
    let workspace = workspace(
        r#"
        systemLandscape "Landscape" { include * }
        systemContext system "Context" { include * }
        container system "Containers" { include * }
        component api "Components" { include * }
        "#,
        "",
    );
    let model = workspace.model();
    let views = workspace.views();
    let renderer = SvgRenderer::default();
    for (properties, svg, expected) in [
        (
            &views.system_landscape_views[0].properties,
            renderer
                .render_system_landscape(&workspace, &views.system_landscape_views[0])
                .unwrap(),
            vec![
                "Reader",
                "Outside",
                "Alone",
                "Library",
                "External",
                "Unrelated",
            ],
        ),
        (
            &views.system_context_views[0].properties,
            renderer
                .render_system_context(&workspace, &views.system_context_views[0])
                .unwrap(),
            vec!["Reader", "Library", "External"],
        ),
        (
            &views.container_views[0].properties,
            renderer
                .render_container(&workspace, &views.container_views[0])
                .unwrap(),
            vec!["Reader", "API", "Store", "Unused", "External"],
        ),
        (
            &views.component_views[0].properties,
            renderer
                .render_component(&workspace, &views.component_views[0])
                .unwrap(),
            vec!["Reader", "Engine", "Spare", "Store", "External"],
        ),
    ] {
        let expected = expected
            .into_iter()
            .map(str::to_owned)
            .collect::<BTreeSet<_>>();
        assert_eq!(members(model, properties), expected, "{}", properties.key);
        assert_eq!(painted(model, &svg), expected, "{}", properties.key);
        assert_eq!(properties.elements.len(), expected.len());
    }
}

#[test]
fn explicit_disconnected_nodes_and_ordered_reinclusion_survive_rendering() {
    let workspace = workspace(
        r#"
        systemLandscape "Explicit" { include "alone" alone }
        systemLandscape "Order" {
            include *
            exclude *
            include alone outside
            exclude outside
        }
        container system "Disconnected" { include unused alone }
        "#,
        "",
    );
    let renderer = SvgRenderer::default();
    for view in &workspace.views().system_landscape_views {
        let svg = renderer.render_system_landscape(&workspace, view).unwrap();
        assert_eq!(
            painted(workspace.model(), &svg),
            ["Alone".to_owned()].into_iter().collect()
        );
        assert_eq!(view.properties.elements.len(), 1);
    }
    let view = &workspace.views().container_views[0];
    let svg = renderer.render_container(&workspace, view).unwrap();
    assert_eq!(
        painted(workspace.model(), &svg),
        ["Alone".to_owned(), "Unused".to_owned()]
            .into_iter()
            .collect()
    );
}

#[test]
fn empty_selections_do_not_fall_back_to_the_whole_model() {
    let workspace = workspace(
        r#"
        systemLandscape "Empty" {}
        systemLandscape "Excluded" { include * exclude * }
        container system "EmptyContainers" {}
        component api "EmptyComponents" { include * exclude * }
        systemContext system "Scope" { include * exclude * }
        "#,
        "",
    );
    let renderer = SvgRenderer::default();
    for view in &workspace.views().system_landscape_views {
        assert!(view.properties.elements.is_empty());
        assert!(painted(
            workspace.model(),
            &renderer.render_system_landscape(&workspace, view).unwrap()
        )
        .is_empty());
    }
    let views = workspace.views();
    assert!(painted(
        workspace.model(),
        &renderer
            .render_container(&workspace, &views.container_views[0])
            .unwrap()
    )
    .is_empty());
    assert!(painted(
        workspace.model(),
        &renderer
            .render_component(&workspace, &views.component_views[0])
            .unwrap()
    )
    .is_empty());
    // The official context view protects its scope from removal.
    assert_eq!(
        painted(
            workspace.model(),
            &renderer
                .render_system_context(&workspace, &views.system_context_views[0])
                .unwrap()
        ),
        ["Library".to_owned()].into_iter().collect()
    );
}

#[test]
fn disabled_implied_relationships_are_not_synthesized_again_by_rendering() {
    let workspace = workspace(
        "systemContext system \"Context\" { include * }",
        "!impliedRelationships false",
    );
    let view = &workspace.views().system_context_views[0];
    let svg = SvgRenderer::default()
        .render_system_context(&workspace, view)
        .unwrap();
    assert_eq!(
        painted(workspace.model(), &svg),
        ["Library".to_owned()].into_iter().collect()
    );
    assert!(view.properties.relationships.is_empty());
    assert!(!svg.contains("data-source="));
}

#[test]
fn filtered_views_cannot_admit_elements_outside_their_base_selection() {
    let workspace = workspace(
        r#"
        systemLandscape "Base" { include alone }
        filtered "Base" include "Element,Relationship" "Filtered" {}
        component api "ComponentBase" { include spare }
        filtered "ComponentBase" include "Element,Relationship" "ComponentFiltered" {}
        filtered "Base" exclude "Person" "EmptyFiltered" {}
        "#,
        "",
    );
    let renderer = SvgRenderer::default();
    for (view, expected) in
        workspace
            .views()
            .filtered_views
            .iter()
            .zip([Some("Alone"), Some("Spare"), None])
    {
        let svg = renderer.render_filtered(&workspace, view).unwrap();
        assert_eq!(
            painted(workspace.model(), &svg),
            expected.into_iter().map(str::to_owned).collect()
        );
    }
}

#[test]
fn invalid_scopes_terms_and_parent_child_mixtures_report_errors() {
    for view in [
        "systemContext missing \"Bad\" { include * }",
        "container user \"Bad\" { include * }",
        "component missing \"Bad\" { include * }",
        "systemLandscape \"Bad\" { include missing }",
        "systemLandscape \"Bad\" { include api }",
        "container system \"Bad\" { include system }",
        "component api \"Bad\" { include api }",
        "container system \"Bad\" { include external api engine }",
        "container system \"Bad\" { include unrelated remote }",
        "systemLandscape \"Bad\" { include \"element.tag==Database\" }",
        "systemLandscape \"Bad\" { include }",
    ] {
        let source = format!("workspace {{\n{MODEL}\nviews {{ {view} }} }}");
        assert!(parse(&source).is_err(), "{view}");
    }
}

#[test]
fn filtered_relationship_tags_retain_only_requested_edges() {
    let workspace = workspace(
        r#"
        systemLandscape "Base" { include user system }
        filtered "Base" include "Element" "NodesOnly" {}
        filtered "Base" include "Element,Relationship" "WithEdge" {}
        "#,
        "",
    );
    let renderer = SvgRenderer::default();
    for (view, edges) in workspace.views().filtered_views.iter().zip([0, 1]) {
        let svg = renderer.render_filtered(&workspace, view).unwrap();
        assert_eq!(
            painted(workspace.model(), &svg),
            ["Reader".to_owned(), "Library".to_owned()]
                .into_iter()
                .collect()
        );
        assert_eq!(svg.matches("data-source=").count(), edges);
    }
}

#[test]
fn common_view_annotations_survive_selection_resolution() {
    let properties = r#"
        title "Review"
        description "Selected architecture"
        properties { "purpose" "Inspect" }
        default
        autoLayout lr
        include *
    "#;
    let workspace = workspace(
        &format!(
            "systemLandscape \"Landscape\" {{ {properties} }}\n\
             systemContext system \"Context\" {{ {properties} }}\n\
             container system \"Containers\" {{ {properties} }}\n\
             component api \"Components\" {{ {properties} }}"
        ),
        "",
    );
    let views = workspace.views();
    for view in [
        &views.system_landscape_views[0].properties,
        &views.system_context_views[0].properties,
        &views.container_views[0].properties,
        &views.component_views[0].properties,
    ] {
        assert_eq!(view.title.as_deref(), Some("Review"));
        assert_eq!(view.description.as_deref(), Some("Selected architecture"));
        assert_eq!(
            view.properties.get("purpose").map(String::as_str),
            Some("Inspect")
        );
        assert!(view.default);
        assert_eq!(
            view.auto_layout.as_ref().unwrap().direction,
            structurizr_core::view::AutoLayoutDirection::LeftRight
        );
    }
}
