//! Lossless numeric adaptation of the pinned usvg XML writer. Its public writer
//! caps decimal precision at twelve places. Restore path and transform tokens
//! from the normalized tree with f32's round-trip spelling before parsing again.
use std::fmt::Write;

use super::super::FormatEncodeError;

fn error() -> FormatEncodeError {
    FormatEncodeError::Encoding("normalized SVG writer/tree structure disagrees".to_owned())
}
type Edits = Vec<(std::ops::Range<usize>, String)>;
pub(super) struct NestedImage<'a> {
    pub tree: &'a usvg::Tree,
    pub encoded: String,
}
pub(super) struct SerializedSvg<'a> {
    pub xml: String,
    pub images: Vec<NestedImage<'a>>,
}
fn image_binding<'a>(
    image: &'a usvg::Image,
    href: &str,
    images: &mut Vec<NestedImage<'a>>,
) -> Result<(), FormatEncodeError> {
    if let usvg::ImageKind::SVG(tree) = image.kind() {
        let encoded = href
            .strip_prefix("data:image/svg+xml;base64,")
            .ok_or_else(error)?
            .trim();
        images.try_reserve(1).map_err(|_| super::raster::memory())?;
        images.push(NestedImage {
            tree,
            encoded: super::super::try_owned(encoded).map_err(|_| super::raster::memory())?,
        });
    }
    Ok(())
}
fn attribute(
    node: roxmltree::Node<'_, '_>,
    name: &str,
    value: String,
    edits: &mut Edits,
) -> Result<(), FormatEncodeError> {
    if let Some(attr) = node.attribute_node(name) {
        edits.try_reserve(1).map_err(|_| super::raster::memory())?;
        let mut replacement = String::new();
        replacement
            .try_reserve(name.len().saturating_add(value.len()).saturating_add(3))
            .map_err(|_| super::raster::memory())?;
        replacement.push_str(name);
        replacement.push_str("=\"");
        replacement.push_str(&value);
        replacement.push('"');
        edits.push((attr.range(), replacement));
    }
    Ok(())
}
fn transform(
    node: roxmltree::Node<'_, '_>,
    name: &str,
    t: tiny_skia::Transform,
    edits: &mut Edits,
) -> Result<(), FormatEncodeError> {
    attribute(
        node,
        name,
        format!(
            "matrix({} {} {} {} {} {})",
            t.sx, t.ky, t.kx, t.sy, t.tx, t.ty
        ),
        edits,
    )
}
fn path(
    path: &usvg::Path,
    node: roxmltree::Node<'_, '_>,
    edits: &mut Edits,
) -> Result<(), FormatEncodeError> {
    if node.tag_name().name() != "path" {
        return Err(error());
    }
    attribute(node, "d", path_data(path)?, edits)
}
fn path_data(path: &usvg::Path) -> Result<String, FormatEncodeError> {
    let mut data = String::new();
    data.try_reserve(path.data().len().saturating_mul(320))
        .map_err(|_| super::raster::memory())?;
    for segment in path.data().segments() {
        match segment {
            tiny_skia::PathSegment::MoveTo(p) => write!(data, "M{} {} ", p.x, p.y),
            tiny_skia::PathSegment::LineTo(p) => write!(data, "L{} {} ", p.x, p.y),
            tiny_skia::PathSegment::QuadTo(c, p) => {
                write!(data, "Q{} {} {} {} ", c.x, c.y, p.x, p.y)
            }
            tiny_skia::PathSegment::CubicTo(a, b, p) => {
                write!(data, "C{} {} {} {} {} {} ", a.x, a.y, b.x, b.y, p.x, p.y)
            }
            tiny_skia::PathSegment::Close => write!(data, "Z "),
        }
        .map_err(|_| super::raster::memory())?;
    }
    Ok(data)
}
fn nodes<'a>(
    group: &'a usvg::Group,
    xml: roxmltree::Node<'_, '_>,
    clip: bool,
    edits: &mut Edits,
    images: &mut Vec<NestedImage<'a>>,
) -> Result<(), FormatEncodeError> {
    let mut children = xml
        .children()
        .filter(|n| n.is_element() && n.tag_name().name() != "defs");
    for child in group.children() {
        if clip {
            let nested = match child {
                usvg::Node::Group(g) => Some(g.as_ref()),
                usvg::Node::Text(t) => Some(t.flattened()),
                _ => None,
            };
            if let Some(g) = nested {
                for p in g.children() {
                    if let usvg::Node::Path(p) = p {
                        let xml = children.next().ok_or_else(error)?;
                        path(p, xml, edits)?;
                        transform(xml, "transform", g.transform(), edits)?;
                    }
                }
                continue;
            }
        }
        node(child, children.next().ok_or_else(error)?, edits, images)?;
    }
    if children.next().is_some() {
        return Err(error());
    }
    Ok(())
}
fn node<'a>(
    source: &'a usvg::Node,
    xml: roxmltree::Node<'_, '_>,
    edits: &mut Edits,
    images: &mut Vec<NestedImage<'a>>,
) -> Result<(), FormatEncodeError> {
    match source {
        usvg::Node::Path(p) => path(p, xml, edits),
        usvg::Node::Image(image) => {
            if xml.tag_name().name() == "image" {
                let href = xml
                    .attributes()
                    .find(|a| a.name() == "href")
                    .ok_or_else(error)?
                    .value();
                image_binding(image, href, images)
            } else {
                Err(error())
            }
        }
        usvg::Node::Group(g) => group(g, xml, edits, images),
        usvg::Node::Text(t) => group(t.flattened(), xml, edits, images),
    }
}
fn group<'a>(
    g: &'a usvg::Group,
    xml: roxmltree::Node<'_, '_>,
    edits: &mut Edits,
    images: &mut Vec<NestedImage<'a>>,
) -> Result<(), FormatEncodeError> {
    if xml.tag_name().name() != "g" {
        return Err(error());
    }
    transform(xml, "transform", g.transform(), edits)?;
    nodes(g, xml, false, edits, images)
}
pub(super) fn write<'a>(
    tree: &'a usvg::Tree,
    prefix: Option<String>,
) -> Result<SerializedSvg<'a>, FormatEncodeError> {
    let xml = tree.to_string(&usvg::WriteOptions {
        id_prefix: prefix.clone(),
        coordinates_precision: 12,
        transforms_precision: 12,
        ..Default::default()
    });
    let document = roxmltree::Document::parse(&xml).map_err(|_| error())?;
    let root = document.root_element();
    let mut edits = Vec::new();
    let mut nested_images = Vec::new();
    nodes(tree.root(), root, false, &mut edits, &mut nested_images)?;
    let resource = |id: &str| {
        let id = format!("{}{id}", prefix.as_deref().unwrap_or(""));
        document
            .descendants()
            .find(|n| n.attribute("id") == Some(&id))
            .ok_or_else(error)
    };
    for clip in tree.clip_paths() {
        let xml = resource(clip.id())?;
        transform(xml, "transform", clip.transform(), &mut edits)?;
        nodes(clip.root(), xml, true, &mut edits, &mut nested_images)?;
    }
    for mask in tree.masks() {
        nodes(
            mask.root(),
            resource(mask.id())?,
            false,
            &mut edits,
            &mut nested_images,
        )?;
    }
    for pattern in tree.patterns() {
        let xml = resource(pattern.id())?;
        transform(xml, "patternTransform", pattern.transform(), &mut edits)?;
        nodes(pattern.root(), xml, false, &mut edits, &mut nested_images)?;
    }
    for gradient in tree.linear_gradients() {
        transform(
            resource(gradient.id())?,
            "gradientTransform",
            gradient.transform(),
            &mut edits,
        )?;
    }
    for gradient in tree.radial_gradients() {
        transform(
            resource(gradient.id())?,
            "gradientTransform",
            gradient.transform(),
            &mut edits,
        )?;
    }
    let mut images = Vec::new();
    let mut input_definitions = String::new();
    let input_prefix = super::raster::namespace(&document, "__pdf_filter_input_")?;
    let mut input_index = 0;
    for filter in tree.filters() {
        let mut image_nodes = resource(filter.id())?
            .children()
            .filter(|n| n.has_tag_name("feImage"));
        for primitive in filter.primitives() {
            if let usvg::filter::Kind::Image(image) = primitive.kind() {
                let image_xml = image_nodes.next().ok_or_else(error)?;
                if let Some(child) = image.root().children().first() {
                    if child.id().is_empty() {
                        // usvg's writer emits href="#" and deduplicates anonymous
                        // feImage inputs by the empty ID. Serialize each input
                        // explicitly from its already-normalized subtree.
                        let id = format!("{input_prefix}{input_index}");
                        input_index += 1;
                        input_definitions
                            .try_reserve(128)
                            .map_err(|_| super::raster::memory())?;
                        write!(input_definitions, "<g id=\"{id}\">")
                            .map_err(|_| super::raster::memory())?;
                        for child in image.root().children() {
                            input_node(
                                child,
                                prefix.as_deref().unwrap_or(""),
                                &mut input_definitions,
                                &mut nested_images,
                            )?;
                        }
                        input_definitions.push_str("</g>");
                        let href = image_xml
                            .attributes()
                            .find(|a| a.name() == "href")
                            .ok_or_else(error)?;
                        edits.try_reserve(1).map_err(|_| super::raster::memory())?;
                        edits.push((href.range(), format!("xlink:href=\"#{id}\"")));
                    } else if !images.contains(&child.id()) {
                        node(child, resource(child.id())?, &mut edits, &mut nested_images)?;
                        images.try_reserve(1).map_err(|_| super::raster::memory())?;
                        images.push(child.id());
                    }
                }
            }
        }
    }
    if !input_definitions.is_empty() {
        let defs = root
            .children()
            .find(|n| n.has_tag_name("defs"))
            .ok_or_else(error)?;
        let end = defs.range().end - 7;
        edits.try_reserve(1).map_err(|_| super::raster::memory())?;
        edits.push((end..end, input_definitions));
        // The pinned writer leaves one unnamed, now-unreferenced anonymous
        // feImage template in defs. Its named replacements above own the inputs.
        for stale in defs.children().filter(|n| {
            n.is_element()
                && n.attribute("id").is_none()
                && matches!(n.tag_name().name(), "g" | "path" | "image")
        }) {
            edits.try_reserve(1).map_err(|_| super::raster::memory())?;
            edits.push((stale.range(), String::new()));
        }
    }
    Ok(SerializedSvg {
        xml: super::raster::edit(&xml, edits)?,
        images: nested_images,
    })
}

