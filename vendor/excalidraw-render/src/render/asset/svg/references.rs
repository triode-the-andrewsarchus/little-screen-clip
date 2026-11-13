//! Reference rewriting that keeps a sanitized document static, local and
//! finite before the viewer library follows any reference. It is planned once
//! against the parsed source and applied by serialization (and, for PDF, by
//! stylesheet lowering), so every consumer sees the same document.
//!
//! - A non-fragment `url(...)` in a presentation attribute, a `style`
//!   attribute or a stylesheet becomes a reference to a fragment no element
//!   has, so it resolves exactly as the reference resolves a missing fragment
//!   (a paint falls back to its fallback, else `none`). Its removal is still
//!   reported.
//! - Reference cycles are detected in source order and the edge closing each
//!   is removed; everything else is drawn. `use` instantiation is checked over
//!   the whole document, since instances are built everywhere. Rendering
//!   references (`clip-path`, `mask`, `filter`, paint, markers, `feImage`, and
//!   gradient/pattern/filter `href` chains) are checked from the root through
//!   what is rendered and what it references. A closing `href` is dropped; a
//!   closing `url(#id)` becomes a reference to the missing fragment, on the
//!   element that inherits it when the declaration is an ancestor's.
//!   Stylesheet rules are part of this graph: each element's value is its
//!   cascade as the viewer library computes it (presentation attribute, then
//!   matching rules by specificity and source order, then `style`, the
//!   first `!important` declaration winning). A closing reference a rule
//!   declares is overridden in the closing element's `style`; when an
//!   `!important` rule declares it, the property's `!important` rule
//!   declarations are removed and every element they matched keeps its value
//!   in its own `style`, so only the closing reference changes.
//! - A gradient whose `href` chain has no stop paints nothing, as the
//!   reference paints it, rather than its fallback: it receives one
//!   transparent stop.
use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};

use super::{is_allowed_svg_node, rewrite_nonlocal_urls, rewrite_stylesheet, ResolutionFailure};

const SVG: &str = "http://www.w3.org/2000/svg";
const XLINK: &str = "http://www.w3.org/1999/xlink";

pub(super) struct AttributeRewrite {
    /// `None` drops the attribute.
    pub(super) value: Option<String>,
    /// The rewrite removed a non-fragment URL, which is reported.
    pub(super) external: bool,
}

#[derive(Default)]
pub(super) struct Plan {
    missing: String,
    attributes: BTreeMap<(usize, usize), AttributeRewrite>,
    added: BTreeMap<usize, Vec<(&'static str, String)>>,
    stylesheets: BTreeMap<usize, String>,
    /// Stylesheets whose rewriting removed something, which is reported.
    reported: BTreeSet<usize>,
    stops: BTreeSet<usize>,
    cascade: Cascade,
}

/// The rendering-property declarations of the admitted stylesheets and the
/// elements each matches, in the viewer library's cascade order.
#[derive(Default)]
struct Cascade {
    /// Each admitted stylesheet's key and its text before cycle breaking.
    sheets: Vec<(usize, String)>,
    declarations: Vec<RuleDeclaration>,
    /// Per element key, its matching declarations in cascade order.
    matched: BTreeMap<usize, Vec<usize>>,
}

struct RuleDeclaration {
    sheet: usize,
    /// The declaration's bytes in its sheet, its separator included.
    range: (usize, usize),
    property: Property,
    value: String,
    important: bool,
    stripped: bool,
}

/// Where an element's value of a property comes from.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Source {
    Presentation,
    Rule { important: bool },
    Style,
}

impl Plan {
    pub(super) fn new(document: &roxmltree::Document<'_>) -> Result<Self, ResolutionFailure> {
        let mut plan = Self {
            missing: missing_fragment(document)?,
            ..Self::default()
        };
        for node in document.descendants().filter(|node| node.is_element()) {
            let key = node.range().start;
            for (index, attribute) in node.attributes().enumerate() {
                if matches!(
                    attribute.name(),
                    "href" | "xlink:href" | "src" | "action" | "formaction"
                ) {
                    continue;
                }
                if let Some(value) = rewrite_nonlocal_urls(attribute.value(), &plan.missing)? {
                    plan.attributes.insert(
                        (key, index),
                        AttributeRewrite {
                            value: Some(value),
                            external: true,
                        },
                    );
                }
            }
            if node.tag_name().name() == "style" && node.tag_name().namespace() == Some(SVG) {
                let mut children = node.children();
                if let (Some(text), None) = (children.next(), children.next()) {
                    if let Some((Cow::Owned(sheet), true)) = text
                        .text()
                        .and_then(|text| rewrite_stylesheet(text, &plan.missing))
                    {
                        plan.stylesheets.insert(key, sheet);
                        plan.reported.insert(key);
                    }
                }
            }
        }
        let graph = Graph::new(document)?;
        plan.cascade = Cascade::new(&graph, &plan.stylesheets)?;
        let breaks = graph.cycles(&plan, Pass::Instances)?;
        plan.apply(&graph, breaks)?;
        let breaks = graph.cycles(&plan, Pass::Rendering)?;
        plan.apply(&graph, breaks)?;
        plan.stops = graph.stopless_gradients(&plan)?;
        plan.strip_stylesheets()?;
        Ok(plan)
    }

