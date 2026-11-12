//! PDF-only stylesheet lowering with the same selector/cascade implementation
//! as the pinned usvg parser. Resolve against the original DOM before adding
//! effect IDs, so selectors such as `[id]` cannot observe our private markers.
use super::*;

pub(super) struct Filter {
    pub value: String,
    pub attribute: Option<(usize, String)>,
    pub css: Option<(u64, u64, u64)>,
}
struct Style {
    offset: usize,
    value: String,
    filter: Option<Filter>,
}
pub(super) struct Sheet<'a> {
    rules: Vec<(simplecss::Rule<'a>, u64, u64)>,
    enabled: bool,
}
// Rules borrow the XML only while resolving; retained rows own their values.
#[derive(Default)]
pub(super) struct Resolved {
    pub enabled: bool,
    rows: Vec<Style>,
}
impl Resolved {
    pub fn get(&self, node: roxmltree::Node<'_, '_>) -> Option<(&str, Option<&Filter>)> {
        self.rows
            .binary_search_by_key(&node.range().start, |s| s.offset)
            .ok()
            .map(|i| (self.rows[i].value.as_str(), self.rows[i].filter.as_ref()))
    }
}
impl<'a> Sheet<'a> {
    pub(super) fn new(
        document: &'a roxmltree::Document<'_>,
        source_nodes: &[SourceNode],
        plan: &'a references::Plan,
        format: OutputFormat,
    ) -> Result<Self, ResolutionFailure> {
        if format != OutputFormat::Pdf {
            return Ok(Self {
                rules: Vec::new(),
                enabled: false,
            });
        }
        let mut rules = Vec::new();
        for node in document.descendants().filter(|n| {
            n.has_tag_name("style")
                && safe_static_style_node(*n)
                && n.attribute("type").is_none_or(|t| t == "text/css")
        }) {
            let text = plan
                .stylesheet(node)
                .unwrap_or_else(|| node.text().unwrap_or(""));
            for (ordinal, block) in stylesheet::Rules::new(text).enumerate() {
                // Admission used these same source-preserving rule boundaries.
                let block = block.map_err(|_| ResolveError::SvgDecodeFailed)?;
                let sheet = simplecss::StyleSheet::parse(block.source);
                rules
                    .try_reserve(sheet.rules.len())
                    .map_err(|_| ResolutionFailure::Memory)?;
                for rule in sheet.rules {
                    rules.push((
                        rule,
                        source_node_ordinal(document, source_nodes, node),
                        ordinal as u64,
                    ));
                }
            }
        }
        let inline_filter = document.descendants().filter(|n| n.is_element()).any(|n| {
            n.attribute("style").is_some_and(|style| {
                simplecss::DeclarationTokenizer::from(style).any(|d| d.name == "filter")
            })
        });
        if !inline_filter
            && !rules
                .iter()
                .any(|(r, _, _)| r.declarations.iter().any(|d| d.name == "filter"))
        {
            return Ok(Self {
                rules: Vec::new(),
                enabled: false,
            });
        }
        rules.sort_unstable_by_key(|(r, style, ordinal)| {
            (r.selector.specificity(), *style, *ordinal)
        });
        Ok(Self {
            rules,
            enabled: true,
        })
    }
    pub(super) fn work_units(&self, document: &roxmltree::Document<'_>) -> u64 {
        (document.descendants().filter(|n| n.is_element()).count() as u64)
            .saturating_mul(self.rules.len() as u64)
            .saturating_mul(16)
    }
    pub(super) fn resolve(
        self,
        document: &roxmltree::Document<'_>,
        plan: &references::Plan,
    ) -> Result<Resolved, ResolutionFailure> {
        let rules = self.rules;
        if !self.enabled {
            return Ok(Resolved::default());
        }
        let mut rows = Vec::new();
        for node in document.descendants().filter(|n| n.is_element()) {
            let mut value = String::new();
            let mut filter = if let Some((i, a)) = node
                .attributes()
                .enumerate()
                .find(|(_, a)| a.name() == "filter")
            {
                Some(Filter {
                    value: super::super::try_owned(a.value())?,
                    attribute: Some((i, "filter".to_owned())),
                    css: None,
                })
            } else {
                None
            };
            let mut important = false;
            for (rule, style, ordinal) in &rules {
                if !rule.selector.matches(&Element(node)) {
                    continue;
                }
                for (i, d) in rule.declarations.iter().enumerate() {
                    value
                        .try_reserve(d.name.len() + d.value.len() + 16)
                        .map_err(|_| ResolutionFailure::Memory)?;
                    value.push_str(d.name);
                    value.push(':');
                    value.push_str(d.value);
                    if d.important {
                        value.push_str("!important");
                    }
                    value.push(';');
                    if d.name == "filter" && !important {
                        filter = Some(Filter {
                            value: super::super::try_owned(d.value)?,
                            attribute: None,
                            css: Some((*style, *ordinal, i as u64)),
                        });
                        important = d.important;
                    }
                }
            }
            // A `style` the reference plan adds breaks a cycle a rule closed.
            let added = plan
                .added(node)
                .iter()
                .find(|(name, _)| *name == "style")
                .map(|(_, value)| (usize::MAX, value.as_str()));
            if let Some((i, style)) = node
                .attributes()
                .enumerate()
                .find_map(|(i, a)| {
                    let value = plan.value(node, i, a.value())?;
                    (a.name() == "style"
                        && !unsafe_attribute(node.tag_name().name(), "style", a.namespace(), value))
                    .then_some((i, value))
                })
                .or(added)
            {
                value
                    .try_reserve(style.len() + 1)
                    .map_err(|_| ResolutionFailure::Memory)?;
                value.push_str(style);
                value.push(';');
                for d in simplecss::DeclarationTokenizer::from(style) {
                    if d.name == "filter" && !important {
                        filter = Some(Filter {
                            value: super::super::try_owned(d.value)?,
                            attribute: Some((i, "style".to_owned())),
                            css: None,
                        });
                        important = d.important;
                    }
                }
            }
            // A no-op overriding a presentation filter must remain an override.
            if filter
                .as_ref()
                .is_some_and(|f| filter_reference_is_noop(node, &f.value))
            {
                let mut without = String::new();
                without
                    .try_reserve(value.len() + 24)
                    .map_err(|_| ResolutionFailure::Memory)?;
                for d in simplecss::DeclarationTokenizer::from(value.as_str())
                    .filter(|d| d.name != "filter")
                {
                    without.push_str(d.name);
                    without.push(':');
                    without.push_str(d.value);
                    if d.important {
                        without.push_str("!important");
                    }
                    without.push(';');
                }
                without.push_str("filter:none;");
                value = without;
            }
            rows.try_reserve(1).map_err(|_| ResolutionFailure::Memory)?;
            rows.push(Style {
                offset: node.range().start,
                value,
                filter,
            });
        }
        Ok(Resolved {
            enabled: true,
            rows,
        })
    }
}
pub(super) struct Element<'a, 'input>(pub(super) roxmltree::Node<'a, 'input>);
impl simplecss::Element for Element<'_, '_> {
    fn parent_element(&self) -> Option<Self> {
        self.0.parent_element().map(Self)
    }
    fn prev_sibling_element(&self) -> Option<Self> {
        self.0.prev_sibling_element().map(Self)
    }
    fn has_local_name(&self, name: &str) -> bool {
        self.0.tag_name().name() == name
    }
    fn attribute_matches(&self, name: &str, operator: simplecss::AttributeOperator) -> bool {
        self.0.attribute(name).is_some_and(|v| operator.matches(v))
    }
    fn pseudo_class_matches(&self, class: simplecss::PseudoClass) -> bool {
        matches!(class, simplecss::PseudoClass::FirstChild)
            && self.0.prev_sibling_element().is_none()
    }
}
