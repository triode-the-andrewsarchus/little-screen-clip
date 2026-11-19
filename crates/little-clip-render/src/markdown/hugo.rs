//! Best-effort parsing of common Hugo shortcodes for Markdown previews.
//!
//! Supports built-in/common (`mermaid`, `katex`, `latex`, `code`, `details`, `hl`)
//! and custom shortcodes (`spoiler`, `smalltext`, `attribution`, `hint`, `quote`,
//! `bigquote`, `tabs`, `tab`, `columns`, `columns2`, `columns3`) in both
//! `{{< ... >}}` and `{{% ... %}}` delimiters while leaving unknown shortcodes
//! and escaped/fenced code intact and preserving UTF-8 source byte offsets.

use super::{
    BlockBoxStyle, InlineCss, MarkdownDocBlock, MarkdownSpan, MarkdownTab, QuoteKind,
    parse_located_body_inner, parse_markdown_spans, push_span,
};

const MAX_SHORTCODE_DEPTH: usize = 16;
const MAX_TAG_BYTES: usize = 2048;

#[derive(Debug, Clone)]
pub(super) enum InlineReplacement {
    Math { source: String, display: bool },
    HighlightedCode { language: String, code: String },
    Spans(Vec<MarkdownSpan>),
}

#[derive(Debug, Clone)]
pub(super) enum BlockReplacement {
    Code {
        language: String,
        text: String,
        incomplete: bool,
    },
    Math {
        source: String,
        display: bool,
    },
    Doc(Vec<MarkdownDocBlock>),
}

#[derive(Debug, Clone, Copy)]
struct OffsetShift {
    rew_pos: usize,
    orig_pos: usize,
}

#[derive(Debug)]
pub(super) struct PreparedShortcodes {
    pub(super) rewritten: String,
    shifts: Vec<OffsetShift>,
    pub(super) inlines: Vec<InlineReplacement>,
    pub(super) blocks: Vec<BlockReplacement>,
    pub(super) inline_open: char,
    pub(super) inline_close: char,
    pub(super) block_open: char,
    pub(super) block_close: char,
}

impl PreparedShortcodes {
    pub(super) fn map_offset(&self, rew_pos: usize) -> usize {
        self.map_offset_bound(rew_pos, false)
    }

    pub(super) fn map_range(&self, range: std::ops::Range<usize>) -> std::ops::Range<usize> {
        self.map_offset_bound(range.start, false)..self.map_offset_bound(range.end, true)
    }

    fn map_offset_bound(&self, rew_pos: usize, is_end: bool) -> usize {
        let idx = self
            .shifts
            .partition_point(|shift| shift.rew_pos <= rew_pos);
        if idx == 0 {
            return rew_pos;
        }
        let prev = self.shifts[idx - 1];
        // Odd `idx` falls inside a synthetic token interval `(rew_start..rew_end)` whose
        // original span is `[orig_start..orig_end]`; avoid mapping into mid-token UTF-8 bytes.
        if idx % 2 == 1 {
            if is_end
                && rew_pos > prev.rew_pos
                && let Some(next) = self.shifts.get(idx)
            {
                return next.orig_pos;
            }
            return prev.orig_pos;
        }
        prev.orig_pos + rew_pos.saturating_sub(prev.rew_pos)
    }

    pub(super) fn block_at(&self, token_text: &str) -> Option<&BlockReplacement> {
        let trimmed = token_text.trim();
        let rest = trimmed.strip_prefix(self.block_open)?;
        let digits = rest.strip_suffix(self.block_close)?;
        let id = digits.parse::<usize>().ok()?;
        self.blocks.get(id)
    }

    pub(super) fn contains_block_sentinel(&self, text: &str) -> bool {
        text.contains(self.block_open)
    }

    pub(super) fn expand_inline_text(
        &self,
        text: &str,
        url: Option<&str>,
        css: InlineCss,
        out: &mut Vec<MarkdownSpan>,
    ) {
        if !text.contains(self.inline_open) {
            super::html::push_styled_text(out, text.to_owned(), url, css);
            return;
        }
        let mut cursor = 0usize;
        while let Some(rel_start) = text[cursor..].find(self.inline_open) {
            let start = cursor + rel_start;
            let after_open = &text[start + self.inline_open.len_utf8()..];
            if let Some(rel_end) = after_open.find(self.inline_close)
                && let Ok(id) = after_open[..rel_end].parse::<usize>()
                && let Some(item) = self.inlines.get(id)
            {
                if start > cursor {
                    super::html::push_styled_text(out, text[cursor..start].to_owned(), url, css);
                }
                match item {
                    InlineReplacement::Math { source, display } => {
                        out.push(MarkdownSpan::Math {
                            source: source.clone(),
                            display: *display,
                            url: url.map(str::to_owned),
                        });
                    }
                    InlineReplacement::HighlightedCode { language, code } => {
                        out.push(MarkdownSpan::HighlightedCode {
                            language: language.clone(),
                            code: code.clone(),
                        });
                    }
                    InlineReplacement::Spans(spans) => {
                        for span in spans {
                            let styled = if url.is_some()
                                || !css.is_plain_compatible()
                                || css.bold
                                || css.italic
                                || css.strikethrough
                                || css.monospace
                            {
                                style_single_span(span.clone(), url, css)
                            } else {
                                span.clone()
                            };
                            push_span(out, styled);
                        }
                    }
                }
                cursor =
                    start + self.inline_open.len_utf8() + rel_end + self.inline_close.len_utf8();
                continue;
            }
            let next = start + self.inline_open.len_utf8();
            super::html::push_styled_text(out, text[cursor..next].to_owned(), url, css);
            cursor = next;
        }
        if cursor < text.len() {
            super::html::push_styled_text(out, text[cursor..].to_owned(), url, css);
        }
    }

    pub(super) fn expand_plain_alt(&self, text: &str) -> String {
        if !text.contains(self.inline_open) {
            return text.to_owned();
        }
        let mut out = String::with_capacity(text.len());
        let mut cursor = 0usize;
        while let Some(rel_start) = text[cursor..].find(self.inline_open) {
            let start = cursor + rel_start;
            let after_open = &text[start + self.inline_open.len_utf8()..];
            if let Some(rel_end) = after_open.find(self.inline_close)
                && let Ok(id) = after_open[..rel_end].parse::<usize>()
                && let Some(item) = self.inlines.get(id)
            {
                out.push_str(&text[cursor..start]);
                match item {
                    InlineReplacement::Math { source, .. } => out.push_str(source),
                    InlineReplacement::HighlightedCode { code, .. } => out.push_str(code),
                    InlineReplacement::Spans(spans) => {
                        out.push_str(&super::plain_text_from_spans(spans));
                    }
                }
                cursor =
                    start + self.inline_open.len_utf8() + rel_end + self.inline_close.len_utf8();
                continue;
            }
            let next = start + self.inline_open.len_utf8();
            out.push_str(&text[cursor..next]);
            cursor = next;
        }
        out.push_str(&text[cursor..]);
        out
    }

    pub(super) fn expand_span_list(&self, spans: Vec<MarkdownSpan>) -> Vec<MarkdownSpan> {
        if !spans.iter().any(|span| self.span_contains_inline(span)) {
            return spans;
        }
        let mut out = Vec::with_capacity(spans.len());
        for span in spans {
            if let Some((text, url, css)) = inline_span_parts(&span)
                && text.contains(self.inline_open)
            {
                self.expand_inline_text(text, url, css, &mut out);
            } else if let MarkdownSpan::Spoiler { spans } = span {
                push_span(
                    &mut out,
                    MarkdownSpan::Spoiler {
                        spans: self.expand_span_list(spans),
                    },
                );
            } else {
                push_span(&mut out, span);
            }
        }
        out
    }

    fn span_contains_inline(&self, span: &MarkdownSpan) -> bool {
        match span {
            MarkdownSpan::Spoiler { spans } => spans.iter().any(|s| self.span_contains_inline(s)),
            other => inline_span_parts(other).is_some_and(|(t, ..)| t.contains(self.inline_open)),
        }
    }

