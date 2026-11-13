//! Native architecture views, composed by the existing bounded SVG pipeline.
use structurizr_core::{Workspace, view::ViewProperties};
use structurizr_render::SvgRenderer;

pub(super) fn svg(source: &str) -> Result<(String, Vec<String>), String> {
    let mut workspace =
        structurizr_dsl::parse(source).map_err(|error| format!("Structurizr: {error}"))?;
    let mut warnings = Vec::new();
    if workspace.views().all_keys().is_empty() && workspace.views().image_views.is_empty() {
        add_overview(&mut workspace);
        warnings.push("No views were defined; showing a generated system landscape".into());
    }
    if workspace
        .styles()
        .is_some_and(|styles| !styles.themes.is_empty())
    {
        warnings.push("External themes are not loaded; using local and built-in styles".into());
    }
    let views = workspace.views();
    if !views.deployment_views.is_empty() {
        return Err("Deployment views are not supported in this Structurizr preview".into());
    }
    if !views.image_views.is_empty() {
        return Err(
            "Structurizr image views are not supported; external image sources are not loaded"
                .into(),
        );
    }
    let keys = views.all_keys();
    if keys.len() > 16 || keys.iter().collect::<std::collections::HashSet<_>>().len() != keys.len()
    {
        return Err("Structurizr previews require at most sixteen views with unique keys".into());
    }
    let renderer = SvgRenderer::new(0, 0);
    let rendered = views
        .system_landscape_views
        .iter()
        .map(|view| {
            (
                renderer.render_system_landscape(&workspace, view),
                &view.properties,
            )
        })
        .chain(views.system_context_views.iter().map(|view| {
            (
                renderer.render_system_context(&workspace, view),
                &view.properties,
            )
        }))
        .chain(views.container_views.iter().map(|view| {
            (
                renderer.render_container(&workspace, view),
                &view.properties,
            )
        }))
        .chain(views.component_views.iter().map(|view| {
            (
                renderer.render_component(&workspace, view),
                &view.properties,
            )
        }))
        .chain(
            views
                .dynamic_views
                .iter()
                .map(|view| (renderer.render_dynamic(&workspace, view), &view.properties)),
        )
        .chain(
            views
                .filtered_views
                .iter()
                .map(|view| (renderer.render_filtered(&workspace, view), &view.properties)),
        )
        .chain(
            views
                .custom_views
                .iter()
                .map(|view| (renderer.render_custom(&workspace, view), &view.properties)),
        );
    let mut pages = Vec::new();
    let mut bytes = 0;
    for (result, properties) in rendered {
        let page = titled_page(
            result.map_err(|error| format!("Structurizr view '{}': {error}", properties.key))?,
            properties,
        )?;
        bytes += page.len();
        if bytes > super::MAX_SVG {
            return Err("Structurizr generated SVG exceeds the preview budget".into());
        }
        pages.push(page);
    }
    let (svg, limitations) = super::compose_svg(
        pages,
        super::fonts::metric_fonts(),
        "Arimo",
        super::ImagePolicy::Block,
        super::ViewportPolicy::FitContent,
    )?;
    warnings.extend(limitations);
    Ok((svg, warnings))
}

fn add_overview(workspace: &mut Workspace) {
    let mut view = structurizr_core::SystemLandscapeView::new("Overview");
    for element in workspace.model().architecture_elements().filter(|element| {
        matches!(
            element,
            structurizr_core::ElementRef::Person(_)
                | structurizr_core::ElementRef::SoftwareSystem(_)
        )
    }) {
        view.properties.add_element(element.id());
    }
    let ids: std::collections::HashSet<_> = view
        .properties
        .elements
        .iter()
        .map(|element| element.id)
        .collect();
    for relationship in &workspace.model().relationships {
        if ids.contains(&relationship.source_id) && ids.contains(&relationship.destination_id) {
            view.properties.add_relationship(relationship.id);
        }
    }
    workspace.views_mut().add_system_landscape_view(view);
}

