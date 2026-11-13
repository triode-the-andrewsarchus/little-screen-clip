// SPDX-License-Identifier: MPL-2.0
//! metadata.go: link/tooltip regions, their budgets and the metadata-free
//! visual bounding box; markdown.go addMarkdownLinkRegion.

use d2_rs_scene::{Box as SceneBox, LinkRegion};
use d2_rs_target::{Connection, Diagram, Point as TargetPoint, Shape};

use crate::Builder;
use crate::target_ext::*;
use crate::util::*;

impl Builder<'_> {
    pub(crate) fn compile_link_regions(&mut self) -> Result<()> {
        let has = |link: &str, tooltip: &str, pretty: &str| {
            !link.is_empty() || !tooltip.is_empty() || !pretty.is_empty()
        };
        let has_metadata = self
            .diagram
            .shapes()
            .iter()
            .any(|s| has(&s.link, &s.tooltip, &s.pretty_link))
            || self
                .diagram
                .connections()
                .iter()
                .any(|c| has(&c.link, &c.tooltip, &c.pretty_link));
        if !has_metadata {
            return Ok(());
        }
        let budget = self.options.link_budget;
        if budget.max_regions <= 0 || budget.max_string_bytes <= 0 {
            return Err(invalid_field(
                "options",
                "linkBudget",
                Some(&budget.go_v()),
                "must provide positive MaxRegions and MaxStringBytes for link/tooltip metadata",
            ));
        }
        let (start_links, start_bytes) = (self.links.len(), self.link_bytes);
        let (start_items, start_item_bytes) =
            (self.appendix_items.len(), self.appendix_string_bytes);
        let result = self.compile_link_regions_inner();
        if result.is_err() {
            self.links.truncate(start_links);
            self.link_bytes = start_bytes;
            self.appendix_items.truncate(start_items);
            self.appendix_string_bytes = start_item_bytes;
        }
        result
    }

    fn compile_link_regions_inner(&mut self) -> Result<()> {
        let diagram = self.diagram;
        for (index, s) in diagram.shapes().iter().enumerate() {
            if s.link.is_empty() && s.tooltip.is_empty() && s.pretty_link.is_empty() {
                continue;
            }
            let object = format!("shape {}", q(&s.id));
            let region = self.shape_link_region(&object, s)?;
            self.add_link_region(&object, region)?;
            let item_object = format!("shape[{index}] {}", q(&s.id));
            self.add_appendix_item(&item_object, "tooltip", &s.tooltip)?;
            self.add_appendix_item(&item_object, "prettyLink", &s.pretty_link)?;
        }
        for (index, c) in diagram.connections().iter().enumerate() {
            if c.link.is_empty() && c.tooltip.is_empty() && c.pretty_link.is_empty() {
                continue;
            }
            let object = format!("connection {}", q(&c.id));
            let region = self.connection_link_region(&object, c)?;
            self.add_link_region(&object, region)?;
            let item_object = format!("connection[{index}] {}", q(&c.id));
            self.add_appendix_item(&item_object, "tooltip", &c.tooltip)?;
            self.add_appendix_item(&item_object, "prettyLink", &c.pretty_link)?;
        }
        Ok(())
    }

    fn shape_link_region(&self, object: &str, s: &Shape) -> Result<LinkRegion> {
        if !s.pretty_link.is_empty() && s.link.is_empty() {
            return Err(invalid_field(
                object,
                "prettyLink",
                Some(&s.pretty_link),
                "requires a non-empty link",
            ));
        }
        self.validate_link_fields(object, &s.link, &s.tooltip)?;
        validate_link_tooltip_security(object, &s.link, &s.tooltip)?;
        let (url, target) = link_destination(&s.link);
        let stroke_width = s.stroke_width as f64;
        Ok(LinkRegion {
            r#box: SceneBox {
                x: s.pos.x as f64 - stroke_width,
                y: s.pos.y as f64 - stroke_width,
                width: s.width as f64 + 2.0 * stroke_width,
                height: s.height as f64 + 2.0 * stroke_width,
            },
            url,
            tooltip: s.tooltip.clone(),
            target,
        })
    }

    fn connection_link_region(&self, object: &str, c: &Connection) -> Result<LinkRegion> {
        if !c.pretty_link.is_empty() && c.link.is_empty() {
            return Err(invalid_field(
                object,
                "prettyLink",
                Some(&c.pretty_link),
                "requires a non-empty link",
            ));
        }
        self.validate_link_fields(object, &c.link, &c.tooltip)?;
        validate_link_tooltip_security(object, &c.link, &c.tooltip)?;
        let Some(top_left) = connection_label_top_left(c) else {
            return Err(invalid_field(
                object,
                "labelPosition",
                Some(&c.label_position),
                "does not resolve to a finite metadata hit region",
            ));
        };
        let (url, target) = link_destination(&c.link);
        Ok(LinkRegion {
            r#box: SceneBox {
                x: top_left.x.round(),
                y: top_left.y.round(),
                width: c.text.label_width as f64,
                height: c.text.label_height as f64,
            },
            url,
            tooltip: c.tooltip.clone(),
            target,
        })
    }

    pub(crate) fn validate_link_fields(
        &self,
        object: &str,
        link: &str,
        tooltip: &str,
    ) -> Result<()> {
        let mut next_bytes = self.link_bytes;
        for (name, value) in [("link", link), ("tooltip", tooltip)] {
            if value.len() as i64 > self.options.link_budget.max_string_bytes - next_bytes {
                return err(format!(
                    "scene: link metadata string bytes exceed limit {}",
                    self.options.link_budget.max_string_bytes
                ));
            }
            validate_link_string(object, name, value)?;
            next_bytes += value.len() as i64;
        }
        Ok(())
    }

    pub(crate) fn add_link_region(&mut self, object: &str, region: LinkRegion) -> Result<()> {
        let budget = self.options.link_budget;
        if self.links.len() as i64 >= budget.max_regions {
            return err(format!(
                "scene: link region count exceeds limit {}",
                budget.max_regions
            ));
        }
        let b = region.r#box;
        if !finite(b.x)
            || !finite(b.y)
            || !finite(b.width)
            || !finite(b.height)
            || b.width <= 0.0
            || b.height <= 0.0
        {
            return Err(invalid_field(
                object,
                "linkRegion",
                Some(&format!(
                    "{{{} {} {} {}}}",
                    fv(b.x),
                    fv(b.y),
                    fv(b.width),
                    fv(b.height)
                )),
                "must be a finite box with positive dimensions",
            ));
        }
        if !region.url.is_empty() && !region.target.is_empty() {
            return err(format!(
                "scene: {object} link metadata has both URL and target destinations"
            ));
        }
        if region.url.is_empty() && region.tooltip.is_empty() && region.target.is_empty() {
            return err(format!("scene: {object} link metadata is empty"));
        }
        let mut next_bytes = self.link_bytes;
        for value in [&region.url, &region.tooltip, &region.target] {
            if value.len() as i64 > budget.max_string_bytes - next_bytes {
                return err(format!(
                    "scene: link metadata string bytes exceed limit {}",
                    budget.max_string_bytes
                ));
            }
            next_bytes += value.len() as i64;
        }
        self.links.push(region);
        self.link_bytes = next_bytes;
        Ok(())
    }

    /// addMarkdownLinkRegion (markdown.go).
    pub(crate) fn add_markdown_link_region(
        &mut self,
        object: &str,
        primitive_index: usize,
        link: &str,
        title: &str,
        b: SceneBox,
    ) -> Result<()> {
        let budget = self.options.link_budget;
        if budget.max_regions <= 0 || budget.max_string_bytes <= 0 {
            return Err(invalid_field(
                "options",
                "linkBudget",
                Some(&budget.go_v()),
                "must provide positive MaxRegions and MaxStringBytes for Markdown links",
            ));
        }
        let metadata_object = format!("{object} Markdown primitive {primitive_index}");
        self.validate_link_fields(&metadata_object, link, title)?;
        let (start_links, start_bytes) = (self.links.len(), self.link_bytes);
        self.add_link_region(
            &metadata_object,
            LinkRegion {
                r#box: b,
                url: link.to_owned(),
                tooltip: title.to_owned(),
                target: String::new(),
            },
        )?;
        if let Err(e) = self.add_appendix_item(&metadata_object, "linkTitle", title) {
            self.links.truncate(start_links);
            self.link_bytes = start_bytes;
            return Err(e);
        }
        Ok(())
    }

    /// visualBoundingBox: the bounding box without appendix-only metadata.
    pub(crate) fn visual_bounding_box(&self) -> (TargetPoint, TargetPoint) {
        let has_shape_metadata = self.diagram.shapes().iter().any(|s| {
            !s.link.is_empty()
                || !s.tooltip.is_empty()
                || !s.pretty_link.is_empty()
                || !s.tooltip_position.is_empty()
        });
        if !has_shape_metadata {
            return bounding_box(self.diagram);
        }
        let mut copy = Diagram {
            font_family: self.diagram.font_family.clone(),
            mono_font_family: self.diagram.mono_font_family.clone(),
            shapes: self.diagram.shapes.clone(),
            connections: self.diagram.connections.clone(),
            ..Default::default()
        };
        for s in copy.shapes.iter_mut().flatten() {
            let positioned_tooltip = !s.tooltip.is_empty() && !s.tooltip_position.is_empty();
            s.link.clear();
            s.pretty_link.clear();
            if !positioned_tooltip {
                s.tooltip.clear();
                s.tooltip_position.clear();
            }
        }
        bounding_box(&copy)
    }
}