    pub(super) fn expand_block_spans(&self, block: &mut MarkdownDocBlock) {
        transform_block_spans(block, &mut |spans| {
            *spans = self.expand_span_list(std::mem::take(spans));
        });
    }
}

fn inline_span_parts(span: &MarkdownSpan) -> Option<(&str, Option<&str>, InlineCss)> {
    match span {
        MarkdownSpan::Text(t) => Some((t, None, InlineCss::default())),
        MarkdownSpan::Bold(t) => Some((
            t,
            None,
            InlineCss {
                bold: true,
                ..Default::default()
            },
        )),
        MarkdownSpan::Italic(t) => Some((
            t,
            None,
            InlineCss {
                italic: true,
                ..Default::default()
            },
        )),
        MarkdownSpan::BoldItalic(t) => Some((
            t,
            None,
            InlineCss {
                bold: true,
                italic: true,
                ..Default::default()
            },
        )),
        MarkdownSpan::Strikethrough(t) => Some((
            t,
            None,
            InlineCss {
                strikethrough: true,
                ..Default::default()
            },
        )),
        MarkdownSpan::Link { label, url } => {
            Some((label, Some(url.as_str()), InlineCss::default()))
        }
        MarkdownSpan::Styled { text, url, css } => Some((text, url.as_deref(), *css)),
        _ => None,
    }
}

/// Applies `transform` to every inline span vector inside a `MarkdownDocBlock` tree,
/// keeping heading plain-text summaries synchronized automatically.
fn transform_block_spans(
    block: &mut MarkdownDocBlock,
    transform: &mut impl FnMut(&mut Vec<MarkdownSpan>),
) {
    match block {
        MarkdownDocBlock::Heading { text, spans, .. } => {
            transform(spans);
            *text = super::plain_text_from_spans(spans);
        }
        MarkdownDocBlock::Paragraph { spans } => transform(spans),
        MarkdownDocBlock::Quote { blocks, .. }
        | MarkdownDocBlock::Footnote { blocks, .. }
        | MarkdownDocBlock::StyledContainer { blocks, .. } => {
            for inner in blocks {
                transform_block_spans(inner, transform);
            }
        }
        MarkdownDocBlock::PullQuote { source, blocks, .. } => {
            transform(source);
            for inner in blocks {
                transform_block_spans(inner, transform);
            }
        }
        MarkdownDocBlock::Details {
            summary, blocks, ..
        } => {
            transform(summary);
            for inner in blocks {
                transform_block_spans(inner, transform);
            }
        }
        MarkdownDocBlock::Tabs { tabs } => {
            for tab in tabs {
                for inner in &mut tab.blocks {
                    transform_block_spans(inner, transform);
                }
            }
        }
        MarkdownDocBlock::Columns { columns, .. } => {
            for col in columns {
                for inner in col {
                    transform_block_spans(inner, transform);
                }
            }
        }
        MarkdownDocBlock::List { items, .. } => {
            for item in items {
                for inner in &mut item.blocks {
                    transform_block_spans(inner, transform);
                }
            }
        }
        MarkdownDocBlock::Table(table) => {
            for cell in &mut table.headers {
                transform(cell);
            }
            for row in &mut table.rows {
                for cell in row {
                    transform(cell);
                }
            }
        }
        MarkdownDocBlock::FrontMatter(_)
        | MarkdownDocBlock::Code { .. }
        | MarkdownDocBlock::Rule
        | MarkdownDocBlock::IncompleteMath { .. } => {}
    }
}

/// Splits any paragraph spans containing Hugo block sentinels into separate
/// paragraphs and resolved native blocks (`Code`, `Mermaid`, `Details`, `Math`).
pub(super) fn split_paragraph_or_blocks(
    spans: Vec<MarkdownSpan>,
    prepared: Option<&PreparedShortcodes>,
) -> Vec<(MarkdownDocBlock, bool)> {
    let Some(prepared) = prepared else {
        return vec![(MarkdownDocBlock::Paragraph { spans }, false)];
    };
    let has_block = spans.iter().any(|span| match span {
        MarkdownSpan::Text(t) => prepared.contains_block_sentinel(t),
        MarkdownSpan::Styled { text, .. } => prepared.contains_block_sentinel(text),
        _ => false,
    });
    if !has_block {
        return vec![(MarkdownDocBlock::Paragraph { spans }, false)];
    }

    let mut out = Vec::new();
    let mut current_spans = Vec::new();
    let flush_para = |current: &mut Vec<MarkdownSpan>, out: &mut Vec<(MarkdownDocBlock, bool)>| {
        trim_edge_breaks(current);
        if !current.is_empty() {
            out.push((
                MarkdownDocBlock::Paragraph {
                    spans: std::mem::take(current),
                },
                false,
            ));
        }
    };

    for span in spans {
        match span {
            MarkdownSpan::Text(t) if prepared.contains_block_sentinel(&t) => {
                split_text_with_block_tokens(
                    &t,
                    None,
                    InlineCss::default(),
                    prepared,
                    &mut current_spans,
                    &flush_para,
                    &mut out,
                );
            }
            MarkdownSpan::Styled { text, url, css } if prepared.contains_block_sentinel(&text) => {
                split_text_with_block_tokens(
                    &text,
                    url.as_deref(),
                    css,
                    prepared,
                    &mut current_spans,
                    &flush_para,
                    &mut out,
                );
            }
            other => push_span(&mut current_spans, other),
        }
    }
    flush_para(&mut current_spans, &mut out);
    out
}

fn trim_edge_breaks(spans: &mut Vec<MarkdownSpan>) {
    while let Some(first) = spans.first_mut() {
        match first {
            MarkdownSpan::HardBreak => {
                spans.remove(0);
            }
            MarkdownSpan::Text(t) if t.trim().is_empty() => {
                spans.remove(0);
            }
            _ => break,
        }
    }
    while let Some(last) = spans.last_mut() {
        match last {
            MarkdownSpan::HardBreak => {
                spans.pop();
            }
            MarkdownSpan::Text(t) if t.trim().is_empty() => {
                spans.pop();
            }
            _ => break,
        }
    }
}

fn split_text_with_block_tokens(
    text: &str,
    url: Option<&str>,
    css: InlineCss,
    prepared: &PreparedShortcodes,
    current_spans: &mut Vec<MarkdownSpan>,
    flush_para: &impl Fn(&mut Vec<MarkdownSpan>, &mut Vec<(MarkdownDocBlock, bool)>),
    out: &mut Vec<(MarkdownDocBlock, bool)>,
) {
    let mut cursor = 0usize;
    while let Some(rel_start) = text[cursor..].find(prepared.block_open) {
        let start = cursor + rel_start;
        let after_open = &text[start + prepared.block_open.len_utf8()..];
        if let Some(rel_end) = after_open.find(prepared.block_close)
            && let Ok(id) = after_open[..rel_end].parse::<usize>()
            && let Some(block) = prepared.blocks.get(id)
        {
            if start > cursor {
                super::html::push_styled_text(
                    current_spans,
                    text[cursor..start].to_owned(),
                    url,
                    css,
                );
            }
            flush_para(current_spans, out);
            for doc in block.to_doc_blocks() {
                out.push(doc);
            }
            cursor =
                start + prepared.block_open.len_utf8() + rel_end + prepared.block_close.len_utf8();
            continue;
        }
        let next = start + prepared.block_open.len_utf8();
        super::html::push_styled_text(current_spans, text[cursor..next].to_owned(), url, css);
        cursor = next;
    }
    if cursor < text.len() {
        super::html::push_styled_text(current_spans, text[cursor..].to_owned(), url, css);
    }
}

