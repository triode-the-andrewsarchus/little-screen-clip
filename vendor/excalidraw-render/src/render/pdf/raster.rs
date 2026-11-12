//! Occurrence-specific SVG raster adaptation. The admitted closure grid is the
//! encoder's grid; the backend draws native content and already-rendered images.
use std::collections::BTreeMap;
use std::fmt::Write;

use base64::Engine;

use super::super::svg_filter_work::{self, FilterWork};
use super::super::{try_owned, FormatEncodeError};
use super::PdfRasterClosure;

fn invalid(message: &str) -> FormatEncodeError {
    FormatEncodeError::Encoding(message.to_owned())
}
pub(super) fn memory() -> FormatEncodeError {
    FormatEncodeError::MemoryUnavailable {
        subject_kind: "format_workspace",
        subject: "pdf_raster_adapter",
    }
}

/// Full-precision transforms until the rasterizer's explicit f32 seam.
#[derive(Clone, Copy, Debug)]
pub(super) struct Matrix(pub(super) [f64; 6]);
impl Matrix {
    /// Matrix multiplication: apply `right` first, then `self`.
    pub(super) fn concat(self, right: Self) -> Self {
        let [a, b, c, d, e, f] = self.0;
        let [g, h, i, j, k, l] = right.0;
        Self([
            a * g + c * h,
            b * g + d * h,
            a * i + c * j,
            b * i + d * j,
            a * k + c * l + e,
            b * k + d * l + f,
        ])
    }
    pub(super) fn from_skia(t: tiny_skia::Transform) -> Self {
        Self([t.sx, t.ky, t.kx, t.sy, t.tx, t.ty].map(f64::from))
    }
    pub(super) fn inverse(self) -> Result<Self, FormatEncodeError> {
        let [a, b, c, d, e, f] = self.0;
        let det = a * d - b * c;
        if det == 0.0 || !det.is_finite() {
            return Err(invalid("singular PDF raster placement"));
        }
        Ok(Self([
            d / det,
            -b / det,
            -c / det,
            a / det,
            (c * f - d * e) / det,
            (b * e - a * f) / det,
        ]))
    }
    pub(super) fn translate(x: f64, y: f64) -> Self {
        Self([1.0, 0.0, 0.0, 1.0, x, y])
    }
    pub(super) fn bounds(self, b: (f64, f64, f64, f64)) -> (f64, f64, f64, f64) {
        let [a, b0, c, d, e, f] = self.0;
        [(b.0, b.1), (b.2, b.1), (b.2, b.3), (b.0, b.3)]
            .into_iter()
            .fold(
                (
                    f64::INFINITY,
                    f64::INFINITY,
                    f64::NEG_INFINITY,
                    f64::NEG_INFINITY,
                ),
                |(x, y, r, b), (px, py)| {
                    let (px, py) = (a * px + c * py + e, b0 * px + d * py + f);
                    (x.min(px), y.min(py), r.max(px), b.max(py))
                },
            )
    }
    fn skia(self) -> Result<tiny_skia::Transform, FormatEncodeError> {
        let m = self.0.map(|v| v as f32);
        if m.iter().any(|v| !v.is_finite()) {
            return Err(invalid("PDF raster transform is not finite"));
        }
        Ok(tiny_skia::Transform::from_row(
            m[0], m[1], m[2], m[3], m[4], m[5],
        ))
    }
}

#[derive(Debug)]
pub(super) struct RasterPlacement {
    pub(super) node_id: String,
    pub(super) parent_to_device: Matrix,
    pub(super) output_x: f64,
    pub(super) output_y: f64,
    pub(super) input_x: f64,
    pub(super) input_y: f64,
    pub(super) input_width: u64,
    pub(super) input_height: u64,
}

