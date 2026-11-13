use std::borrow::Cow;
use std::collections::BTreeMap;
mod css;
mod entities;
mod references;
mod stylesheet;
use std::sync::Arc;

use base64::Engine as _;

use super::{IntrinsicTransaction, ResolutionFailure, ResolveError};
use crate::{CompatibilityRevision, OutputFormat, RenderError, ResourceLimits};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Removal {
    pub(crate) slots: Vec<(u64, u64)>,
    pub(crate) node_ordinal: u64,
    pub(crate) attribute_ordinal: Option<u64>,
    pub(crate) name: String,
    pub(crate) changes_appearance: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Failure {
    pub(crate) path: Vec<(u64, u64)>,
    pub(crate) error: ResolveError,
    pub(crate) placeholder: bool,
    pub(crate) may_lose_semantics: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MediaClaim {
    pub(crate) path: Vec<(u64, u64)>,
    pub(crate) claimed: String,
    pub(crate) validated: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct EffectCause {
    pub(crate) css: Option<(u64, u64)>,
    pub(crate) slots: Vec<(u64, u64)>,
    pub(crate) node_ordinal: u64,
    pub(crate) attribute: Option<(u64, String)>,
    pub(crate) effect_input: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Effect {
    pub(crate) node_ordinal: u64,
    pub(crate) filter_id: String,
    pub(crate) usage_id: String,
    pub(crate) marker: String,
    pub(crate) causes: Vec<EffectCause>,
    pub(crate) unisolatable: bool,
}

#[derive(Debug)]
pub(super) struct SanitizedSvg {
    pub(super) bytes: Vec<u8>,
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) removals: Vec<Removal>,
    pub(super) failures: Vec<Failure>,
    pub(super) media_claims: Vec<MediaClaim>,
    pub(super) effects: Vec<Effect>,
    pub(super) work_units: u64,
    pub(super) working_bytes: u64,
    pub(super) nested_admissions: Vec<NestedAdmission>,
    pub(super) tree: Arc<usvg::Tree>,
}

#[derive(Clone, Debug)]
pub(super) enum NestedAdmission {
    Recovery {
        identity: [u8; 32],
        retained_bytes: u64,
    },
    Occurrence {
        asset_bytes: u64,
    },
    DecodedPixels {
        decoded_pixels: u64,
    },
    Work {
        identity: [u8; 32],
        total_work_units: u64,
    },
}

#[derive(Default)]
struct NestedAccounting {
    admissions: Vec<NestedAdmission>,
    // Recovery depends on remaining recursion depth and element nesting,
    // unlike source identity.
    sanitized_svgs: BTreeMap<([u8; 32], u32, usize), Arc<SanitizedSvg>>,
    charged_validation_work: Vec<[u8; 32]>,
    asset_entries: u64,
    asset_bytes: u64,
    decoded_pixels: u64,
    total_work_units: u64,
}

pub(super) fn looks_like_svg(bytes: &[u8]) -> bool {
    let text = String::from_utf8_lossy(bytes);
    let text = text.trim_start_matches('\u{feff}').trim_start();
    text.starts_with('<')
}

pub(super) fn sanitize(
    bytes: &[u8],
    limits: &ResourceLimits,
    revision: CompatibilityRevision,
    format: OutputFormat,
    shared_rasters: &BTreeMap<[u8; 32], Arc<super::NormalizedNestedRaster>>,
    pending_rasters: &mut BTreeMap<[u8; 32], Arc<super::NormalizedNestedRaster>>,
    transact: &mut dyn FnMut(IntrinsicTransaction) -> Result<(), RenderError>,
) -> Result<SanitizedSvg, ResolutionFailure> {
    let mut nested_accounting = NestedAccounting::default();
    let mut sanitized = sanitize_at(
        bytes,
        limits,
        revision,
        format,
        0,
        MAX_ELEMENT_DEPTH + 1,
        shared_rasters,
        pending_rasters,
        transact,
        &mut nested_accounting,
    )?;
    sanitized.nested_admissions = nested_accounting.admissions;
    Ok(sanitized)
}

#[allow(clippy::too_many_arguments)]
fn sanitize_at(
    bytes: &[u8],
    limits: &ResourceLimits,
    revision: CompatibilityRevision,
    format: OutputFormat,
    depth: u32,
    max_open_elements: usize,
    shared_rasters: &BTreeMap<[u8; 32], Arc<super::NormalizedNestedRaster>>,
    pending_rasters: &mut BTreeMap<[u8; 32], Arc<super::NormalizedNestedRaster>>,
    transact: &mut dyn FnMut(IntrinsicTransaction) -> Result<(), RenderError>,
    nested_accounting: &mut NestedAccounting,
) -> Result<SanitizedSvg, ResolutionFailure> {
    let text = std::str::from_utf8(bytes).map_err(|_| ResolveError::SvgDecodeFailed)?;
    let text = entities::expand(text, limits.max_asset_bytes())?;
    let SvgSource {
        text,
        mut removals,
        source_nodes,
    } = remove_dtd_and_entities(&text, max_open_elements)?;
    let mut failures = Vec::new();
    let mut media_claims = Vec::new();
    let document = roxmltree::Document::parse(&text).map_err(|_| ResolveError::SvgDecodeFailed)?;
    for node in document.descendants().filter(|node| node.pi().is_some()) {
        let Some(instruction) = node.pi() else {
            continue;
        };
        if instruction.target != "xml" {
            removals
                .try_reserve(1)
                .map_err(|_| ResolutionFailure::Memory)?;
            removals.push(Removal {
                slots: Vec::new(),
                node_ordinal: source_node_ordinal(
                    &document,
                    &source_nodes,
                    node.parent()
                        .filter(|n| n.is_element())
                        .unwrap_or(document.root_element()),
                ),
                attribute_ordinal: None,
                name: super::try_owned(instruction.target)?,
                changes_appearance: true,
            });
        }
    }
    let root = document.root_element();
    if root.tag_name().name() != "svg"
        || root.tag_name().namespace() != Some("http://www.w3.org/2000/svg")
    {
        return Err(ResolveError::SvgDecodeFailed.into());
    }
    let plan = references::Plan::new(&document)?;
    let css = css::Sheet::new(&document, &source_nodes, &plan, format)?;
    let nodes = if source_nodes.is_empty() {
        document
            .descendants()
            .filter(|node| node.is_element() || node.is_text())
            .count()
    } else {
        source_nodes.len()
    };
    if nodes > usize::try_from(limits.max_nested_svg_nodes()).unwrap_or(usize::MAX) {
        return Err(ResolveError::SvgComplexity {
            attempted: u64::try_from(nodes).unwrap_or(u64::MAX),
        }
        .into());
    }
    let attributes = document
        .descendants()
        .filter(|node| node.is_element())
        .map(|node| node.attributes().len())
        .sum::<usize>()
        .saturating_add(removals.len())
        .saturating_add(local_removal_count(&document, &plan));
    let tokens = nodes.saturating_add(attributes);
    // Each local nested reference, accepted or rejected: every `href` and
    // every `url(` in an attribute value or a stylesheet.
    let local_references = document
        .descendants()
        .map(|node| {
            if node.is_text() {
                return if node
                    .parent()
                    .is_some_and(|parent| parent.has_tag_name("style"))
                {
                    url_count(node.text().unwrap_or_default())
                } else {
                    0
                };
            }
            node.attributes()
                .map(|attribute| {
                    usize::from(matches!(attribute.name(), "href" | "xlink:href"))
                        .saturating_add(url_count(attribute.value()))
                })
                .fold(0_usize, usize::saturating_add)
        })
        .fold(0_usize, usize::saturating_add);
    let semantic_records = u64::try_from(tokens)
        .unwrap_or(u64::MAX)
        .saturating_add(u64::try_from(nodes).unwrap_or(u64::MAX))
        .saturating_add(u64::try_from(attributes).unwrap_or(u64::MAX))
        .saturating_add(u64::try_from(local_references).unwrap_or(u64::MAX));
    let source_len = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
    let work_units = source_len
        .saturating_add(semantic_records.saturating_mul(16))
        .saturating_add(css.work_units(&document));
    if work_units > limits.max_asset_work_units() {
        if depth != 0 {
            transact(IntrinsicTransaction::PreflightNested {
                asset_entries: nested_accounting.asset_entries,
                asset_bytes: nested_accounting.asset_bytes,
                decoded_pixels: nested_accounting.decoded_pixels,
                total_work_units: nested_accounting
                    .total_work_units
                    .saturating_add(nested_accounting.asset_bytes)
                    .saturating_add(work_units),
            })
            .map_err(ResolutionFailure::Transaction)?;
        }
        let identity = super::semantic_identity(revision, super::MediaKind::Svg, bytes);
        transact(IntrinsicTransaction::RejectLocalWork {
            event: "derived_sanitized_svg",
            identity,
            total_work_units: if depth == 0 {
                work_units.saturating_sub(source_len)
            } else {
                work_units
            },
        })
        .map_err(ResolutionFailure::Transaction)?;
        return Err(ResolveError::AssetWork {
            attempted: work_units,
        }
        .into());
    }
    let working_bytes = 512_u64
        .saturating_add(u64::try_from(nodes).unwrap_or(u64::MAX).saturating_mul(128))
        .saturating_add(
            u64::try_from(
                attributes
                    .saturating_add(tokens)
                    .saturating_add(local_references),
            )
            .unwrap_or(u64::MAX)
            .saturating_mul(64),
        );
    let mut serialized = String::new();
    let css = css.resolve(&document, &plan)?;
    let mut effects = discover_effects(&document, &source_nodes, &css);
    let mut node_ordinal = 0;
    serialize_node(
        root,
        true,
        &mut node_ordinal,
        &source_nodes,
        &css,
        &plan,
        &mut serialized,
        &mut removals,
        &mut failures,
        &mut media_claims,
        &mut effects,
        limits,
        revision,
        format,
        depth,
        max_open_elements,
        shared_rasters,
        pending_rasters,
        transact,
        nested_accounting,
    )?;
    // Backstop: the viewer library recurses through references, so a cycle
    // the plan did not break would overflow the stack there. Refuse it.
    let output = roxmltree::Document::parse_with_options(
        &serialized,
        roxmltree::ParsingOptions {
            allow_dtd: true,
            ..roxmltree::ParsingOptions::default()
        },
    )
    .map_err(|_| ResolveError::SvgDecodeFailed)?;
    if references::Plan::residual_cycle(&output)? {
        return Err(ResolveError::SvgDecodeFailed.into());
    }
    drop(output);
    let options = viewer_options()?;
    super::svg_coordinates::source(&serialized, limits.max_coordinate_magnitude())?;
    let tree =
        usvg::Tree::from_str(&serialized, &options).map_err(|_| ResolveError::SvgDecodeFailed)?;
    super::svg_coordinates::tree(&tree, limits.max_coordinate_magnitude())?;
    let size = tree.size().to_int_size();
    let width = size.width();
    let height = size.height();
    Ok(SanitizedSvg {
        bytes: serialized.into_bytes(),
        width,
        height,
        removals,
        failures,
        media_claims,
        effects,
        work_units,
        working_bytes,
        nested_admissions: Vec::new(),
        tree: Arc::new(tree),
    })
}

fn local_removal_count(document: &roxmltree::Document<'_>, plan: &references::Plan) -> usize {
    document
        .descendants()
        .filter(|node| node.is_element())
        .filter(|node| {
            !node
                .ancestors()
                .skip(1)
                .filter(|ancestor| ancestor.is_element())
                .any(|ancestor| !is_allowed_svg_node(&ancestor))
        })
        .map(|node| {
            let name = node.tag_name().name();
            if !is_allowed_svg_node(&node) || plan.stylesheet_changed(node) {
                return 1;
            }
            node.attributes()
                .filter(|attribute| {
                    unsafe_attribute(
                        name,
                        attribute.name(),
                        attribute.namespace(),
                        attribute.value(),
                    )
                })
                .count()
        })
        .sum()
}

fn is_allowed_svg_node(node: &roxmltree::Node<'_, '_>) -> bool {
    node.tag_name().namespace() == Some("http://www.w3.org/2000/svg")
        && allowed_element(node.tag_name().name())
        && (node.tag_name().name() != "style" || safe_static_style_node(*node))
}

/// Whether `namespace` holds only the metadata an authoring tool records
/// about its document (Inkscape and Sodipodi editor state, RDF licensing,
/// Illustrator and Sketch data). A browser neither renders nor acts on it,
/// so its removal is silent rather than an unsafe-content diagnostic.
fn is_inert_editor_namespace(namespace: Option<&str>) -> bool {
    matches!(
        namespace,
        Some(
            "http://www.inkscape.org/namespaces/inkscape"
                | "http://sodipodi.sourceforge.net/DTD/sodipodi-0.dtd"
                | "http://inkscape.sourceforge.net/DTD/sodipodi-0.dtd"
                | "http://www.w3.org/1999/02/22-rdf-syntax-ns#"
                | "http://creativecommons.org/ns#"
                | "http://web.resource.org/cc/"
                | "http://purl.org/dc/elements/1.1/"
                | "http://ns.adobe.com/AdobeIllustrator/10.0/"
                | "http://ns.adobe.com/Extensibility/1.0/"
                | "http://ns.adobe.com/Graphs/1.0/"
                | "http://ns.adobe.com/SaveForWeb/1.0/"
                | "http://ns.adobe.com/Variables/1.0/"
                | "http://www.bohemiancoding.com/sketch/ns"
        )
    )
}

/// Whether removing the disallowed `node` drops only inert editor metadata:
/// it and every element below it are in an inert editor namespace.
fn is_inert_editor_node(node: roxmltree::Node<'_, '_>) -> bool {
    node.descendants()
        .filter(roxmltree::Node::is_element)
        .all(|element| is_inert_editor_namespace(element.tag_name().namespace()))
}

fn safe_static_style_node(node: roxmltree::Node<'_, '_>) -> bool {
    let mut text = None;
    for child in node.children() {
        if !child.is_text() || text.is_some() {
            return false;
        }
        text = child.text();
    }
    text.is_none_or(safe_static_stylesheet)
}

fn safe_static_stylesheet(stylesheet: &str) -> bool {
    rewrite_stylesheet(stylesheet, "x").is_some()
}

/// The admitted static form of a stylesheet, and whether it differs from the
/// source: its at-rules removed and its non-fragment URLs pointed at the
/// `missing` fragment. `None` refuses the whole sheet.
fn rewrite_stylesheet<'s>(stylesheet: &'s str, missing: &str) -> Option<(Cow<'s, str>, bool)> {
    if stylesheet.contains('\\') || stylesheet.contains("/*") || stylesheet.contains("*/") {
        return None;
    }
    let (sheet, stripped) = stylesheet::without_at_rules(stylesheet)?;
    let (sheet, rewritten) = match rewrite_nonlocal_urls(&sheet, missing).ok()? {
        Some(rewritten) => (Cow::Owned(rewritten), true),
        None => (sheet, false),
    };
    if sheet.contains('@')
        || unsafe_attribute("style", "style", None, &sheet)
        || !stylesheet::Rules::new(&sheet).all(|rule| rule.is_ok_and(|rule| rule.admitted()))
    {
        return None;
    }
    Some((sheet, stripped || rewritten))
}

/// Each terminated `url(...)` token of `value` as its byte range and its
/// unquoted target, found as admission finds them.
fn url_tokens(value: &str) -> impl Iterator<Item = (usize, usize, &str)> {
    let mut cursor = 0;
    std::iter::from_fn(move || {
        let offset = value.as_bytes()[cursor..]
            .windows(4)
            .position(|window| window.eq_ignore_ascii_case(b"url("))?;
        let start = cursor + offset;
        let end = start + 4 + value[start + 4..].find(')')? + 1;
        cursor = end;
        Some((
            start,
            end,
            value[start + 4..end - 1].trim().trim_matches(['\'', '"']),
        ))
    })
}

fn url_count(value: &str) -> usize {
    value
        .as_bytes()
        .windows(4)
        .filter(|window| window.eq_ignore_ascii_case(b"url("))
        .count()
}

/// `value` with each non-fragment `url(...)` replaced by a reference to the
/// `missing` fragment; `None` when it has none. An unterminated `url(` stays
/// for admission to refuse.
fn rewrite_nonlocal_urls(value: &str, missing: &str) -> Result<Option<String>, ResolutionFailure> {
    let mut rewritten: Option<String> = None;
    let mut copied = 0;
    for (start, end, target) in url_tokens(value) {
        if target.starts_with('#') {
            continue;
        }
        let output = match &mut rewritten {
            Some(output) => output,
            None => {
                let mut output = String::new();
                output
                    .try_reserve(value.len().saturating_add(missing.len()))
                    .map_err(|_| ResolutionFailure::Memory)?;
                rewritten.insert(output)
            }
        };
        output
            .try_reserve(start - copied + missing.len() + 6)
            .map_err(|_| ResolutionFailure::Memory)?;
        output.push_str(&value[copied..start]);
        output.push_str("url(#");
        output.push_str(missing);
        output.push(')');
        copied = end;
    }
    if let Some(output) = &mut rewritten {
        output
            .try_reserve(value.len() - copied)
            .map_err(|_| ResolutionFailure::Memory)?;
        output.push_str(&value[copied..]);
    }
    Ok(rewritten)
}

fn allowed_static_style_property(property: &str) -> bool {
    matches!(
        property.to_ascii_lowercase().as_str(),
        "alignment-baseline"
            | "baseline-shift"
            | "clip-path"
            | "clip-rule"
            | "color"
            | "color-interpolation"
            | "color-interpolation-filters"
            | "direction"
            | "display"
            | "dominant-baseline"
            | "fill"
            | "fill-opacity"
            | "fill-rule"
            | "filter"
            | "flood-color"
            | "flood-opacity"
            | "font"
            | "font-family"
            | "font-size"
            | "font-size-adjust"
            | "font-stretch"
            | "font-style"
            | "font-variant"
            | "font-weight"
            | "image-rendering"
            | "isolation"
            | "letter-spacing"
            | "lighting-color"
            | "marker"
            | "marker-end"
            | "marker-mid"
            | "marker-start"
            | "mask"
            | "mix-blend-mode"
            | "opacity"
            | "overflow"
            | "paint-order"
            | "shape-rendering"
            | "stop-color"
            | "stop-opacity"
            | "stroke"
            | "stroke-dasharray"
            | "stroke-dashoffset"
            | "stroke-linecap"
            | "stroke-linejoin"
            | "stroke-miterlimit"
            | "stroke-opacity"
            | "stroke-width"
            | "text-anchor"
            | "text-decoration"
            | "text-rendering"
            | "unicode-bidi"
            | "vector-effect"
            | "visibility"
            | "word-spacing"
            | "writing-mode"
    )
}

/// Deepest admitted element nesting below the outermost nested root, counted
/// on through nested data SVGs: a nested image's root sits one level below its
/// image element. The XML parser, the sanitizer's walks and the viewer's
/// (whose own 1,024-level ceiling skips text content and restarts in each
/// nested image) all recurse per level, so this ceiling is what bounds their
/// combined stack. It keeps the deepest admitted chain, nested text included,
/// well inside a spawned thread's default 2 MiB stack. A linear scan checks it
/// before any of them runs.
const MAX_ELEMENT_DEPTH: usize = 256;

struct SvgSource {
    text: String,
    removals: Vec<Removal>,
    source_nodes: Vec<SourceNode>,
}
struct SourceNode {
    offset: usize,
    end: usize,
    text: bool,
    ordinal: u64,
}

/// The DTD is dropped. Entities left after internal expansion (external ones,
/// and undeclared ones the document may declare where the renderer does not
/// look) are removed. They become one temporary space solely in a location document,
/// so even an entity-only text node keeps its source ordinal. A second, clean
/// document drives semantic operations; no marker is ever serialized or painted.
/// More than `max_open_elements` elements open at once, the root included,
/// reject the document before anything parses it recursively.
fn remove_dtd_and_entities(
    text: &str,
    max_open_elements: usize,
) -> Result<SvgSource, ResolveError> {
    let mut located = String::new();
    located
        .try_reserve_exact(text.len())
        .map_err(|_| ResolveError::Memory)?;
    let mut removals = Vec::new();
    let mut entities = Vec::new();
    let (mut cursor, mut in_tag, mut quote) = (0, false, None);
    // Open elements, the root included, and the last character copied.
    let (mut open_elements, mut previous) = (0_usize, '\0');
    while cursor < text.len() {
        let rest = &text[cursor..];
        if !in_tag {
            let opaque = [("<!--", "-->"), ("<![CDATA[", "]]>"), ("<?", "?>")]
                .into_iter()
                .find(|(open, _)| rest.starts_with(open));
            if let Some((open, close)) = opaque {
                let end = open.len()
                    + rest[open.len()..]
                        .find(close)
                        .ok_or(ResolveError::SvgDecodeFailed)?
                    + close.len();
                located.push_str(&rest[..end]);
                cursor += end;
                continue;
            }
            if rest.starts_with("<!DOCTYPE") {
                let end = doctype_end(rest).ok_or(ResolveError::SvgDecodeFailed)?;
                removals.try_reserve(1).map_err(|_| ResolveError::Memory)?;
                removals.push(Removal {
                    slots: Vec::new(),
                    node_ordinal: 0,
                    attribute_ordinal: None,
                    name: super::try_owned("DOCTYPE")?,
                    changes_appearance: false,
                });
                cursor += end;
                continue;
            }
        }
        if rest.starts_with('&') && (!in_tag || quote.is_some()) {
            let end = rest.as_bytes()[1..]
                .iter()
                .position(|b| {
                    b.is_ascii_whitespace() || matches!(b, b';' | b'<' | b'>' | b'&' | b'"' | b'\'')
                })
                .map(|index| index + 1)
                .filter(|index| rest.as_bytes()[*index] == b';');
            if let Some(end) = end {
                let entity = &rest[1..end];
                let bounded_name = !entity.is_empty()
                    && !entity.bytes().any(|b| {
                        b.is_ascii_whitespace() || matches!(b, b'<' | b'>' | b'&' | b'"' | b'\'')
                    });
                if bounded_name
                    && !matches!(entity, "amp" | "lt" | "gt" | "quot" | "apos")
                    && !entity.strip_prefix('#').is_some_and(valid_numeric_entity)
                {
                    entities.try_reserve(1).map_err(|_| ResolveError::Memory)?;
                    removals.try_reserve(1).map_err(|_| ResolveError::Memory)?;
                    entities.push((removals.len(), located.len()));
                    removals.push(Removal {
                        slots: Vec::new(),
                        node_ordinal: 0,
                        attribute_ordinal: None,
                        name: super::try_owned(entity)?,
                        changes_appearance: true,
                    });
                    located.push(' ');
                    cursor += end + 1;
                    continue;
                }
            }
        }
        let ch = rest.chars().next().ok_or(ResolveError::SvgDecodeFailed)?;
        if in_tag {
            if quote == Some(ch) {
                quote = None;
            } else if quote.is_none() && matches!(ch, '\'' | '"') {
                quote = Some(ch);
            } else if quote.is_none() && ch == '>' {
                in_tag = false;
                if previous == '/' {
                    open_elements = open_elements.saturating_sub(1);
                }
            }
        } else if ch == '<' {
            in_tag = true;
            if rest[1..].starts_with('/') {
                open_elements = open_elements.saturating_sub(1);
            } else {
                open_elements += 1;
                if open_elements > max_open_elements {
                    return Err(ResolveError::SvgDecodeFailed);
                }
            }
        }
        previous = ch;
        located.push(ch);
        cursor += ch.len_utf8();
    }
    if entities.is_empty() {
        return Ok(SvgSource {
            text: located,
            removals,
            source_nodes: Vec::new(),
        });
    }
    let mut source_nodes = Vec::new();
    {
        let document =
            roxmltree::Document::parse(&located).map_err(|_| ResolveError::SvgDecodeFailed)?;
        source_nodes
            .try_reserve_exact(
                document
                    .descendants()
                    .filter(|n| n.is_element() || n.is_text())
                    .count(),
            )
            .map_err(|_| ResolveError::Memory)?;
        for (ordinal, node) in document
            .descendants()
            .filter(|n| n.is_element() || n.is_text())
            .enumerate()
        {
            let offset = node.range().start;
            let removed = entities.partition_point(|(_, position)| *position < offset);
            let mut own_range = node.range();
            if node.is_element() {
                own_range.end = node
                    .first_child()
                    .map_or(own_range.end, |child| child.range().start);
            } else {
                // roxmltree coalesces adjacent CDATA/text but retains only the
                // first token's range. Its complete lexical text span ends at
                // the next sibling, or the end of its parent when it is last.
                own_range.end = node
                    .next_sibling()
                    .map(|next| next.range().start)
                    .or_else(|| node.parent().map(|parent| parent.range().end))
                    .unwrap_or(own_range.end);
            }
            let end =
                own_range.end - entities.partition_point(|(_, position)| *position < own_range.end);
            source_nodes.push(SourceNode {
                offset: offset - removed,
                end,
                text: node.is_text(),
                ordinal: ordinal as u64,
            });
            let visual = !node
                .ancestors()
                .any(|ancestor| ancestor.has_tag_name("script"));
            locate_entities(
                own_range,
                ordinal as u64,
                None,
                visual,
                &entities,
                &mut removals,
            )?;
            for (attribute_ordinal, attribute) in node.attributes().enumerate() {
                let visual = !attribute
                    .name()
                    .as_bytes()
                    .get(..2)
                    .is_some_and(|prefix| prefix.eq_ignore_ascii_case(b"on"))
                    && !(attribute.name() == "href" && node.has_tag_name("a"));
                locate_entities(
                    attribute.range_value(),
                    ordinal as u64,
                    Some((attribute_ordinal as u64, attribute.name())),
                    visual,
                    &entities,
                    &mut removals,
                )?;
            }
        }
    }
    source_nodes.sort_unstable_by_key(|n| (n.offset, n.text, n.ordinal));
    let mut clean = String::new();
    clean
        .try_reserve_exact(located.len())
        .map_err(|_| ResolveError::Memory)?;
    let mut start = 0;
    for (_, offset) in entities {
        clean.push_str(&located[start..offset]);
        start = offset + 1;
    }
    clean.push_str(&located[start..]);
    Ok(SvgSource {
        text: clean,
        removals,
        source_nodes,
    })
}

fn locate_entities(
    range: std::ops::Range<usize>,
    node: u64,
    attribute: Option<(u64, &str)>,
    visual: bool,
    entities: &[(usize, usize)],
    removals: &mut [Removal],
) -> Result<(), ResolveError> {
    let start = entities.partition_point(|(_, position)| *position < range.start);
    for (index, _) in entities[start..]
        .iter()
        .take_while(|(_, position)| *position < range.end)
    {
        let removal = removals
            .get_mut(*index)
            .ok_or(ResolveError::SvgDecodeFailed)?;
        removal.node_ordinal = node;
        removal.attribute_ordinal = attribute.map(|(ordinal, _)| ordinal);
        removal.changes_appearance = visual;
        if let Some((_, name)) = attribute {
            removal.name = super::try_owned(name)?;
        }
    }
    Ok(())
}

fn doctype_end(doctype: &str) -> Option<usize> {
    let mut quote = None;
    let mut subset_depth = 0_u32;
    for (index, character) in doctype.char_indices() {
        if let Some(delimiter) = quote {
            if character == delimiter {
                quote = None;
            }
            continue;
        }
        match character {
            '\'' | '"' => quote = Some(character),
            '[' => subset_depth = subset_depth.saturating_add(1),
            ']' => subset_depth = subset_depth.saturating_sub(1),
            '>' if subset_depth == 0 => return Some(index + 1),
            _ => {}
        }
    }
    None
}

fn valid_numeric_entity(number: &str) -> bool {
    number.strip_prefix(['x', 'X']).map_or_else(
        || !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit()),
        |hex| !hex.is_empty() && hex.bytes().all(|byte| byte.is_ascii_hexdigit()),
    )
}

#[allow(clippy::too_many_arguments)]
fn serialize_node(
    node: roxmltree::Node<'_, '_>,
    is_root: bool,
    node_ordinal: &mut u64,
    source_nodes: &[SourceNode],
    css: &css::Resolved,
    plan: &references::Plan,
    output: &mut String,
    removals: &mut Vec<Removal>,
    failures: &mut Vec<Failure>,
    media_claims: &mut Vec<MediaClaim>,
    effects: &mut Vec<Effect>,
    limits: &ResourceLimits,
    revision: CompatibilityRevision,
    format: OutputFormat,
    depth: u32,
    max_open_elements: usize,
    shared_rasters: &BTreeMap<[u8; 32], Arc<super::NormalizedNestedRaster>>,
    pending_rasters: &mut BTreeMap<[u8; 32], Arc<super::NormalizedNestedRaster>>,
    transact: &mut dyn FnMut(IntrinsicTransaction) -> Result<(), RenderError>,
    nested_accounting: &mut NestedAccounting,
) -> Result<(), ResolutionFailure> {
    let ordinal = if source_nodes.is_empty() {
        *node_ordinal
    } else {
        source_node_ordinal(node.document(), source_nodes, node)
    };
    *node_ordinal = ordinal.saturating_add(1);
    if node.is_text() {
        reserve_escaped(output, node.text().unwrap_or_default(), 0)?;
        escape_text(output, node.text().unwrap_or_default());
        return Ok(());
    }
    if !node.is_element() {
        return Ok(());
    }
    let name = node.tag_name().name();
    if !is_allowed_svg_node(&node) {
        if !is_inert_editor_node(node) {
            removals
                .try_reserve(1)
                .map_err(|_| ResolutionFailure::Memory)?;
            removals.push(Removal {
                slots: Vec::new(),
                node_ordinal: ordinal,
                attribute_ordinal: None,
                name: super::try_owned(name)?,
                changes_appearance: name != "script",
            });
        }
        count_descendants(node, node_ordinal);
        return Ok(());
    }
    if name == "style" && plan.stylesheet_changed(node) {
        // At-rules or non-fragment URLs were removed; every other rule stays.
        removals
            .try_reserve(1)
            .map_err(|_| ResolutionFailure::Memory)?;
        removals.push(Removal {
            slots: Vec::new(),
            node_ordinal: ordinal,
            attribute_ordinal: None,
            name: super::try_owned(name)?,
            changes_appearance: true,
        });
    }

    output
        .try_reserve(name.len().saturating_add(128))
        .map_err(|_| ResolutionFailure::Memory)?;
    output.push('<');
    output.push_str(name);
    if is_root {
        output.push_str(" xmlns=\"http://www.w3.org/2000/svg\"");
    }
    if format == OutputFormat::Pdf
        && (filter_reference(node).is_some()
            || css.get(node).is_some_and(|(_, filter)| filter.is_some()))
        && node.attribute("id").is_none()
    {
        output.push_str(" id=\"__excalidraw_effect_");
        output.push_str(&ordinal.to_string());
        output.push('"');
    }
    if format == OutputFormat::Pdf && name.starts_with("fe") && node.attribute("result").is_none() {
        output.push_str(" result=\"__excalidraw_filter_result_");
        output.push_str(&ordinal.to_string());
        output.push('"');
    }
    // SVG 2 href takes precedence over its SVG 1.1 xlink alias, regardless of
    // attribute order. Only the effective reference can be resolved/recovered.
    let effective_href = node
        .attributes()
        .position(|a| a.name() == "href" && a.namespace().is_none())
        .or_else(|| {
            node.attributes().position(|a| {
                a.name() == "href" && a.namespace() == Some("http://www.w3.org/1999/xlink")
            })
        });
    for (attribute_ordinal, attribute) in node.attributes().enumerate() {
        let attribute_name = attribute.name();
        let rewrite = plan.attribute(node, attribute_ordinal);
        // A dropped attribute closed a reference cycle.
        let Some(value) = plan.value(node, attribute_ordinal, attribute.value()) else {
            continue;
        };
        if attribute_name == "href"
            && matches!(
                attribute.namespace(),
                None | Some("http://www.w3.org/1999/xlink")
            )
            && Some(attribute_ordinal) != effective_href
        {
            continue;
        }
        if attribute_name == "filter" && filter_reference_is_noop(node, value) {
            continue;
        }
        if unsafe_attribute(name, attribute_name, attribute.namespace(), value) {
            if name != "a"
                && matches!(attribute_name, "href" | "xlink:href")
                && matches!(
                    attribute.namespace(),
                    None | Some("http://www.w3.org/1999/xlink")
                )
            {
                let replacement = if name == "image" {
                    nested_limit_placeholder(node, limits, revision, ordinal, transact)?
                } else {
                    None
                };
                if let Some(url) = &replacement {
                    remember_recovery(url, ordinal, revision, nested_accounting)?;
                }
                failures
                    .try_reserve(1)
                    .map_err(|_| ResolutionFailure::Memory)?;
                let mut path = Vec::new();
                path.try_reserve_exact(1)
                    .map_err(|_| ResolutionFailure::Memory)?;
                path.push((ordinal, attribute_ordinal as u64));
                failures.push(Failure {
                    path,
                    error: ResolveError::Forbidden,
                    placeholder: replacement.is_some(),
                    may_lose_semantics: true,
                });
                if let Some(url) = replacement {
                    write_attribute(output, "href", &url)?;
                }
                continue;
            }
            // Editor metadata such as `sodipodi:docname` or
            // `inkscape:export-xdpi` is dropped silently; an event-handler
            // spelling is still reported whatever its namespace.
            if is_inert_editor_namespace(attribute.namespace())
                && !attribute_name.to_ascii_lowercase().starts_with("on")
            {
                continue;
            }
            removals
                .try_reserve(1)
                .map_err(|_| ResolutionFailure::Memory)?;
            removals.push(Removal {
                slots: Vec::new(),
                node_ordinal: ordinal,
                attribute_ordinal: Some(u64::try_from(attribute_ordinal).unwrap_or(u64::MAX)),
                name: super::try_owned(attribute_name)?,
                changes_appearance: !attribute_name.to_ascii_lowercase().starts_with("on")
                    && (!matches!(attribute_name, "href" | "xlink:href") || name != "a"),
            });
            continue;
        }
        if rewrite.is_some_and(|rewrite| rewrite.external) {
            // Its non-fragment URLs now name a missing fragment.
            removals
                .try_reserve(1)
                .map_err(|_| ResolutionFailure::Memory)?;
            removals.push(Removal {
                slots: Vec::new(),
                node_ordinal: ordinal,
                attribute_ordinal: Some(u64::try_from(attribute_ordinal).unwrap_or(u64::MAX)),
                name: super::try_owned(attribute_name)?,
                changes_appearance: true,
            });
        }
        let nested_value = if matches!(attribute_name, "href" | "xlink:href") {
            match sanitize_nested_data_image(
                value,
                limits,
                revision,
                format,
                depth,
                // The nested root opens below every element open at its slot.
                max_open_elements
                    .saturating_sub(node.ancestors().filter(roxmltree::Node::is_element).count()),
                ordinal,
                u64::try_from(attribute_ordinal).unwrap_or(u64::MAX),
                shared_rasters,
                pending_rasters,
                removals,
                failures,
                media_claims,
                effects,
                transact,
                nested_accounting,
            ) {
                Ok(value) => value,
                Err(ResolutionFailure::Asset(error)) => {
                    let resource_limit = matches!(
                        error,
                        ResolveError::AssetBytes { .. }
                            | ResolveError::AssetWork { .. }
                            | ResolveError::Dimensions { .. }
                            | ResolveError::Pixels { .. }
                            | ResolveError::SvgComplexity { .. }
                            | ResolveError::SvgDepth { .. }
                            | ResolveError::Coordinate { .. }
                    );
                    let replacement = if resource_limit && name == "image" {
                        let replacement =
                            nested_limit_placeholder(node, limits, revision, ordinal, transact)?;
                        if let Some(url) = &replacement {
                            remember_recovery(url, ordinal, revision, nested_accounting)?;
                        }
                        replacement
                    } else {
                        None
                    };
                    failures.push(Failure {
                        path: vec![(
                            ordinal,
                            u64::try_from(attribute_ordinal).unwrap_or(u64::MAX),
                        )],
                        error,
                        placeholder: replacement.is_some(),
                        may_lose_semantics: resource_limit
                            && rejected_payload_may_have_semantics(value),
                    });
                    if replacement.is_none() {
                        continue;
                    }
                    replacement
                }
                Err(ResolutionFailure::Transaction(error)) => {
                    return Err(ResolutionFailure::Transaction(error));
                }
                Err(ResolutionFailure::Memory) => return Err(ResolutionFailure::Memory),
            }
        } else if attribute_name == "style" {
            style_without_noop_filter(node, value)
        } else {
            None
        };
        if css.enabled && attribute_name == "style" {
            continue;
        }
        let value = nested_value.as_deref().unwrap_or(value);
        write_attribute(
            output,
            if attribute.namespace() == Some("http://www.w3.org/1999/xlink") {
                "href"
            } else {
                attribute_name
            },
            value,
        )?;
    }
    for (added, value) in plan.added(node) {
        if css.enabled && *added == "style" {
            // Lowered with the rest of its cascade below.
            continue;
        }
        write_attribute(output, added, value)?;
    }
    if let Some((style, _)) = css.get(node) {
        if !style.is_empty() {
            write_attribute(output, "style", style)?;
        }
    }
    if css.enabled && name == "style" {
        output
            .try_reserve(2)
            .map_err(|_| ResolutionFailure::Memory)?;
        output.push_str("/>");
        count_descendants(node, node_ordinal);
        return Ok(());
    }
    output
        .try_reserve(1)
        .map_err(|_| ResolutionFailure::Memory)?;
    output.push('>');
    if plan.needs_stop(node) {
        // No stop anywhere in its chain: it paints nothing, not its fallback.
        output
            .try_reserve(32)
            .map_err(|_| ResolutionFailure::Memory)?;
        output.push_str(r#"<stop stop-opacity="0"/>"#);
    }
    if let Some(sheet) = plan.stylesheet(node).filter(|_| name == "style") {
        reserve_escaped(output, sheet, 0)?;
        escape_text(output, sheet);
        count_descendants(node, node_ordinal);
    }
    for child in node
        .children()
        .filter(|_| name != "style" || plan.stylesheet(node).is_none())
    {
        if child.is_element() || child.is_text() {
            serialize_node(
                child,
                false,
                node_ordinal,
                source_nodes,
                css,
                plan,
                output,
                removals,
                failures,
                media_claims,
                effects,
                limits,
                revision,
                format,
                depth,
                max_open_elements,
                shared_rasters,
                pending_rasters,
                transact,
                nested_accounting,
            )?;
        }
    }
    output
        .try_reserve(name.len().saturating_add(3))
        .map_err(|_| ResolutionFailure::Memory)?;
    output.push_str("</");
    output.push_str(name);
    output.push('>');
    Ok(())
}

fn reserve_escaped(
    output: &mut String,
    value: &str,
    overhead: usize,
) -> Result<(), ResolutionFailure> {
    let size = value
        .len()
        .checked_mul(6)
        .and_then(|n| n.checked_add(overhead))
        .ok_or(ResolutionFailure::Memory)?;
    output
        .try_reserve(size)
        .map_err(|_| ResolutionFailure::Memory)
}
fn write_attribute(output: &mut String, name: &str, value: &str) -> Result<(), ResolutionFailure> {
    reserve_escaped(output, value, name.len().saturating_add(4))?;
    output.push(' ');
    output.push_str(name);
    output.push_str("=\"");
    escape_attribute(output, value);
    output.push('"');
    Ok(())
}

fn remember_recovery(
    url: &str,
    ordinal: u64,
    revision: CompatibilityRevision,
    accounting: &mut NestedAccounting,
) -> Result<(), ResolutionFailure> {
    accounting
        .admissions
        .try_reserve(1)
        .map_err(|_| ResolutionFailure::Memory)?;
    accounting.admissions.push(NestedAdmission::Recovery {
        identity: super::semantic_identity(revision, super::MediaKind::Svg, &ordinal.to_le_bytes()),
        retained_bytes: 256 + (url.len() as u64).div_ceil(64) * 64,
    });
    Ok(())
}

/// Inspect only a fixed prefix: a rejected payload must not be fully decoded
/// merely to classify its diagnostic. Raster signatures establish visual-only
/// content; SVG and unidentified content conservatively retain semantic risk.
/// The untrusted media-type claim never overrides the payload signature.
pub(super) fn rejected_payload_may_have_semantics(value: &str) -> bool {
    let Some(DataUrl { base64, body, .. }) = value.strip_prefix("data:").and_then(DataUrl::parse)
    else {
        return true;
    };
    let mut bytes = [0; 48];
    let len = if base64 {
        let prefix = &body.as_bytes()[..body.len().min(64)];
        let Ok(len) = base64::engine::general_purpose::STANDARD.decode_slice(prefix, &mut bytes)
        else {
            return true;
        };
        len
    } else {
        let mut len = 0;
        for byte in PercentDecoded(body.as_bytes()).take(bytes.len()) {
            bytes[len] = byte;
            len += 1;
        }
        len
    };
    super::sniff_media_kind(&bytes[..len]).is_err()
}

/// A `data:` URL body as the reference's URL parser and data URL processor
/// read it: the header up to the first comma, a final `;base64` parameter
/// selecting base64, otherwise percent-encoded octets; the fragment is not
/// part of the body.
struct DataUrl<'a> {
    claimed_media_type: &'a str,
    base64: bool,
    body: &'a str,
}

impl<'a> DataUrl<'a> {
    fn parse(data: &'a str) -> Option<Self> {
        let (header, body) = data.split_once(',')?;
        let body = body.split_once('#').map_or(body, |(body, _)| body);
        Some(match header.rsplit_once(';') {
            Some((media_type, parameter)) if parameter.trim().eq_ignore_ascii_case("base64") => {
                Self {
                    claimed_media_type: media_type,
                    base64: true,
                    body,
                }
            }
            _ => Self {
                claimed_media_type: header,
                base64: false,
                body,
            },
        })
    }
}

/// Percent-decodes octets; a `%` without two hex digits stays literal.
struct PercentDecoded<'a>(&'a [u8]);