impl BlockReplacement {
    pub(super) fn to_doc_blocks(&self) -> Vec<(MarkdownDocBlock, bool)> {
        match self {
            BlockReplacement::Code {
                language,
                text,
                incomplete,
            } => vec![(
                MarkdownDocBlock::Code {
                    language: language.clone(),
                    text: text.clone(),
                },
                *incomplete,
            )],
            BlockReplacement::Math { source, display } => vec![(
                MarkdownDocBlock::Paragraph {
                    spans: vec![MarkdownSpan::Math {
                        source: source.clone(),
                        display: *display,
                        url: None,
                    }],
                },
                false,
            )],
            BlockReplacement::Doc(blocks) => blocks.iter().cloned().map(|b| (b, false)).collect(),
        }
    }
}

/// Scans one logical line for escaped characters, inline backtick runs, and supported shortcodes.
/// Returns `(next_input_pos, had_shortcode_match)`.
fn scan_prepared_line(
    input: &str,
    line_start: usize,
    line_end: usize,
    depth: usize,
    prepared: &mut PreparedShortcodes,
) -> (usize, bool) {
    let bytes = input.as_bytes();
    let mut pos = line_start;
    let mut has_any = false;

    while pos < line_end {
        let b = bytes[pos];
        if b == b'\\' && pos + 1 < line_end {
            let next_len = input[pos + 1..line_end]
                .chars()
                .next()
                .map_or(1, char::len_utf8);
            let escaped_end = (pos + 1 + next_len).min(line_end);
            prepared.rewritten.push_str(&input[pos..escaped_end]);
            pos = escaped_end;
            continue;
        }
        if b == b'`' {
            let tick_run = bytes[pos..line_end]
                .iter()
                .take_while(|&&ch| ch == b'`')
                .count();
            if let Some(close_end) = find_inline_code_end(input, pos + tick_run, tick_run) {
                prepared.rewritten.push_str(&input[pos..close_end]);
                pos = close_end;
                if pos > line_end {
                    return (pos, has_any);
                }
                continue;
            }
            prepared.rewritten.push_str(&input[pos..pos + tick_run]);
            pos += tick_run;
            continue;
        }
        if has_tag_prefix(&bytes[pos..])
            && let Some(parsed) = parse_shortcode_tag(input, pos)
            && !parsed.close
            && is_supported_name(&parsed.name)
            && let Some(res) = handle_open_shortcode(input, &parsed, depth, prepared)
        {
            has_any = true;
            pos = res.next_input_pos;
            if pos > line_end {
                return (pos, has_any);
            }
            continue;
        }

        let ch = input[pos..].chars().next().expect("valid utf8");
        prepared.rewritten.push(ch);
        pos += ch.len_utf8();
    }

    (pos, has_any)
}

/// Scans `input` for recognized Hugo shortcodes outside Markdown fences and
/// backtick code spans, returning `None` immediately if no shortcode delimiters appear.
pub(super) fn prepare_shortcodes(input: &str, depth: usize) -> Option<PreparedShortcodes> {
    if depth >= MAX_SHORTCODE_DEPTH || (!input.contains("{{<") && !input.contains("{{%")) {
        return None;
    }
    let (inline_open, inline_close, block_open, block_close) = choose_sentinels(input)?;
    let mut prepared = PreparedShortcodes {
        rewritten: String::with_capacity(input.len()),
        shifts: Vec::new(),
        inlines: Vec::new(),
        blocks: Vec::new(),
        inline_open,
        inline_close,
        block_open,
        block_close,
    };

    let bytes = input.as_bytes();
    let mut cursor = 0usize;
    let mut active_fence: Option<(u8, usize)> = None;
    let mut has_any = false;

    while cursor < input.len() {
        let line_start = cursor;
        let line_end = input[line_start..]
            .find('\n')
            .map_or(input.len(), |n| line_start + n + 1);
        let raw_line = input[line_start..line_end].trim_end_matches(['\r', '\n']);
        let is_real_line_start = line_start == 0 || bytes[line_start - 1] == b'\n';

        if let Some((marker, min_run)) = active_fence {
            if is_real_line_start && is_closing_fence(raw_line, marker, min_run) {
                active_fence = None;
            }
            prepared.rewritten.push_str(&input[line_start..line_end]);
            cursor = line_end;
            continue;
        }
        if is_real_line_start && let Some((marker, run)) = opening_fence(raw_line) {
            active_fence = Some((marker, run));
            prepared.rewritten.push_str(&input[line_start..line_end]);
            cursor = line_end;
            continue;
        }

        let (next_pos, line_matched) =
            scan_prepared_line(input, line_start, line_end, depth, &mut prepared);
        has_any |= line_matched;
        cursor = next_pos;
    }

    has_any.then_some(prepared)
}

struct MatchedShortcode {
    next_input_pos: usize,
}

fn parse_container_blocks(raw: &str, depth: usize) -> Vec<MarkdownDocBlock> {
    let cleaned = clean_container_body(raw);
    parse_located_body_inner(&cleaned, 0, depth + 1)
        .into_iter()
        .map(|b| b.block)
        .collect()
}

fn push_block_replacement(
    prepared: &mut PreparedShortcodes,
    replacement: BlockReplacement,
    orig_start: usize,
    orig_end: usize,
) -> MatchedShortcode {
    let id = prepared.blocks.len();
    prepared.blocks.push(replacement);
    emit_sentinel_token(
        prepared,
        prepared.block_open,
        id,
        prepared.block_close,
        orig_start,
        orig_end,
    );
    MatchedShortcode {
        next_input_pos: orig_end,
    }
}

fn push_inline_replacement(
    prepared: &mut PreparedShortcodes,
    replacement: InlineReplacement,
    orig_start: usize,
    orig_end: usize,
) -> MatchedShortcode {
    let id = prepared.inlines.len();
    prepared.inlines.push(replacement);
    emit_sentinel_token(
        prepared,
        prepared.inline_open,
        id,
        prepared.inline_close,
        orig_start,
        orig_end,
    );
    MatchedShortcode {
        next_input_pos: orig_end,
    }
}

fn handle_raw_block_shortcode(
    input: &str,
    open: &ParsedTag,
    prepared: &mut PreparedShortcodes,
) -> MatchedShortcode {
    let language = if open.name == "mermaid" {
        "mermaid".to_owned()
    } else {
        extract_code_language(&open.args)
    };
    let (raw_inner, orig_end, incomplete) = if let Some((close_start, close_end)) =
        find_closing_tag(input, open.end, &open.name, false)
    {
        (&input[open.end..close_start], close_end, false)
    } else {
        (&input[open.end..], input.len(), true)
    };
    let text = clean_raw_block_body(raw_inner, open.name == "mermaid");
    push_block_replacement(
        prepared,
        BlockReplacement::Code {
            language,
            text,
            incomplete,
        },
        open.start,
        orig_end,
    )
}

fn handle_container_block_shortcode(
    input: &str,
    open: &ParsedTag,
    depth: usize,
    prepared: &mut PreparedShortcodes,
) -> Option<MatchedShortcode> {
    let (close_start, close_end) = find_closing_tag(input, open.end, &open.name, true)?;
    let raw_slice = &input[open.end..close_start];
    let doc_blocks = match open.name.as_str() {
        "details" => {
            let (summary_text, is_open) = extract_details_params(&open.args);
            vec![MarkdownDocBlock::Details {
                summary: super::parse_markdown_spans_at_depth(&summary_text, depth + 1),
                open: is_open,
                blocks: parse_container_blocks(raw_slice, depth),
            }]
        }
        "hint" => vec![MarkdownDocBlock::Quote {
            kind: extract_hint_kind(&open.args),
            blocks: parse_container_blocks(raw_slice, depth),
        }],
        "quote" | "bigquote" => {
            let source_text = extract_quote_source(&open.args);
            let source = if source_text.is_empty() {
                Vec::new()
            } else {
                super::parse_markdown_spans_at_depth(&source_text, depth + 1)
            };
            vec![MarkdownDocBlock::PullQuote {
                big: open.name == "bigquote",
                source,
                blocks: parse_container_blocks(raw_slice, depth),
            }]
        }
        "attribution" => {
            let mut blocks = parse_container_blocks(raw_slice, depth);
            let caption_css = InlineCss {
                small: true,
                muted: true,
                ..Default::default()
            };
            style_doc_blocks_inline(&mut blocks, caption_css);
            vec![MarkdownDocBlock::StyledContainer {
                style: BlockBoxStyle {
                    margin_bottom: 10.0,
                    ..Default::default()
                },
                blocks,
            }]
        }
        _ => return None,
    };
    Some(push_block_replacement(
        prepared,
        BlockReplacement::Doc(doc_blocks),
        open.start,
        close_end,
    ))
}