/// A normalized, expanded SVG with stable private IDs for filtered groups.
/// All native ancestor transforms, clipping, masks, opacity and blends stay
/// outside the filtered group, so replacing it cannot bake them twice.
#[derive(Debug)]
pub(super) struct PreparedSvg {
    pub(super) tree: usvg::Tree,
    xml: String,
    usages: BTreeMap<String, String>,
}
impl PreparedSvg {
    pub(super) fn new(tree: &usvg::Tree) -> Result<Self, FormatEncodeError> {
        // The serializer adapter preserves normalized numeric tokens, including
        // subnormals. Text is already outlined by usvg.
        let xml = occurrence_resources(&expanded_xml(tree, &mut 0, None)?)?;
        let document =
            roxmltree::Document::parse(&xml).map_err(|_| invalid("invalid normalized PDF SVG"))?;
        let prefix = namespace(&document, "__pdf_raster_")?;
        let mut edits = Vec::new();
        let mut usages = BTreeMap::new();
        for node in document.descendants().filter(|n| {
            n.is_element() && n.tag_name().name() == "g" && n.attribute("filter").is_some()
        }) {
            let id = format!("{prefix}{}", usages.len());
            let usage = node
                .attribute("id")
                .or_else(|| {
                    node.children()
                        .find(|n| n.is_element())
                        .and_then(|n| n.attribute("id"))
                })
                .unwrap_or("");
            usages.insert(id.clone(), usage.to_owned());
            // Move only the filter to an inner group. References to an original
            // group ID continue to refer to its unchanged outer native group.
            let filter = node
                .attribute_node("filter")
                .ok_or_else(|| invalid("missing SVG group filter"))?;
            let range = node.range();
            let open_end = range.start
                + xml[range.clone()]
                    .find('>')
                    .ok_or_else(|| invalid("missing SVG group opening"))?
                + 1;
            let mut opening = String::new();
            opening
                .try_reserve(open_end - range.start + id.len() + 64)
                .map_err(|_| memory())?;
            opening.push_str(&xml[range.start..filter.range().start]);
            opening.push_str(&xml[filter.range().end..open_end]);
            write!(opening, "<g id=\"{id}\" {}>", &xml[filter.range()]).map_err(|_| memory())?;
            edits.try_reserve(2).map_err(|_| memory())?;
            edits.push((range.start..open_end, opening));
            edits.push((
                range.end - 4..range.end,
                try_owned("</g></g>").map_err(|_| memory())?,
            ));
        }
        let xml = edit(&xml, edits)?;
        let tree = parse(&xml)?;
        Ok(Self { tree, xml, usages })
    }
    pub(super) fn original_usage<'a>(&'a self, id: &'a str) -> &'a str {
        self.usages.get(id).map_or(id, String::as_str)
    }
    pub(super) fn lower(
        &self,
        closures: &[&PdfRasterClosure],
    ) -> Result<usvg::Tree, FormatEncodeError> {
        let document = roxmltree::Document::parse(&self.xml)
            .map_err(|_| invalid("invalid prepared PDF SVG"))?;
        let mut edits = Vec::new();
        for node in document.descendants().filter(|n| {
            n.is_element()
                && n.attribute("id")
                    .is_some_and(|id| self.usages.contains_key(id))
        }) {
            let id = node
                .attribute("id")
                .ok_or_else(|| invalid("missing prepared SVG group ID"))?;
            let replacement =
                if let Some(closure) = closures.iter().find(|c| c.raster.node_id == id) {
                    self.render(closure)?
                } else {
                    // A merged child is consumed by its parent's replacement;
                    // an unselected/cropped-away group must not reach the backend.
                    String::new()
                };
            edits.try_reserve(1).map_err(|_| memory())?;
            // Avoid introducing an otherwise empty native group solely to hold
            // the filter while planning. Real ancestor attributes stay outside.
            let range = node
                .parent_element()
                .filter(|parent| parent.tag_name().name() == "g" && parent.attributes().len() == 0)
                .map_or_else(|| node.range(), |parent| parent.range());
            edits.push((range, replacement));
        }
        let result = parse(&edit(&self.xml, edits)?)?;
        let mut has_filter = false;
        fn inspect(group: &usvg::Group, found: &mut bool) {
            *found |= !group.filters().is_empty();
            for child in group.children() {
                if let usvg::Node::Group(group) = child {
                    inspect(group, found);
                }
                child.subroots(|root| inspect(root, found));
            }
        }
        inspect(result.root(), &mut has_filter);
        if has_filter {
            return Err(invalid(
                "unplanned PDF raster effect reached native encoding",
            ));
        }
        Ok(result)
    }
    /// What resvg will spend on the filter effects of `closure`, measured
    /// with the input grid and transform `render_node` paints it with. The
    /// walk stops past `budget` work units.
    pub(super) fn filter_work(
        &self,
        closure: &PdfRasterClosure,
        budget: u64,
    ) -> Result<FilterWork, FormatEncodeError> {
        with_node(self.tree.root(), &closure.raster.node_id, &mut |node| {
            let (width, height, transform) = input_grid(closure, node)?;
            Ok(svg_filter_work::measure_node(
                node, transform, width, height, budget,
            ))
        })
        .ok_or_else(|| invalid("PDF raster closure lost its node"))?
    }
    fn render(&self, closure: &PdfRasterClosure) -> Result<String, FormatEncodeError> {
        with_node(self.tree.root(), &closure.raster.node_id, &mut |node| {
            self.render_node(closure, node)
        })
        .ok_or_else(|| invalid("PDF raster closure lost its node"))?
    }
    fn render_node(
        &self,
        closure: &PdfRasterClosure,
        node: &usvg::Node,
    ) -> Result<String, FormatEncodeError> {
        let placement = &closure.raster;
        let (width, height, transform) = input_grid(closure, node)?;
        let mut input = pixmap(width, height)?;
        let size = |value| {
            u32::try_from(value).map_err(|_| invalid("PDF raster surface exceeds address space"))
        };
        resvg::render_node(node, transform, &mut input.as_mut())
            .ok_or_else(|| invalid("PDF raster closure could not be painted"))?;
        let mut output = if closure.shares_input_grid() {
            input
        } else {
            let mut output = pixmap(size(closure.pixel_width)?, size(closure.pixel_height)?)?;
            let dx = (placement.output_x - placement.input_x) as i64;
            let dy = (placement.output_y - placement.input_y) as i64;
            // Copy the admitted final grid, preserving transparent out-of-input
            // pixels. No resize/resampling or backend-selected dimensions occur.
            for y in 0..i64::from(output.height()) {
                let iy = y + dy;
                if iy < 0 || iy >= i64::from(input.height()) {
                    continue;
                }
                let left = 0.max(-dx);
                let right = i64::from(output.width()).min(i64::from(input.width()) - dx);
                if right <= left {
                    continue;
                }
                let src = ((iy * i64::from(input.width()) + left + dx) * 4) as usize;
                let dst = ((y * i64::from(output.width()) + left) * 4) as usize;
                let len = ((right - left) * 4) as usize;
                output.data_mut()[dst..dst + len].copy_from_slice(&input.data()[src..src + len]);
            }
            output
        };
        super::super::png_encoder::canonicalize_straight_alpha(output.data_mut())?;
        let png = super::super::png_encoder::compress_rgba(
            output.width(),
            output.height(),
            output.data(),
        )?
        .assemble(None)?;
        let mut data = String::new();
        data.try_reserve(base64::encoded_len(png.len(), true).ok_or_else(memory)?)
            .map_err(|_| memory())?;
        base64::engine::general_purpose::STANDARD.encode_string(png, &mut data);
        let matrix = placement
            .parent_to_device
            .inverse()?
            .concat(Matrix::translate(placement.output_x, placement.output_y));
        let [a, b, c, d, e, f] = matrix.0;
        let mut image = String::new();
        image.try_reserve(data.len() + 2048).map_err(|_| memory())?;
        write!(image,"<image width=\"{}\" height=\"{}\" preserveAspectRatio=\"none\" transform=\"matrix({a} {b} {c} {d} {e} {f})\" href=\"data:image/png;base64,{data}\"/>",closure.pixel_width,closure.pixel_height).map_err(|_|memory())?;
        Ok(image)
    }
}
/// Calls `render` with the node whose ID is `id`, searching resource subroots too.
fn with_node<T>(
    group: &usvg::Group,
    id: &str,
    render: &mut impl FnMut(&usvg::Node) -> Result<T, FormatEncodeError>,
) -> Option<Result<T, FormatEncodeError>> {
    for node in group.children() {
        if node.id() == id {
            return Some(render(node));
        }
        if let usvg::Node::Group(group) = node {
            if let Some(result) = with_node(group, id, render) {
                return Some(result);
            }
        }
        let mut result = None;
        node.subroots(|group| {
            if result.is_none() {
                result = with_node(group, id, render);
            }
        });
        if result.is_some() {
            return result;
        }
    }
    None
}
/// The input surface a closure's node is painted into, and the transform
/// `resvg::render_node` receives for it.
fn input_grid(
    closure: &PdfRasterClosure,
    node: &usvg::Node,
) -> Result<(u32, u32, tiny_skia::Transform), FormatEncodeError> {
    let placement = &closure.raster;
    let bbox = node
        .abs_layer_bounding_box()
        .ok_or_else(|| invalid("PDF raster closure lost its bounds"))?;
    let size = |value| {
        u32::try_from(value).map_err(|_| invalid("PDF raster surface exceeds address space"))
    };
    let transform = Matrix::translate(-placement.input_x, -placement.input_y)
        .concat(placement.parent_to_device)
        .concat(Matrix::translate(f64::from(bbox.x()), f64::from(bbox.y())))
        .skia()?;
    Ok((
        size(placement.input_width)?,
        size(placement.input_height)?,
        transform,
    ))
}
fn pixmap(width: u32, height: u32) -> Result<tiny_skia::Pixmap, FormatEncodeError> {
    let size = tiny_skia::IntSize::from_wh(width, height)
        .ok_or_else(|| invalid("empty PDF raster surface"))?;
    let length = (width as usize)
        .checked_mul(height as usize)
        .and_then(|n| n.checked_mul(4))
        .ok_or_else(memory)?;
    let mut pixels = Vec::new();
    pixels.try_reserve_exact(length).map_err(|_| memory())?;
    pixels.resize(length, 0);
    tiny_skia::Pixmap::from_vec(pixels, size).ok_or_else(memory)
}
pub(super) fn parse(xml: &str) -> Result<usvg::Tree, FormatEncodeError> {
    usvg::Tree::from_str(xml, &usvg::Options::default())
        .map_err(|_| invalid("could not parse PDF raster adapter SVG"))
}