    /// Whether `document`, as the viewer library would read it, still has a
    /// reference cycle. The sanitized document is checked with this before
    /// the viewer library parses it: a cycle the plan did not break (for
    /// example through a selector whose match rewriting changed) would
    /// otherwise recurse without bound there, so the image is refused instead.
    pub(super) fn residual_cycle(
        document: &roxmltree::Document<'_>,
    ) -> Result<bool, ResolutionFailure> {
        let graph = Graph::new(document)?;
        let plan = Self {
            cascade: Cascade::new(&graph, &BTreeMap::new())?,
            ..Self::default()
        };
        Ok(!graph.cycles(&plan, Pass::Instances)?.is_empty()
            || !graph.cycles(&plan, Pass::Rendering)?.is_empty())
    }

    /// Records the stylesheets whose `!important` declarations were removed.
    fn strip_stylesheets(&mut self) -> Result<(), ResolutionFailure> {
        for (sheet, (key, text)) in self.cascade.sheets.iter().enumerate() {
            let mut ranges = self
                .cascade
                .declarations
                .iter()
                .filter(|declaration| declaration.sheet == sheet && declaration.stripped)
                .map(|declaration| declaration.range)
                .collect::<Vec<_>>();
            if ranges.is_empty() {
                continue;
            }
            ranges.sort_unstable();
            ranges.dedup();
            let mut kept = String::new();
            kept.try_reserve(text.len())
                .map_err(|_| ResolutionFailure::Memory)?;
            let mut copied = 0;
            for (start, end) in ranges {
                if start >= copied {
                    kept.push_str(&text[copied..start]);
                    copied = end;
                }
            }
            kept.push_str(&text[copied..]);
            self.stylesheets.insert(*key, kept);
        }
        Ok(())
    }