fn handle_tabs_or_columns_shortcode(
    input: &str,
    open: &ParsedTag,
    depth: usize,
    prepared: &mut PreparedShortcodes,
) -> Option<MatchedShortcode> {
    let (close_start, close_end) = find_closing_tag(input, open.end, &open.name, true)?;
    let raw_inner = &input[open.end..close_start];
    let doc_blocks = match open.name.as_str() {
        "tabs" => {
            let tabs = parse_tabs_inner(raw_inner, depth);
            if tabs.is_empty() {
                Vec::new()
            } else {
                vec![MarkdownDocBlock::Tabs { tabs }]
            }
        }
        "tab" => {
            let title = extract_tab_title(&open.args, 0);
            let blocks = parse_container_blocks(raw_inner, depth);
            vec![MarkdownDocBlock::Tabs {
                tabs: vec![MarkdownTab { title, blocks }],
            }]
        }
        name => {
            let mut columns: Vec<Vec<MarkdownDocBlock>> = split_columns_inner(raw_inner)
                .into_iter()
                .map(|col_raw| parse_container_blocks(col_raw, depth))
                .collect();
            while columns.len() > 1 && columns.last().is_some_and(Vec::is_empty) {
                columns.pop();
            }
            vec![MarkdownDocBlock::Columns {
                dividers: name == "columns3",
                bottom_rule: name == "columns2",
                columns,
            }]
        }
    };
    Some(push_block_replacement(
        prepared,
        BlockReplacement::Doc(doc_blocks),
        open.start,
        close_end,
    ))
}

fn handle_smalltext_or_spoiler_shortcode(
    input: &str,
    open: &ParsedTag,
    depth: usize,
    prepared: &mut PreparedShortcodes,
) -> Option<MatchedShortcode> {
    if open.name == "spoiler" {
        let (close_start, close_end) =
            find_closing_tag_within_paragraph(input, open.end, "spoiler", true)?;
        let inner = input[open.end..close_start].trim();
        let spans = super::parse_markdown_spans_at_depth(inner, depth + 1);
        return Some(push_inline_replacement(
            prepared,
            InlineReplacement::Spans(vec![MarkdownSpan::Spoiler { spans }]),
            open.start,
            close_end,
        ));
    }

    let small_css = InlineCss {
        small: true,
        ..Default::default()
    };
    if let Some((close_start, close_end)) =
        find_closing_tag_within_paragraph(input, open.end, "smalltext", true)
    {
        let inner = input[open.end..close_start].trim();
        let spans = style_span_list(
            super::parse_markdown_spans_at_depth(inner, depth + 1),
            small_css,
        );
        return Some(push_inline_replacement(
            prepared,
            InlineReplacement::Spans(spans),
            open.start,
            close_end,
        ));
    }
    let (close_start, close_end) = find_closing_tag(input, open.end, "smalltext", true)?;
    let mut blocks = parse_container_blocks(&input[open.end..close_start], depth);
    style_doc_blocks_inline(&mut blocks, small_css);
    Some(push_block_replacement(
        prepared,
        BlockReplacement::Doc(blocks),
        open.start,
        close_end,
    ))
}

fn handle_katex_shortcode(
    input: &str,
    open: &ParsedTag,
    prepared: &mut PreparedShortcodes,
) -> Option<MatchedShortcode> {
    let explicit_display = has_display_flag(&open.args);
    if let Some((close_start, close_end)) = find_closing_tag(input, open.end, &open.name, false) {
        let raw_inner = &input[open.end..close_start];
        let standalone =
            line_prefix_is_blank(input, open.start) && line_suffix_is_blank(input, close_end);
        let multiline = raw_inner.contains('\n');
        let source = raw_inner.trim().to_owned();
        if standalone && (explicit_display || multiline) {
            return Some(push_block_replacement(
                prepared,
                BlockReplacement::Math {
                    source,
                    display: true,
                },
                open.start,
                close_end,
            ));
        }
        return Some(push_inline_replacement(
            prepared,
            InlineReplacement::Math {
                source,
                display: explicit_display || (standalone && multiline),
            },
            open.start,
            close_end,
        ));
    }
    // Standalone zero-body script loader call such as `{{< katex >}}` with no matching closing tag.
    if line_prefix_is_blank(input, open.start) && line_suffix_is_blank(input, open.end) {
        emit_empty_replacement(prepared, open.start, open.end);
        return Some(MatchedShortcode {
            next_input_pos: open.end,
        });
    }
    None
}

fn handle_hl_or_link_shortcode(
    input: &str,
    open: &ParsedTag,
    depth: usize,
    prepared: &mut PreparedShortcodes,
) -> Option<MatchedShortcode> {
    match open.name.as_str() {
        "hl" => {
            let language = extract_hl_language(&open.args);
            if let Some((close_start, close_end)) =
                find_closing_tag_within_paragraph(input, open.end, "hl", false)
            {
                let code = input[open.end..close_start]
                    .trim_matches(['\r', '\n'])
                    .trim()
                    .to_owned();
                return Some(push_inline_replacement(
                    prepared,
                    InlineReplacement::HighlightedCode { language, code },
                    open.start,
                    close_end,
                ));
            }
            let code = open
                .args
                .iter()
                .find(|arg| arg.key.as_deref() == Some("code"))
                .map(|arg| arg.value.clone())?;
            Some(push_inline_replacement(
                prepared,
                InlineReplacement::HighlightedCode { language, code },
                open.start,
                open.end,
            ))
        }
        "ref" | "relref" => {
            let target = open
                .args
                .iter()
                .find(|arg| {
                    matches!(arg.key.as_deref(), Some("path" | "page")) || arg.key.is_none()
                })
                .map(|arg| arg.value.trim())
                .filter(|v| !v.is_empty())?;
            let encoded = encode_link_target(target);
            emit_literal_replacement(prepared, &encoded, open.start, open.end);
            Some(MatchedShortcode {
                next_input_pos: open.end,
            })
        }
        "button" => {
            let (close_start, close_end) =
                find_closing_tag_within_paragraph(input, open.end, "button", true)?;
            let target = open
                .args
                .iter()
                .find(|arg| matches!(arg.key.as_deref(), Some("href" | "relref" | "ref")))
                .map(|arg| arg.value.trim().to_owned())
                .filter(|v| !v.is_empty());
            let inner = input[open.end..close_start].trim();
            let mut spans = super::parse_markdown_spans_at_depth(inner, depth + 1);
            if let Some(url) = target {
                spans = spans
                    .into_iter()
                    .map(|span| style_single_span(span, Some(&url), InlineCss::default()))
                    .collect();
            }
            Some(push_inline_replacement(
                prepared,
                InlineReplacement::Spans(spans),
                open.start,
                close_end,
            ))
        }
        _ => None,
    }
}

fn handle_open_shortcode(
    input: &str,
    open: &ParsedTag,
    depth: usize,
    prepared: &mut PreparedShortcodes,
) -> Option<MatchedShortcode> {
    match open.name.as_str() {
        "code" | "mermaid" => Some(handle_raw_block_shortcode(input, open, prepared)),
        "details" | "hint" | "quote" | "bigquote" | "attribution" => {
            handle_container_block_shortcode(input, open, depth, prepared)
        }
        "tabs" | "tab" => handle_tabs_or_columns_shortcode(input, open, depth, prepared),
        name if is_columns_name(name) => {
            handle_tabs_or_columns_shortcode(input, open, depth, prepared)
        }
        "spoiler" | "smalltext" => {
            handle_smalltext_or_spoiler_shortcode(input, open, depth, prepared)
        }
        "katex" | "latex" => handle_katex_shortcode(input, open, prepared),
        "hl" | "ref" | "relref" | "button" => {
            handle_hl_or_link_shortcode(input, open, depth, prepared)
        }
        _ => None,
    }
}

