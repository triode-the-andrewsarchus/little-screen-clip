use structurizr_core::model::ElementRef;
use structurizr_dsl::parse;
use structurizr_render::SvgRenderer;

const MODEL: &str = r#"
    model {
        task = element "Capture" "Task" "Read the source" "Highlighted" {
            properties { "phase" "input" }
        }
        done = element "Ready"
        spare = customElement "Disconnected" "Note"
        user = person "Reader"
        system = softwareSystem "Library"
        task -> done "Produces"
    }
"#;

#[test]
fn canonical_custom_elements_and_views_survive_resolution_and_svg_painting() {
    let source = format!(
        r##"workspace {{
        {MODEL}
        views {{
            custom "Workflow" "Review" "The capture pipeline" {{
                include *
                autoLayout lr
                default
                properties {{ "purpose" "review" }}
            }}
            styles {{ element "Highlighted" {{ background "#b62c81" }} }}
        }}
    }}"##
    );
    let workspace = parse(&source).unwrap();
    let model = workspace.model();
    assert_eq!(model.custom_elements.len(), 3);
    let first = &model.custom_elements[0];
    assert!(matches!(
        model.find_element(first.id()),
        Some(ElementRef::CustomElement(_))
    ));
    assert_eq!(first.metadata, "Task");
    assert_eq!(
        first.properties.description.as_deref(),
        Some("Read the source")
    );
    assert!(first.properties.tags.iter().any(|tag| tag == "Highlighted"));
    assert_eq!(
        first.properties.properties.get("phase").map(String::as_str),
        Some("input")
    );
    assert!(model.custom_elements[1].metadata.is_empty());

    assert_eq!(workspace.views().custom_views.len(), 1);
    let view = &workspace.views().custom_views[0];
    assert_eq!(view.properties.title.as_deref(), Some("Review"));
    assert_eq!(
        view.properties.description.as_deref(),
        Some("The capture pipeline")
    );
    assert!(view.properties.default);
    assert_eq!(
        view.properties
            .properties
            .get("purpose")
            .map(String::as_str),
        Some("review")
    );
    assert_eq!(view.properties.elements.len(), 3);
    assert_eq!(view.properties.relationships.len(), 1);
    let svg = SvgRenderer::new(0, 0)
        .render_custom(&workspace, view)
        .unwrap();
    for element in &model.custom_elements {
        assert!(svg.contains(&format!("data-element-id=\"{}\"", element.id())));
    }
    assert!(svg.contains("#b62c81"));
    assert!(svg.contains("Read the source"));
    assert!(svg.contains("[Task]"));
    assert!(svg.contains("Produces"));
    assert_eq!(svg.matches("class=\"relationship\"").count(), 1);
    assert!(!svg.contains(">Reader</text>"));
    assert!(!svg.contains(">Library</text>"));
}

#[test]
fn custom_selection_preserves_exclusion_reinclusion_and_empty_views() {
    let source = format!(
        r#"workspace {{ {MODEL} views {{
        custom "Selected" {{ include * exclude done task include task }}
        custom "Empty" {{ include * exclude * }}
    }} }}"#
    );
    let workspace = parse(&source).unwrap();
    let views = &workspace.views().custom_views;
    assert_eq!(views.len(), 2);
    let svg = SvgRenderer::default()
        .render_custom(&workspace, &views[0])
        .unwrap();
    assert!(svg.contains(">Capture</text>"));
    assert!(svg.contains(">Disconnected</text>"));
    assert!(!svg.contains(">Ready</text>"));
    assert!(!svg.contains("class=\"relationship\""));
    let svg = SvgRenderer::default()
        .render_custom(&workspace, &views[1])
        .unwrap();
    assert!(!svg.contains("data-element-id="));
    assert!(!svg.contains("class=\"relationship\""));
}

#[test]
fn invalid_custom_members_and_cross_abstraction_selections_are_errors() {
    for view in [
        "custom \"Bad\" { include user }",
        "custom \"Bad\" { include system }",
        "custom \"Bad\" { include missing }",
        "custom \"Bad\" { include element.tag==Highlighted }",
        "systemLandscape \"Bad\" { include task }",
        "dynamic * \"Bad\" { task -> done }",
    ] {
        let source = format!("workspace {{ {MODEL} views {{ {view} }} }}");
        assert!(parse(&source).is_err(), "{view}");
    }
}

#[test]
fn runtime_custom_views_reject_foreign_members_and_keep_resolved_relationship_selection() {
    let workspace = parse(&format!(
        "workspace {{ {MODEL} views {{ custom \"Custom\" {{ include * }} }} }}"
    ))
    .unwrap();
    let renderer = SvgRenderer::default();
    let mut view = workspace.views().custom_views[0].clone();
    view.properties.relationships.clear();
    let svg = renderer.render_custom(&workspace, &view).unwrap();
    assert!(
        !svg.contains("class=\"relationship\""),
        "do not invent excluded edges"
    );
    view.properties
        .add_element(workspace.model().people[0].id());
    assert!(renderer.render_custom(&workspace, &view).is_err());
}