    /// The element's `style` after rewriting: its attribute's index (`None`
    /// when the plan adds it) and value.
    fn style<'v>(&'v self, node: roxmltree::Node<'v, 'v>) -> Option<(Option<usize>, &'v str)> {
        if let Some(index) = node
            .attributes()
            .position(|a| a.name() == "style" && a.namespace().is_none())
        {
            let raw = node.attributes().nth(index)?.value();
            return self
                .value(node, index, raw)
                .map(|value| (Some(index), value));
        }
        self.added(node)
            .iter()
            .find(|(name, _)| *name == "style")
            .map(|(_, value)| (None, value.as_str()))
    }

    fn set_style(
        &mut self,
        node: roxmltree::Node<'_, '_>,
        index: Option<usize>,
        value: String,
    ) -> Result<(), ResolutionFailure> {
        let key = node.range().start;
        match index {
            Some(index) => {
                let external = self
                    .attribute(node, index)
                    .is_some_and(|rewrite| rewrite.external);
                self.attributes.insert(
                    (key, index),
                    AttributeRewrite {
                        value: Some(value),
                        external,
                    },
                );
            }
            None => {
                let added = self.added.entry(key).or_default();
                added.retain(|(name, _)| *name != "style");
                added
                    .try_reserve(1)
                    .map_err(|_| ResolutionFailure::Memory)?;
                added.push(("style", value));
            }
        }
        Ok(())
    }

    /// Declares `values` in the element's own `style`, replacing its
    /// declarations of any of `group`, so they win over non-`!important`
    /// rules.
    fn declare_in_style(
        &mut self,
        node: roxmltree::Node<'_, '_>,
        group: &[Property],
        values: &[(Property, String)],
    ) -> Result<(), ResolutionFailure> {
        let (index, current) = match self.style(node) {
            Some((index, current)) => (index, current),
            None => (
                node.attributes()
                    .position(|a| a.name() == "style" && a.namespace().is_none()),
                "",
            ),
        };
        let mut style = String::new();
        style
            .try_reserve(
                current
                    .len()
                    .saturating_add(values.iter().map(|(_, v)| v.len() + 16).sum()),
            )
            .map_err(|_| ResolutionFailure::Memory)?;
        for declaration in current.split(';') {
            let declares = declaration
                .split_once(':')
                .is_some_and(|(name, _)| group.iter().any(|p| p.declared_by(name)));
            if declares || declaration.trim().is_empty() {
                continue;
            }
            style.push_str(declaration);
            style.push(';');
        }
        for (property, value) in values {
            style.push_str(property.name());
            style.push(':');
            style.push_str(value);
            style.push(';');
        }
        self.set_style(node, index, style)
    }

    /// Removes every `!important` rule declaration of `group`; each element
    /// one matched keeps its value in its own `style`.
    fn strip_important(
        &mut self,
        graph: &Graph<'_, '_>,
        group: &[Property],
    ) -> Result<(), ResolutionFailure> {
        let stripped = self
            .cascade
            .declarations
            .iter()
            .enumerate()
            .filter(|(_, d)| !d.stripped && d.important && group.contains(&d.property))
            .map(|(index, _)| index)
            .collect::<BTreeSet<_>>();
        if stripped.is_empty() {
            return Ok(());
        }
        let mut kept = Vec::new();
        for (element, node) in graph.elements.iter().enumerate() {
            let matched = self
                .cascade
                .matched
                .get(&node.range().start)
                .is_some_and(|matched| matched.iter().any(|d| stripped.contains(d)));
            if !matched {
                continue;
            }
            let mut values = Vec::new();
            for property in group {
                if let Some((value, _)) = graph.winner(self, element, *property) {
                    values.push((*property, value));
                }
            }
            kept.try_reserve(1).map_err(|_| ResolutionFailure::Memory)?;
            kept.push((*node, values));
        }
        // Declarations sharing bytes (a `marker` shorthand) go together.
        let ranges = stripped
            .iter()
            .map(|&d| {
                let declaration = &self.cascade.declarations[d];
                (declaration.sheet, declaration.range)
            })
            .collect::<BTreeSet<_>>();
        for declaration in &mut self.cascade.declarations {
            if ranges.contains(&(declaration.sheet, declaration.range)) {
                declaration.stripped = true;
            }
        }
        for (node, values) in kept {
            self.declare_in_style(node, group, &values)?;
        }
        Ok(())
    }

    pub(super) fn attribute(
        &self,
        node: roxmltree::Node<'_, '_>,
        index: usize,
    ) -> Option<&AttributeRewrite> {
        self.attributes.get(&(node.range().start, index))
    }

    /// The attribute's value after rewriting; `None` when it is dropped.
    pub(super) fn value<'v>(
        &'v self,
        node: roxmltree::Node<'_, '_>,
        index: usize,
        raw: &'v str,
    ) -> Option<&'v str> {
        match self.attribute(node, index) {
            Some(rewrite) => rewrite.value.as_deref(),
            None => Some(raw),
        }
    }

    pub(super) fn added(&self, node: roxmltree::Node<'_, '_>) -> &[(&'static str, String)] {
        self.added
            .get(&node.range().start)
            .map_or(&[], Vec::as_slice)
    }

    /// The rewritten text of an admitted stylesheet, when rewriting changed it.
    pub(super) fn stylesheet(&self, node: roxmltree::Node<'_, '_>) -> Option<&str> {
        self.stylesheets
            .get(&node.range().start)
            .map(String::as_str)
    }

    /// Whether rewriting removed something from the stylesheet, which is
    /// reported. Breaking a reference cycle is not.
    pub(super) fn stylesheet_changed(&self, node: roxmltree::Node<'_, '_>) -> bool {
        self.reported.contains(&node.range().start)
    }

    pub(super) fn needs_stop(&self, node: roxmltree::Node<'_, '_>) -> bool {
        self.stops.contains(&node.range().start)
    }

    fn apply(&mut self, graph: &Graph<'_, '_>, breaks: Vec<Edge>) -> Result<(), ResolutionFailure> {
        for edge in breaks {
            let from = graph.elements[edge.from];
            let Some(id) = graph.elements[edge.target].attribute("id") else {
                continue;
            };
            let key = from.range().start;
            if edge.property == Property::Href {
                if let Some(index) = effective_href(from) {
                    self.attributes.insert(
                        (key, index),
                        AttributeRewrite {
                            value: None,
                            external: false,
                        },
                    );
                }
                continue;
            }
            let group = edge.property.group();
            let winner = graph.winner(self, edge.from, edge.property);
            let own = winner
                .as_ref()
                .filter(|(value, _)| !is_inherit(value))
                .map(|(_, source)| *source);
            match own {
                Some(Source::Rule { important }) => {
                    // A rule cannot be rewritten for one element: the element
                    // overrides it in its own `style`.
                    if important {
                        self.strip_important(graph, group)?;
                    }
                    let values = graph.group_values(self, edge.from, group, edge.property, id)?;
                    self.declare_in_style(from, group, &values)?;
                }
                Some(Source::Presentation | Source::Style) => {
                    let name = edge.property.name();
                    for (index, attribute) in from.attributes().enumerate() {
                        if attribute.name() != name || attribute.namespace().is_some() {
                            continue;
                        }
                        let Some(value) = self.value(from, index, attribute.value()) else {
                            continue;
                        };
                        let value = replace_local_url(value, id, &self.missing)?;
                        let external = self
                            .attribute(from, index)
                            .is_some_and(|rewrite| rewrite.external);
                        self.attributes.insert(
                            (key, index),
                            AttributeRewrite {
                                value: Some(value),
                                external,
                            },
                        );
                    }
                    if let Some((index, value)) = self.style(from) {
                        let mut changed = false;
                        let mut style = String::new();
                        style
                            .try_reserve(value.len().saturating_add(self.missing.len()))
                            .map_err(|_| ResolutionFailure::Memory)?;
                        for (ordinal, declaration) in value.split(';').enumerate() {
                            if ordinal > 0 {
                                style.push(';');
                            }
                            match declaration.split_once(':') {
                                Some((property, declared))
                                    if edge.property.declared_by(property) =>
                                {
                                    changed = true;
                                    style.push_str(property);
                                    style.push(':');
                                    style.push_str(&replace_local_url(
                                        declared,
                                        id,
                                        &self.missing,
                                    )?);
                                }
                                _ => style.push_str(declaration),
                            }
                        }
                        if changed {
                            self.set_style(from, index, style)?;
                        }
                    }
                }
                None if winner.is_some() => {
                    // Its own `inherit`: override it in its own `style`.
                    let values = graph.group_values(self, edge.from, group, edge.property, id)?;
                    self.declare_in_style(from, group, &values)?;
                }
                None => {
                    // Inherited: the closing element overrides its ancestor's
                    // value.
                    let name = edge.property.name();
                    let Some((_, value)) = graph.inherited(self, edge.from, edge.property) else {
                        continue;
                    };
                    let value = replace_local_url(&value, id, &self.missing)?;
                    let added = self.added.entry(key).or_default();
                    if !added.iter().any(|(existing, _)| *existing == name) {
                        added
                            .try_reserve(1)
                            .map_err(|_| ResolutionFailure::Memory)?;
                        added.push((name, value));
                    }
                }
            }
        }
        Ok(())
    }
}