fn encode_link_target(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        match ch {
            ' ' => out.push_str("%20"),
            '<' => out.push_str("%3C"),
            '>' => out.push_str("%3E"),
            '(' => out.push_str("%28"),
            ')' => out.push_str("%29"),
            '"' => out.push_str("%22"),
            '\\' => out.push_str("%5C"),
            other => out.push(other),
        }
    }
    out
}

fn emit_replacement_segment(
    prepared: &mut PreparedShortcodes,
    orig_start: usize,
    orig_end: usize,
    write_rewritten: impl FnOnce(&mut String),
) {
    let rew_start = prepared.rewritten.len();
    prepared.shifts.push(OffsetShift {
        rew_pos: rew_start,
        orig_pos: orig_start,
    });
    write_rewritten(&mut prepared.rewritten);
    let rew_end = prepared.rewritten.len();
    prepared.shifts.push(OffsetShift {
        rew_pos: rew_end,
        orig_pos: orig_end,
    });
}

fn emit_literal_replacement(
    prepared: &mut PreparedShortcodes,
    literal: &str,
    orig_start: usize,
    orig_end: usize,
) {
    emit_replacement_segment(prepared, orig_start, orig_end, |out| out.push_str(literal));
}

fn emit_sentinel_token(
    prepared: &mut PreparedShortcodes,
    open: char,
    id: usize,
    close: char,
    orig_start: usize,
    orig_end: usize,
) {
    emit_replacement_segment(prepared, orig_start, orig_end, |out| {
        use std::fmt::Write as _;
        out.push(open);
        let _ = write!(out, "{id}");
        out.push(close);
    });
}

fn emit_empty_replacement(prepared: &mut PreparedShortcodes, orig_start: usize, orig_end: usize) {
    emit_replacement_segment(prepared, orig_start, orig_end, |_| {});
}

fn choose_sentinels(input: &str) -> Option<(char, char, char, char)> {
    const SETS: [(char, char, char, char); 3] = [
        ('\u{E000}', '\u{E001}', '\u{E002}', '\u{E003}'),
        ('\u{E010}', '\u{E011}', '\u{E012}', '\u{E013}'),
        ('\u{F0000}', '\u{F0001}', '\u{F0002}', '\u{F0003}'),
    ];
    SETS.into_iter()
        .find(|&(a, b, c, d)| !input.contains([a, b, c, d]))
}

#[derive(Debug, Clone)]
pub(super) struct ShortcodeArg {
    pub(super) key: Option<String>,
    pub(super) value: String,
}

#[derive(Debug, Clone)]
pub(super) struct ParsedTag {
    pub(super) start: usize,
    pub(super) end: usize,
    pub(super) close: bool,
    pub(super) name: String,
    pub(super) args: Vec<ShortcodeArg>,
}

fn is_columns_name(name: &str) -> bool {
    name.strip_prefix("columns")
        .is_some_and(|rest| rest.is_empty() || rest.bytes().all(|b| b.is_ascii_digit()))
}

pub(super) fn is_supported_name(name: &str) -> bool {
    matches!(
        name,
        "mermaid"
            | "katex"
            | "latex"
            | "code"
            | "details"
            | "hl"
            | "spoiler"
            | "smalltext"
            | "attribution"
            | "hint"
            | "quote"
            | "bigquote"
            | "tabs"
            | "tab"
            | "ref"
            | "relref"
            | "button"
    ) || is_columns_name(name)
}

fn scan_shortcode_close_delim(body_slice: &str, close_delim: &[u8]) -> Option<usize> {
    let mut quote: Option<u8> = None;
    let mut escaped = false;
    let bytes = body_slice.as_bytes();
    for (idx, &b) in bytes.iter().enumerate() {
        if let Some(q) = quote {
            if escaped {
                escaped = false;
            } else if b == b'\\' && q != b'`' {
                escaped = true;
            } else if b == q {
                quote = None;
            }
            continue;
        }
        if matches!(b, b'"' | b'\'' | b'`') {
            quote = Some(b);
            continue;
        }
        if b == b'\n'
            && (bytes[idx + 1..].starts_with(b"\n") || bytes[idx + 1..].starts_with(b"\r\n"))
        {
            // Avoid matching stray `{{<` across blank lines if unclosed.
            return None;
        }
        if bytes[idx..].starts_with(close_delim) {
            return Some(idx);
        }
    }
    None
}

pub(super) fn parse_shortcode_tag(input: &str, start: usize) -> Option<ParsedTag> {
    let slice = &input[start..];
    let close_delim = if slice.starts_with("{{<") {
        ">}}"
    } else if slice.starts_with("{{%") {
        "%}}"
    } else {
        return None;
    };

    let inner_start = 3usize;
    let max_end = slice.floor_char_boundary(MAX_TAG_BYTES).max(inner_start);
    let body_slice = &slice[inner_start..max_end];
    if body_slice.starts_with("/*") {
        return None;
    }

    let rel_close = scan_shortcode_close_delim(body_slice, close_delim.as_bytes())?;
    let end = start + inner_start + rel_close + close_delim.len();
    let mut content = body_slice[..rel_close].trim();
    // Support Hugo trim hyphen `{{-` / `-}}` / `-%}` variants cleanly.
    if let Some(stripped) = content.strip_prefix('-') {
        content = stripped.trim_start();
    }
    if let Some(stripped) = content.strip_suffix('-') {
        content = stripped.trim_end();
    }

    let (close, rest) = match content.strip_prefix('/') {
        Some(after_slash) => (true, after_slash.trim_start()),
        None => (false, content),
    };

    let name_end = rest
        .find(|c: char| c.is_whitespace() || c == '/')
        .unwrap_or(rest.len());
    let name = rest[..name_end].to_ascii_lowercase();
    if name.is_empty()
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return None;
    }

    let mut args_raw = rest[name_end..].trim_start();
    if rest.as_bytes().get(name_end) == Some(&b'/') {
        args_raw = args_raw[1..].trim_start();
    }
    args_raw = args_raw.trim_end();
    if !matches!(name.as_str(), "ref" | "relref") {
        args_raw = args_raw.trim_end_matches('/').trim_end();
    }
    let args = if close {
        Vec::new()
    } else {
        parse_shortcode_args(args_raw)
    };

    Some(ParsedTag {
        start,
        end,
        close,
        name,
        args,
    })
}

fn parse_shortcode_args(raw: &str) -> Vec<ShortcodeArg> {
    let mut args = Vec::new();
    let mut pos = 0usize;
    let bytes = raw.as_bytes();
    while pos < raw.len() {
        while pos < raw.len() && bytes[pos].is_ascii_whitespace() {
            pos += 1;
        }
        if pos >= raw.len() {
            break;
        }

        // Check for `key=value`.
        let key_end = bytes[pos..].iter().position(|&b| {
            b.is_ascii_whitespace() || b == b'=' || matches!(b, b'"' | b'\'' | b'`')
        });
        if let Some(rel) = key_end
            && bytes[pos + rel] == b'='
            && rel > 0
        {
            let key = raw[pos..pos + rel].to_ascii_lowercase();
            pos += rel + 1;
            let (value, next) = read_arg_value(raw, pos);
            pos = next;
            args.push(ShortcodeArg {
                key: Some(key),
                value,
            });
            continue;
        }

        let (value, next) = read_arg_value(raw, pos);
        pos = next;
        args.push(ShortcodeArg { key: None, value });
    }
    args
}