/// validateLinkString.
pub(crate) fn validate_link_string(object: &str, field: &str, value: &str) -> Result<()> {
    // Rust strings are valid UTF-8; the JSON decoder already mapped invalid
    // bytes to U+FFFD, as Go's encoding/json does.
    if !value.chars().all(valid_xml_metadata_rune) {
        return Err(invalid_field(
            object,
            field,
            None,
            "contains a character forbidden by XML 1.0 metadata consumers",
        ));
    }
    Ok(())
}

fn valid_xml_metadata_rune(r: char) -> bool {
    let r = r as u32;
    r == 0x9
        || r == 0xa
        || r == 0xd
        || (0x20..=0xd7ff).contains(&r)
        || (0xe000..=0xfffd).contains(&r)
        || (0x10000..=0x10ffff).contains(&r)
}

/// validateLinkTooltipSecurity.
fn validate_link_tooltip_security(object: &str, link: &str, tooltip: &str) -> Result<()> {
    if crate::markdown::is_dangerous_link(link) {
        return Err(invalid_field(
            object,
            "link",
            Some(link),
            "uses an unsafe URL scheme",
        ));
    }
    if link.is_empty() || tooltip.is_empty() {
        return Ok(());
    }
    if parse_request_uri_has_host(tooltip) {
        return Err(invalid_field(
            object,
            "tooltip",
            Some(tooltip),
            "must not be a URL when link is also set",
        ));
    }
    Ok(())
}