fn missing_fragment(document: &roxmltree::Document<'_>) -> Result<String, ResolutionFailure> {
    let ids = document
        .descendants()
        .filter_map(|node| node.attribute("id"))
        .collect::<BTreeSet<_>>();
    let mut missing = super::super::try_owned("__excalidraw_missing")?;
    let mut suffix = 0_u64;
    while ids.contains(missing.as_str()) {
        suffix = suffix.saturating_add(1);
        missing = format!("__excalidraw_missing_{suffix}");
    }
    Ok(missing)
}

/// `value` with each `url(#id)` naming `id` replaced by the missing fragment.
fn replace_local_url(value: &str, id: &str, missing: &str) -> Result<String, ResolutionFailure> {
    let mut output = String::new();
    output
        .try_reserve(value.len().saturating_add(missing.len()))
        .map_err(|_| ResolutionFailure::Memory)?;
    let mut copied = 0;
    for (start, end, target) in super::url_tokens(value) {
        if target.strip_prefix('#') == Some(id) {
            output.push_str(&value[copied..start]);
            output.push_str("url(#");
            output.push_str(missing);
            output.push(')');
            copied = end;
        }
    }
    output.push_str(&value[copied..]);
    Ok(output)
}

fn is_inherit(value: &str) -> bool {
    value.trim().eq_ignore_ascii_case("inherit")
}

