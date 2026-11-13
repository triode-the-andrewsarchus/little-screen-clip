//! Worker-side retained SVG preparation for native interactive views.
//!
//! Inputs are the canonical, outlined SVG produced by the diagram renderer.
//! The existing source/structure limits and raster admission policy apply;
//! no filesystem, network or font-discovery resolver is installed.

use super::images;
use std::{
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
    let tree = usvg_vector::Tree::from_str(svg, &options).map_err(|error| error.to_string())?;
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
        assert_eq!(prepared.images.len(), 1);
        assert!(prepared.images.values().any(|image| {
            image
                .pixels()
                .any(|pixel| pixel[2] > 120 && pixel[1] > 70 && pixel[0] < 90)
        }));
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
