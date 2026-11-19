//! Best-effort, inert HTML+CSS parsing for Markdown blocks and inline spans.
//!
//! Uses `scraper` (html5ever) within strict size, node-count, and depth budgets.
//! Scripts, stylesheets, embedded objects, comments, and hidden elements never
//! execute or render raw markup into the document.

use super::{
    BlockBoxStyle, ImagePresentation, InlineCss, ListItem, MarkdownDocBlock, MarkdownSpan,
    MarkdownTable, RgbaColor, TableAlignment, TextAlign, plain_text_from_spans, push_span,
};
use scraper::{ElementRef, Html, Node};

const MAX_HTML_FRAGMENT_BYTES: usize = 256 * 1024;
const MAX_HTML_NODES: usize = 16_384;
const MAX_HTML_DEPTH: usize = 64;

#[derive(Debug, Clone)]
pub(super) enum InlineHtmlToken {
    Ignore,
    HardBreak,
    PushLink(String),
    PopLink,
    PushSuppressed,
    PopSuppressed,
    Image {
        alt: String,
        src: String,
        style: ImagePresentation,
        hidden: bool,
    },
    OpenStyle {
        tag: String,
        css: InlineCss,
        hidden: bool,
    },
    CloseStyle {
        tag: String,
    },
}

#[derive(Debug, Clone)]
pub(super) enum ContainerAction {
    OpenDetails {
        summary: Vec<MarkdownSpan>,
        open: bool,
    },
    CloseDetails,
    OpenBox {
        tag: String,
        style: BlockBoxStyle,
        hidden: bool,
    },
    CloseBox {
        tag: String,
    },
    Blocks(Vec<MarkdownDocBlock>),
}

/// Strips HTML comments (`<!-- ... -->`) while keeping non-comment HTML intact.
pub(super) fn strip_html_comments(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(start) = rest.find("<!--") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 4..];
        if let Some(end) = after.find("-->") {
            rest = &after[end + 3..];
        } else {
            return out;
        }
    }
    out.push_str(rest);
    out
}

/// Parses one `Event::InlineHtml` chunk inside Markdown inline content.
pub(super) fn parse_inline_token(raw: &str) -> InlineHtmlToken {
    let stripped = strip_html_comments(raw);
    let trimmed = stripped.trim();
    if trimmed.is_empty()
        || trimmed.len() > MAX_HTML_FRAGMENT_BYTES
        || trimmed.starts_with("<!")
        || trimmed.starts_with("<?")
    {
        return InlineHtmlToken::Ignore;
    }
    if let Some(closing) = trimmed.strip_prefix("</") {
        let tag = closing
            .split(|c: char| c.is_whitespace() || c == '>')
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        return match tag.as_str() {
            "" => InlineHtmlToken::Ignore,
            "a" => InlineHtmlToken::PopLink,
            "script" | "style" | "noscript" | "template" => InlineHtmlToken::PopSuppressed,
            _ => InlineHtmlToken::CloseStyle { tag },
        };
    }

    let fragment = Html::parse_fragment(trimmed);
    let Some(el) = first_element(&fragment) else {
        return InlineHtmlToken::Ignore;
    };
    let tag = el.value().name().to_ascii_lowercase();
    let hidden = element_is_hidden(el);
    match tag.as_str() {
        "br" => InlineHtmlToken::HardBreak,
        "script" | "style" | "noscript" | "template" => InlineHtmlToken::PushSuppressed,
        "a" => {
            if let Some(href) = el.attr("href") {
                InlineHtmlToken::PushLink(href.to_owned())
            } else {
                InlineHtmlToken::Ignore
            }
        }
        "img" => InlineHtmlToken::Image {
            alt: el.attr("alt").unwrap_or("").to_owned(),
            src: el.attr("src").unwrap_or("").to_owned(),
            style: parse_image_presentation(el),
            hidden,
        },
        _ => {
            let mut css = inline_css_for_tag(&tag);
            merge_element_inline_css(el, &mut css);
            InlineHtmlToken::OpenStyle { tag, css, hidden }
        }
    }
}

/// Classifies a CommonMark `HtmlBlock` into ordered container open/close
/// actions and/or complete rendered HTML blocks.
pub(super) fn classify_html_block(raw: &str) -> Vec<ContainerAction> {
    let stripped = strip_html_comments(raw);
    let trimmed = stripped.trim();
    if trimmed.is_empty() || trimmed.len() > MAX_HTML_FRAGMENT_BYTES {
        return Vec::new();
    }

    let boundaries = scan_container_boundaries(trimmed);
    if boundaries.is_empty() {
        let blocks = parse_fragment_blocks(trimmed);
        return if blocks.is_empty() {
            Vec::new()
        } else {
            vec![ContainerAction::Blocks(blocks)]
        };
    }

    let mut actions = Vec::new();
    let mut cursor = 0usize;
    let push_blocks = |slice: &str, actions: &mut Vec<ContainerAction>| {
        let blocks = parse_fragment_blocks(slice);
        if !blocks.is_empty() {
            actions.push(ContainerAction::Blocks(blocks));
        }
    };

    for idx in 0..boundaries.len() {
        let boundary = boundaries[idx];
        if boundary.start < cursor {
            continue;
        }
        if boundary.start > cursor {
            push_blocks(&trimmed[cursor..boundary.start], &mut actions);
        }
        match boundary.kind {
            BoundaryKind::Close => {
                actions.push(close_action_for(boundary.tag));
                cursor = boundary.end;
            }
            BoundaryKind::Open => {
                let tag_slice = &trimmed[boundary.start..boundary.end];
                let tag_frag = Html::parse_fragment(tag_slice);
                if boundary.tag == "details" {
                    let details_el = first_element_named(&tag_frag, "details");
                    let open = details_el.is_some_and(|el| el.attr("open").is_some());
                    let next_limit = boundaries
                        .get(idx + 1)
                        .map_or(trimmed.len(), |next| next.start);
                    let mut summary = Vec::new();
                    let mut after_open = boundary.end;
                    if let Some(sum_close_end) =
                        find_summary_close_end(&trimmed[boundary.end..next_limit])
                    {
                        let sum_frag = Html::parse_fragment(
                            &trimmed[boundary.start..boundary.end + sum_close_end],
                        );
                        if let Some(sum_el) = first_element_named(&sum_frag, "summary") {
                            collect_inline_spans(sum_el, InlineCss::default(), None, &mut summary);
                        }
                        after_open = boundary.end + sum_close_end;
                    }
                    if summary.is_empty() {
                        summary.push(MarkdownSpan::Text("Details".to_owned()));
                    }
                    actions.push(ContainerAction::OpenDetails { summary, open });
                    cursor = after_open;
                } else {
                    let box_el = first_element_named(&tag_frag, boundary.tag);
                    let hidden = box_el.is_some_and(element_is_hidden);
                    let mut style = box_el.map(parse_block_box_style).unwrap_or_default();
                    if boundary.tag == "center" && style.align.is_none() {
                        style.align = Some(TextAlign::Center);
                    }
                    actions.push(ContainerAction::OpenBox {
                        tag: boundary.tag.to_owned(),
                        style,
                        hidden,
                    });
                    cursor = boundary.end;
                }
            }
        }
    }

    if cursor < trimmed.len() {
        push_blocks(&trimmed[cursor..], &mut actions);
    }
    actions
}

