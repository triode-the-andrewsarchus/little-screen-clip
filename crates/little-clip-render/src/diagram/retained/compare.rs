//! Graphical SVG comparison over the normalized retained scene.

mod geometry;
mod source;

use super::{Limits, usvg};
use geometry::Tolerance;
use std::sync::atomic::{AtomicBool, Ordering};

/// Maximum coordinate delta in either document's root SVG user space.
pub const USER_TOLERANCE: f32 = 0.01;
const MAX_MARKS: usize = 256;

/// Bounds of a changed drawing operation, in the rendered root viewport.
pub type Bounds = [f32; 4];

/// A bounded display preview and the viewport used to position change markers.
pub struct Preview {
    pub pixels: tiny_skia::Pixmap,
    pub viewport: [f32; 2],
    pub marks: Vec<Bounds>,
}

/// Result of comparing supported SVG drawing operations in paint order.
pub struct Comparison {
    pub base: Preview,
    pub revised: Preview,
    pub changed: usize,
    pub removed: usize,
    pub added: usize,
    pub viewport_changed: bool,
}

impl Comparison {
    /// Returns whether the supported drawing operations match within tolerance.
    pub fn matches(&self) -> bool {
        self.changed == 0 && self.removed == 0 && self.added == 0 && !self.viewport_changed
    }

    /// Describes the changes in drawing operations and viewport dimensions.
    pub fn summary(&self) -> String {
        if self.matches() {
            return "No vector changes within tolerance".into();
        }
        let viewport = if self.viewport_changed {
            " · viewport changed"
        } else {
            ""
        };
        format!(
            "{} changed · {} removed · {} added{viewport}",
            self.changed, self.removed, self.added
        )
    }
}

/// Compares the geometry, paint and text of two SVG or SVGZ documents.
///
/// Coordinates are aligned at the viewport origin. Preview images are produced
/// after comparison and are used solely for display.
///
/// # Errors
///
/// Reports the affected operand when a document is invalid, exceeds a budget
/// or contains unsupported features.
pub fn compare(base: &[u8], revised: &[u8], cancelled: &AtomicBool) -> Result<Comparison, String> {
    let check_cancelled = || {
        if cancelled.load(Ordering::Relaxed) {
            Err("SVG comparison cancelled".to_string())
        } else {
            Ok(())
        }
    };
    check_cancelled()?;
    let (base, base_scale) = source::prepare(base).map_err(|error| format!("Base SVG: {error}"))?;
    check_cancelled()?;
    let (revised, revised_scale) =
        source::prepare(revised).map_err(|error| format!("Comparison SVG: {error}"))?;
    let tolerance = Tolerance::new(base_scale, revised_scale);
    let base_items = items(&base).map_err(|error| format!("Base SVG: {error}"))?;
    let revised_items = items(&revised).map_err(|error| format!("Comparison SVG: {error}"))?;
    check_cancelled()?;
    let changes = compare_items(&base_items, &revised_items, tolerance);
    check_cancelled()?;
    let base_preview = preview(&base, changes.base)?;
    check_cancelled()?;
    Ok(Comparison {
        base: base_preview,
        revised: preview(&revised, changes.revised)?,
        changed: changes.changed,
        removed: changes.removed,
        added: changes.added,
        viewport_changed: !tolerance.viewport_size(size(&base), size(&revised)),
    })
}

fn size(tree: &usvg::Tree) -> [f32; 2] {
    [tree.size().width(), tree.size().height()]
}

fn preview(tree: &usvg::Tree, marks: Vec<Bounds>) -> Result<Preview, String> {
    let viewport = size(tree);
    let scale = 768.0 / viewport[0].max(viewport[1]);
    let width = (viewport[0] * scale).ceil().clamp(1.0, 768.0) as u32;
    let height = (viewport[1] * scale).ceil().clamp(1.0, 768.0) as u32;
    let mut pixels =
        tiny_skia::Pixmap::new(width, height).ok_or("SVG preview allocation failed")?;
    resvg::render(
        tree,
        usvg::Transform::from_scale(scale, scale),
        &mut pixels.as_mut(),
    );
    Ok(Preview {
        pixels,
        viewport,
        marks,
    })
}

