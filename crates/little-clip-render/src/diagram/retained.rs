//! Worker-side retained SVG preparation for native interactive views.
//!
//! Inputs are the canonical, outlined SVG produced by the diagram renderer.
//! The existing source/structure limits and raster admission policy apply;
//! no filesystem, network or font-discovery resolver is installed.

use super::images;
use std::{
    borrow::Cow,
    collections::HashMap,
    sync::{Arc, atomic::Ordering},
};

/// Retains normalized vector geometry and worker-decoded raster assets.
///
/// The tree is immutable and thread-safe. Raster keys identify the tree's
/// `ImageKind::PNG` byte allocations, only for this in-memory scene's lifetime;
/// they are neither file identities nor persisted identifiers.
pub struct Svg {
    pub tree: usvg_vector::Tree,
    pub images: HashMap<usize, Arc<image::RgbaImage>>,
}

/// Prepares canonical diagram SVG without doing any work on the UI thread.
///
/// # Errors
///
/// Returns an error for oversized/invalid SVG, unoutlined text, unavailable
/// images, or a retained scene exceeding the structure/geometry budgets.
pub fn prepare(svg: &str) -> Result<Svg, String> {
    let document = super::svg_document(svg)?;
    if document.descendants().any(|node| node.has_tag_name("text")) {
        return Err("Interactive diagram SVG must contain native text outlines.".into());
    }
    let images::RetainedImages {
        resolver,
        budget,
        rasters,
    } = images::retained_resolver();
    let options = usvg_vector::Options {
        image_href_resolver: resolver,
        ..Default::default()
    };
    let interactive = interactive_planes(&document);
    let tree =
        usvg_vector::Tree::from_str(&interactive, &options).map_err(|error| error.to_string())?;
    if tree.size().width() > 16_384.0 || tree.size().height() > 16_384.0 {
        return Err("Interactive diagram dimensions exceed 16384 pixels.".into());
    }
    if budget.rejected.load(Ordering::Relaxed) || images::invalid_declarations(&document) {
        return Err("Interactive diagram contains an unavailable or oversized image.".into());
    }
    let mut limits = Limits::default();
    limits.group(tree.root(), 0)?;
    // A retained definition is counted once, even when many elements use it.
    for clip in tree.clip_paths() {
        limits.group(clip.root(), 0)?;
    }
    for mask in tree.masks() {
        limits.group(mask.root(), 0)?;
    }
    for pattern in tree.patterns() {
        limits.group(pattern.root(), 0)?;
    }
    drop(options);
    let images = std::mem::take(
        &mut *rasters
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
    );
    Ok(Svg { tree, images })
}

fn interactive_planes<'a>(document: &roxmltree::Document<'a>) -> Cow<'a, str> {
    let source = document.input_text();
    let mut edits = Vec::new();
    // compose_svg nests each normalized export SVG inside a direct layout
    // group. Its dimensions place the initial artwork; they are not an editor
    // boundary. Deeper SVGs and all authored clip paths keep their own crops.
    for plane in document
        .root_element()
        .children()
        .filter(|node| node.has_tag_name("g"))
        .flat_map(|node| node.children())
        .filter(|node| node.has_tag_name("svg"))
    {
        if let Some(overflow) = plane.attribute_node("overflow") {
            edits.push((overflow.range_value(), "visible"));
        } else {
            let position = plane
                .attributes()
                .map(|attribute| attribute.range().end)
                .max()
                .unwrap_or_else(|| {
                    let start = plane.range().start + 1;
                    let end = source[start..]
                        .find(|c: char| c.is_ascii_whitespace() || c == '>' || c == '/')
                        .unwrap_or(plane.tag_name().name().len());
                    start + end
                });
            edits.push((position..position, " overflow=\"visible\""));
        }
    }
    if edits.is_empty() {
        return Cow::Borrowed(source);
    }
    let mut interactive = source.to_owned();
    // Reverse document order retains the original byte positions.
    for (range, value) in edits.into_iter().rev() {
        interactive.replace_range(range, value);
    }
    Cow::Owned(interactive)
}