// A pattern/mask definition may be consumed at different scales. Give each use
// its own normalized resource occurrence before assigning raster IDs. Definitions
// and filter inputs otherwise remain intact; only occurrence-local IRIs change.
fn occurrence_resources(xml: &str) -> Result<String, FormatEncodeError> {
    let document =
        roxmltree::Document::parse(xml).map_err(|_| invalid("invalid normalized PDF resources"))?;
    let prefix = namespace(&document, "__pdf_resource_")?;
    struct Expansion<'a, 'input> {
        xml: &'input str,
        document: &'a roxmltree::Document<'input>,
        prefix: String,
        next: usize,
        definitions: String,
        stack: Vec<String>,
    }
    impl Expansion<'_, '_> {
        fn references(
            &mut self,
            node: roxmltree::Node<'_, '_>,
            edits: &mut Vec<(std::ops::Range<usize>, String)>,
        ) -> Result<(), FormatEncodeError> {
            for attr in node
                .attributes()
                .filter(|a| matches!(a.name(), "fill" | "stroke" | "mask"))
            {
                let Some(id) = attr
                    .value()
                    .strip_prefix("url(#")
                    .and_then(|v| v.strip_suffix(')'))
                else {
                    continue;
                };
                let Some(resource) = self.document.descendants().find(|n| {
                    n.attribute("id") == Some(id)
                        && matches!(n.tag_name().name(), "mask" | "pattern")
                }) else {
                    continue;
                };
                if self.stack.iter().any(|s| s == id) {
                    return Err(invalid("cyclic normalized PDF resource"));
                }
                self.stack.try_reserve(1).map_err(|_| memory())?;
                self.stack.push(try_owned(id).map_err(|_| memory())?);
                let unique = format!("{}{}", self.prefix, self.next);
                self.next += 1;
                let mut changes = Vec::new();
                changes.try_reserve(1).map_err(|_| memory())?;
                changes.push((
                    resource
                        .attribute_node("id")
                        .ok_or_else(|| invalid("missing SVG resource ID"))?
                        .range(),
                    format!("id=\"{unique}\""),
                ));
                for child in resource.descendants().filter(|n| n.is_element()) {
                    self.references(child, &mut changes)?;
                }
                let range = resource.range();
                for (r, _) in &mut changes {
                    r.start -= range.start;
                    r.end -= range.start;
                }
                let definition = edit(&self.xml[range], changes)?;
                self.definitions
                    .try_reserve(definition.len())
                    .map_err(|_| memory())?;
                self.definitions.push_str(&definition);
                self.stack.pop();
                edits.try_reserve(1).map_err(|_| memory())?;
                edits.push((attr.range(), format!("{}=\"url(#{unique})\"", attr.name())));
            }
            Ok(())
        }
    }
    let mut expansion = Expansion {
        xml,
        document: &document,
        prefix,
        next: 0,
        definitions: String::new(),
        stack: Vec::new(),
    };
    let mut edits = Vec::new();
    for node in document
        .descendants()
        .filter(|n| n.is_element() && !n.ancestors().any(|a| a.has_tag_name("defs")))
    {
        expansion.references(node, &mut edits)?;
    }
    if !expansion.definitions.is_empty() {
        let defs = document
            .root_element()
            .children()
            .find(|n| n.has_tag_name("defs"))
            .ok_or_else(|| invalid("normalized SVG lost resource definitions"))?;
        let end = defs.range().end - 7;
        edits.try_reserve(1).map_err(|_| memory())?;
        edits.push((end..end, expansion.definitions));
    }
    edit(xml, edits)
}