fn close_action_for(tag: &str) -> ContainerAction {
    if tag == "details" {
        ContainerAction::CloseDetails
    } else {
        ContainerAction::CloseBox {
            tag: tag.to_owned(),
        }
    }
}

#[derive(Clone, Copy)]
enum BoundaryKind {
    Open,
    Close,
}

#[derive(Clone, Copy)]
struct ContainerBoundary {
    tag: &'static str,
    start: usize,
    end: usize,
    kind: BoundaryKind,
}

fn scan_container_boundaries(html: &str) -> Vec<ContainerBoundary> {
    const TRACKED: &[&str] = &["details", "div", "center", "section", "article", "figure"];
    let mut open_stack: Vec<ContainerBoundary> = Vec::new();
    let mut unmatched_closes: Vec<ContainerBoundary> = Vec::new();

    let bytes = html.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'<' {
            i += 1;
            continue;
        }
        let rest = &html[i..];
        let Some(gt_rel) = find_tag_end(rest) else {
            break;
        };
        let tag_end = i + gt_rel + 1;
        let tag_body = &rest[1..gt_rel];
        let trimmed = tag_body.trim();
        let is_close = trimmed.starts_with('/');
        let name_slice = trimmed.trim_start_matches('/').trim_start();
        let name_end = name_slice
            .find(|c: char| c.is_whitespace() || c == '/')
            .unwrap_or(name_slice.len());
        let name = &name_slice[..name_end];
        let matched = TRACKED
            .iter()
            .copied()
            .find(|candidate| candidate.eq_ignore_ascii_case(name));
        if let Some(known) = matched {
            let self_closing = !is_close && trimmed.ends_with('/');
            if is_close {
                if let Some(pos) = open_stack.iter().rposition(|open| open.tag == known) {
                    open_stack.truncate(pos);
                } else {
                    unmatched_closes.push(ContainerBoundary {
                        tag: known,
                        start: i,
                        end: tag_end,
                        kind: BoundaryKind::Close,
                    });
                }
            } else if !self_closing {
                open_stack.push(ContainerBoundary {
                    tag: known,
                    start: i,
                    end: tag_end,
                    kind: BoundaryKind::Open,
                });
            }
        }
        i = tag_end;
    }

    let mut boundaries = unmatched_closes;
    boundaries.extend(open_stack);
    boundaries.sort_by_key(|b| b.start);
    boundaries
}

fn find_summary_close_end(slice: &str) -> Option<usize> {
    let lower = slice.to_ascii_lowercase();
    let mut offset = 0usize;
    while let Some(rel) = lower[offset..].find("</summary") {
        let start = offset + rel;
        let tail = &lower[start + "</summary".len()..];
        if let Some(gt_rel) = tail.find('>')
            && tail[..gt_rel].trim().is_empty()
        {
            return Some(start + "</summary".len() + gt_rel + 1);
        }
        offset = start + "</summary".len();
    }
    None
}

fn find_tag_end(slice: &str) -> Option<usize> {
    let mut quote: Option<u8> = None;
    for (idx, byte) in slice.bytes().enumerate() {
        if let Some(q) = quote {
            if byte == q {
                quote = None;
            }
        } else if byte == b'\'' || byte == b'"' {
            quote = Some(byte);
        } else if byte == b'>' {
            return Some(idx);
        }
    }
    None
}

/// Parses a complete HTML fragment safely into [`MarkdownDocBlock`] values.
pub(super) fn parse_fragment_blocks(raw: &str) -> Vec<MarkdownDocBlock> {
    let stripped = strip_html_comments(raw);
    let trimmed = stripped.trim();
    if trimmed.is_empty() || trimmed.len() > MAX_HTML_FRAGMENT_BYTES {
        return Vec::new();
    }

    let fragment = Html::parse_fragment(trimmed);
    let root = fragment.root_element();
    if root.descendants().take(MAX_HTML_NODES + 1).count() > MAX_HTML_NODES
        || root
            .descendants()
            .any(|node| node.ancestors().take(MAX_HTML_DEPTH + 1).count() > MAX_HTML_DEPTH)
    {
        return Vec::new();
    }

    let mut out = Vec::new();
    collect_child_blocks(root, None, &mut out);
    out
}