/// Whether net/url.ParseRequestURI succeeds with a non-empty Host: an
/// absolute URI whose hierarchical part has an authority.
pub fn parse_request_uri_has_host(raw: &str) -> bool {
    if raw.bytes().any(|b| b < 0x20 || b == 0x7f) {
        return false;
    }
    // getScheme.
    let bytes = raw.as_bytes();
    let mut scheme_end = None;
    for (i, &c) in bytes.iter().enumerate() {
        if c.is_ascii_alphabetic() {
            continue;
        }
        if c.is_ascii_digit() || c == b'+' || c == b'-' || c == b'.' {
            if i == 0 {
                return false;
            }
            continue;
        }
        if c == b':' {
            if i == 0 {
                return false;
            }
            scheme_end = Some(i);
        }
        break;
    }
    let Some(scheme_end) = scheme_end else {
        return false;
    };
    let rest = &raw[scheme_end + 1..];
    // ParseRequestURI does not split a fragment; a query is split.
    let rest = rest.split_once('?').map_or(rest, |(path, _)| path);
    let Some(authority_and_path) = rest.strip_prefix("//") else {
        return false;
    };
    let authority = authority_and_path
        .find('/')
        .map_or(authority_and_path, |i| &authority_and_path[..i]);
    let host = authority
        .rfind('@')
        .map_or(authority, |i| &authority[i + 1..]);
    if host.is_empty() {
        return false;
    }
    valid_parsed_host(host)
}

/// net/url parseHost + unescape(host, encodeHost) acceptance.
fn valid_parsed_host(host: &str) -> bool {
    let (name, port) = if let Some(rest) = host.strip_prefix('[') {
        let Some(close) = rest.find(']') else {
            return false;
        };
        (&rest[..close], &rest[close + 1..])
    } else {
        match host.rfind(':') {
            Some(i) => (&host[..i], &host[i..]),
            None => (host, ""),
        }
    };
    if !port.is_empty() && !(port.starts_with(':') && port[1..].bytes().all(|b| b.is_ascii_digit()))
    {
        return false;
    }
    let bytes = name.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if c == b'%' {
            if !(bytes.get(i + 1).is_some_and(u8::is_ascii_hexdigit)
                && bytes.get(i + 2).is_some_and(u8::is_ascii_hexdigit))
            {
                return false;
            }
            i += 3;
            continue;
        }
        let allowed = c >= 0x80
            || c.is_ascii_alphanumeric()
            || matches!(
                c,
                b'-' | b'_'
                    | b'.'
                    | b'~'
                    | b'!'
                    | b'$'
                    | b'&'
                    | b'\''
                    | b'('
                    | b')'
                    | b'*'
                    | b'+'
                    | b','
                    | b';'
                    | b'='
                    | b':'
                    | b'['
                    | b']'
                    | b'<'
                    | b'>'
                    | b'"'
            );
        if !allowed {
            return false;
        }
        i += 1;
    }
    true
}

/// linkDestination: a parsed D2 key rooted at "root" is a board target.
fn link_destination(value: &str) -> (String, String) {
    if value.is_empty() {
        return (String::new(), String::new());
    }
    let board_target = d2_rs_language::parse_key(value.as_bytes())
        .ok()
        .and_then(|key| key.string_ida())
        .is_some_and(|parts| parts.first().is_some_and(|part| part == "root"));
    if board_target {
        return (String::new(), value.to_owned());
    }
    (value.to_owned(), String::new())
}
