use super::{Limits, usvg};
use crate::diagram::{self, ImagePolicy, fonts, images};
use roxmltree::{Document, Node};
use simplecss::{DeclarationTokenizer, StyleSheet};
use std::{
    borrow::Cow,
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use svgtypes::{Align, AspectRatio, Color, Paint};

const SVG_NS: &str = "http://www.w3.org/2000/svg";
const XLINK_NS: &str = "http://www.w3.org/1999/xlink";
const MAX_TEXT_BYTES: usize = 32 * 1024;

pub(super) fn prepare(bytes: &[u8]) -> Result<(usvg::Tree, [f64; 2]), String> {
    let source = little_clip_core::svg_image::document(bytes)?;
    let source = std::str::from_utf8(&source).map_err(|_| "SVG must use UTF-8")?;
    let document = diagram::svg_document(source)?;
    validate_source(&document)?;

    let root = document.root_element();
    let viewbox = diagram::svg_viewbox(root)?;
    let aspect = root
        .attribute("preserveAspectRatio")
        .map(str::parse)
        .transpose()
        .map_err(|_| "Invalid SVG preserveAspectRatio")?
        .unwrap_or_default();
    let (image_href_resolver, _) = images::resolver(ImagePolicy::Block);
    let missing_font = AtomicBool::new(false);
    let mut options = usvg::Options {
        font_family: "Tinos".into(),
        fontdb: Arc::clone(fonts::metric_fonts()),
        image_href_resolver,
        font_resolver: bundled_fonts(&missing_font),
        max_depth: 64,
        ..Default::default()
    };
    if let Some(viewbox) = viewbox {
        options.default_size = usvg::Size::from_wh(viewbox.width(), viewbox.height())
            .ok_or("Invalid SVG viewBox dimensions")?;
    }
    let tree = usvg::Tree::from_xmltree(&document, &options).map_err(|error| error.to_string())?;
    if missing_font.load(Ordering::Relaxed) {
        return Err("Unsupported SVG text font outside the bundled font set".into());
    }
    if tree.size().width() > 16_384.0 || tree.size().height() > 16_384.0 {
        return Err("SVG viewport exceeds the 16384-unit dimension budget".into());
    }
    Limits::default().group(tree.root(), 0)?;
    let viewport = tree.size();
    Ok((tree, user_scale(aspect, viewbox, viewport)))
}

fn bundled_fonts(missing: &AtomicBool) -> usvg::FontResolver<'_> {
    let select_font = usvg::FontResolver::default_font_selector();
    usvg::FontResolver {
        select_font: Box::new(move |font, database| {
            let has_family = font.families().iter().any(|family| match family {
                usvg::FontFamily::Named(name) => database.faces().any(|face| {
                    face.families
                        .iter()
                        .any(|(known, _)| known.eq_ignore_ascii_case(name))
                }),
                _ => true,
            });
            let selected = select_font(font, database);
            if !has_family || selected.is_none() {
                missing.store(true, Ordering::Relaxed);
            }
            selected
        }),
        ..Default::default()
    }
}

// usvg retains drawing transforms in viewport coordinates. Convert the user
// tolerance to that coordinate space using the root viewBox's effective scale.
// The intrinsic viewport is required here, even when the aspect is sliced.
fn user_scale(
    aspect: AspectRatio,
    viewbox: Option<usvg::NonZeroRect>,
    viewport: usvg::Size,
) -> [f64; 2] {
    let Some(viewbox) = viewbox else {
        return [1.0, 1.0];
    };
    let x = f64::from(viewport.width()) / f64::from(viewbox.width());
    let y = f64::from(viewport.height()) / f64::from(viewbox.height());
    match (aspect.align, aspect.slice) {
        (Align::None, _) => [x, y],
        (_, true) => [x.max(y); 2],
        _ => [x.min(y); 2],
    }
}