fn first_element<'a>(frag: &'a Html) -> Option<ElementRef<'a>> {
    frag.root_element()
        .children()
        .filter_map(ElementRef::wrap)
        .next()
}

fn first_element_named<'a>(frag: &'a Html, name: &str) -> Option<ElementRef<'a>> {
    frag.root_element()
        .descendent_elements()
        .find(|el| el.value().name().eq_ignore_ascii_case(name))
}

fn collect_child_blocks(
    parent: ElementRef<'_>,
    inherited_align: Option<TextAlign>,
    out: &mut Vec<MarkdownDocBlock>,
) {
    let mut inline_spans = Vec::new();
    let flush_inline = |inline_spans: &mut Vec<MarkdownSpan>, out: &mut Vec<MarkdownDocBlock>| {
        if !inline_spans.is_empty() {
            let spans = std::mem::take(inline_spans);
            let only_whitespace = spans
                .iter()
                .all(|span| matches!(span, MarkdownSpan::Text(t) if t.trim().is_empty()));
            if !only_whitespace {
                out.push(wrap_aligned_block(
                    MarkdownDocBlock::Paragraph { spans },
                    inherited_align,
                ));
            }
        }
    };

    for child in parent.children() {
        if let Some(text) = child.value().as_text() {
            let normalized = collapse_html_whitespace(text);
            if !normalized.is_empty() && !(inline_spans.is_empty() && normalized.trim().is_empty())
            {
                push_span(&mut inline_spans, MarkdownSpan::Text(normalized));
            }
            continue;
        }
        let Some(el) = ElementRef::wrap(child) else {
            continue;
        };
        if element_is_hidden(el) {
            continue;
        }
        let name = el.value().name().to_ascii_lowercase();
        if matches!(
            name.as_str(),
            "script"
                | "style"
                | "noscript"
                | "template"
                | "iframe"
                | "object"
                | "link"
                | "meta"
                | "base"
                | "title"
                | "source"
        ) {
            continue;
        }
        if is_block_tag(&name) {
            flush_inline(&mut inline_spans, out);
            convert_block_element(el, &name, inherited_align, out);
        } else {
            push_inline_element(el, InlineCss::default(), None, &mut inline_spans);
        }
    }
    flush_inline(&mut inline_spans, out);
}

fn is_block_tag(name: &str) -> bool {
    matches!(
        name,
        "h1" | "h2"
            | "h3"
            | "h4"
            | "h5"
            | "h6"
            | "p"
            | "div"
            | "center"
            | "section"
            | "article"
            | "header"
            | "footer"
            | "aside"
            | "main"
            | "figure"
            | "figcaption"
            | "blockquote"
            | "pre"
            | "ul"
            | "ol"
            | "hr"
            | "table"
            | "details"
    )
}

fn wrap_aligned_block(block: MarkdownDocBlock, align: Option<TextAlign>) -> MarkdownDocBlock {
    if let Some(align) = align
        && align != TextAlign::Left
    {
        MarkdownDocBlock::StyledContainer {
            style: BlockBoxStyle {
                align: Some(align),
                ..Default::default()
            },
            blocks: vec![block],
        }
    } else {
        block
    }
}

