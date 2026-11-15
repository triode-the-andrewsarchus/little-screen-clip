//! Conservative, immutable theme adaptation of transparent monochrome artwork.
use usvg::{Color, Group, Node, Paint, Tree};

pub(super) fn classify(tree: &Tree) -> Option<bool> {
    let mut color = None;
    collect(tree.root(), tree.size(), &mut color)?;
    let color = color?;
    let [min, max] = extremes(color);
    (max - min <= 24 && (max <= 48 || min >= 207)).then_some(max <= 48)
}

fn collect(group: &Group, size: usvg::Size, ink: &mut Option<Color>) -> Option<()> {
    if !group.filters().is_empty() || group.mask().is_some() {
        return None;
    }
    for node in group.children() {
        match node {
            Node::Group(group) => collect(group, size, ink)?,
            Node::Path(path) if path.is_visible() => path_ink(path, size, ink)?,
            Node::Path(_) => {}
            _ => return None,
        }
    }
    Some(())
}

fn path_ink(path: &usvg::Path, size: usvg::Size, ink: &mut Option<Color>) -> Option<()> {
    let line = path.data().segments().count() == 2
        && matches!(
            path.data().segments().nth(1),
            Some(usvg::tiny_skia_path::PathSegment::LineTo(_))
        );
    if let Some(fill) = path.fill()
        && !line
    {
        if backdrop(path, fill, size) {
            return None;
        }
        if fill.opacity().get() > 0.0 {
            accept(fill.paint(), ink)?;
        }
    }
    if let Some(stroke) = path.stroke()
        && stroke.opacity().get() > 0.0
    {
        accept(stroke.paint(), ink)?;
    }
    Some(())
}

fn backdrop(path: &usvg::Path, fill: &usvg::Fill, size: usvg::Size) -> bool {
    let bounds = path.abs_bounding_box();
    fill.opacity().get() >= 0.999
        && bounds.left() <= 0.01
        && bounds.top() <= 0.01
        && bounds.right() >= size.width() - 0.01
        && bounds.bottom() >= size.height() - 0.01
        && path.data().segments().count() <= 5
}

fn accept(paint: &Paint, ink: &mut Option<Color>) -> Option<()> {
    let Paint::Color(color) = paint else {
        return None;
    };
    if let Some(previous) = ink {
        if previous.red.abs_diff(color.red) > 8
            || previous.green.abs_diff(color.green) > 8
            || previous.blue.abs_diff(color.blue) > 8
        {
            return None;
        }
    } else {
        *ink = Some(*color);
    }
    Some(())
}

fn extremes(color: Color) -> [u8; 2] {
    [
        color.red.min(color.green).min(color.blue),
        color.red.max(color.green).max(color.blue),
    ]
}

pub(super) fn rgb(color: Color, invert: bool) -> [f32; 3] {
    // HSL L=(max+min)/2. Translating each channel by 1-(max+min)
    // maps L to 1-L while preserving channel differences, hue and saturation.
    // This differs from complementing each channel, which rotates the hue.
    let [min, max] = extremes(color);
    let delta = if invert {
        1.0 - f32::from(min) / 255.0 - f32::from(max) / 255.0
    } else {
        0.0
    };
    [color.red, color.green, color.blue].map(|c| (f32::from(c) / 255.0 + delta).clamp(0.0, 1.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn tree(body: &str) -> Tree {
        Tree::from_str(
            &format!(
                r#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">{body}</svg>"#
            ),
            &usvg::Options::default(),
        )
        .unwrap()
    }
    #[test]
    fn only_transparent_neutral_monochrome_art_is_eligible() {
        assert_eq!(
            classify(&tree(
                r##"<path stroke="#11100f" stroke-opacity=".4" d="M10 10L90 90"/>"##
            )),
            Some(true)
        );
        assert_eq!(
            classify(&tree(r##"<path fill="#fefdfc" d="M10 10h40v40h-40Z"/>"##)),
            Some(false)
        );
        for body in [
            r##"<rect width="100" height="100" fill="#fff"/>"##,
            r##"<path stroke="#000" d="M10 10L90 90"/><path stroke="#fff" d="M10 90L90 10"/>"##,
            r##"<path stroke="#ff0000" d="M10 10L90 90"/>"##,
        ] {
            assert_eq!(classify(&tree(body)), None);
        }
    }
    #[test]
    fn inversion_preserves_hue_and_chroma_and_is_reversible() {
        let color = Color {
            red: 24,
            green: 16,
            blue: 8,
        };
        let original = rgb(color, false);
        let adapted = rgb(color, true);
        assert!(adapted[0] > adapted[1] && adapted[1] > adapted[2]);
        assert!((original[0] - original[2] - (adapted[0] - adapted[2])).abs() < 1e-6);
        let back = rgb(
            Color {
                red: (adapted[0] * 255.0).round() as u8,
                green: (adapted[1] * 255.0).round() as u8,
                blue: (adapted[2] * 255.0).round() as u8,
            },
            true,
        );
        for (a, b) in original.into_iter().zip(back) {
            assert!((a - b).abs() < 1e-6);
        }
    }
}
