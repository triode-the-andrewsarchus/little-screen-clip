//! Best-effort parsing of common Hugo shortcodes for Markdown previews.
//!
//! Supports built-in/common (`mermaid`, `katex`, `latex`, `code`, `details`, `hl`)
//! and custom shortcodes (`spoiler`, `smalltext`, `attribution`, `hint`, `quote`,
//! `bigquote`, `tabs`, `tab`, `columns`, `columns2`, `columns3`) in both
//! `{{< ... >}}` and `{{% ... %}}` delimiters while leaving unknown shortcodes
//! and escaped/fenced code intact and preserving UTF-8 source byte offsets.

use super::{
    BlockBoxStyle, InlineCss, LocatedBlock, MarkdownDocBlock, MarkdownSpan, MarkdownTab, QuoteKind,
    TableAlignment, parse_located_body_inner, parse_markdown_spans, push_span,
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
        let has_inline = spans.iter().any(|span| self.span_contains_inline(span));
        if !has_inline {
            return spans;
        }
        let mut out = Vec::with_capacity(spans.len());
        for span in spans {
            match span {
                MarkdownSpan::Text(t) if t.contains(self.inline_open) => {
                    self.expand_inline_text(&t, None, InlineCss::default(), &mut out);
                }
                MarkdownSpan::Styled { text, url, css } if text.contains(self.inline_open) => {
                    self.expand_inline_text(&text, url.as_deref(), css, &mut out);
                }
                MarkdownSpan::Spoiler { spans } => {
                    push_span(
                        &mut out,
                        MarkdownSpan::Spoiler {
                            spans: self.expand_span_list(spans),
                        },
                    );
                }
                other => push_span(&mut out, other),
            }
        }
        out
    }

    fn span_contains_inline(&self, span: &MarkdownSpan) -> bool {
        match span {
            MarkdownSpan::Text(t) => t.contains(self.inline_open),
            MarkdownSpan::Styled { text, .. } => text.contains(self.inline_open),
            MarkdownSpan::Spoiler { spans } => spans.iter().any(|s| self.span_contains_inline(s)),
            _ => false,
        }
    }

    pub(super) fn expand_block_spans(&self, block: &mut MarkdownDocBlock) {
        match block {
            MarkdownDocBlock::Heading { text, spans, .. } => {
                *spans = self.expand_span_list(std::mem::take(spans));
                *text = super::plain_text_from_spans(spans);
            }
            MarkdownDocBlock::Paragraph { spans } => {
                *spans = self.expand_span_list(std::mem::take(spans));
            }
            MarkdownDocBlock::Quote { blocks, .. }
            | MarkdownDocBlock::Footnote { blocks, .. }
            | MarkdownDocBlock::StyledContainer { blocks, .. } => {
                for inner in blocks {
                    self.expand_block_spans(inner);
                }
            }
            MarkdownDocBlock::PullQuote { source, blocks, .. } => {
                *source = self.expand_span_list(std::mem::take(source));
                for inner in blocks {
                    self.expand_block_spans(inner);
                }
            }
            MarkdownDocBlock::Details {
                summary, blocks, ..
            } => {
                *summary = self.expand_span_list(std::mem::take(summary));
                for inner in blocks {
                    self.expand_block_spans(inner);
                }
            }
            MarkdownDocBlock::Tabs { tabs } => {
                for tab in tabs {
                    for inner in &mut tab.blocks {
                        self.expand_block_spans(inner);
                    }
                }
            }
            MarkdownDocBlock::Columns { columns, .. } => {
                for col in columns {
                    for inner in col {
                        self.expand_block_spans(inner);
                    }
                }
            }
            MarkdownDocBlock::List { items, .. } => {
                for item in items {
                    for inner in &mut item.blocks {
                        self.expand_block_spans(inner);
                    }
                }
            }
            MarkdownDocBlock::Table(table) => {
                for cell in &mut table.headers {
                    *cell = self.expand_span_list(std::mem::take(cell));
                }
                for row in &mut table.rows {
                    for cell in row {
                        *cell = self.expand_span_list(std::mem::take(cell));
                    }
                }
                let _ = TableAlignment::None;
            }
            MarkdownDocBlock::FrontMatter(_)
            | MarkdownDocBlock::Code { .. }
            | MarkdownDocBlock::Rule
            | MarkdownDocBlock::IncompleteMath { .. } => {}
        }
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

        // Scan inline/block shortcodes starting from `cursor`, respecting backtick runs.
        let mut pos = line_start;
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
                    cursor = close_end;
                    if cursor > line_end {
                        // Code span crossed newlines; re-sync outer loop past it.
                        active_fence = None;
                        break;
                    }
                    continue;
                }
                prepared.rewritten.push_str(&input[pos..pos + tick_run]);
                pos += tick_run;
                continue;
            }

            if (bytes[pos..].starts_with(b"{{<") || bytes[pos..].starts_with(b"{{%"))
                && let Some(parsed) = parse_shortcode_tag(input, pos)
                && !parsed.close
                && is_supported_name(&parsed.name)
                && let Some(res) = handle_open_shortcode(input, &parsed, depth, &mut prepared)
            {
                has_any = true;
                cursor = res.next_input_pos;
                pos = res.next_input_pos;
                if cursor > line_end {
                    break;
                }
                continue;
            }

            let ch = input[pos..].chars().next().expect("valid utf8");
            prepared.rewritten.push(ch);
            pos += ch.len_utf8();
        }
        if pos >= line_end && cursor < line_end {
            cursor = line_end;
        } else if cursor < pos {
            cursor = pos;
        }
    }

    has_any.then_some(prepared)
}