fn read_arg_value(raw: &str, start: usize) -> (String, usize) {
    if start >= raw.len() {
        return (String::new(), start);
    }
    let bytes = raw.as_bytes();
    let first = bytes[start];
    if matches!(first, b'"' | b'\'' | b'`') {
        let mut out = String::new();
        let mut i = start + 1;
        let mut escaped = false;
        while i < raw.len() {
            let b = bytes[i];
            if escaped {
                let ch = raw[i..].chars().next().expect("valid utf8");
                out.push(ch);
                escaped = false;
                i += ch.len_utf8();
                continue;
            }
            if b == b'\\' && first != b'`' {
                escaped = true;
                i += 1;
                continue;
            }
            if b == first {
                return (out, i + 1);
            }
            let ch = raw[i..].chars().next().expect("valid utf8");
            out.push(ch);
            i += ch.len_utf8();
        }
        return (out, raw.len());
    }

    let end = raw[start..]
        .find(char::is_whitespace)
        .map_or(raw.len(), |n| start + n);
    (raw[start..end].to_owned(), end)
}

pub(super) fn extract_code_language(args: &[ShortcodeArg]) -> String {
    if let Some(lang) = args
        .iter()
        .find(|arg| matches!(arg.key.as_deref(), Some("lang" | "language")))
        .map(|arg| arg.value.trim().to_ascii_lowercase())
        .filter(|v| !v.is_empty())
    {
        return lang;
    }
    if let Some(pos) = args
        .iter()
        .find(|arg| arg.key.is_none() && !arg.value.trim().is_empty())
        .map(|arg| arg.value.trim().to_ascii_lowercase())
    {
        return pos;
    }
    if let Some(file) = args
        .iter()
        .find(|arg| matches!(arg.key.as_deref(), Some("title" | "file" | "src")))
        .map(|arg| arg.value.trim())
        && let Some(ext) = std::path::Path::new(file)
            .extension()
            .and_then(|e| e.to_str())
        && let Some(lang) = little_clip_core::syntax::map_extension_to_language(ext)
    {
        return lang.to_owned();
    }
    String::new()
}

pub(super) fn extract_hl_language(args: &[ShortcodeArg]) -> String {
    if let Some(lang) = args
        .iter()
        .find(|arg| matches!(arg.key.as_deref(), Some("lang" | "language")))
        .map(|arg| arg.value.trim().to_ascii_lowercase())
        .filter(|v| !v.is_empty())
    {
        return lang;
    }
    args.iter()
        .find(|arg| arg.key.is_none() && !arg.value.trim().is_empty() && !arg.value.contains('='))
        .map(|arg| arg.value.trim().to_ascii_lowercase())
        .unwrap_or_default()
}

pub(super) fn has_display_flag(args: &[ShortcodeArg]) -> bool {
    args.iter().any(|arg| match arg.key.as_deref() {
        None => {
            arg.value.eq_ignore_ascii_case("display") || arg.value.eq_ignore_ascii_case("block")
        }
        Some("display" | "block") => matches!(
            arg.value.trim().to_ascii_lowercase().as_str(),
            "" | "true" | "1" | "yes"
        ),
        Some("mode") => arg.value.trim().eq_ignore_ascii_case("display"),
        _ => false,
    })
}

pub(super) fn extract_details_params(args: &[ShortcodeArg]) -> (String, bool) {
    let open = args.iter().any(|arg| match arg.key.as_deref() {
        None => arg.value.eq_ignore_ascii_case("open"),
        Some("open") => matches!(
            arg.value.trim().to_ascii_lowercase().as_str(),
            "" | "true" | "1" | "yes" | "open"
        ),
        _ => false,
    });
    let summary = args
        .iter()
        .find(|arg| matches!(arg.key.as_deref(), Some("summary" | "title")))
        .map(|arg| arg.value.clone())
        .or_else(|| {
            args.iter()
                .find(|arg| {
                    arg.key.is_none()
                        && !arg.value.trim().is_empty()
                        && !arg.value.eq_ignore_ascii_case("open")
                })
                .map(|arg| arg.value.clone())
        })
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "Details".to_owned());
    (summary, open)
}

fn next_char_step(input: &str, pos: usize) -> (u8, usize) {
    let b = input.as_bytes()[pos];
    let step = if b < 0x80 {
        1
    } else {
        input[pos..].chars().next().map_or(1, char::len_utf8)
    };
    (b, step)
}

#[inline]
fn has_tag_prefix(slice: &[u8]) -> bool {
    slice.starts_with(b"{{<") || slice.starts_with(b"{{%")
}

/// Shared cursor state for scanning shortcode bodies while ignoring fenced blocks,
/// inline backtick spans, and raw `{{< code >}}` / `{{< mermaid >}}` payloads.
struct BlockScanState {
    pos: usize,
    fence: Option<(u8, usize)>,
    at_line_start: bool,
}

impl BlockScanState {
    fn new(pos: usize, at_line_start: bool) -> Self {
        Self {
            pos,
            fence: None,
            at_line_start,
        }
    }

    fn step_fence_or_code(&mut self, input: &str, active: bool) -> bool {
        if !active {
            return false;
        }
        if self.at_line_start {
            let line_end = input[self.pos..]
                .find('\n')
                .map_or(input.len(), |n| self.pos + n + 1);
            let raw_line = input[self.pos..line_end].trim_end_matches(['\r', '\n']);
            if let Some((marker, min_run)) = self.fence {
                if is_closing_fence(raw_line, marker, min_run) {
                    self.fence = None;
                }
                self.pos = line_end;
                return true;
            }
            if let Some((marker, run)) = opening_fence(raw_line) {
                self.fence = Some((marker, run));
                self.pos = line_end;
                return true;
            }
        }
        if let Some(next_pos) = advance_inline_code_span(input, self.pos) {
            self.jump_to(input, next_pos);
            return true;
        }
        false
    }

    fn try_skip_raw_shortcode(&mut self, input: &str, tag: &ParsedTag) -> bool {
        if !tag.close
            && matches!(tag.name.as_str(), "code" | "mermaid")
            && let Some((_, end)) = find_closing_tag(input, tag.end, &tag.name, false)
        {
            self.jump_to(input, end);
            return true;
        }
        false
    }

    fn jump_to(&mut self, input: &str, new_pos: usize) {
        self.at_line_start = input.as_bytes().get(new_pos.saturating_sub(1)) == Some(&b'\n');
        self.pos = new_pos;
    }

    fn step_char(&mut self, input: &str) {
        let (b, step) = next_char_step(input, self.pos);
        self.at_line_start = b == b'\n';
        self.pos += step;
    }
}

fn find_closing_tag(
    input: &str,
    from: usize,
    name: &str,
    track_nesting: bool,
) -> Option<(usize, usize)> {
    let bytes = input.as_bytes();
    let mut depth = 1usize;
    let at_line_start = from == 0 || bytes.get(from - 1) == Some(&b'\n');
    let mut scan = BlockScanState::new(from, at_line_start);

    while scan.pos < input.len() {
        if scan.step_fence_or_code(input, track_nesting) {
            continue;
        }
        if has_tag_prefix(&bytes[scan.pos..])
            && let Some(tag) = parse_shortcode_tag(input, scan.pos)
        {
            if track_nesting && scan.try_skip_raw_shortcode(input, &tag) {
                continue;
            }
            if tag.name == name {
                if tag.close {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        return Some((tag.start, tag.end));
                    }
                } else if track_nesting {
                    depth += 1;
                }
            }
            scan.jump_to(input, tag.end);
            continue;
        }
        scan.step_char(input);
    }
    None
}

fn find_closing_tag_within_paragraph(
    input: &str,
    from: usize,
    name: &str,
    skip_backticks: bool,
) -> Option<(usize, usize)> {
    let bytes = input.as_bytes();
    let mut pos = from;
    while pos < input.len() {
        if bytes[pos..].starts_with(b"\n\n") || bytes[pos..].starts_with(b"\r\n\r\n") {
            return None;
        }
        if skip_backticks && let Some(next_pos) = advance_inline_code_span(input, pos) {
            pos = next_pos;
            continue;
        }
        if has_tag_prefix(&bytes[pos..])
            && let Some(tag) = parse_shortcode_tag(input, pos)
            && tag.close
            && tag.name == name
        {
            return Some((tag.start, tag.end));
        }
        let (_, step) = next_char_step(input, pos);
        pos += step;
    }
    None
}