fn convert_block_element(
    el: ElementRef<'_>,
    name: &str,
    inherited_align: Option<TextAlign>,
    out: &mut Vec<MarkdownDocBlock>,
) {
    let mut box_style = parse_block_box_style(el);
    if name == "center" && box_style.align.is_none() {
        box_style.align = Some(TextAlign::Center);
    }
    let effective_align = box_style.align.or(inherited_align);

    match name {
        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
            let level = usize::from(name.as_bytes()[1] - b'0');
            let mut spans = Vec::new();
            collect_inline_spans(el, InlineCss::default(), None, &mut spans);
            let text = plain_text_from_spans(&spans);
            if !spans.is_empty() {
                out.push(wrap_aligned_block(
                    MarkdownDocBlock::Heading { level, text, spans },
                    effective_align,
                ));
            }
        }
        "hr" => out.push(MarkdownDocBlock::Rule),
        "pre" => {
            let language = el
                .descendent_elements()
                .find(|child| child.value().name().eq_ignore_ascii_case("code"))
                .and_then(|code| code.attr("class"))
                .and_then(|classes| {
                    classes
                        .split_whitespace()
                        .find_map(|cls| cls.strip_prefix("language-").or(cls.strip_prefix("lang-")))
                })
                .unwrap_or("")
                .to_owned();
            let text = collect_pre_text(el);
            out.push(MarkdownDocBlock::Code { language, text });
        }
        "blockquote" => {
            let mut blocks = Vec::new();
            collect_child_blocks(el, effective_align, &mut blocks);
            if !blocks.is_empty() {
                out.push(MarkdownDocBlock::Quote { kind: None, blocks });
            }
        }
        "ul" | "ol" => {
            let ordered = name == "ol";
            let start = el
                .attr("start")
                .and_then(|s| s.trim().parse::<u64>().ok())
                .unwrap_or(1);
            let mut items = Vec::new();
            for child in el.children().filter_map(ElementRef::wrap) {
                if child.value().name().eq_ignore_ascii_case("li") && !element_is_hidden(child) {
                    let task = child
                        .children()
                        .filter_map(ElementRef::wrap)
                        .find(|n| {
                            n.value().name().eq_ignore_ascii_case("input")
                                && n.attr("type")
                                    .is_some_and(|t| t.eq_ignore_ascii_case("checkbox"))
                        })
                        .map(|checkbox| checkbox.attr("checked").is_some());
                    let mut item_blocks = Vec::new();
                    collect_child_blocks(child, effective_align, &mut item_blocks);
                    items.push(ListItem {
                        task,
                        blocks: item_blocks,
                    });
                }
            }
            if !items.is_empty() {
                out.push(MarkdownDocBlock::List {
                    ordered,
                    start,
                    items,
                });
            }
        }
        "table" => {
            if let Some(table) = convert_html_table(el) {
                out.push(MarkdownDocBlock::Table(Box::new(table)));
            }
        }
        "details" => {
            let open = el.attr("open").is_some();
            let mut summary = Vec::new();
            let mut body_blocks = Vec::new();
            for child in el.children() {
                if let Some(child_el) = ElementRef::wrap(child)
                    && child_el.value().name().eq_ignore_ascii_case("summary")
                {
                    if summary.is_empty() {
                        collect_inline_spans(child_el, InlineCss::default(), None, &mut summary);
                    }
                    continue;
                }
                if let Some(child_el) = ElementRef::wrap(child) {
                    let child_name = child_el.value().name().to_ascii_lowercase();
                    if is_block_tag(&child_name) {
                        convert_block_element(
                            child_el,
                            &child_name,
                            effective_align,
                            &mut body_blocks,
                        );
                    } else {
                        let mut spans = Vec::new();
                        push_inline_element(child_el, InlineCss::default(), None, &mut spans);
                        if !spans.is_empty() {
                            body_blocks.push(MarkdownDocBlock::Paragraph { spans });
                        }
                    }
                } else if let Some(text) = child.value().as_text() {
                    let clean = collapse_html_whitespace(text);
                    if !clean.trim().is_empty() {
                        body_blocks.push(MarkdownDocBlock::Paragraph {
                            spans: vec![MarkdownSpan::Text(clean.trim().to_owned())],
                        });
                    }
                }
            }
            if summary.is_empty() {
                summary.push(MarkdownSpan::Text("Details".to_owned()));
            }
            out.push(MarkdownDocBlock::Details {
                summary,
                open,
                blocks: body_blocks,
            });
        }
        "p" | "figcaption" => {
            let mut spans = Vec::new();
            collect_inline_spans(el, InlineCss::default(), None, &mut spans);
            if !spans.is_empty() {
                let para = MarkdownDocBlock::Paragraph { spans };
                if box_style.has_visual_box() {
                    out.push(MarkdownDocBlock::StyledContainer {
                        style: box_style,
                        blocks: vec![para],
                    });
                } else {
                    out.push(wrap_aligned_block(para, effective_align));
                }
            }
        }
        _ => {
            let child_align = if box_style.has_visual_box() {
                None
            } else {
                effective_align
            };
            let mut children = Vec::new();
            collect_child_blocks(el, child_align, &mut children);
            if children.is_empty() {
                return;
            }
            if box_style.has_visual_box() {
                out.push(MarkdownDocBlock::StyledContainer {
                    style: box_style,
                    blocks: children,
                });
            } else {
                out.extend(children);
            }
        }
    }
}