fn titled_page(svg: String, properties: &ViewProperties) -> Result<String, String> {
    // Read only the generated root's dimensions. The shared composer remains the
    // sole SVG/font/image-policy owner; this XML reader never resolves resources.
    let mut reader = quick_xml::Reader::from_str(&svg);
    let quick_xml::events::Event::Start(root) =
        reader.read_event().map_err(|error| error.to_string())?
    else {
        return Err("Structurizr generated an invalid SVG root".into());
    };
    let mut size = [0.0_f64; 2];
    for attribute in root.attributes() {
        let attribute = attribute.map_err(|error| error.to_string())?;
        let index = match attribute.key.as_ref() {
            b"width" => 0,
            b"height" => 1,
            _ => continue,
        };
        size[index] = std::str::from_utf8(&attribute.value)
            .ok()
            .and_then(|value| value.parse().ok())
            .filter(|value: &f64| value.is_finite() && *value > 0.0)
            .ok_or("Structurizr generated an invalid page size")?;
    }
    if size.contains(&0.0) {
        return Err("Structurizr generated no page dimensions".into());
    }
    let title = quick_xml::escape::escape(properties.title.as_deref().unwrap_or(&properties.key));
    Ok(format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{}" height="{}" viewBox="0 0 {} {}"><text x="12" y="26" font-family="Arimo" font-size="20" fill="#555555">{title}</text><g transform="translate(0 42)">{svg}</g></svg>"##,
        size[0],
        size[1] + 42.0,
        size[0],
        size[1] + 42.0
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_architecture_views_keep_colors_and_outline_labels() {
        let source = include_str!("../../../../docs/examples/diagrams/architecture.structurizr");
        let (svg, warnings) = svg(source).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        assert!(svg.contains("#295f98"));
        assert!(svg.contains("<path"));
        let rendered = super::super::render_inline(
            little_clip_core::diagram::Format::Structurizr,
            source,
            None,
        )
        .unwrap();
        assert!(!rendered.png.is_empty());
    }

    #[test]
    fn dynamic_custom_and_filtered_views_are_included_in_native_pages() {
        for source in [
            include_str!("../../../../docs/examples/diagrams/requests.structurizr"),
            include_str!("../../../../docs/examples/diagrams/stages.structurizr"),
            r#"workspace { model { app = softwareSystem "Library" } views {
                systemLandscape "Base" { include * }
                filtered "Base" include "Element" "Filtered" {}
            } }"#,
        ] {
            let (svg, warnings) = svg(source).unwrap();
            assert!(warnings.is_empty(), "{warnings:?}");
            assert!(svg.contains("<path"));
        }
    }

    #[test]
    fn page_titles_are_escaped_and_large_view_sets_are_bounded() {
        let properties = ViewProperties::new("<Work & flow>");
        let page = titled_page(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="50"/>"#
                .replace("/>", "></svg>"),
            &properties,
        )
        .unwrap();
        assert!(page.contains("&lt;Work &amp; flow&gt;"));
        assert!(page.contains("height=\"92\""));
        let mut source = String::from("workspace { views { ");
        for index in 0..17 {
            source.push_str(&format!("systemLandscape \"View{index}\" {{}} "));
        }
        source.push_str("} }");
        assert!(svg(&source).unwrap_err().contains("sixteen"));
    }

    #[test]
    fn model_only_workspaces_get_an_explicitly_generated_overview() {
        let (_, warnings) =
            svg(r#"workspace { model { app = softwareSystem "Library" } }"#).unwrap();
        assert!(
            warnings
                .iter()
                .any(|warning| warning.contains("generated system landscape"))
        );
    }

    #[test]
    fn unsupported_views_execution_and_ambiguous_keys_report_errors() {
        for source in [
            r#"workspace { model { deploymentEnvironment "Prod" {} } views { deployment * "Prod" "Runtime" { include * } } }"#,
            r#"workspace { !include "/etc/passwd" }"#,
            r#"workspace { model {} views { systemLandscape "Same" {} systemLandscape "Same" {} } }"#,
            r#"workspace { model { app = softwareSystem "Library" } views { systemContext missing "Wrong" { include * } } }"#,
            "workspace { model {",
        ] {
            assert!(svg(source).is_err(), "{source}");
        }
    }
}