fn advance_inline_code_span(input: &str, pos: usize) -> Option<usize> {
    let bytes = input.as_bytes();
    if bytes.get(pos) != Some(&b'`') {
        return None;
    }
    let tick_run = bytes[pos..].iter().take_while(|&&ch| ch == b'`').count();
    Some(find_inline_code_end(input, pos + tick_run, tick_run).unwrap_or(pos + tick_run))
}

fn find_inline_code_end(input: &str, from: usize, run_len: usize) -> Option<usize> {
    let bytes = input.as_bytes();
    let mut pos = from;
    while pos < input.len() {
        if bytes[pos..].starts_with(b"\n\n") || bytes[pos..].starts_with(b"\r\n\r\n") {
            return None;
        }
        if bytes[pos] == b'`' {
            let count = bytes[pos..].iter().take_while(|&&ch| ch == b'`').count();
            if count == run_len {
                return Some(pos + count);
            }
            pos += count;
            continue;
        }
        let (_, step) = next_char_step(input, pos);
        pos += step;
    }
    None
}

fn opening_fence(raw_line: &str) -> Option<(u8, usize)> {
    let trimmed_start = raw_line.trim_start();
    let leading = raw_line.len() - trimmed_start.len();
    if leading >= 4 {
        return None;
    }
    let marker = *trimmed_start.as_bytes().first()?;
    if marker != b'`' && marker != b'~' {
        return None;
    }
    let run = trimmed_start
        .as_bytes()
        .iter()
        .take_while(|&&b| b == marker)
        .count();
    let rest = &trimmed_start[run..];
    if run >= 3 && (marker != b'`' || !rest.contains('`')) {
        Some((marker, run))
    } else {
        None
    }
}

fn is_closing_fence(raw_line: &str, marker: u8, min_run: usize) -> bool {
    let trimmed_start = raw_line.trim_start();
    let leading = raw_line.len() - trimmed_start.len();
    if leading >= 4 {
        return false;
    }
    let run = trimmed_start
        .as_bytes()
        .iter()
        .take_while(|&&b| b == marker)
        .count();
    run >= min_run && trimmed_start[run..].trim().is_empty()
}

/// Trims a leading newline after the opening tag and trailing indentation on the closing-tag line.
fn trim_shortcode_line_edges(raw: &str) -> &str {
    let s = raw
        .strip_prefix("\r\n")
        .or_else(|| raw.strip_prefix('\n'))
        .unwrap_or(raw);
    if let Some(last_nl) = s.rfind('\n')
        && s[last_nl + 1..].trim().is_empty()
    {
        return &s[..last_nl];
    }
    s
}

pub(super) fn clean_raw_block_body(raw: &str, trim_blank_edges: bool) -> String {
    let s = trim_shortcode_line_edges(raw);
    if trim_blank_edges {
        return s.trim_matches(['\r', '\n']).trim().to_owned();
    }
    // Preserve meaningful indentation on actual code lines, stripping only trailing CR.
    s.trim_end_matches('\r').to_owned()
}

fn clean_container_body(raw: &str) -> String {
    trim_shortcode_line_edges(raw).to_owned()
}

fn line_prefix_is_blank(input: &str, pos: usize) -> bool {
    let line_start = input[..pos].rfind('\n').map_or(0, |n| n + 1);
    input[line_start..pos]
        .chars()
        .all(|c| c == ' ' || c == '\t')
}

fn line_suffix_is_blank(input: &str, pos: usize) -> bool {
    let line_end = input[pos..].find('\n').map_or(input.len(), |n| pos + n);
    input[pos..line_end]
        .trim_end_matches('\r')
        .chars()
        .all(|c| c == ' ' || c == '\t')
}

/// Line-oriented helper for [`super::parse_body_into`] (`MarkdownLine`) so card
/// previews also interpret block Hugo shortcodes consistently.
pub(super) enum LineBlockStart {
    Code {
        tag_name: &'static str,
        language: String,
        inline_body: Option<String>,
    },
    Details {
        summary_spans: Vec<MarkdownSpan>,
        inline_body: Option<String>,
    },
    Math {
        tag_name: &'static str,
        inline_source: Option<String>,
        display: bool,
    },
    IgnoreLine,
}

fn classify_code_line(tag: &ParsedTag, rest: &str) -> Option<LineBlockStart> {
    let tag_name = if tag.name == "mermaid" {
        "mermaid"
    } else {
        "code"
    };
    let language = if tag_name == "mermaid" {
        "mermaid".to_owned()
    } else {
        extract_code_language(&tag.args)
    };
    let inline_body = strip_closing_on_same_line(rest, tag_name).map(str::to_owned);
    if inline_body.is_some() || rest.is_empty() {
        Some(LineBlockStart::Code {
            tag_name,
            language,
            inline_body,
        })
    } else {
        None
    }
}

fn classify_container_line(tag: &ParsedTag, rest: &str) -> Option<LineBlockStart> {
    if tag.name == "details" {
        let (summary_text, _) = extract_details_params(&tag.args);
        let summary_spans = parse_markdown_spans(&summary_text);
        let inline_body =
            strip_closing_on_same_line(rest, "details").map(|body| body.trim().to_owned());
        return (inline_body.is_some() || rest.is_empty()).then_some(LineBlockStart::Details {
            summary_spans,
            inline_body,
        });
    }
    if let Some(inline_body) = strip_closing_on_same_line(rest, &tag.name) {
        let inner = inline_body.trim();
        let summary_spans = if inner.is_empty() {
            Vec::new()
        } else {
            parse_markdown_spans(inner)
        };
        return Some(if summary_spans.is_empty() {
            LineBlockStart::IgnoreLine
        } else {
            LineBlockStart::Details {
                summary_spans,
                inline_body: None,
            }
        });
    }
    if !rest.is_empty() {
        return None;
    }
    if tag.name == "tab" {
        let title = extract_tab_title(&tag.args, 0);
        Some(LineBlockStart::Details {
            summary_spans: vec![MarkdownSpan::Bold(title)],
            inline_body: None,
        })
    } else {
        Some(LineBlockStart::IgnoreLine)
    }
}

fn classify_math_line(tag: &ParsedTag, rest: &str) -> Option<LineBlockStart> {
    let tag_name = if tag.name == "katex" {
        "katex"
    } else {
        "latex"
    };
    if let Some(inner) = strip_closing_on_same_line(rest, tag_name) {
        Some(LineBlockStart::Math {
            tag_name,
            inline_source: Some(inner.trim().to_owned()),
            display: has_display_flag(&tag.args),
        })
    } else if rest.is_empty() {
        Some(LineBlockStart::Math {
            tag_name,
            inline_source: None,
            display: true,
        })
    } else {
        None
    }
}

pub(super) fn classify_block_line(trimmed: &str) -> Option<LineBlockStart> {
    if !has_tag_prefix(trimmed.as_bytes()) {
        return None;
    }
    let tag = parse_shortcode_tag(trimmed, 0)?;
    if !is_supported_name(&tag.name) {
        return None;
    }
    let rest = trimmed[tag.end..].trim_end();
    if tag.close {
        return rest.is_empty().then_some(LineBlockStart::IgnoreLine);
    }
    match tag.name.as_str() {
        "code" | "mermaid" => classify_code_line(&tag, rest),
        "details" | "hint" | "quote" | "bigquote" | "attribution" | "smalltext" | "tabs"
        | "tab" => classify_container_line(&tag, rest),
        name if is_columns_name(name) => rest.is_empty().then_some(LineBlockStart::IgnoreLine),
        "katex" | "latex" => classify_math_line(&tag, rest),
        _ => None,
    }
}