impl Cascade {
    /// The rendering-property declarations of the admitted stylesheets (their
    /// `rewritten` text where rewriting changed it), parsed and ordered as
    /// the viewer library orders them, and the retained elements each
    /// matches.
    fn new(
        graph: &Graph<'_, '_>,
        rewritten: &BTreeMap<usize, String>,
    ) -> Result<Self, ResolutionFailure> {
        let mut cascade = Self::default();
        for node in &graph.elements {
            if node.tag_name().name() != "style"
                || node
                    .attribute("type")
                    .is_some_and(|kind| kind != "text/css")
            {
                continue;
            }
            let key = node.range().start;
            let Some(text) = rewritten
                .get(&key)
                .map(String::as_str)
                .or_else(|| node.text())
            else {
                continue;
            };
            cascade
                .sheets
                .try_reserve(1)
                .map_err(|_| ResolutionFailure::Memory)?;
            cascade.sheets.push((key, super::super::try_owned(text)?));
        }
        if cascade.sheets.is_empty() {
            return Ok(cascade);
        }
        let mut sheet = simplecss::StyleSheet::new();
        for (_, text) in &cascade.sheets {
            sheet.parse_more(text);
        }
        let mut rules = Vec::new();
        for rule in &sheet.rules {
            let mut indices = Vec::new();
            for declaration in &rule.declarations {
                let properties: &[Property] = if declaration.name == "marker" {
                    Property::MarkerStart.group()
                } else if let Some(property) = Property::RENDERING
                    .iter()
                    .find(|property| property.name() == declaration.name)
                {
                    std::slice::from_ref(property)
                } else {
                    continue;
                };
                let Some((sheet, range)) = locate(&cascade.sheets, declaration) else {
                    continue;
                };
                for property in properties {
                    indices
                        .try_reserve(1)
                        .map_err(|_| ResolutionFailure::Memory)?;
                    indices.push(cascade.declarations.len());
                    cascade
                        .declarations
                        .try_reserve(1)
                        .map_err(|_| ResolutionFailure::Memory)?;
                    cascade.declarations.push(RuleDeclaration {
                        sheet,
                        range,
                        property: *property,
                        value: super::super::try_owned(declaration.value)?,
                        important: declaration.important,
                        stripped: false,
                    });
                }
            }
            if !indices.is_empty() {
                rules
                    .try_reserve(1)
                    .map_err(|_| ResolutionFailure::Memory)?;
                rules.push((&rule.selector, indices));
            }
        }
        for node in &graph.elements {
            let mut matched = Vec::new();
            for (selector, indices) in &rules {
                if selector.matches(&super::css::Element(*node)) {
                    matched
                        .try_reserve(indices.len())
                        .map_err(|_| ResolutionFailure::Memory)?;
                    matched.extend_from_slice(indices);
                }
            }
            if !matched.is_empty() {
                cascade.matched.insert(node.range().start, matched);
            }
        }
        Ok(cascade)
    }
}

/// The sheet holding `declaration` and its bytes there: from its name (and
/// any `*` hack before it) through its `;`, or to the rule's `}`.
fn locate(
    sheets: &[(usize, String)],
    declaration: &simplecss::Declaration<'_>,
) -> Option<(usize, (usize, usize))> {
    let name = declaration.name.as_ptr() as usize;
    let value = declaration.value.as_ptr() as usize;
    sheets.iter().enumerate().find_map(|(index, (_, text))| {
        let base = text.as_ptr() as usize;
        let mut start = name.checked_sub(base).filter(|start| *start < text.len())?;
        let value_end = value
            .checked_sub(base)?
            .checked_add(declaration.value.len())
            .filter(|end| *end <= text.len())?;
        if start > 0 && text.as_bytes()[start - 1] == b'*' {
            start -= 1;
        }
        let end = match text[value_end..].find([';', '}']) {
            Some(offset) if text.as_bytes()[value_end + offset] == b';' => value_end + offset + 1,
            Some(offset) => value_end + offset,
            None => text.len(),
        };
        Some((index, (start, end)))
    })
}