enum Item<'a> {
    Path(&'a usvg::Path),
    Text(&'a usvg::Text, Vec<Item<'a>>),
    Layer(&'a usvg::Group, Vec<Item<'a>>),
}

impl Item<'_> {
    fn bounds(&self) -> Option<Bounds> {
        let rect = match self {
            Self::Path(path) => path.abs_stroke_bounding_box()?,
            Self::Text(text, _) => text.abs_stroke_bounding_box(),
            Self::Layer(group, _) => group.abs_stroke_bounding_box()?,
        };
        Some([rect.left(), rect.top(), rect.right(), rect.bottom()])
    }

    fn dash_intervals(&self) -> f64 {
        match self {
            Self::Path(path) => geometry::dash_intervals(path),
            Self::Text(_, children) | Self::Layer(_, children) => {
                children.iter().map(Self::dash_intervals).sum()
            }
        }
    }

    fn matches(&self, other: &Self, tolerance: Tolerance) -> bool {
        match (self, other) {
            (Self::Path(a), Self::Path(b)) => geometry::path(a, b, tolerance),
            (Self::Text(a, a_paths), Self::Text(b, b_paths)) => {
                text_chars(a).eq(text_chars(b)) && matching_items(a_paths, b_paths, tolerance)
            }
            (Self::Layer(a, a_items), Self::Layer(b, b_items)) => {
                geometry::opacity(a.opacity().get(), b.opacity().get())
                    && matching_items(a_items, b_items, tolerance)
            }
            _ => false,
        }
    }
}

fn text_chars(text: &usvg::Text) -> impl Iterator<Item = char> + '_ {
    text.chunks().iter().flat_map(|chunk| {
        chunk
            .spans()
            .iter()
            .filter(|span| span.is_visible())
            .flat_map(|span| chunk.text()[span.start()..span.end()].chars())
    })
}

fn matching_items(a: &[Item<'_>], b: &[Item<'_>], tolerance: Tolerance) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(a, b)| a.matches(b, tolerance))
}

fn items(tree: &usvg::Tree) -> Result<Vec<Item<'_>>, String> {
    let mut items = Vec::new();
    collect(tree.root(), &mut items)?;
    // Preview painting expands SVG dashes into geometry. Keep this work
    // bounded independently of the retained path segment count.
    if items.iter().map(Item::dash_intervals).sum::<f64>() > 262_144.0 {
        return Err("SVG dashes exceed the preview geometry budget".into());
    }
    Ok(items)
}

fn collect<'a>(group: &'a usvg::Group, items: &mut Vec<Item<'a>>) -> Result<(), String> {
    if group.opacity().get() == 0.0 {
        return Ok(());
    }
    if group.clip_path().is_some()
        || group.mask().is_some()
        || !group.filters().is_empty()
        || group.blend_mode() != usvg::BlendMode::Normal
        || group.isolate()
    {
        return Err("Unsupported SVG clipping or compositing effect".into());
    }
    if group.opacity().get() == 1.0 {
        collect_children(group, items)?;
    } else {
        let mut children = Vec::new();
        collect_children(group, &mut children)?;
        if !children.is_empty() {
            items.push(Item::Layer(group, children));
        }
    }
    Ok(())
}