pub(super) fn strip_closing_on_same_line<'a>(rest: &'a str, name: &str) -> Option<&'a str> {
    let close_pos = match (rest.rfind("{{<"), rest.rfind("{{%")) {
        (Some(a), Some(b)) => a.max(b),
        (Some(a), None) | (None, Some(a)) => a,
        (None, None) => return None,
    };
    let tag = parse_shortcode_tag(rest, close_pos)?;
    if tag.close && tag.name == name && rest[tag.end..].trim().is_empty() {
        Some(&rest[..close_pos])
    } else {
        None
    }
}

pub(super) fn is_closing_shortcode_line(trimmed: &str, name: &str) -> bool {
    parse_shortcode_tag(trimmed, 0)
        .is_some_and(|tag| tag.close && tag.name == name && trimmed[tag.end..].trim().is_empty())
}

fn extract_hint_kind(args: &[ShortcodeArg]) -> Option<QuoteKind> {
    let raw = args
        .iter()
        .find(|arg| {
            matches!(
                arg.key.as_deref(),
                Some("type" | "kind" | "style" | "class")
            )
        })
        .or_else(|| args.iter().find(|arg| arg.key.is_none()))
        .map(|arg| arg.value.trim().to_ascii_lowercase())?;
    match raw.as_str() {
        "info" | "note" => Some(QuoteKind::Note),
        "tip" | "success" | "ok" => Some(QuoteKind::Tip),
        "important" => Some(QuoteKind::Important),
        "warning" | "warn" => Some(QuoteKind::Warning),
        "danger" | "caution" | "error" => Some(QuoteKind::Caution),
        "gray" | "grey" | "" => None,
        _ => None,
    }
}

fn extract_quote_source(args: &[ShortcodeArg]) -> String {
    args.iter()
        .find(|arg| matches!(arg.key.as_deref(), Some("source" | "author" | "cite")))
        .or_else(|| args.iter().find(|arg| arg.key.is_none()))
        .map(|arg| arg.value.trim().to_owned())
        .unwrap_or_default()
}

fn extract_tab_title(args: &[ShortcodeArg], index: usize) -> String {
    args.iter()
        .find(|arg| matches!(arg.key.as_deref(), Some("title" | "name" | "label")))
        .or_else(|| args.iter().find(|arg| arg.key.is_none()))
        .map(|arg| arg.value.trim().to_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| format!("Tab {}", index + 1))
}

fn parse_tabs_inner(raw: &str, depth: usize) -> Vec<MarkdownTab> {
    let mut tabs = Vec::new();
    let mut scan = BlockScanState::new(0, true);

    while scan.pos < raw.len() {
        if scan.step_fence_or_code(raw, true) {
            continue;
        }
        if has_tag_prefix(&raw.as_bytes()[scan.pos..])
            && let Some(tag) = parse_shortcode_tag(raw, scan.pos)
            && !tag.close
        {
            if scan.try_skip_raw_shortcode(raw, &tag) {
                continue;
            }
            if tag.name == "tab"
                && let Some((close_start, close_end)) = find_closing_tag(raw, tag.end, "tab", true)
            {
                let title = extract_tab_title(&tag.args, tabs.len());
                let blocks = parse_container_blocks(&raw[tag.end..close_start], depth);
                tabs.push(MarkdownTab { title, blocks });
                scan.jump_to(raw, close_end);
                continue;
            }
            scan.jump_to(raw, tag.end);
            continue;
        }
        scan.step_char(raw);
    }

    if tabs.is_empty() && !raw.trim().is_empty() {
        let blocks = parse_container_blocks(raw, depth);
        if !blocks.is_empty() {
            tabs.push(MarkdownTab {
                title: "Content".to_owned(),
                blocks,
            });
        }
    }
    tabs
}

fn split_columns_inner(raw: &str) -> Vec<&str> {
    let bytes = raw.as_bytes();
    let mut parts = Vec::new();
    let mut start = 0usize;
    let mut col_depth = 0usize;
    let mut scan = BlockScanState::new(0, true);

    while scan.pos < raw.len() {
        if scan.step_fence_or_code(raw, true) {
            continue;
        }
        if has_tag_prefix(&bytes[scan.pos..])
            && let Some(tag) = parse_shortcode_tag(raw, scan.pos)
        {
            if scan.try_skip_raw_shortcode(raw, &tag) {
                continue;
            }
            if is_columns_name(&tag.name) {
                if tag.close {
                    col_depth = col_depth.saturating_sub(1);
                } else {
                    col_depth += 1;
                }
            }
            scan.jump_to(raw, tag.end);
            continue;
        }
        if col_depth == 0 && bytes[scan.pos..].starts_with(b"<--->") {
            parts.push(&raw[start..scan.pos]);
            let next = scan.pos + "<--->".len();
            start = next;
            scan.jump_to(raw, next);
            continue;
        }
        scan.step_char(raw);
    }

    parts.push(&raw[start..]);
    parts
}

fn simple_span_css(span: MarkdownSpan) -> Result<(String, InlineCss), MarkdownSpan> {
    match span {
        MarkdownSpan::Text(t) => Ok((t, InlineCss::default())),
        MarkdownSpan::Bold(t) => Ok((
            t,
            InlineCss {
                bold: true,
                ..Default::default()
            },
        )),
        MarkdownSpan::Italic(t) => Ok((
            t,
            InlineCss {
                italic: true,
                ..Default::default()
            },
        )),
        MarkdownSpan::BoldItalic(t) => Ok((
            t,
            InlineCss {
                bold: true,
                italic: true,
                ..Default::default()
            },
        )),
        MarkdownSpan::Strikethrough(t) => Ok((
            t,
            InlineCss {
                strikethrough: true,
                ..Default::default()
            },
        )),
        MarkdownSpan::Code(t) => Ok((
            t,
            InlineCss {
                monospace: true,
                ..Default::default()
            },
        )),
        other => Err(other),
    }
}

fn style_single_span(
    span: MarkdownSpan,
    outer_url: Option<&str>,
    extra: InlineCss,
) -> MarkdownSpan {
    match simple_span_css(span) {
        Ok((text, base_css)) => MarkdownSpan::Styled {
            text,
            url: outer_url.map(str::to_owned),
            css: extra.merge(base_css),
        },
        Err(MarkdownSpan::Link { label, url }) => MarkdownSpan::Styled {
            text: label,
            url: Some(url),
            css: extra,
        },
        Err(MarkdownSpan::Styled { text, url, css }) => MarkdownSpan::Styled {
            text,
            url: url.or_else(|| outer_url.map(str::to_owned)),
            css: extra.merge(css),
        },
        Err(MarkdownSpan::Spoiler { spans }) => MarkdownSpan::Spoiler {
            spans: spans
                .into_iter()
                .map(|s| style_single_span(s, outer_url, extra))
                .collect(),
        },
        Err(MarkdownSpan::Math {
            source,
            display,
            url,
        }) => MarkdownSpan::Math {
            source,
            display,
            url: url.or_else(|| outer_url.map(str::to_owned)),
        },
        Err(MarkdownSpan::Image { alt, url }) if outer_url.is_some() => MarkdownSpan::StyledImage {
            alt,
            url,
            href: outer_url.map(str::to_owned),
            style: super::ImagePresentation::default(),
        },
        Err(MarkdownSpan::StyledImage {
            alt,
            url,
            href,
            style,
        }) => MarkdownSpan::StyledImage {
            alt,
            url,
            href: href.or_else(|| outer_url.map(str::to_owned)),
            style,
        },
        Err(other) => other,
    }
}

fn style_span_list(spans: Vec<MarkdownSpan>, extra: InlineCss) -> Vec<MarkdownSpan> {
    spans
        .into_iter()
        .map(|span| style_single_span(span, None, extra))
        .collect()
}

fn style_doc_blocks_inline(blocks: &mut [MarkdownDocBlock], extra: InlineCss) {
    for block in blocks {
        transform_block_spans(block, &mut |spans| {
            *spans = style_span_list(std::mem::take(spans), extra);
        });
    }
}