fn convert_html_table(table_el: ElementRef<'_>) -> Option<MarkdownTable> {
    let mut headers: Vec<Vec<MarkdownSpan>> = Vec::new();
    let mut header_aligns: Vec<TableAlignment> = Vec::new();
    let mut rows: Vec<Vec<Vec<MarkdownSpan>>> = Vec::new();
    let mut row_aligns: Vec<TableAlignment> = Vec::new();

    let mut tr_nodes: Vec<(bool, ElementRef<'_>)> = Vec::new();
    collect_table_rows(table_el, false, &mut tr_nodes);

    for (in_thead, tr) in tr_nodes {
        let mut cells: Vec<Vec<MarkdownSpan>> = Vec::new();
        let mut cell_aligns: Vec<TableAlignment> = Vec::new();
        let mut all_th = true;
        let mut has_cells = false;

        for cell_el in tr.children().filter_map(ElementRef::wrap) {
            let cname = cell_el.value().name().to_ascii_lowercase();
            if cname != "th" && cname != "td" {
                continue;
            }
            has_cells = true;
            if cname != "th" {
                all_th = false;
            }
            let align = cell_alignment(cell_el);
            cell_aligns.push(align);

            let mut spans = Vec::new();
            collect_inline_spans(cell_el, InlineCss::default(), None, &mut spans);
            trim_spans_ends(&mut spans);
            cells.push(spans);
        }
        if !has_cells {
            continue;
        }
        if headers.is_empty() && (in_thead || all_th) {
            headers = cells;
            header_aligns = cell_aligns;
        } else {
            if row_aligns.is_empty() {
                row_aligns = cell_aligns;
            }
            rows.push(cells);
        }
    }

    if headers.is_empty() && rows.is_empty() {
        return None;
    }
    let col_count = headers
        .len()
        .max(rows.iter().map(Vec::len).max().unwrap_or(0))
        .max(1);
    let mut alignments = vec![TableAlignment::None; col_count];
    for (i, slot) in alignments.iter_mut().enumerate() {
        *slot = header_aligns
            .get(i)
            .copied()
            .filter(|a| *a != TableAlignment::None)
            .or_else(|| row_aligns.get(i).copied())
            .unwrap_or(TableAlignment::None);
    }
    Some(MarkdownTable {
        alignments,
        headers,
        rows,
    })
}

fn collect_table_rows<'a>(
    el: ElementRef<'a>,
    in_thead: bool,
    out: &mut Vec<(bool, ElementRef<'a>)>,
) {
    for child in el.children().filter_map(ElementRef::wrap) {
        if element_is_hidden(child) {
            continue;
        }
        let name = child.value().name().to_ascii_lowercase();
        match name.as_str() {
            "tr" => out.push((in_thead, child)),
            "thead" => collect_table_rows(child, true, out),
            "tbody" | "tfoot" => collect_table_rows(child, false, out),
            _ => {}
        }
    }
}

fn cell_alignment(cell: ElementRef<'_>) -> TableAlignment {
    let box_style = parse_block_box_style(cell);
    match box_style.align {
        Some(TextAlign::Left) => TableAlignment::Left,
        Some(TextAlign::Center) => TableAlignment::Center,
        Some(TextAlign::Right) => TableAlignment::Right,
        None => TableAlignment::None,
    }
}

fn trim_spans_ends(spans: &mut Vec<MarkdownSpan>) {
    while let Some(MarkdownSpan::Text(first)) = spans.first_mut() {
        let trimmed = first.trim_start().to_owned();
        if trimmed.is_empty() {
            spans.remove(0);
        } else {
            *first = trimmed;
            break;
        }
    }
    while let Some(MarkdownSpan::Text(last)) = spans.last_mut() {
        let trimmed = last.trim_end().to_owned();
        if trimmed.is_empty() {
            spans.pop();
        } else {
            *last = trimmed;
            break;
        }
    }
}

fn collect_pre_text(el: ElementRef<'_>) -> String {
    let mut out = String::new();
    for node in el.descendants() {
        if let Some(el) = ElementRef::wrap(node)
            && el.value().name().eq_ignore_ascii_case("br")
        {
            out.push('\n');
        } else if let Some(text) = node.value().as_text() {
            out.push_str(text);
        }
    }
    out
}

fn collect_inline_spans(
    el: ElementRef<'_>,
    parent_css: InlineCss,
    parent_url: Option<&str>,
    out: &mut Vec<MarkdownSpan>,
) {
    for child in el.children() {
        match child.value() {
            Node::Text(text) => {
                let collapsed = collapse_html_whitespace(text);
                if collapsed.is_empty() {
                    continue;
                }
                if out.is_empty() && collapsed.trim().is_empty() {
                    continue;
                }
                push_styled_text(out, collapsed, parent_url, parent_css);
            }
            Node::Element(_) => {
                if let Some(child_el) = ElementRef::wrap(child) {
                    push_inline_element(child_el, parent_css, parent_url, out);
                }
            }
            _ => {}
        }
    }
}

fn push_inline_element(
    child_el: ElementRef<'_>,
    parent_css: InlineCss,
    parent_url: Option<&str>,
    out: &mut Vec<MarkdownSpan>,
) {
    if element_is_hidden(child_el) {
        return;
    }
    let tag = child_el.value().name().to_ascii_lowercase();
    if matches!(
        tag.as_str(),
        "script"
            | "style"
            | "noscript"
            | "template"
            | "iframe"
            | "object"
            | "svg"
            | "link"
            | "meta"
            | "base"
            | "title"
            | "source"
            | "input"
    ) {
        return;
    }
    if tag == "br" {
        out.push(MarkdownSpan::HardBreak);
        return;
    }
    if tag == "img" {
        let src = child_el.attr("src").unwrap_or("").to_owned();
        if !src.is_empty() {
            let alt = child_el.attr("alt").unwrap_or("").to_owned();
            let style = parse_image_presentation(child_el);
            out.push(MarkdownSpan::StyledImage {
                alt,
                url: src,
                href: parent_url.map(str::to_owned),
                style,
            });
        }
        return;
    }
    let mut next_css = parent_css.merge(inline_css_for_tag(&tag));
    merge_element_inline_css(child_el, &mut next_css);
    let next_url = if tag == "a" {
        child_el.attr("href").or(parent_url)
    } else {
        parent_url
    };
    collect_inline_spans(child_el, next_css, next_url, out);
}

pub(super) fn push_styled_text(
    out: &mut Vec<MarkdownSpan>,
    text: String,
    url: Option<&str>,
    css: InlineCss,
) {
    if text.is_empty() {
        return;
    }
    let needs_styled = !css.is_plain_compatible()
        || (url.is_some() && (css.bold || css.italic || css.strikethrough || css.monospace));
    let span = if needs_styled {
        MarkdownSpan::Styled {
            text,
            url: url.map(str::to_owned),
            css,
        }
    } else if let Some(url) = url {
        MarkdownSpan::Link {
            label: text,
            url: url.to_owned(),
        }
    } else if css.strikethrough {
        MarkdownSpan::Strikethrough(text)
    } else if css.monospace {
        MarkdownSpan::Code(text)
    } else {
        match (css.bold, css.italic) {
            (true, true) => MarkdownSpan::BoldItalic(text),
            (true, false) => MarkdownSpan::Bold(text),
            (false, true) => MarkdownSpan::Italic(text),
            (false, false) => MarkdownSpan::Text(text),
        }
    };
    push_span(out, span);
}

fn collapse_html_whitespace(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut prev_space = false;
    for ch in text.chars() {
        if ch.is_whitespace() {
            if !prev_space {
                out.push(' ');
                prev_space = true;
            }
        } else {
            out.push(ch);
            prev_space = false;
        }
    }
    out
}

pub(super) fn inline_css_for_tag(tag: &str) -> InlineCss {
    let mut css = InlineCss::default();
    match tag {
        "b" | "strong" => css.bold = true,
        "i" | "em" | "cite" | "var" | "dfn" => css.italic = true,
        "s" | "del" | "strike" => css.strikethrough = true,
        "u" | "ins" => css.underline = true,
        "code" | "tt" | "samp" => css.monospace = true,
        "kbd" => {
            css.monospace = true;
            css.kbd = true;
        }
        "mark" => css.mark = true,
        "small" => css.small = true,
        "sub" => css.subscript = true,
        "sup" => css.superscript = true,
        _ => {}
    }
    css
}

fn element_is_hidden(el: ElementRef<'_>) -> bool {
    if el.attr("hidden").is_some() {
        return true;
    }
    if let Some(style) = el.attr("style") {
        for (prop, val) in split_css_declarations(style) {
            if prop.eq_ignore_ascii_case("display") && val.eq_ignore_ascii_case("none") {
                return true;
            }
        }
    }
    false
}

fn merge_element_inline_css(el: ElementRef<'_>, css: &mut InlineCss) {
    if let Some(color_attr) = el.attr("color")
        && let Some(c) = parse_css_color(color_attr)
    {
        css.color = Some(c);
    }
    let Some(style) = el.attr("style") else {
        return;
    };
    for (prop, val) in split_css_declarations(style) {
        let lower_prop = prop.to_ascii_lowercase();
        let lower_val = val.to_ascii_lowercase();
        match lower_prop.as_str() {
            "color" => {
                if let Some(c) = parse_css_color(val) {
                    css.color = Some(c);
                }
            }
            "background-color" | "background" => {
                if let Some(c) = extract_css_color_token(val) {
                    css.background = Some(c);
                }
            }
            "font-weight" => {
                let numeric = lower_val.parse::<u16>().ok().is_some_and(|w| w >= 600);
                if matches!(lower_val.as_str(), "bold" | "bolder") || numeric {
                    css.bold = true;
                }
            }
            "font-style" => {
                if matches!(lower_val.as_str(), "italic" | "oblique") {
                    css.italic = true;
                }
            }
            "text-decoration" | "text-decoration-line" => {
                if lower_val.contains("underline") {
                    css.underline = true;
                }
                if lower_val.contains("line-through") {
                    css.strikethrough = true;
                }
            }
            "font-family" => {
                if lower_val.contains("monospace")
                    || lower_val.contains("courier")
                    || lower_val.contains("consolas")
                    || lower_val.contains("fira")
                    || lower_val.contains("jetbrains")
                {
                    css.monospace = true;
                }
            }
            "vertical-align" => {
                if lower_val == "super" {
                    css.superscript = true;
                } else if lower_val == "sub" {
                    css.subscript = true;
                }
            }
            _ => {}
        }
    }
}

fn parse_block_box_style(el: ElementRef<'_>) -> BlockBoxStyle {
    let mut box_style = BlockBoxStyle::default();
    if let Some(align_attr) = el.attr("align") {
        box_style.align = parse_text_align(align_attr);
    }
    let Some(style) = el.attr("style") else {
        return box_style;
    };
    for (prop, val) in split_css_declarations(style) {
        let lower_prop = prop.to_ascii_lowercase();
        match lower_prop.as_str() {
            "text-align" => {
                if let Some(a) = parse_text_align(val) {
                    box_style.align = Some(a);
                }
            }
            "color" => {
                if let Some(c) = parse_css_color(val) {
                    box_style.text_color = Some(c);
                }
            }
            "background-color" | "background" => {
                if let Some(c) = extract_css_color_token(val)
                    && c.a > 0
                {
                    box_style.background = Some(c);
                }
            }
            "border" => {
                let (width, color) = parse_css_border(val);
                if width > 0.0 {
                    box_style.border_width = width;
                }
                if color.is_some() {
                    box_style.border_color = color;
                }
            }
            "border-color" => {
                if let Some(c) = parse_css_color(val) {
                    box_style.border_color = Some(c);
                }
            }
            "border-width" => {
                if let Some(w) = parse_css_length_px(val, 16.0) {
                    box_style.border_width = w.clamp(0.0, 6.0);
                }
            }
            "border-radius" => {
                if let Some(r) = parse_css_length_px(val, 16.0) {
                    box_style.border_radius = r.clamp(0.0, 24.0);
                }
            }
            "padding" => box_style.padding = parse_css_four_sides(val, 48.0),
            "padding-top" => {
                box_style.padding[0] = parse_css_length_px(val, 16.0)
                    .unwrap_or(0.0)
                    .clamp(0.0, 48.0)
            }
            "padding-right" => {
                box_style.padding[1] = parse_css_length_px(val, 16.0)
                    .unwrap_or(0.0)
                    .clamp(0.0, 48.0)
            }
            "padding-bottom" => {
                box_style.padding[2] = parse_css_length_px(val, 16.0)
                    .unwrap_or(0.0)
                    .clamp(0.0, 48.0)
            }
            "padding-left" => {
                box_style.padding[3] = parse_css_length_px(val, 16.0)
                    .unwrap_or(0.0)
                    .clamp(0.0, 48.0)
            }
            "margin" => {
                let sides = parse_css_four_sides(val, 32.0);
                box_style.margin_top = sides[0];
                box_style.margin_bottom = sides[2];
            }
            "margin-top" => {
                if let Some(m) = parse_css_length_px(val, 16.0) {
                    box_style.margin_top = m.clamp(0.0, 32.0);
                }
            }
            "margin-bottom" => {
                if let Some(m) = parse_css_length_px(val, 16.0) {
                    box_style.margin_bottom = m.clamp(0.0, 32.0);
                }
            }
            _ => {}
        }
    }
    box_style
}

fn parse_image_presentation(el: ElementRef<'_>) -> ImagePresentation {
    let mut pres = ImagePresentation::default();
    if let Some(w) = el.attr("width") {
        if let Some(frac) = parse_css_percentage(w) {
            pres.width_fraction = Some(frac.clamp(0.05, 1.0));
        } else if let Some(px) = parse_css_length_px(w, 16.0) {
            pres.width_px = Some(px);
        }
    }
    if let Some(h) = el.attr("height")
        && let Some(px) = parse_css_length_px(h, 16.0)
    {
        pres.height_px = Some(px);
    }
    if let Some(align_attr) = el.attr("align") {
        pres.align = parse_text_align(align_attr);
    }

    let Some(style) = el.attr("style") else {
        return pres;
    };
    let mut margin_left_auto = false;
    let mut margin_right_auto = false;
    for (prop, val) in split_css_declarations(style) {
        let lower_prop = prop.to_ascii_lowercase();
        let lower_val = val.to_ascii_lowercase();
        match lower_prop.as_str() {
            "width" => {
                if let Some(frac) = parse_css_percentage(&lower_val) {
                    pres.width_fraction = Some(frac.clamp(0.05, 1.0));
                } else if let Some(px) = parse_css_length_px(&lower_val, 16.0) {
                    pres.width_px = Some(px);
                }
            }
            "max-width" => {
                if pres.width_px.is_none() && pres.width_fraction.is_none() {
                    if let Some(frac) = parse_css_percentage(&lower_val) {
                        pres.width_fraction = Some(frac.clamp(0.05, 1.0));
                    } else if let Some(px) = parse_css_length_px(&lower_val, 16.0) {
                        pres.width_px = Some(px);
                    }
                }
            }
            "height" => {
                if lower_val != "auto"
                    && let Some(px) = parse_css_length_px(&lower_val, 16.0)
                {
                    pres.height_px = Some(px);
                }
            }
            "zoom" => {
                if let Some(frac) = parse_css_percentage(&lower_val) {
                    pres.zoom = Some(frac.clamp(0.05, 4.0));
                } else if let Ok(scale) = lower_val.trim().parse::<f32>()
                    && scale.is_finite()
                {
                    pres.zoom = Some(scale.clamp(0.05, 4.0));
                }
            }
            "border-radius" => {
                if let Some(frac) = parse_css_percentage(&lower_val) {
                    pres.border_radius = Some((frac * 256.0).clamp(0.0, 256.0));
                } else if let Some(px) = parse_css_length_px(&lower_val, 16.0) {
                    pres.border_radius = Some(px.clamp(0.0, 256.0));
                }
            }
            "float" => match lower_val.as_str() {
                "right" => pres.float = Some(TextAlign::Right),
                "left" => pres.float = Some(TextAlign::Left),
                _ => {}
            },
            "margin-left" => margin_left_auto = lower_val == "auto",
            "margin-right" => margin_right_auto = lower_val == "auto",
            "text-align" => {
                if let Some(a) = parse_text_align(&lower_val) {
                    pres.align = Some(a);
                }
            }
            _ => {}
        }
    }
    if pres.align.is_none() && margin_left_auto && margin_right_auto {
        pres.align = Some(TextAlign::Center);
    }
    pres
}

fn parse_text_align(val: &str) -> Option<TextAlign> {
    match val.trim().to_ascii_lowercase().as_str() {
        "left" | "start" => Some(TextAlign::Left),
        "center" | "middle" => Some(TextAlign::Center),
        "right" | "end" => Some(TextAlign::Right),
        _ => None,
    }
}

fn split_css_declarations(style: &str) -> Vec<(&str, &str)> {
    let mut decls = Vec::new();
    let mut start = 0usize;
    let mut paren_depth = 0usize;
    let mut quote: Option<u8> = None;

    for (idx, byte) in style.bytes().enumerate() {
        if let Some(q) = quote {
            if byte == q {
                quote = None;
            }
            continue;
        }
        match byte {
            b'\'' | b'"' => quote = Some(byte),
            b'(' => paren_depth += 1,
            b')' => paren_depth = paren_depth.saturating_sub(1),
            b';' if paren_depth == 0 => {
                if let Some(pair) = parse_decl_slice(&style[start..idx]) {
                    decls.push(pair);
                }
                start = idx + 1;
            }
            _ => {}
        }
    }
    if start < style.len()
        && let Some(pair) = parse_decl_slice(&style[start..])
    {
        decls.push(pair);
    }
    decls
}

fn parse_decl_slice(raw: &str) -> Option<(&str, &str)> {
    let (prop, val) = raw.split_once(':')?;
    let prop = prop.trim();
    let val = val.trim().trim_end_matches("!important").trim();
    (!prop.is_empty() && !val.is_empty()).then_some((prop, val))
}

fn parse_css_percentage(raw: &str) -> Option<f32> {
    let pct = raw.trim().strip_suffix('%')?.trim().parse::<f32>().ok()?;
    pct.is_finite().then_some(pct / 100.0)
}

fn parse_css_length_px(raw: &str, base_font_px: f32) -> Option<f32> {
    let s = raw.trim().to_ascii_lowercase();
    let (num_str, factor) = if let Some(n) = s.strip_suffix("px") {
        (n, 1.0)
    } else if let Some(n) = s.strip_suffix("rem") {
        (n, 16.0)
    } else if let Some(n) = s.strip_suffix("em") {
        (n, base_font_px)
    } else if let Some(n) = s.strip_suffix("pt") {
        (n, 1.333_333)
    } else if s.ends_with('%') {
        return None;
    } else {
        (s.as_str(), 1.0)
    };
    let val = num_str.trim().parse::<f32>().ok()?;
    (val.is_finite() && val >= 0.0).then_some((val * factor).min(16_384.0))
}

fn parse_css_four_sides(raw: &str, max_px: f32) -> [f32; 4] {
    let values: Vec<f32> = raw
        .split_whitespace()
        .filter_map(|tok| parse_css_length_px(tok, 16.0))
        .map(|px| px.clamp(0.0, max_px))
        .collect();
    match values.as_slice() {
        [all] => [*all, *all, *all, *all],
        [v, h] => [*v, *h, *v, *h],
        [t, h, b] => [*t, *h, *b, *h],
        [t, r, b, l, ..] => [*t, *r, *b, *l],
        _ => [0.0; 4],
    }
}

fn parse_css_border(raw: &str) -> (f32, Option<RgbaColor>) {
    let mut width = 0.0_f32;
    let color = extract_css_color_token(raw);
    for token in raw.split_whitespace() {
        if token.eq_ignore_ascii_case("none") {
            return (0.0, None);
        }
        if let Some(px) = parse_css_length_px(token, 16.0) {
            width = px.clamp(0.0, 6.0);
        } else if token.eq_ignore_ascii_case("solid") && width == 0.0 {
            width = 1.0;
        }
    }
    (width, color)
}

fn extract_css_color_token(raw: &str) -> Option<RgbaColor> {
    let trimmed = raw.trim();
    if let Some(c) = parse_css_color(trimmed) {
        return Some(c);
    }
    let lower = trimmed.to_ascii_lowercase();
    for prefix in ["rgba(", "rgb("] {
        if let Some(start) = lower.find(prefix)
            && let Some(end_rel) = trimmed[start..].find(')')
            && let Some(c) = parse_css_color(&trimmed[start..=start + end_rel])
        {
            return Some(c);
        }
    }
    for token in trimmed.split_whitespace() {
        if let Some(c) = parse_css_color(token) {
            return Some(c);
        }
    }
    None
}

pub(super) fn parse_css_color(raw: &str) -> Option<RgbaColor> {
    let s = raw.trim().to_ascii_lowercase();
    if s == "transparent" {
        return Some(RgbaColor {
            r: 0,
            g: 0,
            b: 0,
            a: 0,
        });
    }
    if let Some(hex) = s.strip_prefix('#') {
        return parse_hex_rgba(hex);
    }
    if let Some(inner) = s.strip_prefix("rgba(").and_then(|v| v.strip_suffix(')')) {
        return parse_rgb_fn(inner, true);
    }
    if let Some(inner) = s.strip_prefix("rgb(").and_then(|v| v.strip_suffix(')')) {
        return parse_rgb_fn(inner, false);
    }
    named_css_color(&s)
}

fn parse_hex_rgba(hex: &str) -> Option<RgbaColor> {
    let bytes = hex.as_bytes();
    let hex_digit = |b: u8| -> Option<u8> {
        match b {
            b'0'..=b'9' => Some(b - b'0'),
            b'a'..=b'f' => Some(b - b'a' + 10),
            _ => None,
        }
    };
    let byte2 = |hi: u8, lo: u8| -> Option<u8> { Some((hex_digit(hi)? << 4) | hex_digit(lo)?) };
    match bytes.len() {
        3 | 4 => {
            let r = hex_digit(bytes[0])? * 17;
            let g = hex_digit(bytes[1])? * 17;
            let b = hex_digit(bytes[2])? * 17;
            let a = if bytes.len() == 4 {
                hex_digit(bytes[3])? * 17
            } else {
                255
            };
            Some(RgbaColor { r, g, b, a })
        }
        6 | 8 => {
            let r = byte2(bytes[0], bytes[1])?;
            let g = byte2(bytes[2], bytes[3])?;
            let b = byte2(bytes[4], bytes[5])?;
            let a = if bytes.len() == 8 {
                byte2(bytes[6], bytes[7])?
            } else {
                255
            };
            Some(RgbaColor { r, g, b, a })
        }
        _ => None,
    }
}

fn parse_rgb_fn(inner: &str, expect_alpha_default: bool) -> Option<RgbaColor> {
    let (rgb_part, slash_alpha) = if let Some((left, right)) = inner.split_once('/') {
        (left, Some(right.trim()))
    } else {
        (inner, None)
    };
    let parts: Vec<&str> = if rgb_part.contains(',') {
        rgb_part.split(',').map(str::trim).collect()
    } else {
        rgb_part.split_whitespace().collect()
    };
    if parts.len() < 3 {
        return None;
    }
    let parse_chan = |tok: &str| -> Option<u8> {
        if let Some(pct) = parse_css_percentage(tok) {
            Some((pct.clamp(0.0, 1.0) * 255.0).round() as u8)
        } else {
            let v = tok.parse::<f32>().ok()?;
            Some(v.round().clamp(0.0, 255.0) as u8)
        }
    };
    let r = parse_chan(parts[0])?;
    let g = parse_chan(parts[1])?;
    let b = parse_chan(parts[2])?;
    let alpha_tok = slash_alpha.or_else(|| parts.get(3).copied());
    let a = if let Some(tok) = alpha_tok {
        if let Some(pct) = parse_css_percentage(tok) {
            (pct.clamp(0.0, 1.0) * 255.0).round() as u8
        } else {
            let v = tok.parse::<f32>().ok()?;
            if v <= 1.0 && (expect_alpha_default || tok.contains('.')) {
                (v.clamp(0.0, 1.0) * 255.0).round() as u8
            } else {
                v.round().clamp(0.0, 255.0) as u8
            }
        }
    } else {
        255
    };
    Some(RgbaColor { r, g, b, a })
}

fn named_css_color(name: &str) -> Option<RgbaColor> {
    let rgb = match name {
        "black" => (0, 0, 0),
        "white" => (255, 255, 255),
        "red" => (220, 40, 40),
        "crimson" => (220, 20, 60),
        "tomato" => (255, 99, 71),
        "coral" => (255, 127, 80),
        "orange" => (255, 165, 0),
        "gold" => (255, 215, 0),
        "yellow" => (255, 255, 0),
        "lime" => (0, 255, 0),
        "green" => (0, 128, 0),
        "olive" => (128, 128, 0),
        "teal" => (0, 128, 128),
        "cyan" | "aqua" => (0, 255, 255),
        "blue" => (30, 110, 240),
        "navy" => (0, 0, 128),
        "indigo" => (75, 0, 130),
        "purple" => (128, 0, 128),
        "violet" => (238, 130, 238),
        "magenta" | "fuchsia" => (255, 0, 255),
        "pink" => (255, 192, 203),
        "brown" => (165, 42, 42),
        "maroon" => (128, 0, 0),
        "silver" => (192, 192, 192),
        "gray" | "grey" => (128, 128, 128),
        _ => return None,
    };
    Some(RgbaColor {
        r: rgb.0,
        g: rgb.1,
        b: rgb.2,
        a: 255,
    })
}