fn collect_children<'a>(group: &'a usvg::Group, items: &mut Vec<Item<'a>>) -> Result<(), String> {
    for node in group.children() {
        match node {
            usvg::Node::Group(child) => collect(child, items)?,
            usvg::Node::Path(path) if geometry::visible(path) => items.push(Item::Path(path)),
            usvg::Node::Path(_) => {}
            usvg::Node::Text(text) => {
                if text
                    .layouted()
                    .iter()
                    .filter(|span| span.visible)
                    .flat_map(|span| &span.positioned_glyphs)
                    .any(|glyph| glyph.id.0 == 0)
                {
                    return Err("Unsupported SVG text glyph outside the bundled font set".into());
                }
                let mut paths = Vec::new();
                collect(text.flattened(), &mut paths)?;
                if !paths.is_empty() {
                    items.push(Item::Text(text, paths));
                }
            }
            usvg::Node::Image(_) => return Err("Unsupported SVG raster image or glyph".into()),
        }
    }
    Ok(())
}

#[derive(Default)]
struct Changes {
    base: Vec<Bounds>,
    revised: Vec<Bounds>,
    changed: usize,
    added: usize,
    removed: usize,
}

fn compare_items(mut base: &[Item<'_>], mut revised: &[Item<'_>], tolerance: Tolerance) -> Changes {
    let mut changes = Changes::default();
    while let (Some((a, remaining_base)), Some((b, remaining_revised))) =
        (base.split_first(), revised.split_first())
    {
        if !a.matches(b, tolerance) {
            // Look ahead a bounded number of paint operations so local insertions
            // or deletions do not turn the entire remainder into replacements.
            let removed = find_match(b, remaining_base, tolerance);
            let added = find_match(a, remaining_revised, tolerance);
            match (removed, added) {
                (Some(count), _) if added.is_none_or(|n| count < n) => {
                    changes.removed += count;
                    mark(&mut changes.base, &base[..count]);
                    base = &base[count..];
                    continue;
                }
                (_, Some(count)) => {
                    changes.added += count;
                    mark(&mut changes.revised, &revised[..count]);
                    revised = &revised[count..];
                    continue;
                }
                _ => {}
            }
            changes.changed += 1;
            mark(&mut changes.base, &base[..1]);
            mark(&mut changes.revised, &revised[..1]);
        }
        base = remaining_base;
        revised = remaining_revised;
    }
    changes.removed += base.len();
    changes.added += revised.len();
    mark(&mut changes.base, base);
    mark(&mut changes.revised, revised);
    changes
}

fn find_match(item: &Item<'_>, others: &[Item<'_>], tolerance: Tolerance) -> Option<usize> {
    others
        .iter()
        .take(8)
        .position(|other| item.matches(other, tolerance))
        .map(|index| index + 1)
}