fn attr(out: &mut String, name: &str, value: &str) -> Result<(), FormatEncodeError> {
    out.try_reserve(name.len() + value.len().saturating_mul(6) + 4)
        .map_err(|_| super::raster::memory())?;
    out.push(' ');
    out.push_str(name);
    out.push_str("=\"");
    for c in value.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '"' => out.push_str("&quot;"),
            '<' => out.push_str("&lt;"),
            _ => out.push(c),
        }
    }
    out.push('"');
    Ok(())
}
fn matrix(t: tiny_skia::Transform) -> String {
    format!(
        "matrix({} {} {} {} {} {})",
        t.sx, t.ky, t.kx, t.sy, t.tx, t.ty
    )
}
fn paint(p: &usvg::Paint, prefix: &str) -> String {
    match p {
        usvg::Paint::Color(c) => format!("#{:02x}{:02x}{:02x}", c.red, c.green, c.blue),
        usvg::Paint::LinearGradient(g) => format!("url(#{prefix}{})", g.id()),
        usvg::Paint::RadialGradient(g) => format!("url(#{prefix}{})", g.id()),
        usvg::Paint::Pattern(p) => format!("url(#{prefix}{})", p.id()),
    }
}
fn input_node<'a>(
    node: &'a usvg::Node,
    prefix: &str,
    out: &mut String,
    images: &mut Vec<NestedImage<'a>>,
) -> Result<(), FormatEncodeError> {
    out.try_reserve(1024).map_err(|_| super::raster::memory())?;
    match node {
        usvg::Node::Group(g) => input_group(g, prefix, out, images),
        usvg::Node::Text(t) => input_group(t.flattened(), prefix, out, images),
        usvg::Node::Path(p) => {
            out.push_str("<path");
            attr(out, "d", &path_data(p)?)?;
            if !p.is_visible() {
                attr(out, "visibility", "hidden")?;
            }
            attr(
                out,
                "shape-rendering",
                match p.rendering_mode() {
                    usvg::ShapeRendering::OptimizeSpeed => "optimizeSpeed",
                    usvg::ShapeRendering::CrispEdges => "crispEdges",
                    usvg::ShapeRendering::GeometricPrecision => "geometricPrecision",
                },
            )?;
            if let Some(fill) = p.fill() {
                attr(out, "fill", &paint(fill.paint(), prefix))?;
                attr(out, "fill-opacity", &fill.opacity().get().to_string())?;
                attr(
                    out,
                    "fill-rule",
                    match fill.rule() {
                        usvg::FillRule::NonZero => "nonzero",
                        usvg::FillRule::EvenOdd => "evenodd",
                    },
                )?;
            } else {
                attr(out, "fill", "none")?;
            }
            if let Some(stroke) = p.stroke() {
                attr(out, "stroke", &paint(stroke.paint(), prefix))?;
                attr(out, "stroke-opacity", &stroke.opacity().get().to_string())?;
                attr(out, "stroke-width", &stroke.width().get().to_string())?;
                attr(
                    out,
                    "stroke-miterlimit",
                    &stroke.miterlimit().get().to_string(),
                )?;
                attr(
                    out,
                    "stroke-linecap",
                    match stroke.linecap() {
                        usvg::LineCap::Butt => "butt",
                        usvg::LineCap::Round => "round",
                        usvg::LineCap::Square => "square",
                    },
                )?;
                attr(
                    out,
                    "stroke-linejoin",
                    match stroke.linejoin() {
                        usvg::LineJoin::Miter => "miter",
                        usvg::LineJoin::MiterClip => "miter-clip",
                        usvg::LineJoin::Round => "round",
                        usvg::LineJoin::Bevel => "bevel",
                    },
                )?;
                if let Some(dash) = stroke.dasharray() {
                    let mut values = String::new();
                    values
                        .try_reserve(dash.len().saturating_mul(64))
                        .map_err(|_| super::raster::memory())?;
                    for v in dash {
                        write!(values, "{v} ").map_err(|_| super::raster::memory())?;
                    }
                    attr(out, "stroke-dasharray", &values)?;
                    attr(out, "stroke-dashoffset", &stroke.dashoffset().to_string())?;
                }
            }
            if p.paint_order() == usvg::PaintOrder::StrokeAndFill {
                attr(out, "paint-order", "stroke fill")?;
            }
            out.push_str("/>");
            Ok(())
        }
        usvg::Node::Image(image) => {
            use base64::Engine;
            let svg;
            let (kind, bytes) = match image.kind() {
                usvg::ImageKind::PNG(b) => ("png", b.as_slice()),
                usvg::ImageKind::JPEG(b) => ("jpeg", b.as_slice()),
                usvg::ImageKind::GIF(b) => ("gif", b.as_slice()),
                usvg::ImageKind::WEBP(b) => ("webp", b.as_slice()),
                usvg::ImageKind::SVG(tree) => {
                    svg = write(tree, None)?.xml;
                    ("svg+xml", svg.as_bytes())
                }
            };
            let mut data = String::new();
            data.try_reserve(
                base64::encoded_len(bytes.len(), true).ok_or_else(super::raster::memory)? + 32,
            )
            .map_err(|_| super::raster::memory())?;
            write!(data, "data:image/{kind};base64,").map_err(|_| super::raster::memory())?;
            base64::engine::general_purpose::STANDARD.encode_string(bytes, &mut data);
            out.push_str("<image");
            attr(out, "width", &image.size().width().to_string())?;
            attr(out, "height", &image.size().height().to_string())?;
            attr(out, "href", &data)?;
            image_binding(image, &data, images)?;
            if !image.is_visible() {
                attr(out, "visibility", "hidden")?;
            }
            attr(
                out,
                "image-rendering",
                match image.rendering_mode() {
                    usvg::ImageRendering::OptimizeQuality => "auto",
                    usvg::ImageRendering::OptimizeSpeed => "optimizeSpeed",
                    usvg::ImageRendering::Smooth => "smooth",
                    usvg::ImageRendering::HighQuality => "high-quality",
                    usvg::ImageRendering::CrispEdges => "crisp-edges",
                    usvg::ImageRendering::Pixelated => "pixelated",
                },
            )?;
            out.push_str("/>");
            Ok(())
        }
    }
}
fn input_group<'a>(
    g: &'a usvg::Group,
    prefix: &str,
    out: &mut String,
    images: &mut Vec<NestedImage<'a>>,
) -> Result<(), FormatEncodeError> {
    out.try_reserve(8).map_err(|_| super::raster::memory())?;
    out.push_str("<g");
    attr(out, "transform", &matrix(g.transform()))?;
    attr(out, "opacity", &g.opacity().get().to_string())?;
    if let Some(clip) = g.clip_path() {
        attr(out, "clip-path", &format!("url(#{prefix}{})", clip.id()))?;
    }
    if let Some(mask) = g.mask() {
        attr(out, "mask", &format!("url(#{prefix}{})", mask.id()))?;
    }
    if !g.filters().is_empty() {
        let mut filters = String::new();
        for f in g.filters() {
            filters
                .try_reserve(prefix.len() + f.id().len() + 8)
                .map_err(|_| super::raster::memory())?;
            write!(filters, "url(#{prefix}{}) ", f.id()).map_err(|_| super::raster::memory())?;
        }
        attr(out, "filter", &filters)?;
    }
    let blend = match g.blend_mode() {
        usvg::BlendMode::Normal => "normal",
        usvg::BlendMode::Multiply => "multiply",
        usvg::BlendMode::Screen => "screen",
        usvg::BlendMode::Overlay => "overlay",
        usvg::BlendMode::Darken => "darken",
        usvg::BlendMode::Lighten => "lighten",
        usvg::BlendMode::ColorDodge => "color-dodge",
        usvg::BlendMode::ColorBurn => "color-burn",
        usvg::BlendMode::HardLight => "hard-light",
        usvg::BlendMode::SoftLight => "soft-light",
        usvg::BlendMode::Difference => "difference",
        usvg::BlendMode::Exclusion => "exclusion",
        usvg::BlendMode::Hue => "hue",
        usvg::BlendMode::Saturation => "saturation",
        usvg::BlendMode::Color => "color",
        usvg::BlendMode::Luminosity => "luminosity",
        usvg::BlendMode::PlusLighter => "plus-lighter",
    };
    attr(
        out,
        "style",
        &format!(
            "mix-blend-mode:{blend};isolation:{}",
            if g.isolate() { "isolate" } else { "auto" }
        ),
    )?;
    out.push('>');
    for child in g.children() {
        input_node(child, prefix, out, images)?;
    }
    out.try_reserve(4).map_err(|_| super::raster::memory())?;
    out.push_str("</g>");
    Ok(())
}