// usvg writes nested SVG images as opaque data URLs (using default precision).
// Inline their normalized trees before identifying closures, with scoped IDs so
// separate asset documents cannot accidentally share local resources.
pub(super) fn expanded_xml(
    tree: &usvg::Tree,
    ordinal: &mut usize,
    prefix: Option<String>,
) -> Result<String, FormatEncodeError> {
    let serialized = super::raster_svg::write(tree, prefix)?;
    let xml = serialized.xml;
    if serialized.images.is_empty() {
        return Ok(xml);
    }
    let document = roxmltree::Document::parse(&xml)
        .map_err(|_| invalid("invalid normalized nested PDF SVG"))?;
    let mut nested = BTreeMap::new();
    // The serializer paired each blob with its normalized source while walking
    // them together. Reuse that existing representation, never serialize a
    // second copy of a child merely to identify its data URL.
    for image in serialized.images {
        if nested.contains_key(&image.encoded) {
            continue;
        }
        *ordinal = ordinal.checked_add(1).ok_or_else(memory)?;
        let prefix = format!("{}{ordinal}_", namespace(&document, "__pdf_nested_")?);
        nested.insert(
            image.encoded,
            expanded_xml(image.tree, ordinal, Some(prefix))?,
        );
    }
    let mut edits = Vec::new();
    for node in document
        .descendants()
        .filter(|n| n.is_element() && n.tag_name().name() == "image")
    {
        let Some(href) = node.attributes().find(|a| a.name() == "href") else {
            continue;
        };
        let Some(data) = href.value().strip_prefix("data:image/svg+xml;base64,") else {
            continue;
        };
        let child = nested
            .get(data.trim())
            .ok_or_else(|| invalid("nested SVG lost its normalized source"))?;
        let mut replacement = String::new();
        replacement
            .try_reserve(child.len() + 256)
            .map_err(|_| memory())?;
        replacement.push_str("<g");
        for attr in node
            .attributes()
            .filter(|a| matches!(a.name(), "id" | "visibility"))
        {
            replacement.push(' ');
            replacement.push_str(&xml[attr.range()]);
        }
        replacement.push('>');
        replacement.push_str(child);
        replacement.push_str("</g>");
        edits.try_reserve(1).map_err(|_| memory())?;
        edits.push((node.range(), replacement));
    }
    edit(&xml, edits)
}
pub(super) fn edit(
    source: &str,
    mut edits: Vec<(std::ops::Range<usize>, String)>,
) -> Result<String, FormatEncodeError> {
    edits.sort_by_key(|(r, _)| (r.start, std::cmp::Reverse(r.end)));
    let capacity = edits
        .iter()
        .try_fold(source.len(), |n, (_, text)| n.checked_add(text.len()))
        .ok_or_else(memory)?;
    let mut result = String::new();
    result.try_reserve(capacity).map_err(|_| memory())?;
    let mut cursor = 0;
    for (range, text) in edits {
        if range.start < cursor {
            continue;
        }
        result.push_str(&source[cursor..range.start]);
        result.push_str(&text);
        cursor = range.end;
    }
    result.push_str(&source[cursor..]);
    Ok(result)
}

/// Select a disjoint namespace in linear work. Every existing ID can occupy
/// at most one candidate; N IDs therefore leave a hole among N+1 candidates.
pub(super) fn namespace(
    document: &roxmltree::Document<'_>,
    base: &str,
) -> Result<String, FormatEncodeError> {
    let count = document
        .descendants()
        .filter(|n| n.attribute("id").is_some())
        .count();
    let mut occupied = Vec::new();
    occupied
        .try_reserve_exact(count + 1)
        .map_err(|_| memory())?;
    occupied.resize(count + 1, false);
    for id in document.descendants().filter_map(|n| n.attribute("id")) {
        if let Some(index) = id
            .strip_prefix(base)
            .and_then(|s| s.split_once('_'))
            .and_then(|(n, _)| n.parse::<usize>().ok())
        {
            if let Some(slot) = occupied.get_mut(index) {
                *slot = true;
            }
        }
    }
    Ok(format!(
        "{base}{}_",
        occupied
            .iter()
            .position(|taken| !taken)
            .ok_or_else(|| invalid("no free SVG ID namespace"))?
    ))
}