fn mark(marks: &mut Vec<Bounds>, items: &[Item<'_>]) {
    marks.extend(
        items
            .iter()
            .filter_map(Item::bounds)
            .take(MAX_MARKS - marks.len()),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn svg(contents: &str) -> String {
        format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="60">{contents}</svg>"#
        )
    }

    fn run(a: &str, b: &str) -> Comparison {
        compare(a.as_bytes(), b.as_bytes(), &AtomicBool::new(false)).unwrap()
    }

    #[test]
    fn equivalent_commands_transforms_and_css_match() {
        let a = svg(
            r#"<style>.line { fill: none; stroke: red; stroke-width: 2; stroke-linecap: round; stroke-dasharray: 2 3 }</style>
            <g class="line" transform="translate(10 20) scale(2)"><path d="M0 0q3 3 6 0"/></g>
            <rect x="4" y="5" width="11" height="7" fill="blue"/>"#,
        );
        let b = svg(
            r##"<path d="M10 20C14 24 18 24 22 20" style="fill:none;stroke:#f00;stroke-width:4;stroke-linecap:round;stroke-dasharray:4,6"/>
            <path fill="#0000ff" d="M4 5h11v7H4z"/>"##,
        );
        assert!(run(&a, &b).matches());
    }

    #[test]
    fn compares_text_and_its_resolved_paint() {
        let a = svg(
            r#"<g transform="translate(2 1)"><text x="6" y="23" font-size="16" fill="red">Artwork</text></g>"#,
        );
        let b = svg(
            r##"<style>.label { font-size: 16px; fill: #ff0000 }</style><text x="8" y="24" class="label"><tspan>Artwork</tspan></text>"##,
        );
        assert!(run(&a, &b).matches());

        let revised = b.replace("Artwork", "Artworks");
        let result = run(&a, &revised);
        assert_eq!(result.changed, 1);
        assert_eq!(result.revised.marks.len(), 1);
        assert_eq!(run(&a, &b.replace("#ff0000", "#0000ff")).changed, 1);
    }

    #[test]
    fn subpixel_change_uses_root_user_units_under_viewbox_scaling() {
        let a = r#"<svg xmlns="http://www.w3.org/2000/svg" width="8" height="8" viewBox="0 0 1000 1000"><rect x="100" y="200" width="4" height="500"/></svg>"#;
        assert!(run(a, &a.replace("x=\"100\"", "x=\"100.004\"")).matches());
        let result = run(a, &a.replace("x=\"100\"", "x=\"100.125\""));
        assert_eq!(result.changed, 1);
        assert_ne!(result.base.marks, result.revised.marks);
    }

    #[test]
    fn compares_gradient_stops_and_group_opacity() {
        let a = svg(
            r##"<defs><linearGradient id="ink"><stop stop-color="orange"/><stop offset="1" stop-color="black"/></linearGradient></defs>
            <g opacity=".8"><rect width="20" height="24" fill="url(#ink)"/><circle cx="8" cy="8" r="6"/></g>"##,
        );
        let alternate = a.replace("ink", "paint").replace("orange", "#ffa500");
        assert!(run(&a, &alternate).matches());
        assert_eq!(run(&a, &alternate.replace("black", "blue")).changed, 1);
        assert_eq!(run(&a, &alternate.replace(".8", ".6")).changed, 1);
    }

    #[test]
    fn accounts_for_added_shapes_and_viewport_changes() {
        let a = svg(r#"<rect width="20" height="24"/><circle cx="8" cy="8" r="6"/>"#);
        let b = a
            .replace("<circle", "<rect x=\"4\" width=\"2\" height=\"3\"/><circle")
            .replace("width=\"100\"", "width=\"101\"");
        let result = run(&a, &b);
        assert_eq!((result.changed, result.added, result.removed), (0, 1, 0));
        assert!(result.viewport_changed);
        assert_eq!(result.revised.marks.len(), 1);
    }

    #[test]
    fn svgz_matches_plain_svg_and_truncated_input_is_rejected() {
        let compressed = include_bytes!("../../../../little-clip-core/tests/fixtures/panels.svgz");
        let plain = br##"<svg xmlns="http://www.w3.org/2000/svg" width="64" height="32"><path d="M0 0h32v32H0Z" fill="#1e78d2"/><rect x="32" width="32" height="32" fill="#b43c28"/></svg>"##;
        let cancelled = AtomicBool::new(false);
        assert!(compare(compressed, plain, &cancelled).unwrap().matches());
        assert!(compare(&compressed[..compressed.len() - 4], plain, &cancelled).is_err());
    }

    #[test]
    fn unsupported_effects_have_no_match_verdict_even_for_equal_inputs() {
        for effect in [
            r#"<path d="M0 0h10v10z" style="filter: blur(2px)"/>"#,
            r#"<path d="M0 0h10v10z" clip-path="url(#missing)"/>"#,
            r#"<path d="M0 0h10v10z" vector-effect="non-scaling-stroke"/>"#,
            r#"<style>@import url(external.css);</style>"#,
            r#"<foreignObject width="20" height="20"/>"#,
        ] {
            let svg = svg(effect);
            let error = compare(svg.as_bytes(), svg.as_bytes(), &AtomicBool::new(false))
                .err()
                .expect("an unsupported effect must have no match verdict");
            assert!(error.starts_with("Base SVG: Unsupported"), "{error}");
        }
    }
}