impl Iterator for PercentDecoded<'_> {
    type Item = u8;

    fn next(&mut self) -> Option<u8> {
        let (&first, rest) = self.0.split_first()?;
        let hex = |byte: u8| char::from(byte).to_digit(16);
        if first == b'%' {
            if let [high, low, rest @ ..] = rest {
                if let (Some(high), Some(low)) = (hex(*high), hex(*low)) {
                    self.0 = rest;
                    return u8::try_from(high * 16 + low).ok();
                }
            }
        }
        self.0 = rest;
        Some(first)
    }
}

/// A media type without its parameters, compared case-insensitively.
fn media_type_essence(media_type: &str) -> String {
    media_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
}

/// A stopped nested image retains only its own established finite viewport.
/// The data SVG contains no resources, so recovery cannot recurse again.
fn nested_limit_placeholder(
    node: roxmltree::Node<'_, '_>,
    limits: &ResourceLimits,
    revision: CompatibilityRevision,
    ordinal: u64,
    transact: &mut dyn FnMut(IntrinsicTransaction) -> Result<(), RenderError>,
) -> Result<Option<String>, ResolutionFailure> {
    fn length(value: &str, basis: Option<f64>) -> Option<f64> {
        let value = value.trim();
        let number = if let Some(percent) = value.strip_suffix('%') {
            percent.parse::<f64>().ok()? * basis? / 100.0
        } else {
            let (raw, multiplier) = [
                ("px", 1.0),
                ("in", 96.0),
                ("cm", 96.0 / 2.54),
                ("mm", 96.0 / 25.4),
                ("pt", 96.0 / 72.0),
                ("pc", 16.0),
            ]
            .into_iter()
            .find_map(|(unit, factor)| value.strip_suffix(unit).map(|raw| (raw, factor)))
            .unwrap_or((value, 1.0));
            raw.parse::<f64>().ok()? * multiplier
        };
        (number.is_finite() && number.abs() <= f64::from(f32::MAX)).then_some(number)
    }
    let mut ancestors = Vec::new();
    for ancestor in node.ancestors().skip(1).filter(|n| n.has_tag_name("svg")) {
        ancestors
            .try_reserve(1)
            .map_err(|_| ResolutionFailure::Memory)?;
        ancestors.push(ancestor);
    }
    let mut viewport = None;
    for ancestor in ancestors.into_iter().rev() {
        let parent = viewport;
        viewport = None;
        if let Some(view_box) = ancestor.attribute("viewBox") {
            let mut values = view_box
                .split(|c: char| c.is_ascii_whitespace() || c == ',')
                .filter(|s| !s.is_empty());
            let _ = values.next();
            let _ = values.next();
            viewport = values
                .next()
                .and_then(|v| length(v, None))
                .zip(values.next().and_then(|v| length(v, None)));
        }
        if viewport.is_none() {
            viewport = Some(ancestor.attribute("width").unwrap_or("100%"))
                .and_then(|v| length(v, parent.map(|v: (f64, f64)| v.0)))
                .zip(
                    Some(ancestor.attribute("height").unwrap_or("100%"))
                        .and_then(|v| length(v, parent.map(|v| v.1))),
                );
        }
    }
    let x = length(node.attribute("x").unwrap_or("0"), viewport.map(|v| v.0));
    let y = length(node.attribute("y").unwrap_or("0"), viewport.map(|v| v.1));
    let width = node
        .attribute("width")
        .and_then(|v| length(v, viewport.map(|v| v.0)));
    let height = node
        .attribute("height")
        .and_then(|v| length(v, viewport.map(|v| v.1)));
    let (Some(x), Some(y), Some(width), Some(height)) = (x, y, width, height) else {
        return Ok(None);
    };
    if width <= 0.0
        || height <= 0.0
        || [x, y, width, height, x + width, y + height]
            .iter()
            .any(|v| v.abs() > limits.max_coordinate_magnitude())
    {
        return Ok(None);
    }
    resource_placeholder_url(width, height, limits, revision, ordinal, transact)
}