fn validate_source(document: &Document<'_>) -> Result<(), String> {
    let ids: HashMap<_, _> = document
        .descendants()
        .filter_map(|node| node.attribute("id").map(|id| (id, node)))
        .collect();
    let mut referenced_bytes = 0;
    let mut text_bytes = 0;
    let mut style_rules = 0;

    // Walk with subtree skips so metadata never masquerades as painted SVG.
    let mut skip_end = 0;
    for node in document.descendants() {
        if node.range().start < skip_end {
            continue;
        }
        if node
            .pi()
            .is_some_and(|pi| pi.target.eq_ignore_ascii_case("xml-stylesheet"))
        {
            return Err("Unsupported external SVG stylesheet".into());
        }
        text_bytes += svg_text_bytes(node);
        if node.tag_name().namespace() != Some(SVG_NS) {
            continue;
        }
        match node.tag_name().name() {
            "metadata" | "title" | "desc" => {
                skip_end = node.range().end;
                continue;
            }
            "svg" | "g" | "defs" | "symbol" | "a" | "path" | "rect" | "circle" | "ellipse"
            | "line" | "polyline" | "polygon" | "text" | "tspan" | "textPath"
            | "linearGradient" | "radialGradient" | "stop" => {}
            "style" => {
                for css in node.children().filter_map(|node| node.text()) {
                    style_rules += validate_sheet(css, &ids)?;
                    if style_rules > 256 {
                        return Err("SVG stylesheet exceeds the retained rule budget".into());
                    }
                }
            }
            "use" => {
                let target = local_href(node, &ids)?;
                referenced_bytes += target.range().len();
                if referenced_bytes > diagram::MAX_SVG {
                    return Err("SVG instances exceed the retained source budget".into());
                }
                text_bytes += target.descendants().map(svg_text_bytes).sum::<usize>();
                if target
                    .descendants()
                    .any(|node| node.has_tag_name((SVG_NS, "use")))
                {
                    return Err("Unsupported nested SVG instances".into());
                }
            }
            name => return Err(format!("Unsupported SVG element <{name}>")),
        }
        validate_attributes(node, &ids)?;
    }
    if text_bytes > MAX_TEXT_BYTES {
        return Err("SVG text exceeds the retained shaping budget".into());
    }
    Ok(())
}

fn svg_text_bytes(node: Node<'_, '_>) -> usize {
    if node.is_text()
        && node.parent().is_some_and(|parent| {
            matches!(
                parent.tag_name().name(),
                "text" | "tspan" | "textPath" | "a"
            )
        })
    {
        node.text().unwrap_or_default().len()
    } else {
        0
    }
}

fn local_href<'a, 'input>(
    node: Node<'a, 'input>,
    ids: &HashMap<&str, Node<'a, 'input>>,
) -> Result<Node<'a, 'input>, String> {
    let target = node
        .attribute("href")
        .or_else(|| node.attribute((XLINK_NS, "href")))
        .and_then(|href| href.trim().strip_prefix('#'))
        .and_then(|id| ids.get(id))
        .filter(|target| target.tag_name().namespace() == Some(SVG_NS))
        .copied();
    target.ok_or_else(|| {
        format!(
            "Unsupported external or unresolved SVG <{}> reference",
            node.tag_name().name()
        )
    })
}

fn validate_attributes(
    node: Node<'_, '_>,
    ids: &HashMap<&str, Node<'_, '_>>,
) -> Result<(), String> {
    for attribute in node.attributes() {
        if attribute.namespace().is_some() {
            continue;
        }
        let (name, value) = (attribute.name(), attribute.value().trim());
        match name {
            "style" => {
                let css = css_without_comments(value)?;
                validate_declarations(&css, ids)?;
            }
            "color-interpolation" if value == "sRGB" => {}
            _ if neutral_effect_value(name).is_some_and(|default| value != default) => {
                return Err(format!("Unsupported SVG effect {name}"));
            }
            "transform-origin"
            | "transform-box"
            | "pathLength"
            | "requiredFeatures"
            | "requiredExtensions"
            | "systemLanguage"
            | "font-feature-settings"
            | "text-orientation"
            | "white-space"
            | "inline-size"
            | "stroke-alignment"
            | "enable-background" => {
                return Err(format!("Unsupported SVG attribute {name}"));
            }
            "fill" | "stroke" => validate_paint(value, ids)?,
            "color" | "stop-color" => validate_color(value)?,
            "transform" | "gradientTransform" => {
                value
                    .parse::<svgtypes::Transform>()
                    .map_err(|_| "Unsupported SVG transform")?;
            }
            _ => {}
        }
    }
    validate_link(node, ids)
}

fn validate_link(node: Node<'_, '_>, ids: &HashMap<&str, Node<'_, '_>>) -> Result<(), String> {
    let tag = node.tag_name().name();
    let has_reference = node.has_attribute("href") || node.has_attribute((XLINK_NS, "href"));
    if tag == "textPath" || (has_reference && matches!(tag, "linearGradient" | "radialGradient")) {
        local_href(node, ids)?;
    }
    Ok(())
}

fn neutral_effect_value(name: &str) -> Option<&'static str> {
    match name {
        "filter" | "mask" | "clip" | "clip-path" | "marker" | "marker-start" | "marker-mid"
        | "marker-end" | "vector-effect" => Some("none"),
        "mix-blend-mode" => Some("normal"),
        "isolation" | "color-interpolation" => Some("auto"),
        _ => None,
    }
}