#[derive(Default)]
struct Limits {
    nodes: usize,
    segments: usize,
}
impl Limits {
    fn group(&mut self, group: &usvg_vector::Group, depth: usize) -> Result<(), String> {
        if depth > 64 {
            return Err("Interactive diagram groups exceed the depth budget.".into());
        }
        for node in group.children() {
            self.nodes += 1;
            match node {
                usvg_vector::Node::Group(group) => self.group(group, depth + 1)?,
                usvg_vector::Node::Path(path) => self.segments += path.data().segments().count(),
                usvg_vector::Node::Text(_) => {
                    return Err("Interactive diagram text was not outlined.".into());
                }
                usvg_vector::Node::Image(_) => {}
            }
            if self.nodes > 32_768 || self.segments > 262_144 {
                return Err("Interactive diagram exceeds its retained geometry budget.".into());
            }
        }
        Ok(())
    }
}

pub use excalidraw_render::LinearPreview;

/// Prepares native linear styling for live drawing connections on a worker.
///
/// # Errors
///
/// Returns an error for invalid/oversized drawing source or failed normalization.
pub fn drawing_lines(source: &str) -> Result<HashMap<String, LinearPreview>, String> {
    if source.len() > super::MAX_DRAWING_SOURCE {
        return Err("Drawing source exceeds the preview budget.".into());
    }
    let scene = excalidraw_render::SceneDocument::parse(source.as_bytes())
        .map_err(|error| error.to_string())?;
    LinearPreview::prepare(&scene)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn real_drawing_keeps_thread_safe_object_groups_and_decoded_assets() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Svg>();
        let rendered = super::super::excalidraw::render(include_str!(
            "../../../../docs/examples/diagrams/drawing.excalidraw"
        ))
        .unwrap();
        let prepared = prepare(&rendered.svg).unwrap();
        assert_eq!(prepared.tree.size().width(), 500.0);
        assert_eq!(prepared.tree.size().height(), 295.0);
        fn objects(group: &usvg_vector::Group) -> usize {
            usize::from(group.id().starts_with("graph-0-drawing-element-"))
                + group
                    .children()
                    .iter()
                    .map(|node| match node {
                        usvg_vector::Node::Group(child) => objects(child),
                        _ => 0,
                    })
                    .sum::<usize>()
        }
        assert_eq!(objects(prepared.tree.root()), 8);
        assert_eq!(
            prepared.tree.clip_paths().len(),
            1,
            "only the authored image crop remains; the export plane must not clip editing"
        );
        assert_eq!(prepared.images.len(), 1);
        assert!(prepared.images.values().any(|image| {
            image
                .pixels()
                .any(|pixel| pixel[2] > 120 && pixel[1] > 70 && pixel[0] < 90)
        }));
    }

    #[test]
    fn live_connection_paths_match_the_committed_native_export() {
        use little_clip_core::diagram::drawing::{Change, Scene};
        let original = include_str!("../../../../docs/examples/diagrams/drawing.excalidraw");
        let mut source: serde_json::Value = serde_json::from_str(original).unwrap();
        for (head, style, roughness) in [
            ("arrow", "solid", 0.0),
            ("diamond", "dotted", 1.0),
            ("circle", "dashed", 2.0),
        ] {
            source["elements"][2]["endArrowhead"] = serde_json::json!(head);
            source["elements"][2]["strokeStyle"] = serde_json::json!(style);
            source["elements"][2]["roughness"] = serde_json::json!(roughness);
            let text = source.to_string();
            let model = Scene::parse(&text).unwrap();
            let ids = vec!["capture".into()];
            let movement = model.movement(&ids, [-75.0, 43.0]).unwrap();
            let lines = drawing_lines(&text).unwrap();
            let svg = lines["connection"]
                .svg(&movement.paths["connection"].points)
                .unwrap();
            let live = prepare(&svg).unwrap();
            let committed = model
                .apply(Change::Move {
                    ids,
                    by: [-75.0, 43.0],
                })
                .unwrap()
                .unwrap();
            let exported = super::super::excalidraw::render(&committed).unwrap();
            let retained = prepare(&exported.svg).unwrap();
            fn paths(group: &usvg_vector::Group) -> Vec<&usvg_vector::Path> {
                group
                    .children()
                    .iter()
                    .flat_map(|node| match node {
                        usvg_vector::Node::Path(path) => vec![path.as_ref()],
                        usvg_vector::Node::Group(group) => paths(group),
                        _ => Vec::new(),
                    })
                    .collect()
            }
            let encoded = "connection"
                .bytes()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();
            let id = format!("graph-0-drawing-geometry-{encoded}");
            let usvg_vector::Node::Group(group) = retained.tree.node_by_id(&id).unwrap() else {
                panic!("Missing retained geometry group")
            };
            let expected = paths(group);
            let actual = paths(live.tree.root());
            assert_eq!(actual.len(), expected.len(), "{head}/{style}");
            for (actual, expected) in actual.iter().zip(expected) {
                let actual_data = actual.data();
                let expected_data = expected.data();
                assert_eq!(actual_data.points().len(), expected_data.points().len());
                for (a, b) in actual_data.points().iter().zip(expected_data.points()) {
                    assert!(
                        (a.x - b.x).abs() < 0.001 && (a.y - b.y).abs() < 0.001,
                        "{head}/{style}: {a:?} != {b:?}"
                    );
                }
                assert_eq!(
                    actual_data
                        .segments()
                        .map(|segment| std::mem::discriminant(&segment))
                        .collect::<Vec<_>>(),
                    expected_data
                        .segments()
                        .map(|segment| std::mem::discriminant(&segment))
                        .collect::<Vec<_>>()
                );
                assert_eq!(
                    format!("{:?}", actual.stroke()),
                    format!("{:?}", expected.stroke()),
                    "{head}/{style}"
                );
                assert_eq!(
                    format!("{:?}", actual.fill()),
                    format!("{:?}", expected.fill()),
                    "{head}/{style}"
                );
            }
        }
    }

    #[test]
    fn drawing_export_bounds_are_not_editor_bounds_but_authored_svg_crops_remain() {
        let source = r#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"><g><svg width="100" height="100" overflow="hidden"><path d="M0 0h140v20H0Z"/><g id="authored"><svg width="10" height="10" overflow="hidden"><path d="M0 0h30v30H0Z"/></svg></g><defs><clipPath id="frame"><path d="M0 0h20v20H0Z"/></clipPath></defs><g clip-path="url(#frame)"><path d="M0 0h60v60H0Z"/></g></svg></g></svg>"#;
        let prepared = prepare(source).unwrap();
        assert_eq!(
            prepared.tree.size().width(),
            100.0,
            "initial placement still uses the export's logical size"
        );
        assert_eq!(
            prepared.tree.clip_paths().len(),
            2,
            "the authored nested SVG and frame retain their crops"
        );
        assert!(prepared.tree.root().abs_bounding_box().width() >= 140.0);
    }

    #[test]
    fn retained_preparation_rejects_external_images_unoutlined_text_and_structure_limits() {
        for content in [
            r#"<image href="file:///unavailable.png" width="5" height="5"/>"#,
            r#"<image href="https://example.invalid/a.png" width="5" height="5"/>"#,
            r#"<image href="data:image/png;base64,bad" width="5" height="5"/>"#,
            "<text>already outlined upstream</text>",
        ] {
            let svg = format!(
                r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">{content}</svg>"#
            );
            assert!(prepare(&svg).is_err(), "{content}");
        }
        assert!(prepare(&" ".repeat(super::super::MAX_SVG + 1)).is_err());
        let svg = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">{}{}{}</svg>"#,
            "<g>".repeat(65),
            "<path d=\"M0 0L1 1\"/>",
            "</g>".repeat(65)
        );
        assert!(prepare(&svg).is_err());
        let expanded = format!(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><defs><g id="many">{}</g></defs>{}</svg>"##,
            r#"<path d="M0 0h1v1Z"/>"#.repeat(256),
            r##"<use href="#many"/>"##.repeat(256),
        );
        assert!(
            prepare(&expanded)
                .err()
                .unwrap()
                .contains("geometry budget")
        );
    }
}