fn effective_href(node: roxmltree::Node<'_, '_>) -> Option<usize> {
    node.attributes()
        .position(|a| a.name() == "href" && a.namespace().is_none())
        .or_else(|| {
            node.attributes()
                .position(|a| a.name() == "href" && a.namespace() == Some(XLINK))
        })
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Property {
    ClipPath,
    Mask,
    Filter,
    Fill,
    Stroke,
    MarkerStart,
    MarkerMid,
    MarkerEnd,
    Href,
}

impl Property {
    const RENDERING: [Self; 8] = [
        Self::ClipPath,
        Self::Mask,
        Self::Filter,
        Self::Fill,
        Self::Stroke,
        Self::MarkerStart,
        Self::MarkerMid,
        Self::MarkerEnd,
    ];

    fn name(self) -> &'static str {
        match self {
            Self::ClipPath => "clip-path",
            Self::Mask => "mask",
            Self::Filter => "filter",
            Self::Fill => "fill",
            Self::Stroke => "stroke",
            Self::MarkerStart => "marker-start",
            Self::MarkerMid => "marker-mid",
            Self::MarkerEnd => "marker-end",
            Self::Href => "href",
        }
    }

    /// The properties one declaration may set together.
    fn group(self) -> &'static [Self] {
        match self {
            Self::ClipPath => &[Self::ClipPath],
            Self::Mask => &[Self::Mask],
            Self::Filter => &[Self::Filter],
            Self::Fill => &[Self::Fill],
            Self::Stroke => &[Self::Stroke],
            Self::MarkerStart | Self::MarkerMid | Self::MarkerEnd => {
                &[Self::MarkerStart, Self::MarkerMid, Self::MarkerEnd]
            }
            Self::Href => &[Self::Href],
        }
    }

    fn inherited(self) -> bool {
        matches!(
            self,
            Self::Fill | Self::Stroke | Self::MarkerStart | Self::MarkerMid | Self::MarkerEnd
        )
    }

    /// Whether a `style` declaration of `property` sets this property.
    fn declared_by(self, property: &str) -> bool {
        let property = property.trim();
        property.eq_ignore_ascii_case(self.name())
            || (property.eq_ignore_ascii_case("marker")
                && matches!(self, Self::MarkerStart | Self::MarkerMid | Self::MarkerEnd))
    }

    fn targets(self, source: &str, target: &str) -> bool {
        match self {
            Self::ClipPath => target == "clipPath",
            Self::Mask => target == "mask",
            Self::Filter => target == "filter",
            Self::Fill | Self::Stroke => {
                matches!(target, "linearGradient" | "radialGradient" | "pattern")
            }
            Self::MarkerStart | Self::MarkerMid | Self::MarkerEnd => target == "marker",
            Self::Href => match source {
                "use" | "feImage" => true,
                "linearGradient" | "radialGradient" => {
                    matches!(target, "linearGradient" | "radialGradient")
                }
                "pattern" | "filter" => target == source,
                _ => false,
            },
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Edge {
    from: usize,
    property: Property,
    target: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Pass {
    /// `use` instantiation, over the whole document.
    Instances,
    /// What rendering follows, from the root.
    Rendering,
}

struct Graph<'a, 'input> {
    /// Retained elements in document order; the root is first.
    elements: Vec<roxmltree::Node<'a, 'input>>,
    index: BTreeMap<usize, usize>,
    ids: BTreeMap<&'a str, Vec<usize>>,
    parents: Vec<Option<usize>>,
}

impl<'a, 'input> Graph<'a, 'input> {
    fn new(document: &'a roxmltree::Document<'input>) -> Result<Self, ResolutionFailure> {
        let mut graph = Self {
            elements: Vec::new(),
            index: BTreeMap::new(),
            ids: BTreeMap::new(),
            parents: Vec::new(),
        };
        for node in document.descendants().filter(|node| node.is_element()) {
            let parent = node
                .parent_element()
                .and_then(|parent| graph.index.get(&parent.range().start).copied());
            let root = node == document.root_element();
            if !is_allowed_svg_node(&node) || (!root && parent.is_none()) {
                continue;
            }
            let ordinal = graph.elements.len();
            graph
                .elements
                .try_reserve(1)
                .map_err(|_| ResolutionFailure::Memory)?;
            graph.elements.push(node);
            graph
                .parents
                .try_reserve(1)
                .map_err(|_| ResolutionFailure::Memory)?;
            graph.parents.push(parent);
            graph.index.insert(node.range().start, ordinal);
            if let Some(id) = node.attribute("id") {
                let targets = graph.ids.entry(id).or_default();
                targets
                    .try_reserve(1)
                    .map_err(|_| ResolutionFailure::Memory)?;
                targets.push(ordinal);
            }
        }
        Ok(graph)
    }

    /// The value of `property` on element `element` itself, `inherit`
    /// excluded: its `href`, or its cascaded value.
    fn declared(&self, plan: &Plan, element: usize, property: Property) -> Option<String> {
        let node = self.elements[element];
        if property == Property::Href {
            let index = effective_href(node)?;
            let raw = node.attributes().nth(index)?.value();
            return plan.value(node, index, raw).map(str::to_owned);
        }
        self.winner(plan, element, property)
            .map(|(value, _)| value)
            .filter(|value| !is_inherit(value))
    }

    /// The cascaded value of a rendering `property` on element `element`
    /// itself and where it comes from, as the viewer library cascades it: the
    /// presentation attribute, then each matching rule declaration, then each
    /// `style` declaration, a later one replacing an earlier one unless the
    /// earlier is `!important`.
    fn winner(&self, plan: &Plan, element: usize, property: Property) -> Option<(String, Source)> {
        fn insert<'v>(
            winner: &mut Option<(&'v str, bool, Source)>,
            value: &'v str,
            important: bool,
            source: Source,
        ) {
            if !winner.is_some_and(|(_, important, _)| important) {
                *winner = Some((value, important, source));
            }
        }
        let node = self.elements[element];
        let mut winner = None;
        for (index, attribute) in node.attributes().enumerate() {
            if attribute.name() != property.name() || attribute.namespace().is_some() {
                continue;
            }
            if let Some(value) = plan.value(node, index, attribute.value()) {
                insert(
                    &mut winner,
                    value.trim().trim_end_matches("!important"),
                    false,
                    Source::Presentation,
                );
            }
        }
        for &index in plan
            .cascade
            .matched
            .get(&node.range().start)
            .map_or(&[][..], Vec::as_slice)
        {
            let declaration = &plan.cascade.declarations[index];
            if !declaration.stripped && declaration.property == property {
                insert(
                    &mut winner,
                    &declaration.value,
                    declaration.important,
                    Source::Rule {
                        important: declaration.important,
                    },
                );
            }
        }
        if let Some((_, style)) = plan.style(node) {
            for declaration in style.split(';') {
                if let Some((name, declared)) = declaration.split_once(':') {
                    if property.declared_by(name) {
                        let declared = declared.trim();
                        let (value, important) = match declared.strip_suffix("!important") {
                            Some(value) => (value, true),
                            None => (declared, false),
                        };
                        insert(&mut winner, value, important, Source::Style);
                    }
                }
            }
        }
        winner.map(|(value, _, source)| (value.trim().to_owned(), source))
    }

    /// The element's own values of `group`, `property`'s with its `url(#id)`
    /// replaced by the missing fragment; an inherited `property` is replaced
    /// in the value it inherits.
    fn group_values(
        &self,
        plan: &Plan,
        element: usize,
        group: &[Property],
        property: Property,
        id: &str,
    ) -> Result<Vec<(Property, String)>, ResolutionFailure> {
        let mut values = Vec::new();
        for member in group {
            let value = if *member == property {
                self.inherited(plan, element, property)
                    .map(|(_, value)| value)
                    .map(|value| replace_local_url(&value, id, &plan.missing))
                    .transpose()?
            } else {
                self.winner(plan, element, *member).map(|(value, _)| value)
            };
            if let Some(value) = value {
                values
                    .try_reserve(1)
                    .map_err(|_| ResolutionFailure::Memory)?;
                values.push((*member, value));
            }
        }
        Ok(values)
    }

    /// The nearest element, `element` included, declaring the inherited
    /// `property`, and its value.
    fn inherited(
        &self,
        plan: &Plan,
        element: usize,
        property: Property,
    ) -> Option<(usize, String)> {
        let mut current = Some(element);
        while let Some(candidate) = current {
            if let Some(value) = self.declared(plan, candidate, property) {
                return Some((candidate, value));
            }
            if !property.inherited() {
                return None;
            }
            current = self.parents[candidate];
        }
        None
    }

    fn edges(
        &self,
        plan: &Plan,
        element: usize,
        pass: Pass,
        inherited: &[Option<usize>],
    ) -> Result<Vec<(Option<Edge>, usize)>, ResolutionFailure> {
        let node = self.elements[element];
        let name = node.tag_name().name();
        let mut edges = Vec::new();
        let properties: &[Property] = match pass {
            Pass::Instances if name == "use" => &[Property::Href],
            Pass::Instances => &[],
            Pass::Rendering => &Property::RENDERING,
        };
        let href = [Property::Href];
        let chained = pass == Pass::Rendering
            && matches!(
                name,
                "use" | "feImage" | "linearGradient" | "radialGradient" | "pattern" | "filter"
            );
        for property in properties
            .iter()
            .chain(if chained { href.iter() } else { [].iter() })
        {
            let value = if property.inherited() {
                let slot = element
                    .checked_mul(Property::RENDERING.len())
                    .and_then(|base| base.checked_add(*property as usize))
                    .and_then(|slot| inherited.get(slot).copied())
                    .flatten();
                slot.and_then(|owner| self.declared(plan, owner, *property))
            } else {
                self.declared(plan, element, *property)
            };
            let Some(value) = value else {
                continue;
            };
            let mut references = Vec::new();
            if *property == Property::Href {
                if let Some(id) = value.trim().strip_prefix('#') {
                    references.push(id.to_owned());
                }
            } else {
                for (_, _, target) in super::url_tokens(&value) {
                    if let Some(id) = target.strip_prefix('#') {
                        references.push(id.to_owned());
                    }
                }
            }
            for id in references {
                for &target in self.ids.get(id.as_str()).map_or(&[][..], Vec::as_slice) {
                    if property.targets(name, self.elements[target].tag_name().name()) {
                        edges
                            .try_reserve(1)
                            .map_err(|_| ResolutionFailure::Memory)?;
                        edges.push((
                            Some(Edge {
                                from: element,
                                property: *property,
                                target,
                            }),
                            target,
                        ));
                    }
                }
            }
        }
        for child in node.children().filter(|child| child.is_element()) {
            let Some(&child_index) = self.index.get(&child.range().start) else {
                continue;
            };
            let rendered = !matches!(
                child.tag_name().name(),
                "defs"
                    | "mask"
                    | "clipPath"
                    | "pattern"
                    | "marker"
                    | "symbol"
                    | "linearGradient"
                    | "radialGradient"
                    | "filter"
            );
            if pass == Pass::Instances || rendered {
                edges
                    .try_reserve(1)
                    .map_err(|_| ResolutionFailure::Memory)?;
                edges.push((None, child_index));
            }
        }
        Ok(edges)
    }

    /// Depth-first in source order from the root; returns each edge that
    /// closes a cycle.
    fn cycles(&self, plan: &Plan, pass: Pass) -> Result<Vec<Edge>, ResolutionFailure> {
        let mut breaks = Vec::new();
        if self.elements.is_empty() {
            return Ok(breaks);
        }
        let inherited = if pass == Pass::Rendering {
            self.inherited_owners(plan)?
        } else {
            Vec::new()
        };
        const WHITE: u8 = 0;
        const GRAY: u8 = 1;
        const BLACK: u8 = 2;
        let mut state = Vec::new();
        state
            .try_reserve_exact(self.elements.len())
            .map_err(|_| ResolutionFailure::Memory)?;
        state.resize(self.elements.len(), WHITE);
        struct Frame {
            element: usize,
            edges: Vec<(Option<Edge>, usize)>,
            next: usize,
            entered_by: Option<Edge>,
        }
        let mut frames = vec![Frame {
            element: 0,
            edges: self.edges(plan, 0, pass, &inherited)?,
            next: 0,
            entered_by: None,
        }];
        state[0] = GRAY;
        while let Some(frame) = frames.last_mut() {
            let Some(&(edge, target)) = frame.edges.get(frame.next) else {
                state[frame.element] = BLACK;
                frames.pop();
                continue;
            };
            frame.next += 1;
            match state[target] {
                WHITE => {
                    state[target] = GRAY;
                    let edges = self.edges(plan, target, pass, &inherited)?;
                    frames
                        .try_reserve(1)
                        .map_err(|_| ResolutionFailure::Memory)?;
                    frames.push(Frame {
                        element: target,
                        edges,
                        next: 0,
                        entered_by: edge,
                    });
                }
                GRAY => {
                    // A containment edge cannot be removed: the latest
                    // reference entering the cycle closes it instead.
                    let closing = edge.or_else(|| {
                        frames
                            .iter()
                            .rev()
                            .take_while(|frame| frame.element != target)
                            .find_map(|frame| frame.entered_by)
                    });
                    if let Some(closing) = closing {
                        if !breaks.contains(&closing) {
                            breaks
                                .try_reserve(1)
                                .map_err(|_| ResolutionFailure::Memory)?;
                            breaks.push(closing);
                        }
                    }
                }
                _ => {}
            }
        }
        Ok(breaks)
    }

    /// For each element and inherited rendering property, the nearest
    /// element declaring it.
    fn inherited_owners(&self, plan: &Plan) -> Result<Vec<Option<usize>>, ResolutionFailure> {
        let width = Property::RENDERING.len();
        let mut owners = Vec::new();
        owners
            .try_reserve_exact(self.elements.len().saturating_mul(width))
            .map_err(|_| ResolutionFailure::Memory)?;
        for element in 0..self.elements.len() {
            for property in Property::RENDERING {
                let owner = if !property.inherited() {
                    None
                } else if self.declared(plan, element, property).is_some() {
                    Some(element)
                } else {
                    self.parents[element]
                        .and_then(|parent| owners.get(parent * width + property as usize))
                        .copied()
                        .flatten()
                };
                owners.push(owner);
            }
        }
        Ok(owners)
    }

    /// Gradients whose `href` chain has no stop.
    fn stopless_gradients(&self, plan: &Plan) -> Result<BTreeSet<usize>, ResolutionFailure> {
        const UNKNOWN: u8 = 0;
        const VISITING: u8 = 1;
        const STOPS: u8 = 2;
        const NONE: u8 = 3;
        let gradient = |element: usize| {
            matches!(
                self.elements[element].tag_name().name(),
                "linearGradient" | "radialGradient"
            )
        };
        let mut state = Vec::new();
        state
            .try_reserve_exact(self.elements.len())
            .map_err(|_| ResolutionFailure::Memory)?;
        state.resize(self.elements.len(), UNKNOWN);
        let mut stopless = BTreeSet::new();
        let mut path = Vec::new();
        for start in (0..self.elements.len()).filter(|element| gradient(*element)) {
            path.clear();
            let mut current = Some(start);
            let mut result = NONE;
            while let Some(element) = current {
                match state[element] {
                    STOPS | NONE => {
                        result = state[element];
                        break;
                    }
                    VISITING => break,
                    _ => {}
                }
                if !gradient(element) {
                    break;
                }
                state[element] = VISITING;
                path.try_reserve(1).map_err(|_| ResolutionFailure::Memory)?;
                path.push(element);
                let node = self.elements[element];
                if node.children().any(|child| {
                    child.is_element()
                        && child.tag_name().name() == "stop"
                        && self.index.contains_key(&child.range().start)
                }) {
                    result = STOPS;
                    break;
                }
                current = self
                    .declared(plan, element, Property::Href)
                    .and_then(|href| {
                        href.trim()
                            .strip_prefix('#')
                            .and_then(|id| self.ids.get(id))
                            .and_then(|targets| targets.last().copied())
                    });
            }
            for &element in &path {
                state[element] = result;
                if result == NONE {
                    stopless.insert(self.elements[element].range().start);
                }
            }
        }
        Ok(stopless)
    }
}