struct MatchedShortcode {
    next_input_pos: usize,
}

fn handle_open_shortcode(
    input: &str,
    open: &ParsedTag,
    depth: usize,
    prepared: &mut PreparedShortcodes,
) -> Option<MatchedShortcode> {
    let orig_start = open.start;
    let body_start = open.end;
    match open.name.as_str() {
        "code" | "mermaid" => {
            let language = if open.name == "mermaid" {
                "mermaid".to_owned()
            } else {
                extract_code_language(&open.args)
            };
            let (raw_inner, orig_end, incomplete) = if let Some((close_start, close_end)) =
                find_closing_tag(input, body_start, &open.name, false)
            {
                (&input[body_start..close_start], close_end, false)
            } else {
                (&input[body_start..], input.len(), true)
            };
            let text = clean_raw_block_body(raw_inner, open.name == "mermaid");
            let id = prepared.blocks.len();
            prepared.blocks.push(BlockReplacement::Code {
                language,
                text,
                incomplete,
            });
            emit_block_token(prepared, id, orig_start, orig_end);
            Some(MatchedShortcode {
                next_input_pos: orig_end,
            })
        }
        "details" => {
            let (close_start, close_end) = find_closing_tag(input, body_start, "details", true)?;
            let (summary_text, is_open) = extract_details_params(&open.args);
            let summary = super::parse_markdown_spans_at_depth(&summary_text, depth + 1);
            let raw_inner = clean_container_body(&input[body_start..close_start]);
            let inner_located: Vec<LocatedBlock> =
                parse_located_body_inner(&raw_inner, 0, depth + 1);
            let blocks = inner_located.into_iter().map(|b| b.block).collect();
            let id = prepared.blocks.len();
            prepared
                .blocks
                .push(BlockReplacement::Doc(vec![MarkdownDocBlock::Details {
                    summary,
                    open: is_open,
                    blocks,
                }]));
            emit_block_token(prepared, id, orig_start, close_end);
            Some(MatchedShortcode {
                next_input_pos: close_end,
            })
        }
        "hint" => {
            let (close_start, close_end) = find_closing_tag(input, body_start, "hint", true)?;
            let kind = extract_hint_kind(&open.args);
            let raw_inner = clean_container_body(&input[body_start..close_start]);
            let blocks = parse_located_body_inner(&raw_inner, 0, depth + 1)
                .into_iter()
                .map(|b| b.block)
                .collect();
            let id = prepared.blocks.len();
            prepared
                .blocks
                .push(BlockReplacement::Doc(vec![MarkdownDocBlock::Quote {
                    kind,
                    blocks,
                }]));
            emit_block_token(prepared, id, orig_start, close_end);
            Some(MatchedShortcode {
                next_input_pos: close_end,
            })
        }
        "quote" | "bigquote" => {
            let (close_start, close_end) = find_closing_tag(input, body_start, &open.name, true)?;
            let source_text = extract_quote_source(&open.args);
            let source = if source_text.is_empty() {
                Vec::new()
            } else {
                super::parse_markdown_spans_at_depth(&source_text, depth + 1)
            };
            let raw_inner = clean_container_body(&input[body_start..close_start]);
            let blocks = parse_located_body_inner(&raw_inner, 0, depth + 1)
                .into_iter()
                .map(|b| b.block)
                .collect();
            let id = prepared.blocks.len();
            prepared
                .blocks
                .push(BlockReplacement::Doc(vec![MarkdownDocBlock::PullQuote {
                    big: open.name == "bigquote",
                    source,
                    blocks,
                }]));
            emit_block_token(prepared, id, orig_start, close_end);
            Some(MatchedShortcode {
                next_input_pos: close_end,
            })
        }
        "tabs" => {
            let (close_start, close_end) = find_closing_tag(input, body_start, "tabs", true)?;
            let raw_inner = &input[body_start..close_start];
            let tabs = parse_tabs_inner(raw_inner, depth);
            let id = prepared.blocks.len();
            if !tabs.is_empty() {
                prepared
                    .blocks
                    .push(BlockReplacement::Doc(vec![MarkdownDocBlock::Tabs { tabs }]));
            } else {
                prepared.blocks.push(BlockReplacement::Doc(Vec::new()));
            }
            emit_block_token(prepared, id, orig_start, close_end);
            Some(MatchedShortcode {
                next_input_pos: close_end,
            })
        }
        "tab" => {
            let (close_start, close_end) = find_closing_tag(input, body_start, "tab", true)?;
            let title = extract_tab_title(&open.args, 0);
            let raw_inner = clean_container_body(&input[body_start..close_start]);
            let blocks = parse_located_body_inner(&raw_inner, 0, depth + 1)
                .into_iter()
                .map(|b| b.block)
                .collect();
            let id = prepared.blocks.len();
            prepared
                .blocks
                .push(BlockReplacement::Doc(vec![MarkdownDocBlock::Tabs {
                    tabs: vec![MarkdownTab { title, blocks }],
                }]));
            emit_block_token(prepared, id, orig_start, close_end);
            Some(MatchedShortcode {
                next_input_pos: close_end,
            })
        }
        name if is_columns_name(name) => {
            let (close_start, close_end) = find_closing_tag(input, body_start, name, true)?;
            let raw_inner = &input[body_start..close_start];
            let mut columns: Vec<Vec<MarkdownDocBlock>> = split_columns_inner(raw_inner)
                .into_iter()
                .map(|col_raw| {
                    let cleaned = clean_container_body(col_raw);
                    parse_located_body_inner(&cleaned, 0, depth + 1)
                        .into_iter()
                        .map(|b| b.block)
                        .collect()
                })
                .collect();
            while columns.len() > 1 && columns.last().is_some_and(Vec::is_empty) {
                columns.pop();
            }
            let dividers = name == "columns3";
            let bottom_rule = name == "columns2";
            let id = prepared.blocks.len();
            prepared
                .blocks
                .push(BlockReplacement::Doc(vec![MarkdownDocBlock::Columns {
                    dividers,
                    bottom_rule,
                    columns,
                }]));
            emit_block_token(prepared, id, orig_start, close_end);
            Some(MatchedShortcode {
                next_input_pos: close_end,
            })
        }
        "attribution" => {
            let (close_start, close_end) =
                find_closing_tag(input, body_start, "attribution", true)?;
            let raw_inner = clean_container_body(&input[body_start..close_start]);
            let mut blocks: Vec<MarkdownDocBlock> =
                parse_located_body_inner(&raw_inner, 0, depth + 1)
                    .into_iter()
                    .map(|b| b.block)
                    .collect();
            let caption_css = InlineCss {
                small: true,
                muted: true,
                ..Default::default()
            };
            style_doc_blocks_inline(&mut blocks, caption_css);
            let id = prepared.blocks.len();
            prepared.blocks.push(BlockReplacement::Doc(vec![
                MarkdownDocBlock::StyledContainer {
                    style: BlockBoxStyle {
                        margin_bottom: 10.0,
                        ..Default::default()
                    },
                    blocks,
                },
            ]));
            emit_block_token(prepared, id, orig_start, close_end);
            Some(MatchedShortcode {
                next_input_pos: close_end,
            })
        }
        "spoiler" => {
            let (close_start, close_end) =
                find_closing_tag_within_paragraph(input, body_start, "spoiler")?;
            let inner = input[body_start..close_start].trim();
            let spans = super::parse_markdown_spans_at_depth(inner, depth + 1);
            let id = prepared.inlines.len();
            prepared
                .inlines
                .push(InlineReplacement::Spans(vec![MarkdownSpan::Spoiler {
                    spans,
                }]));
            emit_inline_token(prepared, id, orig_start, close_end);
            Some(MatchedShortcode {
                next_input_pos: close_end,
            })
        }
        "smalltext" => {
            if let Some((close_start, close_end)) =
                find_closing_tag_within_paragraph(input, body_start, "smalltext")
            {
                let inner = input[body_start..close_start].trim();
                let mut spans = super::parse_markdown_spans_at_depth(inner, depth + 1);
                let small_css = InlineCss {
                    small: true,
                    ..Default::default()
                };
                spans = style_span_list(spans, small_css);
                let id = prepared.inlines.len();
                prepared.inlines.push(InlineReplacement::Spans(spans));
                emit_inline_token(prepared, id, orig_start, close_end);
                return Some(MatchedShortcode {
                    next_input_pos: close_end,
                });
            }
            if let Some((close_start, close_end)) =
                find_closing_tag(input, body_start, "smalltext", true)
            {
                let raw_inner = clean_container_body(&input[body_start..close_start]);
                let mut blocks: Vec<MarkdownDocBlock> =
                    parse_located_body_inner(&raw_inner, 0, depth + 1)
                        .into_iter()
                        .map(|b| b.block)
                        .collect();
                style_doc_blocks_inline(
                    &mut blocks,
                    InlineCss {
                        small: true,
                        ..Default::default()
                    },
                );
                let id = prepared.blocks.len();
                prepared.blocks.push(BlockReplacement::Doc(blocks));
                emit_block_token(prepared, id, orig_start, close_end);
                return Some(MatchedShortcode {
                    next_input_pos: close_end,
                });
            }
            None
        }
        "katex" | "latex" => {
            let explicit_display = has_display_flag(&open.args);
            if let Some((close_start, close_end)) =
                find_closing_tag(input, body_start, &open.name, false)
            {
                let raw_inner = &input[body_start..close_start];
                let line_before_empty = line_prefix_is_blank(input, orig_start);
                let line_after_empty = line_suffix_is_blank(input, close_end);
                let multiline = raw_inner.contains('\n');
                let standalone = line_before_empty && line_after_empty;
                let source = raw_inner.trim().to_owned();
                if standalone && (explicit_display || multiline) {
                    let id = prepared.blocks.len();
                    prepared.blocks.push(BlockReplacement::Math {
                        source,
                        display: true,
                    });
                    emit_block_token(prepared, id, orig_start, close_end);
                } else {
                    let id = prepared.inlines.len();
                    prepared.inlines.push(InlineReplacement::Math {
                        source,
                        display: explicit_display || (standalone && multiline),
                    });
                    emit_inline_token(prepared, id, orig_start, close_end);
                }
                return Some(MatchedShortcode {
                    next_input_pos: close_end,
                });
            }
            // Standalone zero-body script loader call such as `{{< katex >}}` with no matching closing tag.
            if line_prefix_is_blank(input, orig_start) && line_suffix_is_blank(input, open.end) {
                emit_empty_replacement(prepared, orig_start, open.end);
                return Some(MatchedShortcode {
                    next_input_pos: open.end,
                });
            }
            None
        }
        "hl" => {
            let language = extract_hl_language(&open.args);
            if let Some((close_start, close_end)) =
                find_closing_tag_within_paragraph(input, body_start, "hl")
            {
                let code = input[body_start..close_start]
                    .trim_matches(['\r', '\n'])
                    .trim()
                    .to_owned();
                let id = prepared.inlines.len();
                prepared
                    .inlines
                    .push(InlineReplacement::HighlightedCode { language, code });
                emit_inline_token(prepared, id, orig_start, close_end);
                return Some(MatchedShortcode {
                    next_input_pos: close_end,
                });
            }
            if let Some(code_attr) = open
                .args
                .iter()
                .find(|arg| arg.key.as_deref() == Some("code"))
                .map(|arg| arg.value.clone())
            {
                let id = prepared.inlines.len();
                prepared.inlines.push(InlineReplacement::HighlightedCode {
                    language,
                    code: code_attr,
                });
                emit_inline_token(prepared, id, orig_start, open.end);
                return Some(MatchedShortcode {
                    next_input_pos: open.end,
                });
            }
            None
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
            emit_literal_replacement(prepared, &encoded, orig_start, open.end);
            Some(MatchedShortcode {
                next_input_pos: open.end,
            })
        }
        "button" => {
            let (close_start, close_end) =
                find_closing_tag_within_paragraph(input, body_start, "button")?;
            let target = open
                .args
                .iter()
                .find(|arg| matches!(arg.key.as_deref(), Some("href" | "relref" | "ref")))
                .map(|arg| arg.value.trim().to_owned())
                .filter(|v| !v.is_empty());
            let inner = input[body_start..close_start].trim();
            let mut spans = super::parse_markdown_spans_at_depth(inner, depth + 1);
            if let Some(url) = target {
                spans = spans
                    .into_iter()
                    .map(|span| style_single_span(span, Some(&url), InlineCss::default()))
                    .collect();
            }
            let id = prepared.inlines.len();
            prepared.inlines.push(InlineReplacement::Spans(spans));
            emit_inline_token(prepared, id, orig_start, close_end);
            Some(MatchedShortcode {
                next_input_pos: close_end,
            })
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

fn emit_literal_replacement(
    prepared: &mut PreparedShortcodes,
    literal: &str,
    orig_start: usize,
    orig_end: usize,
) {
    let rew_start = prepared.rewritten.len();
    prepared.shifts.push(OffsetShift {
        rew_pos: rew_start,
        orig_pos: orig_start,
    });
    prepared.rewritten.push_str(literal);
    let rew_end = prepared.rewritten.len();
    prepared.shifts.push(OffsetShift {
        rew_pos: rew_end,
        orig_pos: orig_end,
    });
}

fn emit_inline_token(
    prepared: &mut PreparedShortcodes,
    id: usize,
    orig_start: usize,
    orig_end: usize,
) {
    let rew_start = prepared.rewritten.len();
    prepared.shifts.push(OffsetShift {
        rew_pos: rew_start,
        orig_pos: orig_start,
    });
    prepared.rewritten.push(prepared.inline_open);
    prepared.rewritten.push_str(&id.to_string());
    prepared.rewritten.push(prepared.inline_close);
    let rew_end = prepared.rewritten.len();
    prepared.shifts.push(OffsetShift {
        rew_pos: rew_end,
        orig_pos: orig_end,
    });
}

fn emit_block_token(
    prepared: &mut PreparedShortcodes,
    id: usize,
    orig_start: usize,
    orig_end: usize,
) {
    let rew_start = prepared.rewritten.len();
    prepared.shifts.push(OffsetShift {
        rew_pos: rew_start,
        orig_pos: orig_start,
    });
    prepared.rewritten.push(prepared.block_open);
    prepared.rewritten.push_str(&id.to_string());
    prepared.rewritten.push(prepared.block_close);
    let rew_end = prepared.rewritten.len();
    prepared.shifts.push(OffsetShift {
        rew_pos: rew_end,
        orig_pos: orig_end,
    });
}

fn emit_empty_replacement(prepared: &mut PreparedShortcodes, orig_start: usize, orig_end: usize) {
    let rew_pos = prepared.rewritten.len();
    prepared.shifts.push(OffsetShift {
        rew_pos,
        orig_pos: orig_start,
    });
    prepared.shifts.push(OffsetShift {
        rew_pos,
        orig_pos: orig_end,
    });
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

    let mut quote: Option<u8> = None;
    let mut escaped = false;
    let bytes = body_slice.as_bytes();
    let mut rel_close = None;
    let mut idx = 0usize;
    while idx < bytes.len() {
        let b = bytes[idx];
        if let Some(q) = quote {
            if escaped {
                escaped = false;
            } else if b == b'\\' && q != b'`' {
                escaped = true;
            } else if b == q {
                quote = None;
            }
            idx += 1;
            continue;
        }
        if matches!(b, b'"' | b'\'' | b'`') {
            quote = Some(b);
            idx += 1;
            continue;
        }
        if b == b'\n' {
            // Avoid matching stray `{{<` across blank lines if unclosed.
            if bytes[idx + 1..].starts_with(b"\n") || bytes[idx + 1..].starts_with(b"\r\n") {
                return None;
            }
        }
        if bytes[idx..].starts_with(close_delim.as_bytes()) {
            rel_close = Some(idx);
            break;
        }
        idx += 1;
    }

    let rel_close = rel_close?;
    let end = start + inner_start + rel_close + close_delim.len();
    let mut content = body_slice[..rel_close].trim();
    // Support Hugo trim hyphen `{{-` / `-}}` / `-%}` variants cleanly.
    if let Some(stripped) = content.strip_prefix('-') {
        content = stripped.trim_start();
    }
    if let Some(stripped) = content.strip_suffix('-') {
        content = stripped.trim_end();
    }

    let (close, rest) = if let Some(after_slash) = content.strip_prefix('/') {
        (true, after_slash.trim_start())
    } else {
        (false, content)
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

    let args_raw = rest[name_end..].trim().trim_end_matches('/').trim();
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

fn find_closing_tag(
    input: &str,
    from: usize,
    name: &str,
    track_nesting: bool,
) -> Option<(usize, usize)> {
    let bytes = input.as_bytes();
    let mut pos = from;
    let mut depth = 1usize;
    let mut fence: Option<(u8, usize)> = None;
    let mut at_line_start = from == 0 || bytes.get(from - 1) == Some(&b'\n');

    while pos < input.len() {
        if track_nesting && at_line_start {
            let line_end = input[pos..].find('\n').map_or(input.len(), |n| pos + n + 1);
            let raw_line = input[pos..line_end].trim_end_matches(['\r', '\n']);
            if let Some((marker, min_run)) = fence {
                if is_closing_fence(raw_line, marker, min_run) {
                    fence = None;
                }
                pos = line_end;
                continue;
            }
            if let Some((marker, run)) = opening_fence(raw_line) {
                fence = Some((marker, run));
                pos = line_end;
                continue;
            }
        }

        if (bytes[pos..].starts_with(b"{{<") || bytes[pos..].starts_with(b"{{%"))
            && let Some(tag) = parse_shortcode_tag(input, pos)
        {
            if track_nesting
                && !tag.close
                && matches!(tag.name.as_str(), "code" | "mermaid")
                && let Some((_, raw_end)) = find_closing_tag(input, tag.end, &tag.name, false)
            {
                at_line_start = bytes.get(raw_end.saturating_sub(1)) == Some(&b'\n');
                pos = raw_end;
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
            pos = tag.end;
            at_line_start = false;
            continue;
        }

        let (b, step) = next_char_step(input, pos);
        at_line_start = b == b'\n';
        pos += step;
    }
    None
}

fn find_closing_tag_within_paragraph(
    input: &str,
    from: usize,
    name: &str,
) -> Option<(usize, usize)> {
    let bytes = input.as_bytes();
    let mut pos = from;
    while pos < input.len() {
        if bytes[pos..].starts_with(b"\n\n") || bytes[pos..].starts_with(b"\r\n\r\n") {
            return None;
        }
        if (bytes[pos..].starts_with(b"{{<") || bytes[pos..].starts_with(b"{{%"))
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

pub(super) fn clean_raw_block_body(raw: &str, trim_blank_edges: bool) -> String {
    let mut s = raw
        .strip_prefix("\r\n")
        .or_else(|| raw.strip_prefix('\n'))
        .unwrap_or(raw);
    if trim_blank_edges {
        return s.trim_matches(['\r', '\n']).trim().to_owned();
    }
    // Trim only whitespace on the closing-tag line (e.g. `    {{< /code >}}`),
    // preserving meaningful indentation on actual code lines.
    if let Some(last_nl) = s.rfind('\n')
        && s[last_nl + 1..].trim().is_empty()
    {
        s = &s[..last_nl];
    }
    s = s.trim_end_matches('\r');
    s.to_owned()
}

fn clean_container_body(raw: &str) -> String {
    let mut s = raw
        .strip_prefix("\r\n")
        .or_else(|| raw.strip_prefix('\n'))
        .unwrap_or(raw);
    if let Some(last_nl) = s.rfind('\n')
        && s[last_nl + 1..].trim().is_empty()
    {
        s = &s[..last_nl];
    }
    s.to_owned()
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

pub(super) fn classify_block_line(trimmed: &str) -> Option<LineBlockStart> {
    if !(trimmed.starts_with("{{<") || trimmed.starts_with("{{%")) {
        return None;
    }
    let tag = parse_shortcode_tag(trimmed, 0)?;
    if !is_supported_name(&tag.name) {
        return None;
    }
    if tag.close {
        return Some(LineBlockStart::IgnoreLine);
    }
    let rest = trimmed[tag.end..].trim_end();
    match tag.name.as_str() {
        "code" | "mermaid" => {
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
            if let Some(inline_body) = strip_closing_on_same_line(rest, tag_name) {
                Some(LineBlockStart::Code {
                    tag_name,
                    language,
                    inline_body: Some(inline_body.to_owned()),
                })
            } else if rest.is_empty() {
                Some(LineBlockStart::Code {
                    tag_name,
                    language,
                    inline_body: None,
                })
            } else {
                None
            }
        }
        "details" => {
            let (summary_text, _) = extract_details_params(&tag.args);
            let summary_spans = parse_markdown_spans(&summary_text);
            if let Some(inline_body) = strip_closing_on_same_line(rest, "details") {
                Some(LineBlockStart::Details {
                    summary_spans,
                    inline_body: Some(inline_body.trim().to_owned()),
                })
            } else if rest.is_empty() {
                Some(LineBlockStart::Details {
                    summary_spans,
                    inline_body: None,
                })
            } else {
                None
            }
        }
        "hint" | "quote" | "bigquote" | "attribution" | "smalltext" | "tabs" | "tab" => {
            if let Some(inline_body) = strip_closing_on_same_line(rest, &tag.name) {
                let inner = inline_body.trim();
                let summary_spans = if inner.is_empty() {
                    Vec::new()
                } else {
                    parse_markdown_spans(inner)
                };
                if summary_spans.is_empty() {
                    Some(LineBlockStart::IgnoreLine)
                } else {
                    Some(LineBlockStart::Details {
                        summary_spans,
                        inline_body: None,
                    })
                }
            } else if tag.name == "tab" && rest.is_empty() {
                let title = extract_tab_title(&tag.args, 0);
                Some(LineBlockStart::Details {
                    summary_spans: vec![MarkdownSpan::Bold(title)],
                    inline_body: None,
                })
            } else if rest.is_empty() {
                Some(LineBlockStart::IgnoreLine)
            } else {
                None
            }
        }
        name if is_columns_name(name) => {
            if rest.is_empty() {
                Some(LineBlockStart::IgnoreLine)
            } else {
                None
            }
        }
        "katex" | "latex" => {
            let tag_name = if tag.name == "katex" {
                "katex"
            } else {
                "latex"
            };
            let display = has_display_flag(&tag.args);
            if let Some(inner) = strip_closing_on_same_line(rest, tag_name) {
                Some(LineBlockStart::Math {
                    tag_name,
                    inline_source: Some(inner.trim().to_owned()),
                    display,
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
    let bytes = raw.as_bytes();
    let mut tabs = Vec::new();
    let mut pos = 0usize;
    let mut fence: Option<(u8, usize)> = None;
    let mut at_line_start = true;

    while pos < raw.len() {
        if at_line_start {
            let line_end = raw[pos..].find('\n').map_or(raw.len(), |n| pos + n + 1);
            let raw_line = raw[pos..line_end].trim_end_matches(['\r', '\n']);
            if let Some((marker, min_run)) = fence {
                if is_closing_fence(raw_line, marker, min_run) {
                    fence = None;
                }
                pos = line_end;
                continue;
            }
            if let Some((marker, run)) = opening_fence(raw_line) {
                fence = Some((marker, run));
                pos = line_end;
                continue;
            }
        }

        if (bytes[pos..].starts_with(b"{{<") || bytes[pos..].starts_with(b"{{%"))
            && let Some(tag) = parse_shortcode_tag(raw, pos)
            && !tag.close
        {
            if matches!(tag.name.as_str(), "code" | "mermaid")
                && let Some((_, end)) = find_closing_tag(raw, tag.end, &tag.name, false)
            {
                at_line_start = bytes.get(end.saturating_sub(1)) == Some(&b'\n');
                pos = end;
                continue;
            }
            if tag.name == "tab"
                && let Some((close_start, close_end)) = find_closing_tag(raw, tag.end, "tab", true)
            {
                let title = extract_tab_title(&tag.args, tabs.len());
                let body = clean_container_body(&raw[tag.end..close_start]);
                let blocks = parse_located_body_inner(&body, 0, depth + 1)
                    .into_iter()
                    .map(|b| b.block)
                    .collect();
                tabs.push(MarkdownTab { title, blocks });
                at_line_start = bytes.get(close_end.saturating_sub(1)) == Some(&b'\n');
                pos = close_end;
                continue;
            }
            pos = tag.end;
            at_line_start = false;
            continue;
        }

        let (b, step) = next_char_step(raw, pos);
        at_line_start = b == b'\n';
        pos += step;
    }

    if tabs.is_empty() && !raw.trim().is_empty() {
        let cleaned = clean_container_body(raw);
        let blocks: Vec<MarkdownDocBlock> = parse_located_body_inner(&cleaned, 0, depth + 1)
            .into_iter()
            .map(|b| b.block)
            .collect();
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
    let mut pos = 0usize;
    let mut fence: Option<(u8, usize)> = None;
    let mut col_depth = 0usize;
    let mut at_line_start = true;

    while pos < raw.len() {
        if at_line_start {
            let line_end = raw[pos..].find('\n').map_or(raw.len(), |n| pos + n + 1);
            let raw_line = raw[pos..line_end].trim_end_matches(['\r', '\n']);
            if let Some((marker, min_run)) = fence {
                if is_closing_fence(raw_line, marker, min_run) {
                    fence = None;
                }
                pos = line_end;
                continue;
            }
            if let Some((marker, run)) = opening_fence(raw_line) {
                fence = Some((marker, run));
                pos = line_end;
                continue;
            }
        }

        if (bytes[pos..].starts_with(b"{{<") || bytes[pos..].starts_with(b"{{%"))
            && let Some(tag) = parse_shortcode_tag(raw, pos)
        {
            if !tag.close
                && matches!(tag.name.as_str(), "code" | "mermaid")
                && let Some((_, end)) = find_closing_tag(raw, tag.end, &tag.name, false)
            {
                at_line_start = bytes.get(end.saturating_sub(1)) == Some(&b'\n');
                pos = end;
                continue;
            }
            if is_columns_name(&tag.name) {
                if tag.close {
                    col_depth = col_depth.saturating_sub(1);
                } else {
                    col_depth += 1;
                }
            }
            at_line_start = false;
            pos = tag.end;
            continue;
        }

        if col_depth == 0 && bytes[pos..].starts_with(b"<--->") {
            parts.push(&raw[start..pos]);
            pos += "<--->".len();
            start = pos;
            at_line_start = false;
            continue;
        }

        let (b, step) = next_char_step(raw, pos);
        at_line_start = b == b'\n';
        pos += step;
    }

    parts.push(&raw[start..]);
    parts
}

fn style_single_span(
    span: MarkdownSpan,
    outer_url: Option<&str>,
    extra: InlineCss,
) -> MarkdownSpan {
    match span {
        MarkdownSpan::Text(t) => MarkdownSpan::Styled {
            text: t,
            url: outer_url.map(str::to_owned),
            css: extra,
        },
        MarkdownSpan::Bold(t) => MarkdownSpan::Styled {
            text: t,
            url: outer_url.map(str::to_owned),
            css: InlineCss {
                bold: true,
                ..extra
            },
        },
        MarkdownSpan::Italic(t) => MarkdownSpan::Styled {
            text: t,
            url: outer_url.map(str::to_owned),
            css: InlineCss {
                italic: true,
                ..extra
            },
        },
        MarkdownSpan::BoldItalic(t) => MarkdownSpan::Styled {
            text: t,
            url: outer_url.map(str::to_owned),
            css: InlineCss {
                bold: true,
                italic: true,
                ..extra
            },
        },
        MarkdownSpan::Strikethrough(t) => MarkdownSpan::Styled {
            text: t,
            url: outer_url.map(str::to_owned),
            css: InlineCss {
                strikethrough: true,
                ..extra
            },
        },
        MarkdownSpan::Code(t) => MarkdownSpan::Styled {
            text: t,
            url: outer_url.map(str::to_owned),
            css: InlineCss {
                monospace: true,
                ..extra
            },
        },
        MarkdownSpan::Link { label, url } => MarkdownSpan::Styled {
            text: label,
            url: Some(url),
            css: extra,
        },
        MarkdownSpan::Styled { text, url, css } => MarkdownSpan::Styled {
            text,
            url: url.or_else(|| outer_url.map(str::to_owned)),
            css: extra.merge(css),
        },
        MarkdownSpan::Spoiler { spans } => MarkdownSpan::Spoiler {
            spans: spans
                .into_iter()
                .map(|s| style_single_span(s, outer_url, extra))
                .collect(),
        },
        MarkdownSpan::Math {
            source,
            display,
            url,
        } => MarkdownSpan::Math {
            source,
            display,
            url: url.or_else(|| outer_url.map(str::to_owned)),
        },
        MarkdownSpan::Image { alt, url } if outer_url.is_some() => MarkdownSpan::StyledImage {
            alt,
            url,
            href: outer_url.map(str::to_owned),
            style: super::ImagePresentation::default(),
        },
        MarkdownSpan::StyledImage {
            alt,
            url,
            href,
            style,
        } => MarkdownSpan::StyledImage {
            alt,
            url,
            href: href.or_else(|| outer_url.map(str::to_owned)),
            style,
        },
        other => other,
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
        match block {
            MarkdownDocBlock::Paragraph { spans } => {
                *spans = style_span_list(std::mem::take(spans), extra);
            }
            MarkdownDocBlock::Heading { text, spans, .. } => {
                *spans = style_span_list(std::mem::take(spans), extra);
                *text = super::plain_text_from_spans(spans);
            }
            MarkdownDocBlock::Quote { blocks: inner, .. }
            | MarkdownDocBlock::Footnote { blocks: inner, .. }
            | MarkdownDocBlock::StyledContainer { blocks: inner, .. } => {
                style_doc_blocks_inline(inner, extra);
            }
            MarkdownDocBlock::PullQuote {
                source,
                blocks: inner,
                ..
            } => {
                *source = style_span_list(std::mem::take(source), extra);
                style_doc_blocks_inline(inner, extra);
            }
            MarkdownDocBlock::Details {
                summary,
                blocks: inner,
                ..
            } => {
                *summary = style_span_list(std::mem::take(summary), extra);
                style_doc_blocks_inline(inner, extra);
            }
            MarkdownDocBlock::Tabs { tabs } => {
                for tab in tabs {
                    style_doc_blocks_inline(&mut tab.blocks, extra);
                }
            }
            MarkdownDocBlock::Columns { columns, .. } => {
                for col in columns {
                    style_doc_blocks_inline(col, extra);
                }
            }
            MarkdownDocBlock::List { items, .. } => {
                for item in items {
                    style_doc_blocks_inline(&mut item.blocks, extra);
                }
            }
            MarkdownDocBlock::Table(table) => {
                for cell in &mut table.headers {
                    *cell = style_span_list(std::mem::take(cell), extra);
                }
                for row in &mut table.rows {
                    for cell in row {
                        *cell = style_span_list(std::mem::take(cell), extra);
                    }
                }
            }
            MarkdownDocBlock::FrontMatter(_)
            | MarkdownDocBlock::Code { .. }
            | MarkdownDocBlock::Rule
            | MarkdownDocBlock::IncompleteMath { .. } => {}
        }
    }
}