fn css_without_comments(css: &str) -> Result<Cow<'_, str>, String> {
    if !css.contains("/*") {
        return Ok(Cow::Borrowed(css));
    }
    let mut output = String::with_capacity(css.len());
    let mut tail = css;
    while let Some((before, comment)) = tail.split_once("/*") {
        output.push_str(before);
        output.push(' ');
        tail = comment.split_once("*/").ok_or("Invalid SVG CSS comment")?.1;
    }
    output.push_str(tail);
    Ok(Cow::Owned(output))
}

fn validate_sheet(css: &str, ids: &HashMap<&str, Node<'_, '_>>) -> Result<usize, String> {
    let css = css_without_comments(css)?;
    let mut rules = 0;
    for rule in css.trim().split_inclusive('}') {
        let (selector, body) = rule.split_once('{').ok_or("Unsupported SVG CSS rule")?;
        if selector.contains(['@', '\\', ':', '|']) || selector.len() > 1024 {
            return Err("Unsupported SVG CSS selector or at-rule".into());
        }
        validate_declarations(body.strip_suffix('}').ok_or("Invalid SVG CSS rule")?, ids)?;
        let parsed = StyleSheet::parse(rule).rules.len();
        if !body
            .trim_matches(|ch: char| ch == '}' || ch.is_whitespace())
            .is_empty()
            && parsed == 0
        {
            return Err("Unsupported SVG CSS rule".into());
        }
        rules += parsed;
        if rules > 256 {
            return Err("SVG stylesheet exceeds the retained rule budget".into());
        }
    }
    Ok(rules)
}

fn validate_declarations(css: &str, ids: &HashMap<&str, Node<'_, '_>>) -> Result<(), String> {
    if css.contains(['@', '\\', '{', '}']) {
        return Err("Unsupported SVG CSS declaration syntax".into());
    }
    for part in css
        .split(';')
        .map(str::trim)
        .filter(|part| !part.is_empty())
    {
        let declaration = DeclarationTokenizer::from(part)
            .next()
            .ok_or("Unsupported SVG CSS declaration")?;
        validate_declaration(declaration, ids)?;
    }
    Ok(())
}

fn validate_declaration(
    declaration: simplecss::Declaration<'_>,
    ids: &HashMap<&str, Node<'_, '_>>,
) -> Result<(), String> {
    let name = declaration.name;
    let value = declaration.value;
    // usvg resolves the cascade for these presentation properties. Other
    // declarations may affect browser paint while disappearing from its tree.
    const PROPERTIES: &str = concat!(
        "display visibility opacity fill-opacity fill-rule paint-order ",
        "stroke-opacity stroke-width stroke-linecap stroke-linejoin stroke-miterlimit ",
        "stroke-dasharray stroke-dashoffset stop-opacity shape-rendering text-rendering ",
        "font-family font-size font-style font-weight font-stretch font-variant font-kerning ",
        "font-optical-sizing font-variation-settings letter-spacing word-spacing text-anchor ",
        "dominant-baseline alignment-baseline baseline-shift text-decoration writing-mode"
    );
    match name {
        "fill" | "stroke" => validate_paint(value, ids)?,
        "color" | "stop-color" => validate_color(value)?,
        "color-interpolation" if value == "sRGB" => {}
        _ if neutral_effect_value(name) == Some(value) => {}
        _ if PROPERTIES
            .split_ascii_whitespace()
            .any(|property| property == name) => {}
        _ => return Err(format!("Unsupported SVG CSS property {name}")),
    }
    if ["var(", "calc(", "env("]
        .iter()
        .any(|function| value.contains(function))
    {
        return Err(format!("Unsupported SVG CSS value for {name}"));
    }
    Ok(())
}

fn validate_paint(value: &str, ids: &HashMap<&str, Node<'_, '_>>) -> Result<(), String> {
    let paint = Paint::from_str(value).map_err(|_| "Unsupported SVG paint value")?;
    if let Paint::FuncIRI(id, _) = paint
        && ids.get(id).is_none_or(|node| {
            !matches!(node.tag_name().name(), "linearGradient" | "radialGradient")
                || node.tag_name().namespace() != Some(SVG_NS)
        })
    {
        return Err("Unsupported or unresolved SVG gradient reference".into());
    }
    Ok(())
}

fn validate_color(value: &str) -> Result<(), String> {
    if matches!(value, "inherit" | "currentColor") || value.parse::<Color>().is_ok() {
        Ok(())
    } else {
        Err("Unsupported SVG color value".into())
    }
}