pub(super) fn resource_placeholder_url(
    width: f64,
    height: f64,
    limits: &ResourceLimits,
    revision: CompatibilityRevision,
    ordinal: u64,
    transact: &mut dyn FnMut(IntrinsicTransaction) -> Result<(), RenderError>,
) -> Result<Option<String>, ResolutionFailure> {
    use std::fmt::Write;
    if !width.is_finite()
        || !height.is_finite()
        || width <= 0.0
        || height <= 0.0
        || width > limits.max_coordinate_magnitude()
        || height > limits.max_coordinate_magnitude()
        // This recipe's nested viewBox is 668 units wide, independent of the
        // destination. A small clip cannot waive its own coordinate admission.
        || limits.max_coordinate_magnitude() < 668.0
        || width as f32 <= 0.0
        || height as f32 <= 0.0
        || !(width as f32).is_finite()
        || !(height as f32).is_finite()
    {
        return Ok(None);
    }
    // The manifested two-glyph fallback has a fixed bounded construction
    // envelope, shared by no other rejected occurrence.
    const WORK: u64 = 8192;
    if WORK > limits.max_asset_work_units() {
        return Ok(None);
    }
    let identity =
        super::semantic_identity(revision, super::MediaKind::Svg, &ordinal.to_le_bytes());
    transact(IntrinsicTransaction::Acquire {
        event: "nested_resource_placeholder",
        identity,
        persistent_working_bytes: 0,
        temporary_working_bytes: 8192,
        decoded_pixels: 0,
        total_work_units: WORK,
    })
    .map_err(ResolutionFailure::Transaction)?;
    let size = width.min(height).min((width.min(height) * 0.4).min(100.0));
    // Exact official image-error recipe; transforms are preserved by the
    // enclosing source image, and its existing viewport clips this content.
    let mut svg = String::new();
    svg.try_reserve_exact(8192)
        .map_err(|_| ResolutionFailure::Memory)?;
    write!(svg,r##"<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" viewBox="0 0 {width} {height}"><rect width="{width}" height="{height}" fill="#E7E7E7"/><svg x="{}" y="{}" width="{size}" height="{size}" viewBox="0 0 668 668"><path fill="#888" transform="matrix(0.81709 0 0 0.81709 124.825 145.825)" d="M464 448H48c-26.51 0-48-21.49-48-48V112c0-26.51 21.49-48 48-48h416c26.51 0 48 21.49 48 48v288c0 26.51-21.49 48-48 48ZM112 120c-30.928 0-56 25.072-56 56s25.072 56 56 56 56-25.072 56-56-25.072-56-56-56ZM64 384h384V272l-87.515-87.515c-4.686-4.686-12.284-4.686-16.971 0L208 320l-55.515-55.515c-4.686-4.686-12.284-4.686-16.971 0L64 336v48Z"/><path fill="#888" transform="matrix(0.30366 0 0 0.30366 506.822 60.065)" d="M256 8C119.034 8 8 119.033 8 256c0 136.967 111.034 248 248 248s248-111.034 248-248S392.967 8 256 8Zm130.108 117.892c65.448 65.448 70 165.481 20.677 235.637L150.47 105.216c70.204-49.356 170.226-44.735 235.638 20.676ZM125.892 386.108c-65.448-65.448-70-165.481-20.677-235.637L361.53 406.784c-70.203 49.356-170.226 44.736-235.638-20.676Z"/></svg></svg>"##, (width-size)/2.0, (height-size)/2.0).map_err(|_| ResolutionFailure::Memory)?;
    let mut url = String::new();
    url.try_reserve_exact(32 + svg.len().div_ceil(3) * 4)
        .map_err(|_| ResolutionFailure::Memory)?;
    url.push_str("data:image/svg+xml;base64,");
    base64::engine::general_purpose::STANDARD.encode_string(svg, &mut url);
    transact(IntrinsicTransaction::Acquire {
        event: "nested_resource_placeholder_bytes",
        identity,
        persistent_working_bytes: 256 + (url.len() as u64).div_ceil(64) * 64,
        temporary_working_bytes: 0,
        decoded_pixels: 0,
        total_work_units: 0,
    })
    .map_err(ResolutionFailure::Transaction)?;
    transact(IntrinsicTransaction::ReleaseTemporary {
        event: "nested_resource_placeholder",
        identity,
        bytes: 8192,
    })
    .map_err(ResolutionFailure::Transaction)?;
    Ok(Some(url))
}

pub(super) fn replay_recovery(
    admission: &NestedAdmission,
    transact: &mut dyn FnMut(IntrinsicTransaction) -> Result<(), RenderError>,
) -> Result<(), ResolutionFailure> {
    if let NestedAdmission::Recovery {
        identity,
        retained_bytes,
    } = admission
    {
        transact(IntrinsicTransaction::Acquire {
            event: "nested_resource_placeholder",
            identity: *identity,
            persistent_working_bytes: 0,
            temporary_working_bytes: 8192,
            decoded_pixels: 0,
            total_work_units: 8192,
        })
        .map_err(ResolutionFailure::Transaction)?;
        transact(IntrinsicTransaction::Acquire {
            event: "nested_resource_placeholder_bytes",
            identity: *identity,
            persistent_working_bytes: *retained_bytes,
            temporary_working_bytes: 0,
            decoded_pixels: 0,
            total_work_units: 0,
        })
        .map_err(ResolutionFailure::Transaction)?;
        transact(IntrinsicTransaction::ReleaseTemporary {
            event: "nested_resource_placeholder",
            identity: *identity,
            bytes: 8192,
        })
        .map_err(ResolutionFailure::Transaction)?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn sanitize_nested_data_image(
    value: &str,
    limits: &ResourceLimits,
    revision: CompatibilityRevision,
    format: OutputFormat,
    depth: u32,
    max_open_elements: usize,
    slot_node_ordinal: u64,
    slot_attribute_ordinal: u64,
    shared_rasters: &BTreeMap<[u8; 32], Arc<super::NormalizedNestedRaster>>,
    pending_rasters: &mut BTreeMap<[u8; 32], Arc<super::NormalizedNestedRaster>>,
    removals: &mut Vec<Removal>,
    failures: &mut Vec<Failure>,
    media_claims: &mut Vec<MediaClaim>,
    effects: &mut Vec<Effect>,
    transact: &mut dyn FnMut(IntrinsicTransaction) -> Result<(), RenderError>,
    nested_accounting: &mut NestedAccounting,
) -> Result<Option<String>, ResolutionFailure> {
    let Some(data) = value.strip_prefix("data:") else {
        return Ok(None);
    };
    let Some(DataUrl {
        claimed_media_type,
        base64,
        body,
    }) = DataUrl::parse(data)
    else {
        return Err(ResolveError::SvgDecodeFailed.into());
    };
    let attempted_depth = depth.saturating_add(1);
    let depth_error = ResolveError::SvgDepth {
        attempted: u64::from(attempted_depth),
    };
    // Base64 and percent-encoded payloads are one occurrence of their decoded
    // octets under the same bounds, accounting and recursion.
    let bytes = if base64 {
        let (decoded_prefix, valid_base64) = super::payload::decoded_prefix(body);
        admit_nested_occurrence(decoded_prefix, transact, nested_accounting)?;
        if !valid_base64 {
            return Err(ResolveError::SvgDecodeFailed.into());
        }
        if attempted_depth > limits.max_nested_asset_depth() {
            return Err(depth_error.into());
        }
        base64::engine::general_purpose::STANDARD
            .decode(body)
            .map_err(|_| ResolveError::SvgDecodeFailed)?
    } else {
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(PercentDecoded(body.as_bytes()).count())
            .map_err(|_| ResolutionFailure::Memory)?;
        bytes.extend(PercentDecoded(body.as_bytes()));
        admit_nested_occurrence(
            u64::try_from(bytes.len()).unwrap_or(u64::MAX),
            transact,
            nested_accounting,
        )?;
        if attempted_depth > limits.max_nested_asset_depth() {
            return Err(depth_error.into());
        }
        bytes
    };
    let decoded_bytes = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
    if decoded_bytes > limits.max_asset_bytes() {
        return Err(ResolveError::AssetBytes {
            attempted: decoded_bytes,
        }
        .into());
    }
    let validated_media_type = if looks_like_svg(&bytes) {
        "image/svg+xml"
    } else {
        super::sniff_media_kind(&bytes)?.media_type()
    };
    let (media_type, canonical_bytes) = if looks_like_svg(&bytes) {
        let identity = super::semantic_identity(revision, super::MediaKind::Svg, &bytes);
        let cache_key = (identity, attempted_depth, max_open_elements);
        let cached = nested_accounting.sanitized_svgs.get(&cache_key).cloned();
        let (nested, shared) = if let Some(nested) = cached {
            for admission in &nested.nested_admissions {
                replay_recovery(admission, transact)?;
                if matches!(admission, NestedAdmission::Recovery { .. }) {
                    nested_accounting
                        .admissions
                        .try_reserve(1)
                        .map_err(|_| ResolutionFailure::Memory)?;
                    nested_accounting.admissions.push(admission.clone());
                }
                if let NestedAdmission::Occurrence { asset_bytes } = admission {
                    admit_nested_occurrence(*asset_bytes, transact, nested_accounting)?;
                }
            }
            (nested, true)
        } else {
            let admission_start = nested_accounting.admissions.len();
            let mut nested = sanitize_at(
                &bytes,
                limits,
                revision,
                format,
                attempted_depth,
                max_open_elements,
                shared_rasters,
                pending_rasters,
                transact,
                nested_accounting,
            )?;
            let child_admissions = &nested_accounting.admissions[admission_start..];
            nested
                .nested_admissions
                .try_reserve_exact(child_admissions.len())
                .map_err(|_| ResolutionFailure::Memory)?;
            nested
                .nested_admissions
                .extend(child_admissions.iter().cloned());
            let nested = Arc::new(nested);
            nested_accounting
                .sanitized_svgs
                .insert(cache_key, Arc::clone(&nested));
            (nested, false)
        };
        if !shared
            && !nested_accounting
                .charged_validation_work
                .contains(&identity)
        {
            nested_accounting
                .charged_validation_work
                .try_reserve(1)
                .map_err(|_| ResolutionFailure::Memory)?;
            nested_accounting.charged_validation_work.push(identity);
            let total_work_units = nested_accounting
                .total_work_units
                .saturating_add(nested.work_units);
            transact(IntrinsicTransaction::PreflightNested {
                asset_entries: nested_accounting.asset_entries,
                asset_bytes: nested_accounting.asset_bytes,
                decoded_pixels: nested_accounting.decoded_pixels,
                total_work_units: total_work_units.saturating_add(nested_accounting.asset_bytes),
            })
            .map_err(ResolutionFailure::Transaction)?;
            nested_accounting
                .admissions
                .try_reserve(1)
                .map_err(|_| ResolutionFailure::Memory)?;
            nested_accounting.admissions.push(NestedAdmission::Work {
                identity,
                total_work_units: nested.work_units,
            });
            nested_accounting.total_work_units = total_work_units;
        }
        for mut removal in nested.removals.iter().cloned() {
            removal
                .slots
                .insert(0, (slot_node_ordinal, slot_attribute_ordinal));
            removals.push(removal);
        }
        for mut failure in nested.failures.iter().cloned() {
            failure
                .path
                .insert(0, (slot_node_ordinal, slot_attribute_ordinal));
            failures.push(failure);
        }
        for mut claim in nested.media_claims.iter().cloned() {
            claim
                .path
                .insert(0, (slot_node_ordinal, slot_attribute_ordinal));
            media_claims.push(claim);
        }
        for mut effect in nested.effects.iter().cloned() {
            for cause in &mut effect.causes {
                cause
                    .slots
                    .insert(0, (slot_node_ordinal, slot_attribute_ordinal));
            }
            effects.push(effect);
        }
        ("image/svg+xml", nested.bytes.clone())
    } else {
        let (kind, canonical_bytes) = if format == OutputFormat::Svg {
            let (kind, _, validation_work) = super::validate_nested_raster(&bytes, limits)?;
            let identity = super::semantic_identity(revision, kind, &bytes);
            if !nested_accounting
                .charged_validation_work
                .contains(&identity)
            {
                nested_accounting
                    .charged_validation_work
                    .try_reserve(1)
                    .map_err(|_| ResolutionFailure::Memory)?;
                nested_accounting.charged_validation_work.push(identity);
                let total_work_units = nested_accounting
                    .total_work_units
                    .saturating_add(validation_work);
                transact(IntrinsicTransaction::PreflightNested {
                    asset_entries: nested_accounting.asset_entries,
                    asset_bytes: nested_accounting.asset_bytes,
                    decoded_pixels: nested_accounting.decoded_pixels,
                    total_work_units: total_work_units
                        .saturating_add(nested_accounting.asset_bytes),
                })
                .map_err(ResolutionFailure::Transaction)?;
                nested_accounting
                    .admissions
                    .try_reserve(1)
                    .map_err(|_| ResolutionFailure::Memory)?;
                nested_accounting.admissions.push(NestedAdmission::Work {
                    identity,
                    total_work_units: validation_work,
                });
                nested_accounting.total_work_units = total_work_units;
            }
            (kind, Arc::new(bytes.clone()))
        } else {
            let kind = super::sniff_media_kind(&bytes)?;
            let identity = super::semantic_identity(revision, kind, &bytes);
            let normalized = if let Some(normalized) = pending_rasters
                .get(&identity)
                .or_else(|| shared_rasters.get(&identity))
            {
                Arc::clone(normalized)
            } else {
                let validated = super::validate_nested_raster_semantics(&bytes, limits)?;
                let work = u64::try_from(bytes.len())
                    .unwrap_or(u64::MAX)
                    .saturating_mul(5)
                    .saturating_add(validated.pixels.saturating_mul(4))
                    .saturating_add(validated.validation_work);
                let decoded_pixels = nested_accounting
                    .decoded_pixels
                    .saturating_add(validated.pixels);
                transact(IntrinsicTransaction::PreflightNested {
                    asset_entries: nested_accounting.asset_entries,
                    asset_bytes: nested_accounting.asset_bytes,
                    decoded_pixels,
                    total_work_units: nested_accounting
                        .total_work_units
                        .saturating_add(nested_accounting.asset_bytes)
                        .saturating_add(work),
                })
                .map_err(ResolutionFailure::Transaction)?;
                if let Some(dimension) = [validated.source_width, validated.source_height]
                    .into_iter()
                    .find(|side| *side > limits.max_raster_dimension())
                {
                    return Err(ResolveError::Dimensions {
                        attempted: u64::from(dimension),
                    }
                    .into());
                }
                if validated.pixels > limits.max_decoded_pixels_per_asset() {
                    return Err(ResolveError::Pixels {
                        attempted: validated.pixels,
                    }
                    .into());
                }
                if work > limits.max_asset_work_units() {
                    transact(IntrinsicTransaction::RejectLocalWork {
                        event: "nested_raster_normalization",
                        identity,
                        total_work_units: work,
                    })
                    .map_err(ResolutionFailure::Transaction)?;
                    return Err(ResolveError::AssetWork { attempted: work }.into());
                }
                nested_accounting
                    .admissions
                    .try_reserve(1)
                    .map_err(|_| ResolutionFailure::Memory)?;
                nested_accounting
                    .admissions
                    .push(NestedAdmission::DecodedPixels {
                        decoded_pixels: validated.pixels,
                    });
                nested_accounting.decoded_pixels = decoded_pixels;
                nested_accounting
                    .admissions
                    .try_reserve(1)
                    .map_err(|_| ResolutionFailure::Memory)?;
                nested_accounting.admissions.push(NestedAdmission::Work {
                    identity,
                    total_work_units: work,
                });
                nested_accounting.total_work_units =
                    nested_accounting.total_work_units.saturating_add(work);
                let normalized =
                    Arc::new(super::normalize_nested_raster(&bytes, validated, format)?);
                pending_rasters.insert(identity, Arc::clone(&normalized));
                normalized
            };
            (
                normalized.source_media_kind,
                Arc::new(normalized.normalized_png.clone()),
            )
        };
        let emitted_type = if format == OutputFormat::Svg {
            kind.media_type()
        } else {
            "image/png"
        };
        (emitted_type, canonical_bytes.as_ref().clone())
    };
    if media_type_essence(claimed_media_type) != validated_media_type {
        media_claims.push(MediaClaim {
            path: vec![(slot_node_ordinal, slot_attribute_ordinal)],
            claimed: claimed_media_type.to_owned(),
            validated: validated_media_type.to_owned(),
        });
    }
    let mut canonical = format!("data:{media_type};base64,");
    base64::engine::general_purpose::STANDARD.encode_string(canonical_bytes, &mut canonical);
    Ok(Some(canonical))
}

fn admit_nested_occurrence(
    decoded_bytes: u64,
    transact: &mut dyn FnMut(IntrinsicTransaction) -> Result<(), RenderError>,
    accounting: &mut NestedAccounting,
) -> Result<(), ResolutionFailure> {
    let asset_entries = accounting.asset_entries.saturating_add(1);
    let asset_bytes = accounting.asset_bytes.saturating_add(decoded_bytes);
    transact(IntrinsicTransaction::PreflightNested {
        asset_entries,
        asset_bytes,
        decoded_pixels: accounting.decoded_pixels,
        total_work_units: accounting.total_work_units.saturating_add(asset_bytes),
    })
    .map_err(ResolutionFailure::Transaction)?;
    accounting
        .admissions
        .try_reserve(1)
        .map_err(|_| ResolutionFailure::Memory)?;
    accounting.admissions.push(NestedAdmission::Occurrence {
        asset_bytes: decoded_bytes,
    });
    accounting.asset_entries = asset_entries;
    accounting.asset_bytes = asset_bytes;
    Ok(())
}

fn count_descendants(node: roxmltree::Node<'_, '_>, node_ordinal: &mut u64) {
    let descendants = node
        .descendants()
        .skip(1)
        .filter(|node| node.is_element() || node.is_text())
        .count();
    *node_ordinal = node_ordinal.saturating_add(u64::try_from(descendants).unwrap_or(u64::MAX));
}

fn allowed_element(name: &str) -> bool {
    matches!(
        name,
        "svg"
            | "g"
            | "defs"
            | "style"
            | "symbol"
            | "use"
            | "title"
            | "desc"
            | "metadata"
            | "path"
            | "rect"
            | "circle"
            | "ellipse"
            | "line"
            | "polyline"
            | "polygon"
            | "linearGradient"
            | "radialGradient"
            | "stop"
            | "pattern"
            | "clipPath"
            | "mask"
            | "marker"
            | "image"
            | "text"
            | "tspan"
            | "textPath"
            | "a"
            | "filter"
            | "feBlend"
            | "feColorMatrix"
            | "feComponentTransfer"
            | "feFuncR"
            | "feFuncG"
            | "feFuncB"
            | "feFuncA"
            | "feComposite"
            | "feConvolveMatrix"
            | "feDiffuseLighting"
            | "feDisplacementMap"
            | "feDistantLight"
            | "feDropShadow"
            | "feFlood"
            | "feGaussianBlur"
            | "feImage"
            | "feMerge"
            | "feMergeNode"
            | "feMorphology"
            | "feOffset"
            | "fePointLight"
            | "feSpecularLighting"
            | "feSpotLight"
            | "feTile"
            | "feTurbulence"
    )
}

fn unsafe_attribute(element_name: &str, name: &str, namespace: Option<&str>, value: &str) -> bool {
    if name.len() >= 2 && name[..2].eq_ignore_ascii_case("on") {
        return true;
    }
    if namespace.is_some_and(|namespace| {
        namespace != "http://www.w3.org/1999/xlink"
            && namespace != "http://www.w3.org/XML/1998/namespace"
    }) {
        return true;
    }
    if matches!(
        name,
        "href" | "xlink:href" | "src" | "action" | "formaction"
    ) {
        return !safe_href(element_name, value);
    }
    if element_name == "feConvolveMatrix"
        && name == "order"
        && namespace.is_none()
        && convolve_order_overflows(value)
    {
        return true;
    }
    let lowercase = value.to_ascii_lowercase();
    lowercase.contains("@import")
        || lowercase.contains("javascript:")
        || lowercase.contains("expression(")
        || lowercase.contains("-moz-binding")
        || lowercase.contains("behavior:")
        || contains_nonlocal_url(&lowercase)
}

/// Whether usvg's `convert_convolve_matrix` (unchanged from 0.48 to 0.49)
/// would multiply this `order` past `u32`, which panics where overflow checks
/// are on (and wraps into a kernel resvg indexes out of bounds where they are
/// off). It reads the
/// value as usvg does: two numbers, each truncated to `i32`, the second
/// defaulting to the first, either one non-positive leaving the 3x3 default.
/// No kernel matrix an asset could carry has that many cells, so dropping
/// such an `order` changes no kernel that could have been drawn.
fn convolve_order_overflows(value: &str) -> bool {
    let mut numbers = svgtypes::NumberListParser::from(value);
    // `as` saturates, like usvg's own conversion.
    let x = numbers.next().and_then(Result::ok).map_or(3, |n| n as i32);
    let y = numbers.next().and_then(Result::ok).map_or(x, |n| n as i32);
    match (u32::try_from(x), u32::try_from(y)) {
        (Ok(x), Ok(y)) if x > 0 && y > 0 => x.checked_mul(y).is_none(),
        _ => false,
    }
}

fn discover_effects(
    document: &roxmltree::Document<'_>,
    source_nodes: &[SourceNode],
    css: &css::Resolved,
) -> Vec<Effect> {
    let mut effects = Vec::new();
    for node in document.descendants().filter(|node| node.is_element()) {
        let direct = if css.enabled {
            None
        } else {
            filter_reference(node).map(|(ordinal, name, value)| css::Filter {
                attribute: Some((ordinal, name)),
                value,
                css: None,
            })
        };
        let reference = if css.enabled {
            css.get(node).and_then(|(_, filter)| filter)
        } else {
            direct.as_ref()
        };
        let Some(reference) = reference else {
            continue;
        };
        let filter_value = &reference.value;
        let Some(filter_id) = local_url_target(filter_value) else {
            continue;
        };
        let Some(filter) = referenced_filter(node, filter_value) else {
            continue;
        };
        let primitives = filter
            .children()
            .filter(|child| child.is_element() && child.tag_name().name().starts_with("fe"))
            .collect::<Vec<_>>();
        if effect_fully_clipped(node)
            || primitives.is_empty()
            || primitives
                .iter()
                .all(|primitive| primitive_is_noop(*primitive))
        {
            continue;
        }
        let node_ordinal = source_node_ordinal(document, source_nodes, node);
        let mut causes = Vec::new();
        causes.push(EffectCause {
            css: None,
            slots: Vec::new(),
            node_ordinal,
            attribute: reference
                .attribute
                .as_ref()
                .map(|(ordinal, name)| (*ordinal as u64, name.clone())),
            effect_input: false,
        });
        if let Some((style, rule, declaration)) = reference.css {
            causes.push(EffectCause {
                slots: Vec::new(),
                node_ordinal: style,
                attribute: None,
                effect_input: false,
                css: Some((rule, declaration)),
            });
        }
        for primitive in &primitives {
            let primitive_ordinal = source_node_ordinal(document, source_nodes, *primitive);
            causes.push(EffectCause {
                css: None,
                slots: Vec::new(),
                node_ordinal: primitive_ordinal,
                attribute: None,
                effect_input: false,
            });
            for (attribute_ordinal, attribute) in primitive.attributes().enumerate() {
                if matches!(attribute.name(), "in" | "in2") {
                    causes.push(EffectCause {
                        css: None,
                        slots: Vec::new(),
                        node_ordinal: primitive_ordinal,
                        attribute: Some((
                            u64::try_from(attribute_ordinal).unwrap_or(u64::MAX),
                            attribute.name().to_owned(),
                        )),
                        effect_input: true,
                    });
                }
            }
        }
        let unisolatable = primitives.iter().any(|primitive| {
            primitive.attributes().any(|attribute| {
                matches!(attribute.name(), "in" | "in2")
                    && matches!(attribute.value(), "BackgroundImage" | "BackgroundAlpha")
            })
        });
        effects.push(Effect {
            node_ordinal,
            filter_id: filter_id.to_owned(),
            usage_id: node
                .attribute("id")
                .map(str::to_owned)
                .unwrap_or_else(|| format!("__excalidraw_effect_{node_ordinal}")),
            marker: primitives[0]
                .attribute("result")
                .map(str::to_owned)
                .unwrap_or_else(|| {
                    format!(
                        "__excalidraw_filter_result_{}",
                        source_node_ordinal(document, source_nodes, primitives[0])
                    )
                }),
            causes,
            unisolatable,
        });
    }
    effects
}

fn filter_reference(node: roxmltree::Node<'_, '_>) -> Option<(usize, String, String)> {
    for (attribute_ordinal, attribute) in node.attributes().enumerate() {
        if attribute.name() == "filter" {
            return Some((
                attribute_ordinal,
                "filter".to_owned(),
                attribute.value().to_owned(),
            ));
        }
        if attribute.name() == "style" {
            for declaration in attribute.value().split(';') {
                let Some((name, value)) = declaration.split_once(':') else {
                    continue;
                };
                if name.trim().eq_ignore_ascii_case("filter") {
                    return Some((
                        attribute_ordinal,
                        "style".to_owned(),
                        value.trim().to_owned(),
                    ));
                }
            }
        }
    }
    None
}

fn local_url_target(value: &str) -> Option<&str> {
    value
        .trim()
        .strip_prefix("url(")?
        .strip_suffix(')')?
        .trim()
        .trim_matches(['\'', '"'])
        .strip_prefix('#')
}

fn filter_reference_is_noop(node: roxmltree::Node<'_, '_>, value: &str) -> bool {
    effect_fully_clipped(node) || filter_value_is_noop(node, value)
}

fn filter_value_is_noop(node: roxmltree::Node<'_, '_>, value: &str) -> bool {
    let Some(filter) = referenced_filter(node, value) else {
        return false;
    };
    let primitives = filter
        .children()
        .filter(|child| child.is_element() && child.tag_name().name().starts_with("fe"))
        .collect::<Vec<_>>();
    primitives.is_empty()
        || primitives
            .iter()
            .all(|primitive| primitive_is_noop(*primitive))
}

fn referenced_filter<'a, 'input>(
    node: roxmltree::Node<'a, 'input>,
    value: &str,
) -> Option<roxmltree::Node<'a, 'input>> {
    let filter_id = local_url_target(value)?;
    node.document().descendants().find(|candidate| {
        candidate.is_element()
            && candidate.tag_name().name() == "filter"
            && candidate.attribute("id") == Some(filter_id)
    })
}

fn style_without_noop_filter(node: roxmltree::Node<'_, '_>, style: &str) -> Option<String> {
    let mut removed = false;
    let mut retained = Vec::new();
    for declaration in style.split(';') {
        let declaration = declaration.trim();
        if declaration.is_empty() {
            continue;
        }
        if let Some((name, value)) = declaration.split_once(':') {
            if name.trim().eq_ignore_ascii_case("filter")
                && filter_reference_is_noop(node, value.trim())
            {
                removed = true;
                continue;
            }
        }
        retained.push(declaration);
    }
    removed.then(|| retained.join(";"))
}

fn effect_fully_clipped(node: roxmltree::Node<'_, '_>) -> bool {
    let Some(clip_id) = node.attribute("clip-path").and_then(local_url_target) else {
        return false;
    };
    let Some(subject) = source_shape_bounds(node) else {
        return false;
    };
    let Some(clip) = node.document().descendants().find(|candidate| {
        candidate.is_element()
            && candidate.tag_name().name() == "clipPath"
            && candidate.attribute("id") == Some(clip_id)
    }) else {
        return false;
    };
    let clip = clip
        .descendants()
        .filter(|candidate| candidate.is_element())
        .filter_map(source_shape_bounds)
        .reduce(|left, right| {
            (
                left.0.min(right.0),
                left.1.min(right.1),
                left.2.max(right.2),
                left.3.max(right.3),
            )
        });
    clip.is_none_or(|clip| {
        subject.2 <= clip.0 || subject.0 >= clip.2 || subject.3 <= clip.1 || subject.1 >= clip.3
    })
}

fn source_shape_bounds(node: roxmltree::Node<'_, '_>) -> Option<(f64, f64, f64, f64)> {
    let number = |name: &str, default: f64| {
        node.attribute(name)
            .map_or(Some(default), |value| value.parse::<f64>().ok())
    };
    match node.tag_name().name() {
        "rect" | "image" => {
            let x = number("x", 0.0)?;
            let y = number("y", 0.0)?;
            let width = number("width", 0.0)?.max(0.0);
            let height = number("height", 0.0)?.max(0.0);
            Some((x, y, x + width, y + height))
        }
        "circle" => {
            let cx = number("cx", 0.0)?;
            let cy = number("cy", 0.0)?;
            let radius = number("r", 0.0)?.max(0.0);
            Some((cx - radius, cy - radius, cx + radius, cy + radius))
        }
        "ellipse" => {
            let cx = number("cx", 0.0)?;
            let cy = number("cy", 0.0)?;
            let radius_x = number("rx", 0.0)?.max(0.0);
            let radius_y = number("ry", 0.0)?.max(0.0);
            Some((cx - radius_x, cy - radius_y, cx + radius_x, cy + radius_y))
        }
        _ => None,
    }
}

fn source_node_ordinal(
    document: &roxmltree::Document<'_>,
    source_nodes: &[SourceNode],
    target: roxmltree::Node<'_, '_>,
) -> u64 {
    if !source_nodes.is_empty() {
        if target.is_text() {
            let position = target.range().start;
            let end = source_nodes.partition_point(|node| node.offset <= position);
            return source_nodes[..end]
                .last()
                .filter(|node| node.text && position < node.end)
                .map_or(u64::MAX, |node| node.ordinal);
        }
        let key = (target.range().start, target.is_text());
        let index = source_nodes.partition_point(|node| (node.offset, node.text) < key);
        return source_nodes
            .get(index)
            .filter(|node| (node.offset, node.text) == key)
            .map_or(u64::MAX, |node| node.ordinal);
    }
    document
        .descendants()
        .filter(|node| node.is_element() || node.is_text())
        .position(|node| node == target)
        .and_then(|ordinal| u64::try_from(ordinal).ok())
        .unwrap_or(u64::MAX)
}

fn primitive_is_noop(node: roxmltree::Node<'_, '_>) -> bool {
    match node.tag_name().name() {
        "feGaussianBlur" => number_pair_is_zero(node.attribute("stdDeviation").unwrap_or("0")),
        "feOffset" => {
            number_is_zero(node.attribute("dx").unwrap_or("0"))
                && number_is_zero(node.attribute("dy").unwrap_or("0"))
        }
        "feColorMatrix" => color_matrix_is_identity(node),
        "feComponentTransfer" => node
            .children()
            .filter(|child| child.is_element() && child.tag_name().name().starts_with("feFunc"))
            .all(|function| function.attribute("type").unwrap_or("identity") == "identity"),
        _ => false,
    }
}

fn color_matrix_is_identity(node: roxmltree::Node<'_, '_>) -> bool {
    match node.attribute("type").unwrap_or("matrix") {
        "saturate" => node
            .attribute("values")
            .is_none_or(|value| value.trim() == "1"),
        "hueRotate" => node.attribute("values").is_none_or(number_is_zero),
        "matrix" => {
            let identity = [
                1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0,
                0.0, 0.0, 1.0, 0.0,
            ];
            node.attribute("values").is_none_or(|values| {
                values
                    .split(|character: char| character.is_ascii_whitespace() || character == ',')
                    .filter(|value| !value.is_empty())
                    .map(str::parse::<f64>)
                    .collect::<Result<Vec<_>, _>>()
                    .is_ok_and(|values| values == identity)
            })
        }
        "luminanceToAlpha" => false,
        _ => false,
    }
}

fn number_pair_is_zero(value: &str) -> bool {
    let mut values = value
        .split(|character: char| character.is_ascii_whitespace() || character == ',')
        .filter(|value| !value.is_empty());
    values.next().is_none_or(number_is_zero) && values.next().is_none_or(number_is_zero)
}

fn number_is_zero(value: &str) -> bool {
    value.parse::<f64>().is_ok_and(|value| value == 0.0)
}

fn safe_href(element_name: &str, value: &str) -> bool {
    let value = value.trim();
    value.starts_with('#')
        || matches!(element_name, "image" | "feImage")
            && value.starts_with("data:image/")
            && value.contains(',')
}

fn contains_nonlocal_url(value: &str) -> bool {
    let mut rest = value;
    while let Some(index) = rest.find("url(") {
        rest = &rest[index + 4..];
        let Some(end) = rest.find(')') else {
            return true;
        };
        let target = rest[..end].trim().trim_matches(['\'', '"']);
        if !target.starts_with('#') {
            return true;
        }
        rest = &rest[end + 1..];
    }
    false
}

fn escape_text(output: &mut String, value: &str) {
    for character in value.chars() {
        match character {
            '&' => output.push_str("&amp;"),
            '<' => output.push_str("&lt;"),
            '>' => output.push_str("&gt;"),
            _ => output.push(character),
        }
    }
}

fn escape_attribute(output: &mut String, value: &str) {
    for character in value.chars() {
        match character {
            '&' => output.push_str("&amp;"),
            '<' => output.push_str("&lt;"),
            '"' => output.push_str("&quot;"),
            '\r' => output.push_str("&#13;"),
            '\n' => output.push_str("&#10;"),
            '\t' => output.push_str("&#9;"),
            _ => output.push(character),
        }
    }
}

pub(super) fn viewer_options() -> Result<usvg::Options<'static>, ResolveError> {
    use crate::fonts::{excalifont_latin, FamilyId};
    let latin = excalifont_latin().ok_or(ResolveError::SvgDecodeFailed)?;
    let excalifont =
        crate::fonts::load_sfnt(latin.id()).map_err(|_| ResolveError::SvgDecodeFailed)?;
    let liberation_sans = crate::fonts::load_sfnt(FamilyId::Liberation.faces()[0].id())
        .map_err(|_| ResolveError::SvgDecodeFailed)?;
    let mut options = usvg::Options {
        resources_dir: None,
        font_family: "Liberation Sans".to_owned(),
        ..usvg::Options::default()
    };
    let mut fontdb = usvg::fontdb::Database::new();
    fontdb.load_font_data(excalifont);
    fontdb.load_font_data(liberation_sans);
    fontdb.set_serif_family("Liberation Sans");
    fontdb.set_sans_serif_family("Liberation Sans");
    fontdb.set_cursive_family("Liberation Sans");
    fontdb.set_fantasy_family("Liberation Sans");
    fontdb.set_monospace_family("Liberation Sans");
    options.fontdb = Arc::new(fontdb);
    options.image_href_resolver = usvg::ImageHrefResolver {
        resolve_data: Box::new(|media_type, bytes, options| match media_type {
            "image/png" => Some(usvg::ImageKind::PNG(bytes)),
            "image/svg+xml" => usvg::Tree::from_data(&bytes, options)
                .ok()
                .map(usvg::ImageKind::SVG),
            _ => None,
        }),
        resolve_string: Box::new(|_, _| None),
    };
    Ok(options)
}

#[cfg(test)]
mod tests {
    //! The nested-SVG sanitizer at its own seam: what it removes, what it
    //! refuses, and where it stops recursing, read off `SanitizedSvg` rather
    //! than off the diagnostics a session later maps them to.

    use super::*;

    const PNG_BASE64: &str =
        "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=";

    /// Sanitizes `bytes` under `limits` for the SVG format with a transaction
    /// sink that admits everything: nothing here tests the meter.
    fn sanitize_with(
        bytes: &[u8],
        limits: &ResourceLimits,
        format: OutputFormat,
    ) -> Result<SanitizedSvg, ResolutionFailure> {
        let shared_rasters = BTreeMap::new();
        let mut pending_rasters = BTreeMap::new();
        sanitize(
            bytes,
            limits,
            CompatibilityRevision::REVISION_1,
            format,
            &shared_rasters,
            &mut pending_rasters,
            &mut |_| Ok(()),
        )
    }

    fn sanitize_default(bytes: &[u8]) -> SanitizedSvg {
        sanitize_with(bytes, &ResourceLimits::default(), OutputFormat::Svg)
            .expect("the SVG sanitizes")
    }

    fn sanitized_text(sanitized: &SanitizedSvg) -> &str {
        std::str::from_utf8(&sanitized.bytes).expect("sanitized SVG is UTF-8")
    }

    fn removal_names(sanitized: &SanitizedSvg) -> Vec<&str> {
        sanitized
            .removals
            .iter()
            .map(|removal| removal.name.as_str())
            .collect()
    }

    fn svg_with_nested_data_image(child: &[u8]) -> String {
        let child = base64::engine::general_purpose::STANDARD.encode(child);
        format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10"><image width="20" height="10" href="data:image/svg+xml;base64,{child}"/></svg>"#
        )
    }

    fn asset_error(result: Result<SanitizedSvg, ResolutionFailure>) -> ResolveError {
        match result {
            Err(ResolutionFailure::Asset(error)) => error,
            other => panic!("the sanitizer refuses the asset: {other:?}"),
        }
    }

    #[test]
    fn a_safe_svg_is_reserialized_with_its_root_namespace_and_size() {
        let sanitized = sanitize_default(
            br##"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10" viewBox="0 0 20 10"><rect width="10" height="10" fill="#ff0000"/><rect x="10" width="10" height="10" fill="#0000ff"/></svg>"##,
        );
        assert_eq!((sanitized.width, sanitized.height), (20, 10));
        assert!(sanitized.removals.is_empty());
        assert!(sanitized.failures.is_empty());
        assert!(sanitized.effects.is_empty());
        assert!(sanitized.nested_admissions.is_empty());
        let text = sanitized_text(&sanitized);
        assert!(text.starts_with(r#"<svg xmlns="http://www.w3.org/2000/svg""#));
        assert_eq!(text.matches("<rect").count(), 2);
        assert!(sanitized.work_units > 0 && sanitized.working_bytes > 512);
    }

    #[test]
    fn scripts_handlers_animation_and_foreign_content_are_removed() {
        let sanitized = sanitize_default(
            br##"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10" onload="alert(1)">
            <rect width="20" height="10" fill="#00ff00" onclick="alert(2)"/>
            <script>alert(3)</script>
            <animate attributeName="opacity" values="0;1"/>
            <foreignObject><div xmlns="http://www.w3.org/1999/xhtml">unsafe</div></foreignObject>
            <image href="https://example.com/tracker.png" width="20" height="10"/>
            <style>@import url(https://example.com/theme.css); rect { fill: red }</style>
        </svg>"##,
        );

        let text = sanitized_text(&sanitized);
        for forbidden in [
            "onload",
            "onclick",
            "script",
            "animate",
            "foreignObject",
            "example.com",
            "@import",
        ] {
            assert!(!text.contains(forbidden), "removed {forbidden}: {text}");
        }
        assert!(text.contains("<rect"));
        assert_eq!(
            removal_names(&sanitized),
            [
                "onload",
                "onclick",
                "script",
                "animate",
                "foreignObject",
                "style"
            ]
        );
        assert_eq!(sanitized.failures.len(), 1);
        assert_eq!(sanitized.failures[0].error, ResolveError::Forbidden);
        assert!(sanitized.failures[0].placeholder);
        // Scripts and handlers never execute in static rendering. The rejected
        // image href has a separate resource replacement, not an unsafe boundary.
        assert_eq!(
            sanitized
                .removals
                .iter()
                .filter(|removal| !removal.changes_appearance)
                .map(|removal| removal.name.as_str())
                .collect::<Vec<_>>(),
            ["onload", "onclick", "script"]
        );
        // Attribute removals name their slot; element removals do not.
        assert!(sanitized.removals[0].attribute_ordinal.is_some());
        assert!(sanitized.removals[2].attribute_ordinal.is_none());
    }

    #[test]
    fn inert_editor_metadata_is_removed_without_a_diagnostic() {
        // An Inkscape export: Sodipodi and Inkscape state a browser ignores.
        let sanitized = sanitize_default(
            br##"<svg xmlns="http://www.w3.org/2000/svg" xmlns:inkscape="http://www.inkscape.org/namespaces/inkscape" xmlns:sodipodi="http://sodipodi.sourceforge.net/DTD/sodipodi-0.dtd" xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#" width="20" height="10" sodipodi:docname="d.svg" inkscape:version="1.3" inkscape:export-xdpi="96">
            <sodipodi:namedview pagecolor="#ffffff"><inkscape:page x="0" y="0"/></sodipodi:namedview>
            <metadata><rdf:RDF/></metadata>
            <path d="M0 0H20" stroke="#000" sodipodi:nodetypes="cc" inkscape:onclick="x"/>
            <sodipodi:guide><rect width="20" height="10"/></sodipodi:guide>
        </svg>"##,
        );

        let text = sanitized_text(&sanitized);
        for removed in ["sodipodi", "inkscape", "rdf"] {
            assert!(!text.contains(removed), "removed {removed}: {text}");
        }
        assert!(text.contains("<path"));
        // An `on*` spelling and editor markup wrapping SVG content stay
        // reported, whatever their namespace.
        assert_eq!(removal_names(&sanitized), ["onclick", "guide"]);
    }

    #[test]
    fn internal_entities_expand_and_the_dtd_is_removed() {
        let sanitized = sanitize_default(
            br##"<?xml version="1.0"?><!DOCTYPE svg [<!ENTITY secret "EXPANDED"><!ENTITY paint "#00ff00">]><svg xmlns="http://www.w3.org/2000/svg" width="20" height="10"><text>&secret;</text><rect width="20" height="10" fill="&paint;"/></svg>"##,
        );
        let text = sanitized_text(&sanitized);
        assert!(!text.contains("DOCTYPE"));
        assert!(!text.contains("ENTITY"));
        assert!(text.contains("<text>EXPANDED</text>"), "{text}");
        assert!(text.contains(r##"fill="#00ff00""##), "{text}");
        // Only the DTD is removed, and it paints nothing.
        assert_eq!(
            sanitized
                .removals
                .iter()
                .map(|removal| (removal.name.as_str(), removal.changes_appearance))
                .collect::<Vec<_>>(),
            [("DOCTYPE", false)]
        );
    }

    #[test]
    fn entity_replacement_text_is_markup_in_content_and_text_in_attributes() {
        let sanitized = sanitize_default(
            br##"<!DOCTYPE svg [<!ENTITY a "#e0"><!ENTITY c "&a;&#x65;0e020"><!ENTITY q 'say "hi"'><!ENTITY r "<rect id='r' width='4' height='4' fill='&c;'/>">]><svg xmlns="http://www.w3.org/2000/svg" width="20" height="10">&r;<desc title="&q;"/></svg>"##,
        );
        let text = sanitized_text(&sanitized);
        assert!(
            text.contains(r##"<rect id="r" width="4" height="4" fill="#e0e0e020">"##),
            "{text}"
        );
        assert!(text.contains(r#"title="say &quot;hi&quot;""#), "{text}");
        assert_eq!(removal_names(&sanitized), ["DOCTYPE"]);
    }

    #[test]
    fn external_entities_expand_to_nothing_and_are_reported() {
        let sanitized = sanitize_default(
            br##"<!DOCTYPE svg [<!ENTITY x SYSTEM "https://example.invalid/x"><!ENTITY y PUBLIC "-//X//Y" "https://example.invalid/y">]><svg xmlns="http://www.w3.org/2000/svg" width="20" height="10"><text>A&x;B</text><rect width="20" height="10" fill="red&y;"/></svg>"##,
        );
        let text = sanitized_text(&sanitized);
        assert!(text.contains("<text>AB</text>"), "{text}");
        assert!(!text.contains("example.invalid"));
        assert_eq!(
            sanitized
                .removals
                .iter()
                .map(|removal| (
                    removal.name.as_str(),
                    removal.node_ordinal,
                    removal.attribute_ordinal
                ))
                .collect::<Vec<_>>(),
            [("DOCTYPE", 0, None), ("x", 2, None), ("fill", 3, Some(2))]
        );
    }

    #[test]
    fn undeclared_entities_reject_unless_the_dtd_may_declare_them() {
        for bytes in [
            br#"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10"><rect fill="&nope;"/></svg>"#.as_slice(),
            br#"<!DOCTYPE svg [<!ENTITY a "x">]><svg xmlns="http://www.w3.org/2000/svg" width="20" height="10"><text>&nope;</text></svg>"#.as_slice(),
            br#"<?xml version="1.0" standalone="yes"?><!DOCTYPE svg SYSTEM "https://example.invalid/svg.dtd"><svg xmlns="http://www.w3.org/2000/svg" width="20" height="10"><text>&nope;</text></svg>"#.as_slice(),
        ] {
            assert_eq!(
                asset_error(sanitize_with(bytes, &ResourceLimits::default(), OutputFormat::Svg)),
                ResolveError::SvgDecodeFailed,
                "{}",
                String::from_utf8_lossy(bytes)
            );
        }
        // An external subset or parameter-entity reference may declare it
        // where the reference does not look: it is removed instead.
        for bytes in [
            br#"<!DOCTYPE svg PUBLIC "-//W3C//DTD SVG 1.1//EN" "http://www.w3.org/Graphics/SVG/1.1/DTD/svg11.dtd"><svg xmlns="http://www.w3.org/2000/svg" width="20" height="10"><text>A&nbsp;B</text></svg>"#.as_slice(),
            br#"<!DOCTYPE svg [<!ENTITY % p "x"> %p;]><svg xmlns="http://www.w3.org/2000/svg" width="20" height="10"><text>A&nbsp;B</text></svg>"#.as_slice(),
        ] {
            let sanitized = sanitize_default(bytes);
            assert!(sanitized_text(&sanitized).contains("<text>AB</text>"));
            assert_eq!(removal_names(&sanitized), ["DOCTYPE", "nbsp"]);
        }
    }

    #[test]
    fn entity_loops_depth_and_amplification_are_bounded() {
        let expand = |text: &str, cap: u64| entities::expand(text, cap).map(|text| text.len());
        let looped = r#"<!DOCTYPE svg [<!ENTITY a "&b;"><!ENTITY b "&a;">]><svg>&a;</svg>"#;
        assert_eq!(expand(looped, u64::MAX), Err(ResolveError::SvgDecodeFailed));
        let deep = |levels: u32| {
            let mut dtd = String::from(r#"<!DOCTYPE svg [<!ENTITY e0 "x">"#);
            for level in 1..levels {
                dtd.push_str(&format!(r#"<!ENTITY e{level} "&e{};">"#, level - 1));
            }
            format!("{dtd}]><svg>&e{};</svg>", levels - 1)
        };
        assert!(expand(&deep(entities::MAX_DEPTH), u64::MAX).is_ok());
        assert_eq!(
            expand(&deep(entities::MAX_DEPTH + 1), u64::MAX),
            Err(ResolveError::SvgDecodeFailed)
        );
        // Each reference costs 20 plus the 1,000 bytes it produces; a small
        // source may expand to 1,000,000 cost units and no more.
        let repeated = |references: usize| {
            format!(
                r#"<!DOCTYPE svg [<!ENTITY a "{}">]><svg><desc>{}</desc></svg>"#,
                "x".repeat(1000),
                "&a;".repeat(references)
            )
        };
        assert!(expand(&repeated(980), u64::MAX).is_ok());
        assert_eq!(
            expand(&repeated(981), u64::MAX),
            Err(ResolveError::SvgDecodeFailed)
        );
        // Never beyond the per-asset byte ceiling.
        assert_eq!(
            expand(&repeated(3), 3060),
            Ok(repeated(3).len() - 3 * 3 + 3000)
        );
        assert_eq!(
            expand(&repeated(3), 3059),
            Err(ResolveError::SvgDecodeFailed)
        );
        // Beyond it, five cost units per source byte: 1,020k <= 5 * (301,064 + 3k).
        let padded =
            |references: usize| format!("{}<!-- {} -->", repeated(references), " ".repeat(300_000));
        assert_eq!(padded(0).len(), 301_064);
        assert!(expand(&padded(1497), u64::MAX).is_ok());
        assert_eq!(
            expand(&padded(1498), u64::MAX),
            Err(ResolveError::SvgDecodeFailed)
        );
    }

    #[test]
    fn processing_instructions_other_than_the_xml_declaration_are_removed() {
        let sanitized = sanitize_default(
            br##"<?xml-stylesheet href="https://example.com/theme.css"?><svg xmlns="http://www.w3.org/2000/svg" width="20" height="10"><rect width="20" height="10" fill="#00ff00"/></svg>"##,
        );
        let text = sanitized_text(&sanitized);
        assert!(!text.contains("xml-stylesheet"));
        assert!(!text.contains("example.com"));
        assert_eq!(removal_names(&sanitized), ["xml-stylesheet"]);

        let declared = sanitize_default(
            br##"<?xml version="1.0" encoding="UTF-8"?><svg xmlns="http://www.w3.org/2000/svg" width="20" height="10"><rect width="20" height="10"/></svg>"##,
        );
        assert!(
            declared.removals.is_empty(),
            "the XML declaration is not a removal"
        );
    }

    #[test]
    fn external_references_are_removed_and_same_document_fragments_kept() {
        let sanitized = sanitize_default(
            br##"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="20" height="10">
            <defs><linearGradient id="paint"><stop stop-color="#ff0000"/><stop offset="1" stop-color="#0000ff"/></linearGradient><rect id="tile" width="10" height="10" fill="url(#paint)"/></defs>
            <use href="#tile"/><use xlink:href="#tile" x="10"/>
            <use href="https://example.com/shapes.svg#tile"/>
            <image xlink:href="//example.com/tracker.png" width="1" height="1"/>
            <a href="javascript:alert(1)"><rect width="1" height="1"/></a>
        </svg>"##,
        );
        let text = sanitized_text(&sanitized);
        assert!(!text.contains("example.com"));
        assert!(!text.contains("javascript:"));
        assert_eq!(text.matches(r##"href="#tile""##).count(), 2, "{text}");
        assert!(text.contains(r##"fill="url(#paint)""##));
        assert_eq!(removal_names(&sanitized), ["href"]);
        assert_eq!(sanitized.failures.len(), 2);
        assert!(sanitized
            .failures
            .iter()
            .all(|failure| failure.error == ResolveError::Forbidden));
        assert!(!sanitized.failures[0].placeholder);
        assert!(sanitized.failures[1].placeholder);
        assert!(
            !sanitized
                .nested_admissions
                .iter()
                .any(|event| matches!(event, NestedAdmission::Occurrence { .. })),
            "non-local references admit no fetched payload bytes"
        );
        // xlink:href is lowered to a plain href on the way out.
        assert!(!text.contains("xlink:href"));
    }

    #[test]
    fn data_urls_are_removed_from_link_actions() {
        let sanitized = sanitize_default(
            format!(
                r##"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10"><a href="data:image/png;base64,{PNG_BASE64}"><rect width="20" height="10" fill="#00ff00"/></a></svg>"##
            )
            .as_bytes(),
        );
        assert!(!sanitized_text(&sanitized).contains("data:image/png"));
        assert_eq!(removal_names(&sanitized), ["href"]);
    }

    #[test]
    fn nested_raster_data_is_validated_and_kept_byte_for_byte() {
        let sanitized = sanitize_default(
            format!(
                r#"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10"><image width="20" height="10" href="data:image/png;base64,{PNG_BASE64}"/></svg>"#
            )
            .as_bytes(),
        );
        assert!(sanitized_text(&sanitized).contains(&format!("data:image/png;base64,{PNG_BASE64}")));
        assert!(sanitized.removals.is_empty());
        assert!(sanitized.failures.is_empty());
        // The nested payload is one asset occurrence of its decoded size.
        assert!(matches!(
            sanitized.nested_admissions.as_slice(),
            [NestedAdmission::Occurrence { asset_bytes }, ..] if *asset_bytes > 0
        ));
    }

    #[test]
    fn nested_data_svgs_are_recursively_sanitized() {
        let child = br##"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10"><rect width="20" height="10" fill="#00ff00"/><script>alert(1)</script></svg>"##;
        let sanitized = sanitize_default(svg_with_nested_data_image(child).as_bytes());

        let text = sanitized_text(&sanitized);
        let (_, nested) = text
            .split_once("data:image/svg+xml;base64,")
            .expect("the nested SVG is re-embedded as a data URL");
        let nested = base64::engine::general_purpose::STANDARD
            .decode(nested.split('"').next().expect("terminated data URL"))
            .expect("re-embedded nested SVG is base64");
        let nested = std::str::from_utf8(&nested).expect("nested SVG is UTF-8");
        assert!(nested.contains("<rect"));
        assert!(!nested.contains("script"));
        // The child's removal surfaces on the parent.
        assert_eq!(removal_names(&sanitized), ["script"]);
        assert!(sanitized
            .nested_admissions
            .iter()
            .any(|admission| matches!(admission, NestedAdmission::Work { .. })));
    }

    #[test]
    fn a_malformed_nested_image_fails_locally_without_discarding_its_siblings() {
        let sanitized = sanitize_default(
            br##"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10"><rect width="20" height="10" fill="#ff0000"/><image width="20" height="10" href="data:image/png;base64,AA=="/></svg>"##,
        );
        assert!(sanitized_text(&sanitized).contains("<rect"));
        let [failure] = sanitized.failures.as_slice() else {
            panic!("one nested failure: {:#?}", sanitized.failures);
        };
        assert_eq!(failure.error, ResolveError::Unsupported);
        // The failing slot: the image is the third node (root, rect, image),
        // and href its third attribute.
        assert_eq!(failure.path, [(2, 2)]);
    }

    #[test]
    fn the_depth_ceiling_stops_recursive_resolution_as_a_local_failure() {
        let leaf = br##"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10"><rect width="20" height="10" fill="#00ff00"/></svg>"##;
        let child = svg_with_nested_data_image(leaf);
        let parent = svg_with_nested_data_image(child.as_bytes());
        let limits = ResourceLimits::builder()
            .max_nested_asset_depth(1)
            .build()
            .expect("depth limit is valid");

        let sanitized = sanitize_with(parent.as_bytes(), &limits, OutputFormat::Svg)
            .expect("the parent survives its child's refusal");
        let [failure] = sanitized.failures.as_slice() else {
            panic!("one depth failure: {:#?}", sanitized.failures);
        };
        assert_eq!(failure.error, ResolveError::SvgDepth { attempted: 2 });
        assert_eq!(failure.path.len(), 2, "the failing slot is two levels down");

        let permitted = sanitize_with(
            parent.as_bytes(),
            &ResourceLimits::default(),
            OutputFormat::Svg,
        )
        .expect("the default depth admits two levels");
        assert!(permitted.failures.is_empty());
    }

    /// A document whose deepest element is `levels` below its root.
    fn svg_with_element_levels(levels: usize, filler: &str) -> String {
        let groups = levels - 1;
        format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10">{}{filler}<rect width="20" height="10"/>{}</svg>"#,
            "<g>".repeat(groups),
            "</g>".repeat(groups)
        )
    }

    #[test]
    fn the_element_depth_ceiling_refuses_the_document_before_parsing() {
        let deepest = svg_with_element_levels(MAX_ELEMENT_DEPTH, "");
        sanitize_default(deepest.as_bytes());
        let deeper = svg_with_element_levels(MAX_ELEMENT_DEPTH + 1, "");
        assert_eq!(
            asset_error(sanitize_with(
                deeper.as_bytes(),
                &ResourceLimits::default(),
                OutputFormat::Svg
            )),
            ResolveError::SvgDecodeFailed
        );
        // Closed, empty and commented-out elements, markup characters in
        // attribute values and character data do not stay open.
        let filler = concat!(
            r#"<g></g><g/><g id="a>"/><g id='/>'></g><rect width="1" height="1" />"#,
            "<!-- <g> --><![CDATA[<g>]]><?pi <g>?>&lt;g&gt;"
        )
        .repeat(MAX_ELEMENT_DEPTH);
        sanitize_default(svg_with_element_levels(MAX_ELEMENT_DEPTH, &filler).as_bytes());
    }

    #[test]
    fn the_element_depth_ceiling_spans_nested_data_svgs() {
        // The image sits one level below the parent root and the nested root
        // one level below the image, so the child keeps two levels fewer.
        for (levels, admitted) in [
            (MAX_ELEMENT_DEPTH - 2, true),
            (MAX_ELEMENT_DEPTH - 1, false),
        ] {
            let child = svg_with_element_levels(levels, "");
            let parent = svg_with_nested_data_image(child.as_bytes());
            let sanitized = sanitize_default(parent.as_bytes());
            if admitted {
                assert!(sanitized.failures.is_empty(), "{levels} levels fit");
            } else {
                let [failure] = sanitized.failures.as_slice() else {
                    panic!("one nesting failure: {:#?}", sanitized.failures);
                };
                assert_eq!(failure.error, ResolveError::SvgDecodeFailed);
            }
        }
    }

    #[test]
    fn the_node_ceiling_refuses_the_document_before_viewer_parsing() {
        let nested = br##"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10"><rect width="20" height="10" fill="#00ff00"/></svg>"##;
        let limits = ResourceLimits::builder()
            .max_nested_svg_nodes(1)
            .build()
            .expect("node limit is valid");
        assert_eq!(
            asset_error(sanitize_with(nested, &limits, OutputFormat::Svg)),
            ResolveError::SvgComplexity { attempted: 2 }
        );
        let limits = ResourceLimits::builder()
            .max_nested_svg_nodes(2)
            .build()
            .expect("node limit is valid");
        sanitize_with(nested, &limits, OutputFormat::Svg).expect("two nodes fit the ceiling");
    }

    #[test]
    fn the_asset_work_ceiling_refuses_the_document() {
        let nested = br##"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10"><rect width="20" height="10" fill="#00ff00"/></svg>"##;
        let work = sanitize_default(nested).work_units;
        let limits = ResourceLimits::builder()
            .max_asset_work_units(work - 1)
            .build()
            .expect("work limit is valid");
        assert_eq!(
            asset_error(sanitize_with(nested, &limits, OutputFormat::Svg)),
            ResolveError::AssetWork { attempted: work }
        );
    }

    #[test]
    fn documents_that_are_not_svg_are_refused_as_decode_failures() {
        for bytes in [
            br#"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10"><rect>"#.as_slice(),
            br#"<svg width="20" height="10"><rect width="1" height="1"/></svg>"#.as_slice(),
            br#"<html xmlns="http://www.w3.org/1999/xhtml"><body/></html>"#.as_slice(),
            b"\xff\xfe not text".as_slice(),
        ] {
            assert_eq!(
                asset_error(sanitize_with(
                    bytes,
                    &ResourceLimits::default(),
                    OutputFormat::Svg
                )),
                ResolveError::SvgDecodeFailed,
                "{}",
                String::from_utf8_lossy(bytes)
            );
        }
    }

    #[test]
    fn effects_are_discovered_with_their_backdrop_dependence() {
        let isolated = sanitize_default(
            br##"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10"><defs><filter id="blur"><feGaussianBlur stdDeviation="1"/></filter></defs><rect width="20" height="10" filter="url(#blur)"/></svg>"##,
        );
        let [effect] = isolated.effects.as_slice() else {
            panic!("one effect: {:#?}", isolated.effects);
        };
        assert_eq!(effect.filter_id, "blur");
        assert!(!effect.unisolatable);
        assert!(!effect.causes.is_empty());

        let backdrop = sanitize_default(
            br##"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10"><defs><filter id="blend"><feBlend in="SourceGraphic" in2="BackgroundImage" mode="multiply"/></filter></defs><rect width="20" height="10" filter="url(#blend)"/></svg>"##,
        );
        assert!(backdrop.effects[0].unisolatable);

        let noop = sanitize_default(
            br##"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10"><defs><filter id="effect"><feGaussianBlur stdDeviation="0"/></filter></defs><rect width="20" height="10" filter="url(#effect)"/></svg>"##,
        );
        assert!(
            noop.effects.is_empty(),
            "a no-op filter is culled: {:#?}",
            noop.effects
        );
        assert!(
            !sanitized_text(&noop).contains("filter="),
            "and its reference dropped from the rect"
        );
    }

    #[test]
    fn the_pdf_format_labels_effect_nodes_for_closure_isolation() {
        let nested = br##"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10"><defs><filter id="blur"><feGaussianBlur stdDeviation="1"/></filter></defs><rect width="20" height="10" filter="url(#blur)"/></svg>"##;
        let pdf = sanitize_with(nested, &ResourceLimits::default(), OutputFormat::Pdf)
            .expect("the SVG sanitizes for PDF");
        let text = sanitized_text(&pdf);
        assert!(text.contains(r#"id="__excalidraw_effect_"#), "{text}");
        assert!(
            text.contains(r#"result="__excalidraw_filter_result_"#),
            "{text}"
        );
        let svg = sanitize_default(nested);
        assert!(!sanitized_text(&svg).contains("__excalidraw_"));
    }

    #[test]
    fn at_rules_are_removed_from_stylesheets_and_every_other_rule_kept() {
        for (sheet, kept) in [
            (
                "@import url(https://example.invalid/a.css); rect.a{fill:#e02020}",
                " rect.a{fill:#e02020}",
            ),
            (
                "rect.a{fill:#2020e0} @import url(https://example.invalid/a.css); rect.b{fill:#e02020}",
                "rect.a{fill:#2020e0}  rect.b{fill:#e02020}",
            ),
            (
                "@font-face{font-family:X;src:url(data:font/woff2;base64,AAAA)} rect.a{fill:#e02020}",
                " rect.a{fill:#e02020}",
            ),
        ] {
            let sanitized = sanitize_default(
                format!(
                    r#"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10"><style>{sheet}</style><rect class="a b" width="20" height="10"/></svg>"#
                )
                .as_bytes(),
            );
            let text = sanitized_text(&sanitized);
            assert!(text.contains(&format!("<style>{kept}</style>")), "{text}");
            // The partially removed stylesheet is reported at its node.
            let [removal] = sanitized.removals.as_slice() else {
                panic!("one removal: {:#?}", sanitized.removals);
            };
            assert_eq!(
                (removal.name.as_str(), removal.node_ordinal, removal.attribute_ordinal),
                ("style", 1, None)
            );
            assert!(removal.changes_appearance);
        }
        // An unadmitted ordinary rule still refuses the whole stylesheet.
        let sanitized = sanitize_default(
            br#"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10"><style>@keyframes k{from{fill:red}} rect{animation:k 1s}</style><rect width="20" height="10"/></svg>"#,
        );
        assert!(!sanitized_text(&sanitized).contains("<style"));
        assert_eq!(removal_names(&sanitized), ["style"]);
    }

    #[test]
    fn external_urls_resolve_as_a_missing_fragment_and_are_reported() {
        let sanitized = sanitize_default(
            br##"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10"><style>rect.a{fill:url(http://example.invalid/a.svg#g) #2020e0}</style><rect class="a" id="__excalidraw_missing" width="20" height="10" fill="url(http://example.invalid/a.svg#g) #2020e0" style="stroke:url('https://example.invalid/s')" mask="url(#local)"/></svg>"##,
        );
        let text = sanitized_text(&sanitized);
        assert!(!text.contains("example.invalid"), "{text}");
        // The fragment names no element, even one spelled like it.
        assert!(
            text.contains("<style>rect.a{fill:url(#__excalidraw_missing_1) #2020e0}</style>"),
            "{text}"
        );
        assert!(
            text.contains(r##"fill="url(#__excalidraw_missing_1) #2020e0""##),
            "{text}"
        );
        assert!(
            text.contains(r##"style="stroke:url(#__excalidraw_missing_1)""##),
            "{text}"
        );
        assert!(text.contains(r##"mask="url(#local)""##), "{text}");
        assert_eq!(
            sanitized
                .removals
                .iter()
                .map(|removal| (
                    removal.name.as_str(),
                    removal.node_ordinal,
                    removal.attribute_ordinal
                ))
                .collect::<Vec<_>>(),
            [
                ("style", 1, None),
                ("fill", 3, Some(4)),
                ("style", 3, Some(5))
            ]
        );
        // Anything else unsafe in the value still removes the attribute once.
        let removed = sanitize_default(
            br#"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10"><rect width="20" height="10" style="fill:url(https://example.invalid/p);width:expression(alert(1))"/></svg>"#,
        );
        assert!(!sanitized_text(&removed).contains("style="));
        assert_eq!(removal_names(&removed), ["style"]);
    }

    #[test]
    fn use_cycles_drop_the_closing_reference_in_source_order() {
        let sanitized = sanitize_default(
            br##"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10"><defs><g id="a"><rect width="4" height="4"/><use id="inner" href="#a"/></g><g id="b"><use id="first" href="#c"/></g><g id="c"><use id="second" href="#b"/></g></defs><use id="self" href="#self"/><use id="outer" href="#a"/><use href="#b"/></svg>"##,
        );
        let text = sanitized_text(&sanitized);
        for dropped in ["inner", "second", "self"] {
            assert!(text.contains(&format!(r#"<use id="{dropped}">"#)), "{text}");
        }
        assert!(text.contains(r##"<use id="first" href="#c">"##), "{text}");
        assert!(text.contains(r##"<use id="outer" href="#a">"##), "{text}");
        // Breaking a cycle removes nothing unsafe.
        assert!(sanitized.removals.is_empty());
        let tree = &sanitized.tree;
        assert!(tree.root().has_children());
    }

    #[test]
    fn rendering_reference_cycles_are_broken_and_the_rest_drawn() {
        // Three-element cycles the viewer library does not detect itself.
        let masks = sanitize_default(
            br#"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10"><mask id="a"><rect width="9" height="9" fill="white" mask="url(#b)"/></mask><mask id="b"><rect width="9" height="9" fill="white" mask="url(#c)"/></mask><mask id="c"><rect width="9" height="9" fill="white" mask="url(#a)"/></mask><rect width="20" height="10" mask="url(#a)"/></svg>"#,
        );
        let text = sanitized_text(&masks);
        assert_eq!(
            text.matches("url(#__excalidraw_missing)").count(),
            1,
            "{text}"
        );
        assert!(text.contains(r#"fill="white" mask="url(#__excalidraw_missing)""#));
        assert!(text.contains(r#"<rect width="20" height="10" mask="url(#a)">"#));

        // Pattern content inheriting its own pattern overrides the inherited
        // value on the closing element.
        let inherited = sanitize_default(
            br#"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10"><g fill="url(#p) red"><pattern id="p" width="4" height="4" patternUnits="userSpaceOnUse"><rect width="4" height="4"/></pattern><rect width="20" height="10"/></g></svg>"#,
        );
        let text = sanitized_text(&inherited);
        assert!(
            text.contains(r#"<rect width="4" height="4" fill="url(#__excalidraw_missing) red">"#),
            "{text}"
        );
        assert!(text.contains(r#"<g fill="url(#p) red">"#), "{text}");

        let filters = sanitize_default(
            br##"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10"><defs><filter id="a"><feImage href="#r1"/></filter><filter id="b"><feImage href="#r2"/></filter></defs><rect id="r1" width="9" height="9" filter="url(#b)"/><rect id="r2" width="9" height="9" filter="url(#a)"/></svg>"##,
        );
        let text = sanitized_text(&filters);
        assert!(text.contains(r##"<feImage href="#r2">"##), "{text}");
        assert!(text.contains("<feImage>"), "{text}");
    }

    /// The sanitized document's pixels as the viewer library draws them.
    fn drawn(sanitized: &SanitizedSvg) -> Vec<u8> {
        let size = sanitized.tree.size().to_int_size();
        let mut pixmap = resvg::tiny_skia::Pixmap::new(size.width(), size.height())
            .expect("the drawing has an area");
        resvg::render(
            &sanitized.tree,
            resvg::tiny_skia::Transform::default(),
            &mut pixmap.as_mut(),
        );
        pixmap.data().to_vec()
    }

    /// Sanitizes a stylesheet-authored cycle twice and its attribute-authored
    /// twin once: the result is deterministic, draws as the twin draws, and
    /// breaking the cycle is not reported as a removal.
    fn assert_breaks_like_twin(styled: &[u8], twin: &[u8]) -> SanitizedSvg {
        let first = sanitize_default(styled);
        let second = sanitize_default(styled);
        assert_eq!(first.bytes, second.bytes);
        assert!(first.removals.is_empty(), "{:?}", removal_names(&first));
        let twin = sanitize_default(twin);
        assert_eq!(
            drawn(&first),
            drawn(&twin),
            "{}\n{}",
            sanitized_text(&first),
            sanitized_text(&twin)
        );
        first
    }

    #[test]
    fn stylesheet_mask_cycles_break_like_attribute_cycles() {
        let three = assert_breaks_like_twin(
            br#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24"><style>.a{mask:url(#b)} .b{mask:url(#c)} .c{mask:url(#a)}</style><defs><mask id="a"><rect class="a" width="24" height="24" fill="white"/></mask><mask id="b"><rect class="b" width="24" height="24" fill="white"/></mask><mask id="c"><rect class="c" width="24" height="24" fill="white"/></mask></defs><rect width="24" height="24" fill="red" mask="url(#a)"/></svg>"#,
            br#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24"><defs><mask id="a"><rect mask="url(#b)" width="24" height="24" fill="white"/></mask><mask id="b"><rect mask="url(#c)" width="24" height="24" fill="white"/></mask><mask id="c"><rect mask="url(#a)" width="24" height="24" fill="white"/></mask></defs><rect width="24" height="24" fill="red" mask="url(#a)"/></svg>"#,
        );
        let text = sanitized_text(&three);
        // The rule stays; the closing element overrides it in its `style`.
        assert!(
            text.contains(".a{mask:url(#b)} .b{mask:url(#c)} .c{mask:url(#a)}"),
            "{text}"
        );
        assert_eq!(text.matches("__excalidraw_missing").count(), 1, "{text}");
        assert!(
            text.contains(r#"<rect class="c" width="24" height="24" fill="white" style="mask:url(#__excalidraw_missing);">"#),
            "{text}"
        );
        // The red square is drawn through masks whose content is unmasked.
        let pixels = drawn(&three);
        assert_eq!(&pixels[..4], &[255, 0, 0, 255]);

        let two = assert_breaks_like_twin(
            br#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24"><style>.a{clip-path:url(#b)} .b{clip-path:url(#a)}</style><defs><clipPath id="a"><rect class="a" width="12" height="24"/></clipPath><clipPath id="b"><rect class="b" width="24" height="12"/></clipPath></defs><rect width="24" height="24" fill="red" clip-path="url(#a)"/></svg>"#,
            br#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24"><defs><clipPath id="a"><rect clip-path="url(#b)" width="12" height="24"/></clipPath><clipPath id="b"><rect clip-path="url(#a)" width="24" height="12"/></clipPath></defs><rect width="24" height="24" fill="red" clip-path="url(#a)"/></svg>"#,
        );
        let text = sanitized_text(&two);
        assert!(
            text.contains(r#"<rect class="b" width="24" height="12" style="clip-path:url(#__excalidraw_missing);">"#),
            "{text}"
        );
    }

    #[test]
    fn a_rule_closing_a_filter_on_itself_breaks_like_its_attribute() {
        let sanitized = assert_breaks_like_twin(
            br##"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24"><style>.x{filter:url(#f)}</style><defs><filter id="f"><feImage href="#r"/></filter></defs><rect width="24" height="24" fill="blue"/><rect id="r" class="x" width="12" height="12" fill="red"/></svg>"##,
            br##"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24"><defs><filter id="f"><feImage href="#r"/></filter></defs><rect width="24" height="24" fill="blue"/><rect id="r" filter="url(#f)" width="12" height="12" fill="red"/></svg>"##,
        );
        // Reached first through the rule, the `feImage` link closes it.
        let text = sanitized_text(&sanitized);
        assert!(text.contains("<feImage></feImage>"), "{text}");
    }

    #[test]
    fn a_value_inherited_from_a_rule_styled_ancestor_is_overridden_on_the_closing_element() {
        let sanitized = assert_breaks_like_twin(
            br#"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10"><style>.p{fill:url(#p) red}</style><g class="p"><pattern id="p" width="4" height="4" patternUnits="userSpaceOnUse"><rect width="2" height="2"/></pattern><rect width="20" height="10"/></g></svg>"#,
            br#"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10"><g fill="url(#p) red"><pattern id="p" width="4" height="4" patternUnits="userSpaceOnUse"><rect width="2" height="2"/></pattern><rect width="20" height="10"/></g></svg>"#,
        );
        let text = sanitized_text(&sanitized);
        assert!(
            text.contains(r#"<rect width="2" height="2" fill="url(#__excalidraw_missing) red">"#),
            "{text}"
        );
        assert!(text.contains(r#"<g class="p">"#), "{text}");
    }

    #[test]
    fn use_of_a_rule_styled_element_breaks_like_its_attribute() {
        let sanitized = assert_breaks_like_twin(
            br##"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24"><style>.m{mask:url(#m)}</style><defs><mask id="m"><rect width="24" height="24" fill="white"/><use href="#r"/></mask></defs><rect width="24" height="24" fill="blue"/><rect id="r" class="m" width="12" height="12" fill="red"/></svg>"##,
            br##"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24"><defs><mask id="m"><rect width="24" height="24" fill="white"/><use href="#r"/></mask></defs><rect width="24" height="24" fill="blue"/><rect id="r" mask="url(#m)" width="12" height="12" fill="red"/></svg>"##,
        );
        let text = sanitized_text(&sanitized);
        assert!(text.contains("<use></use>"), "{text}");
        let pixels = drawn(&sanitized);
        assert_eq!(&pixels[..4], &[255, 0, 0, 255]);
    }

    #[test]
    fn important_rules_closing_a_cycle_keep_every_other_value() {
        // The `!important` rule beats the inline `style` and the more specific
        // rule; only the closing element's value changes.
        let sanitized = assert_breaks_like_twin(
            br#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24"><style>.a{mask:url(#b)!important} .b{mask:url(#a) !important} rect.b{mask:none}</style><defs><mask id="a"><rect class="a" style="mask:none" width="24" height="24" fill="white"/></mask><mask id="b"><rect class="b" width="24" height="24" fill="white"/></mask></defs><rect width="24" height="24" fill="red" mask="url(#a)"/><rect class="b" width="4" height="4" fill="blue"/></svg>"#,
            br#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24"><defs><mask id="a"><rect mask="url(#b)" width="24" height="24" fill="white"/></mask><mask id="b"><rect width="24" height="24" fill="white"/></mask></defs><rect width="24" height="24" fill="red" mask="url(#a)"/><rect mask="url(#a)" width="4" height="4" fill="blue"/></svg>"#,
        );
        let text = sanitized_text(&sanitized);
        assert!(text.contains(".a{} .b{} rect.b{mask:none}"), "{text}");
        assert!(text.contains(r#"style="mask:url(#b);""#), "{text}");
        assert!(
            text.contains(r#"fill="blue" style="mask:url(#a);""#),
            "{text}"
        );
        assert_eq!(text.matches("__excalidraw_missing").count(), 1, "{text}");
    }

    #[test]
    fn a_cycle_the_plan_cannot_see_is_refused_before_the_viewer_library() {
        // PDF marks a filtered element with an id, which the `[id]` selector
        // then matches in the sanitized document only: a mask cycle the viewer
        // library would recurse through without bound.
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24"><style>rect[id]{mask:url(#m)}</style><mask id="m"><rect filter="none" width="24" height="24" fill="white"/></mask><rect width="24" height="24" fill="red" mask="url(#m)"/></svg>"#;
        sanitize_default(svg);
        assert_eq!(
            asset_error(sanitize_with(
                svg,
                &ResourceLimits::default(),
                OutputFormat::Pdf
            )),
            ResolveError::SvgDecodeFailed
        );
    }

    #[test]
    fn gradient_chains_without_stops_paint_nothing() {
        let sanitized = sanitize_default(
            br##"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10"><defs><linearGradient id="a" href="#b"/><linearGradient id="b" href="#c"/><linearGradient id="c" href="#b"/><linearGradient id="d" href="#e"/><linearGradient id="e"><stop stop-color="red"/></linearGradient></defs><rect width="20" height="10" fill="url(#a) blue"/><rect width="20" height="10" fill="url(#d) blue"/></svg>"##,
        );
        let text = sanitized_text(&sanitized);
        // The closing link of the chain is dropped.
        assert!(text.contains(r#"<linearGradient id="c">"#), "{text}");
        for stopless in ["a", "b", "c"] {
            assert!(
                text.contains(&format!(r#"id="{stopless}"#))
                    && text
                        .split(&format!(r#"id="{stopless}""#))
                        .nth(1)
                        .is_some_and(|rest| rest.split_once('>').is_some_and(
                            |(_, after)| after.starts_with(r#"<stop stop-opacity="0"/>"#)
                        )),
                "{stopless}: {text}"
            );
        }
        assert_eq!(text.matches(r#"stop-opacity="0""#).count(), 3, "{text}");
    }

    #[test]
    fn non_base64_data_urls_are_nested_payloads_like_base64_ones() {
        let child = r##"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10"><rect width="20" height="10" fill="#00ff00"/><script>alert(1)</script></svg>"##;
        let percent = child
            .replace('%', "%25")
            .replace('#', "%23")
            .replace('<', "&lt;")
            .replace('"', "&quot;");
        let base64_encoded =
            sanitize_default(svg_with_nested_data_image(child.as_bytes()).as_bytes());
        for header in [
            "data:image/svg+xml,",
            "data:image/svg+xml;utf8,",
            "data:image/svg+xml;charset=utf-8,",
        ] {
            let sanitized = sanitize_default(
                format!(
                    r#"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10"><image width="20" height="10" href="{header}{percent}"/></svg>"#
                )
                .as_bytes(),
            );
            // Recursively sanitized and re-embedded in canonical base64.
            assert_eq!(removal_names(&sanitized), ["script"]);
            assert!(sanitized.media_claims.is_empty(), "{header}");
            let (_, nested) = sanitized_text(&sanitized)
                .split_once("data:image/svg+xml;base64,")
                .expect("the nested SVG is re-embedded as base64");
            let (_, expected) = sanitized_text(&base64_encoded)
                .split_once("data:image/svg+xml;base64,")
                .expect("the nested SVG is re-embedded as base64");
            assert_eq!(nested, expected);
            // One occurrence of the same decoded octets, and the same work.
            let occurrences = |sanitized: &SanitizedSvg| {
                sanitized
                    .nested_admissions
                    .iter()
                    .filter_map(|admission| match admission {
                        NestedAdmission::Occurrence { asset_bytes } => Some(*asset_bytes),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
            };
            assert_eq!(occurrences(&sanitized), [child.len() as u64]);
            assert_eq!(occurrences(&sanitized), occurrences(&base64_encoded));
        }
        let png = base64::engine::general_purpose::STANDARD
            .decode(PNG_BASE64)
            .expect("fixture PNG");
        let percent_png = png
            .iter()
            .map(|byte| format!("%{byte:02X}"))
            .collect::<String>();
        let sanitized = sanitize_default(
            format!(
                r#"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10"><image width="20" height="10" href="data:image/png,{percent_png}"/></svg>"#
            )
            .as_bytes(),
        );
        assert!(sanitized_text(&sanitized).contains(&format!("data:image/png;base64,{PNG_BASE64}")));
        assert!(sanitized.failures.is_empty());
        // The depth ceiling applies alike.
        let limits = ResourceLimits::builder()
            .max_nested_asset_depth(1)
            .build()
            .expect("depth limit is valid");
        let middle = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10"><image width="20" height="10" href="data:image/svg+xml,{percent}"/></svg>"#
        );
        let refused = sanitize_with(
            svg_with_nested_data_image(middle.as_bytes()).as_bytes(),
            &limits,
            OutputFormat::Svg,
        )
        .expect("the parent survives");
        let [failure] = refused.failures.as_slice() else {
            panic!("one depth failure: {:#?}", refused.failures);
        };
        assert_eq!(failure.error, ResolveError::SvgDepth { attempted: 2 });
        assert_eq!(failure.path.len(), 2);
    }

    #[test]
    fn each_nested_reference_is_charged_once() {
        let work = |filter: &str| {
            let bytes = format!(
                r##"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10"><defs><filter id="a"><feOffset dx="1"/></filter><filter id="b"><feOffset dx="1"/></filter></defs><rect width="20" height="10" filter="{filter}"/></svg>"##
            );
            (
                sanitize_default(bytes.as_bytes()).work_units,
                bytes.len() as u64,
            )
        };
        let (one, one_len) = work("url(#a)");
        let (two, two_len) = work("url(#a) url(#b)");
        assert_eq!(two - two_len, one - one_len + 16);
    }
}
