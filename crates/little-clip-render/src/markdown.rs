//! Lightweight Markdown parsing for ambient screen previews and desktop document views.
//!
//! Supports:
//! - Headings (# H1 .. ###### H6 and Setext headings)
//! - Bulleted, numbered, nested, and task lists (- [ ] / - [x])
//! - Blockquotes and GFM alerts (> quote, > [!NOTE])
//! - GitHub-Flavored Markdown tables (| col | col |)
//! - Horizontal rules (---, ***, ___)
//! - Fenced or indented code blocks
//! - Best-effort inline HTML formatting, `<details>`, `<table>`, and styled containers
//! - Inline bold, italics, bold-italics, strikethrough, code, links, images, and native math

mod html;
mod hugo;

/// Simple sRGBA color parsed from inline HTML/CSS attributes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RgbaColor {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

/// Block text/image alignment parsed from HTML or inline CSS.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextAlign {
    Left,
    Center,
    Right,
}

/// Whitelisted visual inline CSS properties carried across styled HTML spans.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct InlineCss {
    pub color: Option<RgbaColor>,
    pub background: Option<RgbaColor>,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strikethrough: bool,
    pub monospace: bool,
    pub superscript: bool,
    pub subscript: bool,
    pub kbd: bool,
    pub mark: bool,
    pub small: bool,
    pub muted: bool,
}

impl InlineCss {
    pub(crate) fn merge(self, child: Self) -> Self {
        Self {
            color: child.color.or(self.color),
            background: child.background.or(self.background),
            bold: self.bold || child.bold,
            italic: self.italic || child.italic,
            underline: self.underline || child.underline,
            strikethrough: self.strikethrough || child.strikethrough,
            monospace: self.monospace || child.monospace,
            superscript: self.superscript || child.superscript,
            subscript: self.subscript || child.subscript,
            kbd: self.kbd || child.kbd,
            mark: self.mark || child.mark,
            small: self.small || child.small,
            muted: self.muted || child.muted,
        }
    }

    pub(crate) fn is_plain_compatible(self) -> bool {
        self.color.is_none()
            && self.background.is_none()
            && !self.underline
            && !self.superscript
            && !self.subscript
            && !self.kbd
            && !self.mark
            && !self.small
            && !self.muted
    }
}

/// Best-effort block box style parsed from HTML `<div style="...">` / `align=`.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct BlockBoxStyle {
    pub align: Option<TextAlign>,
    pub text_color: Option<RgbaColor>,
    pub background: Option<RgbaColor>,
    pub border_color: Option<RgbaColor>,
    pub border_width: f32,
    pub border_radius: f32,
    pub padding: [f32; 4],
    pub margin_top: f32,
    pub margin_bottom: f32,
}

impl BlockBoxStyle {
    pub fn has_visual_box(&self) -> bool {
        self.align.is_some()
            || self.text_color.is_some()
            || self.background.is_some()
            || self.border_width > 0.0
            || self.padding.iter().any(|p| *p > 0.0)
            || self.margin_top > 0.0
            || self.margin_bottom > 0.0
    }
}

/// Optional size, cropping/radius, and alignment metadata on an HTML or linked image.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ImagePresentation {
    pub width_px: Option<f32>,
    pub height_px: Option<f32>,
    pub width_fraction: Option<f32>,
    pub zoom: Option<f32>,
    pub border_radius: Option<f32>,
    pub align: Option<TextAlign>,
    pub float: Option<TextAlign>,
}

/// A styled inline segment within a Markdown line or block.
#[derive(Debug, Clone, PartialEq)]
pub enum MarkdownSpan {
    /// Plain unformatted text.
    Text(String),
    /// Bold text (`**text**` or `__text__`).
    Bold(String),
    /// Italic text (`*text*` or `_text_`).
    Italic(String),
    /// Bold and italic text (`***text***`).
    BoldItalic(String),
    /// Strikethrough text (`~~text~~`).
    Strikethrough(String),
    /// Inline code span (`` `code` ``).
    Code(String),
    /// Inline syntax-highlighted code span (such as Hugo `{{< hl >}}`).
    HighlightedCode { language: String, code: String },
    /// Inline HTML/CSS-styled text run (with optional enclosing link).
    Styled {
        text: String,
        url: Option<String>,
        css: InlineCss,
    },
    /// Hyperlink with label and URL (`[label](url)`).
    Link { label: String, url: String },
    /// Image reference (`![alt](url)`).
    Image { alt: String, url: String },
    /// Image reference with HTML/CSS sizing, radius, alignment, or outer link.
    StyledImage {
        alt: String,
        url: String,
        href: Option<String>,
        style: ImagePresentation,
    },
    /// Native equation; a link around it remains attached to the rendered image.
    Math {
        source: String,
        display: bool,
        url: Option<String>,
    },
    /// Inline interactive spoiler run (revealed on hover or click).
    Spoiler { spans: Vec<MarkdownSpan> },
    /// Explicit hard line break inside a paragraph (`  \n` or `\\\n`).
    HardBreak,
}

/// Column alignment in a GitHub-Flavored Markdown table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TableAlignment {
    None,
    Left,
    Center,
    Right,
}

/// A parsed GitHub-Flavored Markdown table with header and body cells.
#[derive(Debug, Clone, PartialEq)]
pub struct MarkdownTable {
    pub alignments: Vec<TableAlignment>,
    pub headers: Vec<Vec<MarkdownSpan>>,
    pub rows: Vec<Vec<Vec<MarkdownSpan>>>,
}

/// Optional GitHub-Flavored Markdown alert category on a blockquote.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuoteKind {
    Note,
    Tip,
    Important,
    Warning,
    Caution,
}

/// One item inside a bulleted, numbered, or task list.
#[derive(Debug, Clone, PartialEq)]
pub struct ListItem {
    pub task: Option<bool>,
    pub blocks: Vec<MarkdownDocBlock>,
}

/// One labeled panel inside a tabbed block (such as Hugo `{{< tabs >}}`).
#[derive(Debug, Clone, PartialEq)]
pub struct MarkdownTab {
    pub title: String,
    pub blocks: Vec<MarkdownDocBlock>,
}

/// A structured CommonMark/GFM block for desktop document rendering.
#[derive(Debug, Clone, PartialEq)]
pub enum MarkdownDocBlock {
    /// Parsed document metadata, or an explicit framing/parse diagnostic.
    FrontMatter(Box<little_clip_core::markdown_metadata::FrontMatter>),
    /// Section heading (`1..=6`).
    Heading {
        level: usize,
        text: String,
        spans: Vec<MarkdownSpan>,
    },
    /// Reflowed paragraph of inline spans.
    Paragraph { spans: Vec<MarkdownSpan> },
    /// Unified blockquote (optionally tagged as a GFM alert) containing nested blocks.
    Quote {
        kind: Option<QuoteKind>,
        blocks: Vec<MarkdownDocBlock>,
    },
    /// Attributed quote (`{{< quote >}}`) or centered display quote (`{{< bigquote >}}`).
    PullQuote {
        big: bool,
        source: Vec<MarkdownSpan>,
        blocks: Vec<MarkdownDocBlock>,
    },
    /// Switchable tabbed panel container (`{{< tabs >}}` / `{{< tab >}}`).
    Tabs { tabs: Vec<MarkdownTab> },
    /// Multi-column flex-like layout (`{{< columns >}}` / `columns2` / `columns3`).
    Columns {
        dividers: bool,
        bottom_rule: bool,
        columns: Vec<Vec<MarkdownDocBlock>>,
    },
    /// Ordered or unordered list containing nested list items.
    List {
        ordered: bool,
        start: u64,
        items: Vec<ListItem>,
    },
    /// Fenced or indented code block.
    Code { language: String, text: String },
    /// GitHub-Flavored Markdown table.
    Table(Box<MarkdownTable>),
    /// Footnote definition block.
    Footnote {
        label: String,
        blocks: Vec<MarkdownDocBlock>,
    },
    /// Collapsible `<details>` / `<summary>` section containing nested blocks.
    Details {
        summary: Vec<MarkdownSpan>,
        open: bool,
        blocks: Vec<MarkdownDocBlock>,
    },
    /// Styled HTML container (`<div style="...">`, `<center>`, etc.) wrapping blocks.
    StyledContainer {
        style: BlockBoxStyle,
        blocks: Vec<MarkdownDocBlock>,
    },
    /// Horizontal divider rule (`---`, `***`, `___`).
    Rule,
    /// An unfinished display equation; never render a misleading partial formula.
    IncompleteMath { source: String },
}

/// A structured Markdown block paired with its UTF-8 byte range in the source.
#[derive(Debug, Clone, PartialEq)]
pub struct LocatedBlock {
    pub source: std::ops::Range<usize>,
    pub block: MarkdownDocBlock,
    pub incomplete_fence: bool,
}

/// A parsed line of Markdown content for ambient preview rendering.
#[derive(Debug, Clone, PartialEq)]
pub enum MarkdownLine {
    /// Parsed document metadata, or an explicit framing/parse diagnostic.
    FrontMatter(Box<little_clip_core::markdown_metadata::FrontMatter>),
    /// Section heading with level 1..=6.
    Heading {
        level: usize,
        text: String,
        spans: Vec<MarkdownSpan>,
    },
    /// Bulleted list item with indentation level.
    Bullet {
        indent: usize,
        spans: Vec<MarkdownSpan>,
    },
    /// Task list item (`- [ ]` or `- [x]`) with indentation level.
    Task {
        indent: usize,
        checked: bool,
        spans: Vec<MarkdownSpan>,
    },
    /// Numbered list item with number prefix.
    Numbered {
        number: String,
        spans: Vec<MarkdownSpan>,
    },
    /// Blockquote line.
    Quote { spans: Vec<MarkdownSpan> },
    /// GitHub-Flavored Markdown table.
    Table(Box<MarkdownTable>),
    /// Horizontal divider rule (`---`, `***`, `___`).
    Rule,
    /// Code fence boundary with optional language tag.
    CodeFence { language: String },
    /// Raw unparsed line within a code block.
    CodeLine { line: String },
    /// Standard text paragraph line.
    Paragraph { spans: Vec<MarkdownSpan> },
    /// An unfinished display equation; never render a misleading partial formula.
    IncompleteMath { source: String },
    /// Blank line between blocks.
    Empty,
}

/// A native Markdown element and the UTF-8 byte range that produced it.
#[derive(Debug, Clone, PartialEq)]
pub struct LocatedLine {
    pub source: std::ops::Range<usize>,
    pub line: MarkdownLine,
}

/// Parses raw Markdown text into a sequence of [`MarkdownLine`] blocks.
pub fn parse_markdown(input: &str) -> Vec<MarkdownLine> {
    let mut lines = Vec::new();
    parse_document(input, |line, _| lines.push(line));
    lines
}

/// Parses the same native elements while retaining their original source ranges.
///
/// Multiline equations cover all consumed lines. A trailing paragraph after an
/// equation shares that source range; front matter retains its full framing.
/// Run preparation on a bounded worker for an interactive source.
pub fn parse_located(input: &str) -> Vec<LocatedLine> {
    let mut lines = Vec::new();
    parse_document(input, |line, source| {
        lines.push(LocatedLine { source, line })
    });
    lines
}

fn parse_document(input: &str, mut emit: impl FnMut(MarkdownLine, std::ops::Range<usize>)) {
    let metadata = little_clip_core::markdown_metadata::parse(input);
    let offset = metadata.as_ref().map_or(0, |value| value.body_start);
    if let Some(metadata) = metadata {
        let range = if metadata.body_start == 0 {
            metadata.source.clone()
        } else {
            0..metadata.body_start
        };
        emit(MarkdownLine::FrontMatter(Box::new(metadata)), range);
    }
    parse_body_into(&input[offset..], |line, range| {
        emit(line, range.start + offset..range.end + offset)
    });
}

// Coalesce physical lines only across complete native math ranges. Keeping this
// lexical step separate preserves the existing code-fence and paragraph parser.
fn logical_lines(input: &str) -> impl Iterator<Item = std::ops::Range<usize>> {
    let mut math = math_ranges(input).into_iter().peekable();
    let mut physical = input.split_inclusive('\n');
    let mut offset = 0;
    std::iter::from_fn(move || {
        let physical_line = physical.next()?;
        let start = offset;
        offset += physical_line.len();
        while let Some(range) = math.peek() {
            if range.end <= offset {
                math.next();
            } else if range.start >= offset {
                break;
            } else {
                while offset < range.end {
                    let Some(line) = physical.next() else { break };
                    offset += line.len();
                }
                math.next();
            }
        }
        Some(start..offset)
    })
}

enum DisplayLine {
    Ordinary,
    Pending,
    Complete {
        source: String,
        tail: String,
        start: usize,
    },
}

// Display blocks have their own framing: Markdown list/rule/fence markers
// inside an equation must not interrupt it. Scan each incoming line once.
fn display_math_line(line: &str, offset: usize, open: &mut Option<(usize, String)>) -> DisplayLine {
    let (rest, start, mut source) = if let Some((start, source)) = open.take() {
        (line, start, source)
    } else if let Some(rest) = line.trim_start().strip_prefix("$$") {
        (rest, offset, String::new())
    } else {
        return DisplayLine::Ordinary;
    };
    let mut escaped = false;
    for (index, value) in rest.char_indices() {
        if !escaped && rest[index..].starts_with("$$") {
            source.push_str(&rest[..index]);
            return DisplayLine::Complete {
                source,
                tail: rest[index + 2..].to_owned(),
                start,
            };
        }
        escaped = !escaped && value == '\\';
    }
    source.push_str(rest);
    source.push('\n');
    *open = Some((start, source));
    DisplayLine::Pending
}

fn parse_body(input: &str) -> Vec<MarkdownLine> {
    let mut lines = Vec::new();
    parse_body_into(input, |line, _| lines.push(line));
    lines
}

fn gfm_options() -> pulldown_cmark::Options {
    use pulldown_cmark::Options;
    Options::ENABLE_TABLES
        | Options::ENABLE_MATH
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_GFM
}

fn is_gfm_delimiter_row(line: &str) -> bool {
    let trimmed = line.trim();
    if !trimmed.contains('|') || !trimmed.contains('-') {
        return false;
    }
    let cells: Vec<&str> = trimmed.trim_matches('|').split('|').collect();
    !cells.is_empty()
        && cells.iter().all(|cell| {
            let cell = cell.trim();
            let dashes = cell.trim_start_matches(':').trim_end_matches(':');
            !dashes.is_empty() && dashes.bytes().all(|b| b == b'-')
        })
}

fn is_gfm_table_start(header: &str, separator: &str) -> bool {
    let header_trimmed = header.trim_start();
    !header_trimmed.is_empty()
        && header_trimmed.contains('|')
        && !header_trimmed.starts_with(['#', '>', '`', '~', '$'])
        && is_gfm_delimiter_row(separator)
}

fn is_table_continuation(line: &str) -> bool {
    let trimmed = line.trim();
    !trimmed.is_empty()
        && trimmed.contains('|')
        && !trimmed.starts_with("```")
        && !trimmed.starts_with("~~~")
        && !trimmed.starts_with('#')
        && !trimmed.starts_with('>')
        && !trimmed.starts_with("$$")
}

fn parse_table_slice(slice: &str) -> Option<MarkdownTable> {
    use pulldown_cmark::{Event, Parser, Tag};
    let prepared = hugo::prepare_shortcodes(slice, 0);
    let effective = prepared.as_ref().map_or(slice, |p| p.rewritten.as_str());
    let mut iter = Parser::new_ext(effective, gfm_options()).peekable();
    let alignments = match iter.next()? {
        Event::Start(Tag::Table(alignments)) => alignments,
        _ => return None,
    };
    Some(consume_table(&mut iter, alignments, prepared.as_ref()))
}

fn map_alignment(alignment: pulldown_cmark::Alignment) -> TableAlignment {
    match alignment {
        pulldown_cmark::Alignment::None => TableAlignment::None,
        pulldown_cmark::Alignment::Left => TableAlignment::Left,
        pulldown_cmark::Alignment::Center => TableAlignment::Center,
        pulldown_cmark::Alignment::Right => TableAlignment::Right,
    }
}

fn consume_table<'a>(
    iter: &mut impl Iterator<Item = pulldown_cmark::Event<'a>>,
    alignments: Vec<pulldown_cmark::Alignment>,
    prepared: Option<&hugo::PreparedShortcodes>,
) -> MarkdownTable {
    use pulldown_cmark::{Event, Tag, TagEnd};
    let mut headers = Vec::new();
    let mut rows = Vec::new();
    let mut current_row: Vec<Vec<MarkdownSpan>> = Vec::new();
    let mut cell_builder: Option<SpanBuilder> = None;

    for event in iter {
        match event {
            Event::End(TagEnd::Table) => break,
            Event::Start(Tag::TableHead) => {
                current_row.clear();
            }
            Event::End(TagEnd::TableHead) => {
                headers = std::mem::take(&mut current_row);
            }
            Event::Start(Tag::TableRow) => {
                current_row.clear();
            }
            Event::End(TagEnd::TableRow) => {
                rows.push(std::mem::take(&mut current_row));
            }
            Event::Start(Tag::TableCell) => {
                cell_builder = Some(SpanBuilder::default());
            }
            Event::End(TagEnd::TableCell) => {
                current_row.push(cell_builder.take().map_or_else(Vec::new, |b| b.finish()));
            }
            other => {
                if let Some(builder) = &mut cell_builder {
                    builder.push_event(other, prepared);
                }
            }
        }
    }

    MarkdownTable {
        alignments: alignments.into_iter().map(map_alignment).collect(),
        headers,
        rows,
    }
}

fn parse_body_into(input: &str, mut sink: impl FnMut(MarkdownLine, std::ops::Range<usize>)) {
    let original = input;
    let input = input.trim_start_matches('\u{feff}');
    let bom = original.len() - input.len();
    let ranges: Vec<_> = logical_lines(input).collect();
    let mut fence: Option<(char, usize)> = None;
    let mut hugo_code: Option<&'static str> = None;
    let mut hugo_math: Option<(&'static str, String, usize)> = None;
    let mut display = None;
    let mut index = 0;
    while index < ranges.len() {
        let range = ranges[index].clone();
        let mut source_range = range.clone();
        let raw_line = input[range.clone()].trim_end_matches(['\r', '\n']);

        if let Some(tag_name) = hugo_code {
            let trimmed = raw_line.trim();
            if hugo::is_closing_shortcode_line(trimmed, tag_name) {
                hugo_code = None;
                sink(
                    MarkdownLine::CodeFence {
                        language: String::new(),
                    },
                    source_range.start + bom..source_range.end + bom,
                );
            } else {
                sink(
                    MarkdownLine::CodeLine {
                        line: raw_line.to_string(),
                    },
                    source_range.start + bom..source_range.end + bom,
                );
            }
            index += 1;
            continue;
        }

        if let Some((tag_name, mut buf, math_start)) = hugo_math.take() {
            let trimmed = raw_line.trim();
            if hugo::is_closing_shortcode_line(trimmed, tag_name) {
                sink(
                    MarkdownLine::Paragraph {
                        spans: vec![MarkdownSpan::Math {
                            source: buf.trim().to_owned(),
                            display: true,
                            url: None,
                        }],
                    },
                    math_start + bom..range.end + bom,
                );
                index += 1;
                continue;
            }
            if let Some(before) = hugo::strip_closing_on_same_line(raw_line, tag_name) {
                if !buf.is_empty() {
                    buf.push('\n');
                }
                buf.push_str(before);
                sink(
                    MarkdownLine::Paragraph {
                        spans: vec![MarkdownSpan::Math {
                            source: buf.trim().to_owned(),
                            display: true,
                            url: None,
                        }],
                    },
                    math_start + bom..range.end + bom,
                );
                index += 1;
                continue;
            }
            if !buf.is_empty() || !trimmed.is_empty() {
                if !buf.is_empty() {
                    buf.push('\n');
                }
                buf.push_str(raw_line);
            }
            hugo_math = Some((tag_name, buf, math_start));
            index += 1;
            continue;
        }

        let tail;
        let raw_line = if fence.is_none() {
            match display_math_line(raw_line, range.start, &mut display) {
                DisplayLine::Ordinary => raw_line,
                DisplayLine::Pending => {
                    index += 1;
                    continue;
                }
                DisplayLine::Complete {
                    source,
                    tail: after,
                    start,
                } => {
                    source_range.start = start;
                    sink(
                        MarkdownLine::Paragraph {
                            spans: vec![MarkdownSpan::Math {
                                source: source.trim().to_owned(),
                                display: true,
                                url: None,
                            }],
                        },
                        source_range.start + bom..source_range.end + bom,
                    );
                    tail = after;
                    if tail.trim().is_empty() {
                        index += 1;
                        continue;
                    }
                    tail.as_str()
                }
            }
        } else {
            raw_line
        };
        let mut emit = |line| sink(line, source_range.start + bom..source_range.end + bom);
        let trimmed_start = raw_line.trim_start();
        let trimmed = raw_line.trim();

        if let Some((marker, minimum)) = fence {
            let run = trimmed.chars().take_while(|value| *value == marker).count();
            if run >= minimum && trimmed.chars().skip(run).all(char::is_whitespace) {
                fence = None;
                emit(MarkdownLine::CodeFence {
                    language: String::new(),
                });
            } else {
                emit(MarkdownLine::CodeLine {
                    line: raw_line.to_string(),
                });
            }
            index += 1;
            continue;
        }
        if let Some(marker @ ('`' | '~')) = trimmed.chars().next() {
            let run = trimmed.chars().take_while(|value| *value == marker).count();
            let language = &trimmed[run..];
            if run >= 3 && (marker != '`' || !language.contains('`')) {
                fence = Some((marker, run));
                emit(MarkdownLine::CodeFence {
                    language: language.trim().to_string(),
                });
                index += 1;
                continue;
            }
        }

        if trimmed == "<--->" {
            index += 1;
            continue;
        }

        if let Some(hugo_start) = hugo::classify_block_line(trimmed) {
            match hugo_start {
                hugo::LineBlockStart::IgnoreLine => {}
                hugo::LineBlockStart::Code {
                    tag_name,
                    language,
                    inline_body: Some(body),
                } => {
                    let _ = tag_name;
                    emit(MarkdownLine::CodeFence { language });
                    emit(MarkdownLine::CodeLine { line: body });
                    emit(MarkdownLine::CodeFence {
                        language: String::new(),
                    });
                }
                hugo::LineBlockStart::Code {
                    tag_name,
                    language,
                    inline_body: None,
                } => {
                    hugo_code = Some(tag_name);
                    emit(MarkdownLine::CodeFence { language });
                }
                hugo::LineBlockStart::Details {
                    summary_spans,
                    inline_body,
                } => {
                    emit(MarkdownLine::Paragraph {
                        spans: summary_spans,
                    });
                    if let Some(body) = inline_body
                        && !body.is_empty()
                    {
                        emit(MarkdownLine::Paragraph {
                            spans: parse_markdown_spans(&body),
                        });
                    }
                }
                hugo::LineBlockStart::Math {
                    inline_source: Some(source),
                    display,
                    ..
                } => {
                    emit(MarkdownLine::Paragraph {
                        spans: vec![MarkdownSpan::Math {
                            source,
                            display,
                            url: None,
                        }],
                    });
                }
                hugo::LineBlockStart::Math {
                    tag_name,
                    inline_source: None,
                    ..
                } => {
                    let has_close = ranges[index + 1..].iter().any(|r| {
                        let raw_next = input[r.clone()].trim_end_matches(['\r', '\n']);
                        hugo::is_closing_shortcode_line(raw_next.trim(), tag_name)
                            || hugo::strip_closing_on_same_line(raw_next, tag_name).is_some()
                    });
                    if has_close {
                        hugo_math = Some((tag_name, String::new(), range.start));
                    }
                }
            }
            index += 1;
            continue;
        }

        if trimmed.is_empty() {
            emit(MarkdownLine::Empty);
            index += 1;
            continue;
        }

        // GFM tables (header + delimiter row + optional body rows).
        if source_range.start == range.start
            && index + 1 < ranges.len()
            && is_gfm_table_start(
                raw_line,
                input[ranges[index + 1].clone()].trim_end_matches(['\r', '\n']),
            )
        {
            let mut end_idx = index + 2;
            while end_idx < ranges.len()
                && is_table_continuation(
                    input[ranges[end_idx].clone()].trim_end_matches(['\r', '\n']),
                )
            {
                end_idx += 1;
            }
            let table_slice = &input[range.start..ranges[end_idx - 1].end];
            if let Some(table) = parse_table_slice(table_slice) {
                sink(
                    MarkdownLine::Table(Box::new(table)),
                    range.start + bom..ranges[end_idx - 1].end + bom,
                );
                index = end_idx;
                continue;
            }
        }

        // Horizontal rules and headings share one framing path each; adding
        // inline content must not create another copy for every heading level.
        if let Some(marker @ (b'-' | b'*' | b'_')) = trimmed.as_bytes().first()
            && trimmed.len() >= 3
            && trimmed.bytes().all(|value| value == *marker)
        {
            emit(MarkdownLine::Rule);
            index += 1;
            continue;
        }
        if let Some((marker, rest)) = trimmed_start.split_once(' ')
            && matches!(marker, "#" | "##" | "###" | "####" | "#####" | "######")
        {
            emit(MarkdownLine::Heading {
                level: marker.len(),
                text: rest.trim().to_owned(),
                spans: parse_markdown_spans(rest.trim()),
            });
            index += 1;
            continue;
        }

        // Blockquotes
        if let Some(rest) = trimmed_start.strip_prefix("> ") {
            let spans = parse_markdown_spans(rest.trim());
            emit(MarkdownLine::Quote { spans });
            index += 1;
            continue;
        }

        // Bulleted & task lists
        let indent = raw_line.len() - trimmed_start.len();
        if let Some(rest) = trimmed_start
            .strip_prefix("- ")
            .or_else(|| trimmed_start.strip_prefix("* "))
            .or_else(|| trimmed_start.strip_prefix("+ "))
        {
            if let Some(task_rest) = rest.strip_prefix("[ ] ") {
                emit(MarkdownLine::Task {
                    indent,
                    checked: false,
                    spans: parse_markdown_spans(task_rest.trim()),
                });
            } else if let Some(task_rest) = rest
                .strip_prefix("[x] ")
                .or_else(|| rest.strip_prefix("[X] "))
            {
                emit(MarkdownLine::Task {
                    indent,
                    checked: true,
                    spans: parse_markdown_spans(task_rest.trim()),
                });
            } else {
                let spans = parse_markdown_spans(rest.trim());
                emit(MarkdownLine::Bullet { indent, spans });
            }
            index += 1;
            continue;
        }

        // Numbered lists (e.g. "1. ", "10. ")
        if let Some(dot_pos) = trimmed_start.find(". ") {
            let num_part = &trimmed_start[..dot_pos];
            if !num_part.is_empty() && num_part.chars().all(|c| c.is_ascii_digit()) {
                let rest = &trimmed_start[dot_pos + 2..];
                let spans = parse_markdown_spans(rest.trim());
                emit(MarkdownLine::Numbered {
                    number: format!("{}.", num_part),
                    spans,
                });
                index += 1;
                continue;
            }
        }

        // Normal paragraph
        let spans = parse_markdown_spans(trimmed);
        emit(MarkdownLine::Paragraph { spans });
        index += 1;
    }

    if let Some((start, source)) = display {
        sink(
            MarkdownLine::IncompleteMath { source },
            start + bom..original.len(),
        );
    }
}

/// Parses Markdown into structured CommonMark/GFM blocks with reflowed paragraphs,
/// nested lists, grouped blockquotes, and tables.
pub fn parse_blocks(input: &str) -> Vec<MarkdownDocBlock> {
    parse_located_blocks(input)
        .into_iter()
        .map(|item| item.block)
        .collect()
}

/// Parses structured Markdown blocks while retaining each top-level block's UTF-8 byte range.
pub fn parse_located_blocks(input: &str) -> Vec<LocatedBlock> {
    let mut blocks = Vec::new();
    let metadata = little_clip_core::markdown_metadata::parse(input);
    let offset = metadata.as_ref().map_or(0, |value| value.body_start);
    if let Some(metadata) = metadata {
        let source = if metadata.body_start == 0 {
            metadata.source.clone()
        } else {
            0..metadata.body_start
        };
        blocks.push(LocatedBlock {
            source,
            block: MarkdownDocBlock::FrontMatter(Box::new(metadata)),
            incomplete_fence: false,
        });
    }
    let original_body = &input[offset..];
    let trimmed_body = original_body.trim_start_matches('\u{feff}');
    let bom = original_body.len() - trimmed_body.len();
    let base = offset + bom;
    blocks.extend(parse_located_body_inner(trimmed_body, base, 0));
    blocks
}

pub(super) fn parse_located_body_inner(
    trimmed_body: &str,
    base: usize,
    depth: usize,
) -> Vec<LocatedBlock> {
    let mut blocks = Vec::new();
    let prepared = hugo::prepare_shortcodes(trimmed_body, depth);
    let effective_body = prepared
        .as_ref()
        .map_or(trimmed_body, |p| p.rewritten.as_str());
    let mut container_stack: Vec<OpenFrame> = Vec::new();

    let map_segment_range = |range: std::ops::Range<usize>| match &prepared {
        Some(p) => {
            let r = p.map_range(range);
            r.start + base..r.end + base
        }
        None => range.start + base..range.end + base,
    };

    for segment in split_display_math_segments(effective_body) {
        match segment {
            BodySegment::DisplayMath { range, source } => {
                let mapped = map_segment_range(range);
                push_doc_block(
                    &mut container_stack,
                    &mut blocks,
                    MarkdownDocBlock::Paragraph {
                        spans: vec![MarkdownSpan::Math {
                            source,
                            display: true,
                            url: None,
                        }],
                    },
                    mapped,
                    false,
                );
            }
            BodySegment::IncompleteMath { range, source } => {
                let mapped = map_segment_range(range);
                push_doc_block(
                    &mut container_stack,
                    &mut blocks,
                    MarkdownDocBlock::IncompleteMath { source },
                    mapped,
                    false,
                );
            }
            BodySegment::Markdown {
                range,
                incomplete_fence_start,
            } => {
                parse_markdown_segment(
                    &effective_body[range.clone()],
                    range.start,
                    base,
                    incomplete_fence_start,
                    prepared.as_ref(),
                    &mut container_stack,
                    &mut blocks,
                );
            }
        }
    }

    while !container_stack.is_empty() {
        if matches!(
            container_stack.last(),
            Some(OpenFrame::HtmlDetails { .. } | OpenFrame::HtmlContainer { .. })
        ) {
            close_top_html_container(&mut container_stack, &mut blocks, None);
        } else {
            container_stack.pop();
        }
    }

    blocks
}

enum BodySegment {
    Markdown {
        range: std::ops::Range<usize>,
        incomplete_fence_start: Option<usize>,
    },
    DisplayMath {
        range: std::ops::Range<usize>,
        source: String,
    },
    IncompleteMath {
        range: std::ops::Range<usize>,
        source: String,
    },
}

fn split_display_math_segments(body: &str) -> Vec<BodySegment> {
    let mut segments = Vec::new();
    let mut fence: Option<(char, usize, usize)> = None;
    let mut display: Option<(usize, String)> = None;
    let mut md_start = 0usize;
    let mut offset = 0usize;

    for physical_line in body.split_inclusive('\n') {
        let line_start = offset;
        offset += physical_line.len();
        let raw_line = physical_line.trim_end_matches(['\r', '\n']);

        if let Some((marker, minimum, _)) = fence {
            let trimmed = raw_line.trim();
            let run = trimmed.chars().take_while(|value| *value == marker).count();
            if run >= minimum && trimmed.chars().skip(run).all(char::is_whitespace) {
                fence = None;
            }
            continue;
        }

        match display_math_line(raw_line, line_start, &mut display) {
            DisplayLine::Pending => continue,
            DisplayLine::Complete {
                source,
                tail,
                start,
            } => {
                if md_start < start {
                    segments.push(BodySegment::Markdown {
                        range: md_start..start,
                        incomplete_fence_start: None,
                    });
                }
                let math_end = if tail.trim().is_empty() {
                    offset
                } else {
                    line_start + (raw_line.len() - tail.len())
                };
                segments.push(BodySegment::DisplayMath {
                    range: start..math_end,
                    source: source.trim().to_owned(),
                });
                md_start = math_end;
                if !tail.trim().is_empty() {
                    let trimmed = tail.trim();
                    if let Some(marker @ ('`' | '~')) = trimmed.chars().next() {
                        let run = trimmed.chars().take_while(|v| *v == marker).count();
                        let language = &trimmed[run..];
                        if run >= 3 && (marker != '`' || !language.contains('`')) {
                            fence = Some((marker, run, md_start));
                        }
                    }
                }
                continue;
            }
            DisplayLine::Ordinary => {}
        }

        let trimmed_start = raw_line.trim_start();
        let leading = raw_line.len() - trimmed_start.len();
        let trimmed = trimmed_start.trim_end();
        if leading < 4
            && let Some(marker @ ('`' | '~')) = trimmed.chars().next()
        {
            let run = trimmed.chars().take_while(|value| *value == marker).count();
            let language = &trimmed[run..];
            if run >= 3 && (marker != '`' || !language.contains('`')) {
                fence = Some((marker, run, line_start));
            }
        }
    }

    if let Some((start, source)) = display {
        if md_start < start {
            segments.push(BodySegment::Markdown {
                range: md_start..start,
                incomplete_fence_start: None,
            });
        }
        segments.push(BodySegment::IncompleteMath {
            range: start..body.len(),
            source,
        });
    } else if md_start < body.len() {
        segments.push(BodySegment::Markdown {
            range: md_start..body.len(),
            incomplete_fence_start: fence.map(|(_, _, start)| start),
        });
    }

    segments
}

fn map_quote_kind(kind: pulldown_cmark::BlockQuoteKind) -> QuoteKind {
    use pulldown_cmark::BlockQuoteKind;
    match kind {
        BlockQuoteKind::Note => QuoteKind::Note,
        BlockQuoteKind::Tip => QuoteKind::Tip,
        BlockQuoteKind::Important => QuoteKind::Important,
        BlockQuoteKind::Warning => QuoteKind::Warning,
        BlockQuoteKind::Caution => QuoteKind::Caution,
    }
}

fn plain_text_from_spans(spans: &[MarkdownSpan]) -> String {
    let mut out = String::new();
    for span in spans {
        match span {
            MarkdownSpan::Text(s)
            | MarkdownSpan::Bold(s)
            | MarkdownSpan::Italic(s)
            | MarkdownSpan::BoldItalic(s)
            | MarkdownSpan::Strikethrough(s)
            | MarkdownSpan::Code(s) => out.push_str(s),
            MarkdownSpan::HighlightedCode { code, .. } => out.push_str(code),
            MarkdownSpan::Styled { text, .. } => out.push_str(text),
            MarkdownSpan::Link { label, .. } => out.push_str(label),
            MarkdownSpan::Image { alt, .. } | MarkdownSpan::StyledImage { alt, .. } => {
                out.push_str(alt)
            }
            MarkdownSpan::Math { source, .. } => out.push_str(source),
            MarkdownSpan::Spoiler { spans } => out.push_str(&plain_text_from_spans(spans)),
            MarkdownSpan::HardBreak => out.push(' '),
        }
    }
    out
}

enum OpenFrame {
    Heading {
        level: usize,
        spans: SpanBuilder,
    },
    Paragraph {
        spans: SpanBuilder,
    },
    Quote {
        kind: Option<QuoteKind>,
        blocks: Vec<MarkdownDocBlock>,
    },
    List {
        ordered: bool,
        start: u64,
        items: Vec<ListItem>,
    },
    Item {
        task: Option<bool>,
        blocks: Vec<MarkdownDocBlock>,
        inline: Option<SpanBuilder>,
    },
    Code {
        language: String,
        text: String,
    },
    Footnote {
        label: String,
        blocks: Vec<MarkdownDocBlock>,
    },
    HtmlDetails {
        summary: Vec<MarkdownSpan>,
        open: bool,
        blocks: Vec<MarkdownDocBlock>,
        source: std::ops::Range<usize>,
    },
    HtmlContainer {
        tag: String,
        style: BlockBoxStyle,
        hidden: bool,
        blocks: Vec<LocatedBlock>,
        source: std::ops::Range<usize>,
    },
    Html {
        text: String,
    },
}

fn flush_item_inline(frame: &mut OpenFrame, prepared: Option<&hugo::PreparedShortcodes>) {
    if let OpenFrame::Item { blocks, inline, .. } = frame
        && let Some(builder) = inline.take()
    {
        let spans = builder.finish();
        if !spans.is_empty() {
            for (block, _) in hugo::split_paragraph_or_blocks(spans, prepared) {
                blocks.push(block);
            }
        }
    }
}

fn push_doc_block(
    stack: &mut [OpenFrame],
    top: &mut Vec<LocatedBlock>,
    block: MarkdownDocBlock,
    source: std::ops::Range<usize>,
    incomplete_fence: bool,
) {
    if let Some(parent) = stack.last_mut() {
        flush_item_inline(parent, None);
        match parent {
            OpenFrame::Quote { blocks, .. }
            | OpenFrame::Item { blocks, .. }
            | OpenFrame::Footnote { blocks, .. } => blocks.push(block),
            OpenFrame::HtmlDetails {
                blocks,
                source: parent_src,
                ..
            } => {
                parent_src.end = parent_src.end.max(source.end);
                blocks.push(block);
            }
            OpenFrame::HtmlContainer {
                blocks,
                source: parent_src,
                ..
            } => {
                parent_src.end = parent_src.end.max(source.end);
                blocks.push(LocatedBlock {
                    source,
                    block,
                    incomplete_fence,
                });
            }
            _ => {}
        }
    } else {
        top.push(LocatedBlock {
            source,
            block,
            incomplete_fence,
        });
    }
}

fn is_at_block_root(stack: &[OpenFrame]) -> bool {
    stack.iter().all(|f| {
        matches!(
            f,
            OpenFrame::HtmlDetails { .. } | OpenFrame::HtmlContainer { .. }
        )
    })
}

fn parse_markdown_segment(
    segment: &str,
    segment_rew_start: usize,
    base: usize,
    incomplete_fence_rew_start: Option<usize>,
    prepared: Option<&hugo::PreparedShortcodes>,
    stack: &mut Vec<OpenFrame>,
    out: &mut Vec<LocatedBlock>,
) {
    use pulldown_cmark::{CodeBlockKind, Event, Parser, Tag, TagEnd};
    let mut iter = Parser::new_ext(segment, gfm_options()).into_offset_iter();
    let mut top_range: Option<std::ops::Range<usize>> = None;
    let map_src = |rel: std::ops::Range<usize>| {
        let rew = (segment_rew_start + rel.start)..(segment_rew_start + rel.end);
        let orig = prepared.map_or(rew.clone(), |p| p.map_range(rew));
        (orig.start + base)..(orig.end + base)
    };
    let incomplete_fence_orig =
        incomplete_fence_rew_start.map(|s| prepared.map_or(s, |p| p.map_offset(s)) + base);

    while let Some((event, range)) = iter.next() {
        if is_at_block_root(stack) {
            top_range = Some(map_src(range.clone()));
        }
        match event {
            Event::Start(Tag::Table(alignments)) => {
                let mut table_events = std::iter::from_fn(|| iter.next().map(|(ev, _)| ev));
                let table = consume_table(&mut table_events, alignments, prepared);
                let src = top_range.clone().unwrap_or_else(|| map_src(range.clone()));
                push_doc_block(
                    stack,
                    out,
                    MarkdownDocBlock::Table(Box::new(table)),
                    src,
                    false,
                );
            }
            Event::Start(Tag::Heading { level, .. }) => {
                stack.push(OpenFrame::Heading {
                    level: level as usize,
                    spans: SpanBuilder::default(),
                });
            }
            Event::End(TagEnd::Heading(_)) => {
                if let Some(OpenFrame::Heading { level, spans }) = stack.pop() {
                    let spans = spans.finish();
                    let text = plain_text_from_spans(&spans);
                    let src = top_range.clone().unwrap_or_else(|| map_src(range.clone()));
                    push_doc_block(
                        stack,
                        out,
                        MarkdownDocBlock::Heading { level, text, spans },
                        src,
                        false,
                    );
                }
            }
            Event::Start(Tag::Paragraph) => {
                if let Some(parent) = stack.last_mut() {
                    flush_item_inline(parent, prepared);
                }
                stack.push(OpenFrame::Paragraph {
                    spans: SpanBuilder::default(),
                });
            }
            Event::End(TagEnd::Paragraph) => {
                if let Some(OpenFrame::Paragraph { spans }) = stack.pop() {
                    let spans = spans.finish();
                    if !spans.is_empty() {
                        let src = top_range.clone().unwrap_or_else(|| map_src(range.clone()));
                        for (block, incomplete) in hugo::split_paragraph_or_blocks(spans, prepared)
                        {
                            push_doc_block(stack, out, block, src.clone(), incomplete);
                        }
                    }
                }
            }
            Event::Start(Tag::BlockQuote(kind)) => {
                if let Some(parent) = stack.last_mut() {
                    flush_item_inline(parent, prepared);
                }
                stack.push(OpenFrame::Quote {
                    kind: kind.map(map_quote_kind),
                    blocks: Vec::new(),
                });
            }
            Event::End(TagEnd::BlockQuote(_)) => {
                close_trailing_html_containers(stack, out);
                if let Some(OpenFrame::Quote { kind, blocks }) = stack.pop() {
                    let src = top_range.clone().unwrap_or_else(|| map_src(range.clone()));
                    push_doc_block(
                        stack,
                        out,
                        MarkdownDocBlock::Quote { kind, blocks },
                        src,
                        false,
                    );
                }
            }
            Event::Start(Tag::List(start)) => {
                if let Some(parent) = stack.last_mut() {
                    flush_item_inline(parent, prepared);
                }
                stack.push(OpenFrame::List {
                    ordered: start.is_some(),
                    start: start.unwrap_or(1),
                    items: Vec::new(),
                });
            }
            Event::End(TagEnd::List(_)) => {
                close_trailing_html_containers(stack, out);
                if let Some(OpenFrame::List {
                    ordered,
                    start,
                    items,
                }) = stack.pop()
                {
                    let src = top_range.clone().unwrap_or_else(|| map_src(range.clone()));
                    push_doc_block(
                        stack,
                        out,
                        MarkdownDocBlock::List {
                            ordered,
                            start,
                            items,
                        },
                        src,
                        false,
                    );
                }
            }
            Event::Start(Tag::Item) => {
                stack.push(OpenFrame::Item {
                    task: None,
                    blocks: Vec::new(),
                    inline: None,
                });
            }
            Event::End(TagEnd::Item) => {
                close_trailing_html_containers(stack, out);
                if let Some(mut frame) = stack.pop() {
                    flush_item_inline(&mut frame, prepared);
                    if let OpenFrame::Item { task, blocks, .. } = frame
                        && let Some(OpenFrame::List { items, .. }) = stack.last_mut()
                    {
                        items.push(ListItem { task, blocks });
                    }
                }
            }
            Event::TaskListMarker(checked) => {
                if let Some(OpenFrame::Item { task, .. }) = stack.last_mut() {
                    *task = Some(checked);
                }
            }
            Event::Start(Tag::CodeBlock(kind)) => {
                if let Some(parent) = stack.last_mut() {
                    flush_item_inline(parent, prepared);
                }
                let language = match kind {
                    CodeBlockKind::Fenced(info) => {
                        info.split_whitespace().next().unwrap_or("").to_owned()
                    }
                    CodeBlockKind::Indented => String::new(),
                };
                stack.push(OpenFrame::Code {
                    language,
                    text: String::new(),
                });
            }
            Event::End(TagEnd::CodeBlock) => {
                if let Some(OpenFrame::Code { language, text }) = stack.pop() {
                    let src = top_range.clone().unwrap_or_else(|| map_src(range.clone()));
                    if language.is_empty()
                        && let Some(rep) = prepared.and_then(|p| p.block_at(&text))
                    {
                        for (block, incomplete) in rep.to_doc_blocks() {
                            push_doc_block(stack, out, block, src.clone(), incomplete);
                        }
                    } else {
                        let incomplete_fence = incomplete_fence_orig
                            .is_some_and(|start| src.start <= start && start < src.end);
                        push_doc_block(
                            stack,
                            out,
                            MarkdownDocBlock::Code { language, text },
                            src,
                            incomplete_fence,
                        );
                    }
                }
            }
            Event::Start(Tag::FootnoteDefinition(label)) => {
                stack.push(OpenFrame::Footnote {
                    label: label.into_string(),
                    blocks: Vec::new(),
                });
            }
            Event::End(TagEnd::FootnoteDefinition) => {
                close_trailing_html_containers(stack, out);
                if let Some(OpenFrame::Footnote { label, blocks }) = stack.pop() {
                    let src = top_range.clone().unwrap_or_else(|| map_src(range.clone()));
                    push_doc_block(
                        stack,
                        out,
                        MarkdownDocBlock::Footnote { label, blocks },
                        src,
                        false,
                    );
                }
            }
            Event::Start(Tag::HtmlBlock) => {
                stack.push(OpenFrame::Html {
                    text: String::new(),
                });
            }
            Event::End(TagEnd::HtmlBlock) => {
                if let Some(OpenFrame::Html { text }) = stack.pop() {
                    let src = top_range.clone().unwrap_or_else(|| map_src(range.clone()));
                    apply_html_block(&text, src, prepared, stack, out);
                }
            }
            Event::Rule => {
                let src = top_range.clone().unwrap_or_else(|| map_src(range.clone()));
                push_doc_block(stack, out, MarkdownDocBlock::Rule, src, false);
            }
            other => match stack.last_mut() {
                Some(OpenFrame::Heading { spans, .. } | OpenFrame::Paragraph { spans }) => {
                    spans.push_event(other, prepared);
                }
                Some(OpenFrame::Item { inline, .. }) => {
                    inline
                        .get_or_insert_with(SpanBuilder::default)
                        .push_event(other, prepared);
                }
                Some(OpenFrame::Code { text, .. } | OpenFrame::Html { text }) => {
                    if let Event::Text(value) | Event::Html(value) = other {
                        text.push_str(&value);
                    }
                }
                _ => {
                    if let Event::Html(value) | Event::InlineHtml(value) = other {
                        let src = map_src(range);
                        apply_html_block(&value, src, prepared, stack, out);
                    }
                }
            },
        }
    }

    while let Some(top) = stack.last() {
        if matches!(
            top,
            OpenFrame::HtmlDetails { .. } | OpenFrame::HtmlContainer { .. }
        ) {
            break;
        }
        stack.pop();
    }
}

fn apply_html_block(
    raw: &str,
    src: std::ops::Range<usize>,
    prepared: Option<&hugo::PreparedShortcodes>,
    stack: &mut Vec<OpenFrame>,
    out: &mut Vec<LocatedBlock>,
) {
    let html_floor = |stack: &[OpenFrame]| {
        stack
            .iter()
            .rposition(|f| {
                !matches!(
                    f,
                    OpenFrame::HtmlDetails { .. } | OpenFrame::HtmlContainer { .. }
                )
            })
            .map_or(0, |idx| idx + 1)
    };
    for action in html::classify_html_block(raw) {
        match action {
            html::ContainerAction::OpenDetails { mut summary, open } => {
                if let Some(p) = prepared {
                    summary = p.expand_span_list(summary);
                }
                stack.push(OpenFrame::HtmlDetails {
                    summary,
                    open,
                    blocks: Vec::new(),
                    source: src.clone(),
                });
            }
            html::ContainerAction::CloseDetails => {
                let floor = html_floor(stack);
                if let Some(rel) = stack[floor..]
                    .iter()
                    .rposition(|f| matches!(f, OpenFrame::HtmlDetails { .. }))
                {
                    let pos = floor + rel;
                    while stack.len() > pos + 1 {
                        close_top_html_container(stack, out, Some(src.end));
                    }
                    close_top_html_container(stack, out, Some(src.end));
                }
            }
            html::ContainerAction::OpenBox { tag, style, hidden } => {
                stack.push(OpenFrame::HtmlContainer {
                    tag,
                    style,
                    hidden,
                    blocks: Vec::new(),
                    source: src.clone(),
                });
            }
            html::ContainerAction::CloseBox { tag } => {
                let floor = html_floor(stack);
                if let Some(rel) = stack[floor..].iter().rposition(|f| {
                    matches!(f, OpenFrame::HtmlContainer { tag: open_tag, .. } if *open_tag == tag)
                }) {
                    let pos = floor + rel;
                    while stack.len() > pos + 1 {
                        close_top_html_container(stack, out, Some(src.end));
                    }
                    close_top_html_container(stack, out, Some(src.end));
                }
            }
            html::ContainerAction::Blocks(blocks) => {
                for mut block in blocks {
                    if let Some(p) = prepared {
                        p.expand_block_spans(&mut block);
                    }
                    push_doc_block(stack, out, block, src.clone(), false);
                }
            }
        }
    }
}

fn close_trailing_html_containers(stack: &mut Vec<OpenFrame>, out: &mut Vec<LocatedBlock>) {
    while matches!(
        stack.last(),
        Some(OpenFrame::HtmlDetails { .. } | OpenFrame::HtmlContainer { .. })
    ) {
        close_top_html_container(stack, out, None);
    }
}

fn close_top_html_container(
    stack: &mut Vec<OpenFrame>,
    out: &mut Vec<LocatedBlock>,
    close_end: Option<usize>,
) {
    match stack.pop() {
        Some(OpenFrame::HtmlDetails {
            summary,
            open,
            blocks,
            mut source,
        }) => {
            if let Some(end) = close_end {
                source.end = source.end.max(end);
            }
            push_doc_block(
                stack,
                out,
                MarkdownDocBlock::Details {
                    summary,
                    open,
                    blocks,
                },
                source,
                false,
            );
        }
        Some(OpenFrame::HtmlContainer {
            style,
            hidden,
            blocks,
            mut source,
            ..
        }) => {
            if hidden || blocks.is_empty() {
                return;
            }
            if let Some(end) = close_end {
                source.end = source.end.max(end);
            }
            if style.has_visual_box() {
                let inner_blocks = blocks.into_iter().map(|b| b.block).collect();
                push_doc_block(
                    stack,
                    out,
                    MarkdownDocBlock::StyledContainer {
                        style,
                        blocks: inner_blocks,
                    },
                    source,
                    false,
                );
            } else {
                for item in blocks {
                    push_doc_block(stack, out, item.block, item.source, item.incomplete_fence);
                }
            }
        }
        _ => {}
    }
}

/// Converts line-oriented [`MarkdownLine`] items (such as bounded notebook cells)
/// into structured [`MarkdownDocBlock`] values for shared desktop rendering.
pub fn blocks_from_lines(lines: impl IntoIterator<Item = MarkdownLine>) -> Vec<MarkdownDocBlock> {
    let mut blocks = Vec::new();
    let mut code: Option<(String, String)> = None;
    let mut paragraph: Option<Vec<MarkdownSpan>> = None;
    let mut quote: Option<Vec<MarkdownDocBlock>> = None;
    let mut list: Option<(bool, u64, Vec<ListItem>)> = None;

    let flush_paragraph = |paragraph: &mut Option<Vec<MarkdownSpan>>,
                           blocks: &mut Vec<MarkdownDocBlock>| {
        if let Some(spans) = paragraph.take()
            && !spans.is_empty()
        {
            blocks.push(MarkdownDocBlock::Paragraph { spans });
        }
    };
    let flush_quote = |quote: &mut Option<Vec<MarkdownDocBlock>>,
                       blocks: &mut Vec<MarkdownDocBlock>| {
        if let Some(inner) = quote.take()
            && !inner.is_empty()
        {
            blocks.push(MarkdownDocBlock::Quote {
                kind: None,
                blocks: inner,
            });
        }
    };
    let flush_list = |list: &mut Option<(bool, u64, Vec<ListItem>)>,
                      blocks: &mut Vec<MarkdownDocBlock>| {
        if let Some((ordered, start, items)) = list.take()
            && !items.is_empty()
        {
            blocks.push(MarkdownDocBlock::List {
                ordered,
                start,
                items,
            });
        }
    };

    for line in lines {
        match (line, &mut code) {
            (MarkdownLine::CodeLine { line }, Some((_, text))) => {
                text.push_str(&line);
                text.push('\n');
                continue;
            }
            (MarkdownLine::Empty, Some((_, text))) => {
                text.push('\n');
                continue;
            }
            (MarkdownLine::CodeFence { language }, None) => {
                flush_paragraph(&mut paragraph, &mut blocks);
                flush_quote(&mut quote, &mut blocks);
                flush_list(&mut list, &mut blocks);
                code = Some((language, String::new()));
                continue;
            }
            (MarkdownLine::CodeFence { .. }, Some(_)) => {
                if let Some((language, text)) = code.take() {
                    blocks.push(MarkdownDocBlock::Code { language, text });
                }
                continue;
            }
            (other, _) => {
                if let Some((language, text)) = code.take() {
                    blocks.push(MarkdownDocBlock::Code { language, text });
                }
                match other {
                    MarkdownLine::Paragraph { spans } => {
                        flush_quote(&mut quote, &mut blocks);
                        flush_list(&mut list, &mut blocks);
                        if let Some(existing) = &mut paragraph {
                            push_span(existing, MarkdownSpan::Text(" ".into()));
                            for span in spans {
                                push_span(existing, span);
                            }
                        } else {
                            paragraph = Some(spans);
                        }
                    }
                    MarkdownLine::Quote { spans } => {
                        flush_paragraph(&mut paragraph, &mut blocks);
                        flush_list(&mut list, &mut blocks);
                        let inner = quote.get_or_insert_with(Vec::new);
                        if let Some(MarkdownDocBlock::Paragraph { spans: existing }) =
                            inner.last_mut()
                        {
                            push_span(existing, MarkdownSpan::Text(" ".into()));
                            for span in spans {
                                push_span(existing, span);
                            }
                        } else {
                            inner.push(MarkdownDocBlock::Paragraph { spans });
                        }
                    }
                    MarkdownLine::Bullet { spans, .. } => {
                        flush_paragraph(&mut paragraph, &mut blocks);
                        flush_quote(&mut quote, &mut blocks);
                        if list.as_ref().is_some_and(|(ordered, _, _)| *ordered) {
                            flush_list(&mut list, &mut blocks);
                        }
                        let (_, _, items) = list.get_or_insert_with(|| (false, 1, Vec::new()));
                        items.push(ListItem {
                            task: None,
                            blocks: vec![MarkdownDocBlock::Paragraph { spans }],
                        });
                    }
                    MarkdownLine::Task { checked, spans, .. } => {
                        flush_paragraph(&mut paragraph, &mut blocks);
                        flush_quote(&mut quote, &mut blocks);
                        if list.as_ref().is_some_and(|(ordered, _, _)| *ordered) {
                            flush_list(&mut list, &mut blocks);
                        }
                        let (_, _, items) = list.get_or_insert_with(|| (false, 1, Vec::new()));
                        items.push(ListItem {
                            task: Some(checked),
                            blocks: vec![MarkdownDocBlock::Paragraph { spans }],
                        });
                    }
                    MarkdownLine::Numbered { number, spans } => {
                        flush_paragraph(&mut paragraph, &mut blocks);
                        flush_quote(&mut quote, &mut blocks);
                        if list.as_ref().is_some_and(|(ordered, _, _)| !*ordered) {
                            flush_list(&mut list, &mut blocks);
                        }
                        let start = number.trim_end_matches('.').parse::<u64>().unwrap_or(1);
                        let (_, _, items) = list.get_or_insert_with(|| (true, start, Vec::new()));
                        items.push(ListItem {
                            task: None,
                            blocks: vec![MarkdownDocBlock::Paragraph { spans }],
                        });
                    }
                    MarkdownLine::Empty => {
                        flush_paragraph(&mut paragraph, &mut blocks);
                        flush_quote(&mut quote, &mut blocks);
                        flush_list(&mut list, &mut blocks);
                    }
                    MarkdownLine::FrontMatter(meta) => {
                        flush_paragraph(&mut paragraph, &mut blocks);
                        flush_quote(&mut quote, &mut blocks);
                        flush_list(&mut list, &mut blocks);
                        blocks.push(MarkdownDocBlock::FrontMatter(meta));
                    }
                    MarkdownLine::Heading { level, text, spans } => {
                        flush_paragraph(&mut paragraph, &mut blocks);
                        flush_quote(&mut quote, &mut blocks);
                        flush_list(&mut list, &mut blocks);
                        blocks.push(MarkdownDocBlock::Heading { level, text, spans });
                    }
                    MarkdownLine::Table(table) => {
                        flush_paragraph(&mut paragraph, &mut blocks);
                        flush_quote(&mut quote, &mut blocks);
                        flush_list(&mut list, &mut blocks);
                        blocks.push(MarkdownDocBlock::Table(table));
                    }
                    MarkdownLine::Rule => {
                        flush_paragraph(&mut paragraph, &mut blocks);
                        flush_quote(&mut quote, &mut blocks);
                        flush_list(&mut list, &mut blocks);
                        blocks.push(MarkdownDocBlock::Rule);
                    }
                    MarkdownLine::IncompleteMath { source } => {
                        flush_paragraph(&mut paragraph, &mut blocks);
                        flush_quote(&mut quote, &mut blocks);
                        flush_list(&mut list, &mut blocks);
                        blocks.push(MarkdownDocBlock::IncompleteMath { source });
                    }
                    MarkdownLine::CodeFence { .. } | MarkdownLine::CodeLine { .. } => {}
                }
            }
        }
    }

    if let Some((language, text)) = code {
        blocks.push(MarkdownDocBlock::Code { language, text });
    }
    flush_paragraph(&mut paragraph, &mut blocks);
    flush_quote(&mut quote, &mut blocks);
    flush_list(&mut list, &mut blocks);
    blocks
}

/// Prepares only the bounded prefix visible in a screen card. Full line counting
/// preserves its overflow badge without allocating/parsing the rest of the document.
pub(crate) fn parse_markdown_preview(input: &str) -> (Vec<MarkdownLine>, usize) {
    const MAX_PREVIEW_BYTES: usize = 4096;
    const MAX_PREVIEW_LINES: usize = 64;
    let metadata = little_clip_core::markdown_metadata::parse(input);
    let body = &input[metadata.as_ref().map_or(0, |value| value.body_start)..];
    let mut end = body.len().min(MAX_PREVIEW_BYTES);
    while !body.is_char_boundary(end) {
        end -= 1;
    }
    let prefix = &body[..end];
    if let Some((offset, _)) = prefix.match_indices('\n').nth(MAX_PREVIEW_LINES - 1) {
        end = offset + 1;
    }
    let mut lines = Vec::new();
    let total = body.lines().count() + usize::from(metadata.is_some());
    if let Some(metadata) = metadata {
        lines.push(MarkdownLine::FrontMatter(Box::new(metadata)));
    }
    let limit = body.floor_char_boundary(body.len().min(256 * 1024));
    if end < limit
        && body[..end].contains('$')
        && let Some(range) = math_ranges(&body[..limit])
            .into_iter()
            .find(|range| range.start < end && range.end > end)
    {
        end = body[range.end..limit]
            .find('\n')
            .map_or(range.end, |after| range.end + after + 1);
    }
    let mut prepared = parse_body(&body[..end]);
    if matches!(prepared.last(), Some(MarkdownLine::IncompleteMath { .. })) {
        let index = prepared.len() - 1;
        let expanded = parse_body(&body[..limit]);
        if matches!(expanded.get(index), Some(MarkdownLine::Paragraph { spans }) if matches!(spans.as_slice(), [MarkdownSpan::Math { display: true, .. }]))
        {
            prepared = expanded.into_iter().take(index + 1).collect();
        }
    }
    let mut open = None;
    for (index, line) in prepared.iter().enumerate() {
        if let MarkdownLine::CodeFence { language } = line {
            open = if open.is_some() {
                None
            } else {
                Some((index, language.as_str()))
            };
        }
    }
    if let Some((index, language)) = open
        && little_clip_core::diagram::Format::from_language(language).is_some()
    {
        // A visible diagram needs its complete source, not a misleading partial parse.
        let mut limit = body.len().min(256 * 1024);
        while !body.is_char_boundary(limit) {
            limit -= 1;
        }
        let expanded = parse_body(&body[..limit]);
        if let Some(last) = expanded
            .iter()
            .enumerate()
            .skip(index + 1)
            .find_map(|(end, line)| matches!(line, MarkdownLine::CodeFence { .. }).then_some(end))
        {
            prepared = expanded.into_iter().take(last + 1).collect();
        }
    }
    lines.extend(prepared);
    (lines, total)
}

// Delegate dollar escaping, code exclusion and delimiter matching to the existing
// CommonMark dependency, rather than maintaining a second equation lexer.
fn math_ranges(text: &str) -> Vec<std::ops::Range<usize>> {
    use pulldown_cmark::{Event, Options, Parser};
    Parser::new_ext(text, Options::ENABLE_MATH)
        .into_offset_iter()
        .filter_map(|(event, range)| {
            matches!(event, Event::InlineMath(_) | Event::DisplayMath(_)).then_some(range)
        })
        .collect()
}

#[derive(Default)]
struct SpanBuilder {
    spans: Vec<MarkdownSpan>,
    strong: usize,
    emphasis: usize,
    strikethrough: usize,
    url: Option<String>,
    image: Option<(String, String)>,
    html_styles: Vec<(String, InlineCss, bool)>,
    html_links: Vec<String>,
    suppressed_html_depth: usize,
}

impl SpanBuilder {
    fn active_url(&self) -> Option<&str> {
        self.html_links
            .last()
            .map(String::as_str)
            .or(self.url.as_deref())
    }

    fn is_suppressed(&self) -> bool {
        self.suppressed_html_depth > 0 || self.html_styles.iter().any(|(_, _, hidden)| *hidden)
    }

    fn effective_css(&self) -> InlineCss {
        let mut css = InlineCss {
            bold: self.strong > 0,
            italic: self.emphasis > 0,
            strikethrough: self.strikethrough > 0,
            ..Default::default()
        };
        for (_, layer, _) in &self.html_styles {
            css = css.merge(*layer);
        }
        css
    }

    fn push_inline_html(&mut self, raw: &str) {
        match html::parse_inline_token(raw) {
            html::InlineHtmlToken::Ignore => {}
            html::InlineHtmlToken::HardBreak => {
                if !self.is_suppressed() {
                    self.spans.push(MarkdownSpan::HardBreak);
                }
            }
            html::InlineHtmlToken::PushLink(href) => self.html_links.push(href),
            html::InlineHtmlToken::PopLink => {
                self.html_links.pop();
            }
            html::InlineHtmlToken::PushSuppressed => {
                self.suppressed_html_depth += 1;
            }
            html::InlineHtmlToken::PopSuppressed => {
                self.suppressed_html_depth = self.suppressed_html_depth.saturating_sub(1);
            }
            html::InlineHtmlToken::Image {
                alt,
                src,
                style,
                hidden,
            } => {
                if !self.is_suppressed() && !hidden && !src.is_empty() {
                    self.spans.push(MarkdownSpan::StyledImage {
                        alt,
                        url: src,
                        href: self.active_url().map(str::to_owned),
                        style,
                    });
                }
            }
            html::InlineHtmlToken::OpenStyle { tag, css, hidden } => {
                self.html_styles.push((tag, css, hidden));
            }
            html::InlineHtmlToken::CloseStyle { tag } => {
                if let Some(pos) = self
                    .html_styles
                    .iter()
                    .rposition(|(open, _, _)| *open == tag)
                {
                    self.html_styles.truncate(pos);
                }
            }
        }
    }

    fn push_event<'a>(
        &mut self,
        event: pulldown_cmark::Event<'a>,
        prepared: Option<&hugo::PreparedShortcodes>,
    ) {
        use pulldown_cmark::{Event, Tag, TagEnd};
        if let Some((_, alt)) = &mut self.image {
            match event {
                Event::End(TagEnd::Image) => {
                    let (url, alt) = self.image.take().expect("image buffer present");
                    if !self.is_suppressed() {
                        if let Some(href) = self.active_url() {
                            self.spans.push(MarkdownSpan::StyledImage {
                                alt,
                                url,
                                href: Some(href.to_owned()),
                                style: ImagePresentation::default(),
                            });
                        } else {
                            self.spans.push(MarkdownSpan::Image { alt, url });
                        }
                    }
                }
                Event::Text(v) | Event::Code(v) | Event::InlineHtml(v) | Event::Html(v) => {
                    if let Some(p) = prepared {
                        alt.push_str(&p.expand_plain_alt(&v));
                    } else {
                        alt.push_str(&v);
                    }
                }
                Event::InlineMath(v) | Event::DisplayMath(v) => alt.push_str(&v),
                Event::SoftBreak | Event::HardBreak => alt.push(' '),
                _ => {}
            }
            return;
        }

        match event {
            Event::Start(Tag::Strong) => self.strong += 1,
            Event::End(TagEnd::Strong) => self.strong = self.strong.saturating_sub(1),
            Event::Start(Tag::Emphasis) => self.emphasis += 1,
            Event::End(TagEnd::Emphasis) => self.emphasis = self.emphasis.saturating_sub(1),
            Event::Start(Tag::Strikethrough) => self.strikethrough += 1,
            Event::End(TagEnd::Strikethrough) => {
                self.strikethrough = self.strikethrough.saturating_sub(1);
            }
            Event::Start(Tag::Link { dest_url, .. }) => self.url = Some(dest_url.into_string()),
            Event::End(TagEnd::Link) => self.url = None,
            Event::Start(Tag::Image { dest_url, .. }) => {
                self.image = Some((dest_url.into_string(), String::new()));
            }
            Event::InlineMath(source) if !self.is_suppressed() => {
                self.spans.push(MarkdownSpan::Math {
                    source: source.into_string(),
                    display: false,
                    url: self.active_url().map(str::to_owned),
                });
            }
            Event::DisplayMath(source) if !self.is_suppressed() => {
                self.spans.push(MarkdownSpan::Math {
                    source: source.into_string(),
                    display: true,
                    url: self.active_url().map(str::to_owned),
                });
            }
            Event::Code(source) => {
                if !self.is_suppressed() {
                    let mut css = self.effective_css();
                    css.monospace = true;
                    let url = self.active_url().map(str::to_owned);
                    html::push_styled_text(
                        &mut self.spans,
                        source.into_string(),
                        url.as_deref(),
                        css,
                    );
                }
            }
            Event::FootnoteReference(name) if !self.is_suppressed() => {
                push_span(&mut self.spans, MarkdownSpan::Text(format!("[^{name}]")));
            }
            Event::InlineHtml(value) => self.push_inline_html(&value),
            Event::Html(value) => self.push_inline_html(&value),
            Event::Text(value) => {
                if self.is_suppressed() {
                    return;
                }
                let css = self.effective_css();
                let url = self.active_url().map(str::to_owned);
                if let Some(p) = prepared {
                    p.expand_inline_text(&value, url.as_deref(), css, &mut self.spans);
                } else {
                    html::push_styled_text(
                        &mut self.spans,
                        value.into_string(),
                        url.as_deref(),
                        css,
                    );
                }
            }
            Event::SoftBreak => {
                if !self.is_suppressed() {
                    let css = self.effective_css();
                    let url = self.active_url().map(str::to_owned);
                    html::push_styled_text(&mut self.spans, " ".to_owned(), url.as_deref(), css);
                }
            }
            Event::HardBreak if !self.is_suppressed() => {
                self.spans.push(MarkdownSpan::HardBreak);
            }
            _ => {}
        }
    }

    fn finish(self) -> Vec<MarkdownSpan> {
        self.spans
    }
}

/// Parses inline formatting and native math with CommonMark delimiter semantics.
pub fn parse_markdown_spans(text: &str) -> Vec<MarkdownSpan> {
    parse_markdown_spans_at_depth(text, 0)
}

pub(super) fn parse_markdown_spans_at_depth(text: &str, depth: usize) -> Vec<MarkdownSpan> {
    let prepared = hugo::prepare_shortcodes(text, depth);
    let effective = prepared.as_ref().map_or(text, |p| p.rewritten.as_str());
    let mut builder = SpanBuilder::default();
    for event in pulldown_cmark::Parser::new_ext(effective, gfm_options()) {
        builder.push_event(event, prepared.as_ref());
    }
    builder.finish()
}

fn push_span(spans: &mut Vec<MarkdownSpan>, span: MarkdownSpan) {
    // Entities and escaped punctuation arrive as separate native text events.
    // Coalesce equal styles so those scanner details do not create layout gaps.
    let target = match (spans.last_mut(), &span) {
        (Some(MarkdownSpan::Text(old)), MarkdownSpan::Text(value))
        | (Some(MarkdownSpan::Bold(old)), MarkdownSpan::Bold(value))
        | (Some(MarkdownSpan::Italic(old)), MarkdownSpan::Italic(value))
        | (Some(MarkdownSpan::BoldItalic(old)), MarkdownSpan::BoldItalic(value))
        | (Some(MarkdownSpan::Strikethrough(old)), MarkdownSpan::Strikethrough(value)) => {
            Some((old, value))
        }
        (
            Some(MarkdownSpan::Styled {
                text: old,
                url: old_url,
                css: old_css,
            }),
            MarkdownSpan::Styled { text, url, css },
        ) if old_url == url && old_css == css => Some((old, text)),
        (
            Some(MarkdownSpan::Link {
                label: old,
                url: old_url,
            }),
            MarkdownSpan::Link { label, url },
        ) if old_url == url => Some((old, label)),
        _ => None,
    };
    if let Some((old, value)) = target {
        old.push_str(value);
    } else {
        spans.push(span);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn located_elements_preserve_original_utf8_frontmatter_fences_and_multiline_math() {
        let source = "\u{feff}---\r\ntitle: Café\r\n---\r\n# Résumé\r\n\r\n```mermaid\r\nflowchart LR\r\n A --> B\r\n```\r\n\r\n$$\r\nx^2 + y^2\r\n$$ tail\r\n";
        let located = parse_located(source);
        assert_eq!(
            located
                .iter()
                .map(|item| item.line.clone())
                .collect::<Vec<_>>(),
            parse_markdown(source)
        );
        assert_eq!(
            &source[located[0].source.clone()],
            "\u{feff}---\r\ntitle: Café\r\n---\r\n"
        );
        assert!(located.iter().any(
            |item| matches!(&item.line, MarkdownLine::Heading { text, .. } if text == "Résumé")
                && &source[item.source.clone()] == "# Résumé\r\n"
        ));
        assert!(located.iter().any(
            |item| matches!(&item.line, MarkdownLine::CodeLine { line } if line == " A --> B")
                && &source[item.source.clone()] == " A --> B\r\n"
        ));
        assert!(located.iter().any(|item| matches!(&item.line, MarkdownLine::Paragraph { spans } if matches!(spans.first(), Some(MarkdownSpan::Math {display: true, ..})))
            && &source[item.source.clone()] == "$$\r\nx^2 + y^2\r\n$$ tail\r\n"));
        for item in located {
            assert!(source.is_char_boundary(item.source.start));
            assert!(source.is_char_boundary(item.source.end));
        }
    }

    #[test]
    fn located_incomplete_equation_retains_the_entire_unfinished_source_range() {
        let source = "Intro\n\n$$\nx + y\n";
        let located = parse_located(source);
        let last = located.last().unwrap();
        assert!(matches!(last.line, MarkdownLine::IncompleteMath { .. }));
        assert_eq!(&source[last.source.clone()], "$$\nx + y\n");
    }

    #[test]
    fn display_blocks_ignore_inner_markdown_markers_but_not_code_fences() {
        let lines = parse_markdown("$$\n+ x\n---\n```\n# x\n$$\nTail");
        assert!(
            matches!(&lines[..], [MarkdownLine::Paragraph { spans }, MarkdownLine::Paragraph { .. }] if matches!(&spans[..], [MarkdownSpan::Math { source, display: true, .. }] if source.contains("+ x\n---\n```\n# x")))
        );
        let lines = parse_markdown("```text\n$$\n+ x\n$$\n```");
        assert_eq!(lines.len(), 5);
        assert!(matches!(&lines[1], MarkdownLine::CodeLine { line } if line == "$$"));
        let lines = parse_markdown("$$x + \\$$\ny$$ tail");
        assert!(
            matches!(&lines[0], MarkdownLine::Paragraph { spans } if matches!(&spans[..], [MarkdownSpan::Math { source, .. }] if source.contains("\\$$\ny")))
        );
        assert!(
            matches!(&lines[1], MarkdownLine::Paragraph { spans } if spans == &[MarkdownSpan::Text("tail".into())])
        );
        assert!(matches!(
            parse_markdown("$$\n1+2").last(),
            Some(MarkdownLine::IncompleteMath { .. })
        ));
    }

    #[test]
    fn math_respects_escapes_code_links_and_surrounding_emphasis() {
        assert_eq!(
            parse_markdown_spans(r"\$5 and \$10"),
            vec![MarkdownSpan::Text("$5 and $10".into())]
        );
        assert_eq!(
            parse_markdown_spans(r"**mass $m$ end**"),
            vec![
                MarkdownSpan::Bold("mass ".into()),
                MarkdownSpan::Math {
                    source: "m".into(),
                    display: false,
                    url: None
                },
                MarkdownSpan::Bold(" end".into()),
            ]
        );
        assert_eq!(
            parse_markdown_spans(r"`$x$` [$\alpha$](https://example.com)"),
            vec![
                MarkdownSpan::Code("$x$".into()),
                MarkdownSpan::Text(" ".into()),
                MarkdownSpan::Math {
                    source: r"\alpha".into(),
                    display: false,
                    url: Some("https://example.com".into())
                },
            ]
        );
        assert_eq!(
            parse_markdown_spans(r"value $x_1^2$"),
            vec![
                MarkdownSpan::Text("value ".into()),
                MarkdownSpan::Math {
                    source: "x_1^2".into(),
                    display: false,
                    url: None
                }
            ]
        );
    }

    #[test]
    fn multiline_math_is_one_block_and_compact_source_is_never_truncated() {
        let source = "# Heading\n$$\n\\frac{1}{2}\n+ x\n$$\nAfter";
        let lines = parse_markdown(source);
        assert_eq!(lines.len(), 3);
        assert!(
            matches!(&lines[1], MarkdownLine::Paragraph { spans } if matches!(&spans[..], [MarkdownSpan::Math { source, display: true, .. }] if source.contains("\\frac{1}{2}\n+ x")))
        );
        let source = format!("$$\\text{{{}}}$$\nAfter", "x".repeat(6000));
        let (compact, total) = parse_markdown_preview(&source);
        assert_eq!(total, 2);
        assert!(
            matches!(&compact[0], MarkdownLine::Paragraph { spans } if matches!(&spans[..], [MarkdownSpan::Math { source, .. }] if source.len() > 6000))
        );
        let source = format!("$$\\text{{{}}}$$", "x".repeat(300_000));
        let (compact, _) = parse_markdown_preview(&source);
        assert!(matches!(&compact[0], MarkdownLine::IncompleteMath { .. }));
    }

    #[test]
    fn diagram_fences_obey_marker_lengths_and_keep_complete_screen_source() {
        let input = "~~~~pikchr\nbox \"``` is text\"\n~~~\ncircle\n~~~~\n# Body";
        let lines = parse_markdown(input);
        assert!(matches!(&lines[2], MarkdownLine::CodeLine { line } if line == "~~~"));
        assert!(matches!(&lines[4], MarkdownLine::CodeFence { language } if language.is_empty()));
        assert!(matches!(&lines[5], MarkdownLine::Heading { text, .. } if text == "Body"));
        let source = format!("```pikchr\n# {}\nbox\n```\n", "x".repeat(6000));
        let (compact, _) = parse_markdown_preview(&source);
        assert!(
            matches!(compact.last(), Some(MarkdownLine::CodeFence { language }) if language.is_empty())
        );
        assert!(
            compact
                .iter()
                .any(|line| matches!(line, MarkdownLine::CodeLine { line } if line == "box"))
        );
    }

    #[test]
    fn metadata_is_shared_without_parsing_truncated_front_matter() {
        let input = format!(
            "---\ntitle: Example\nnotes: '{}'\n---\n# Body",
            "x".repeat(5000)
        );
        let full = parse_markdown(&input);
        let (compact, total) = parse_markdown_preview(&input);
        assert_eq!(full, compact);
        assert_eq!(total, 2);
        assert!(matches!(&compact[0], MarkdownLine::FrontMatter(value) if value.value.is_ok()));
        assert!(matches!(&compact[1], MarkdownLine::Heading { text, .. } if text == "Body"));
        let malformed = parse_markdown("+++\nnot closed\n# Body");
        assert!(matches!(&malformed[0], MarkdownLine::FrontMatter(value) if value.value.is_err()));
        assert!(
            malformed
                .iter()
                .any(|line| matches!(line, MarkdownLine::Heading { text, .. } if text == "Body"))
        );
    }

    #[test]
    fn preview_preserves_prefix_fence_state_and_counts_unparsed_lines() {
        let prefix = "# Title\n\n```rust\nlet n = 1;\n```\n";
        let input = format!("{prefix}{}", "more **text**\n".repeat(100_000));
        let (lines, total) = parse_markdown_preview(&input);
        assert_eq!(&lines[..5], parse_markdown(prefix).as_slice());
        assert!(lines.len() <= 64);
        assert_eq!(total, 100_005);
    }

    #[test]
    fn preview_bounds_utf8_single_line_parsing_without_changing_the_source() {
        let input = "日本語😀".repeat(100_000);
        let (lines, total) = parse_markdown_preview(&input);
        assert_eq!(total, 1);
        assert_eq!(lines.len(), 1);
        assert!(format!("{lines:?}").len() < 5000);
        assert_eq!(input.chars().count(), 400_000);
    }

    #[test]
    fn test_parse_markdown_spans() {
        let sample =
            "This has **bold text**, *italic*, `code()` and [Docs](https://example.com) end";
        let spans = parse_markdown_spans(sample);
        assert_eq!(
            spans,
            vec![
                MarkdownSpan::Text("This has ".to_string()),
                MarkdownSpan::Bold("bold text".to_string()),
                MarkdownSpan::Text(", ".to_string()),
                MarkdownSpan::Italic("italic".to_string()),
                MarkdownSpan::Text(", ".to_string()),
                MarkdownSpan::Code("code()".to_string()),
                MarkdownSpan::Text(" and ".to_string()),
                MarkdownSpan::Link {
                    label: "Docs".to_string(),
                    url: "https://example.com".to_string()
                },
                MarkdownSpan::Text(" end".to_string()),
            ]
        );
    }

    #[test]
    fn test_parse_markdown_spans_with_multibyte_utf8() {
        let sample = "Prefix — café: **太字**, *斜体*, ***両方***, `コード()` & [リンク🌐](https://example.com/path?q=あ) suffix";
        let spans = parse_markdown_spans(sample);
        assert_eq!(
            spans,
            vec![
                MarkdownSpan::Text("Prefix — café: ".to_string()),
                MarkdownSpan::Bold("太字".to_string()),
                MarkdownSpan::Text(", ".to_string()),
                MarkdownSpan::Italic("斜体".to_string()),
                MarkdownSpan::Text(", ".to_string()),
                MarkdownSpan::BoldItalic("両方".to_string()),
                MarkdownSpan::Text(", ".to_string()),
                MarkdownSpan::Code("コード()".to_string()),
                MarkdownSpan::Text(" & ".to_string()),
                MarkdownSpan::Link {
                    label: "リンク🌐".to_string(),
                    url: "https://example.com/path?q=あ".to_string(),
                },
                MarkdownSpan::Text(" suffix".to_string()),
            ]
        );
    }

    #[test]
    fn test_parse_markdown_lines() {
        let doc = "# Main Heading\n\n- First bullet\n- Second with **bold**\n\n> A famous quote\n\n1. Step one\n\n```rust\nfn main() {}\n```";
        let lines = parse_markdown(doc);
        assert!(matches!(lines[0], MarkdownLine::Heading { level: 1, .. }));
        assert_eq!(lines[1], MarkdownLine::Empty);
        assert!(matches!(lines[2], MarkdownLine::Bullet { .. }));
        assert!(matches!(lines[3], MarkdownLine::Bullet { .. }));
        assert_eq!(lines[4], MarkdownLine::Empty);
        assert!(matches!(lines[5], MarkdownLine::Quote { .. }));
        assert_eq!(lines[6], MarkdownLine::Empty);
        assert!(matches!(lines[7], MarkdownLine::Numbered { .. }));
        assert_eq!(lines[8], MarkdownLine::Empty);
        assert!(matches!(lines[9], MarkdownLine::CodeFence { .. }));
        assert!(matches!(lines[10], MarkdownLine::CodeLine { .. }));
        assert!(matches!(lines[11], MarkdownLine::CodeFence { .. }));
    }

    #[test]
    fn gfm_tables_parse_in_both_line_and_block_models_with_alignments_and_spans() {
        let doc = "| Name | Count | Notes |\n| :--- | :---: | ---: |\n| **Alpha** | `42` | [Docs](https://example.com) |\n| Beta | ~~9~~ | $x^2$ |\n";
        let blocks = parse_blocks(doc);
        assert_eq!(blocks.len(), 1);
        let MarkdownDocBlock::Table(table) = &blocks[0] else {
            panic!("expected Table block: {blocks:?}");
        };
        assert_eq!(
            table.alignments,
            vec![
                TableAlignment::Left,
                TableAlignment::Center,
                TableAlignment::Right
            ]
        );
        assert_eq!(table.headers.len(), 3);
        assert_eq!(table.rows.len(), 2);
        assert_eq!(table.rows[0][0], vec![MarkdownSpan::Bold("Alpha".into())]);
        assert_eq!(table.rows[0][1], vec![MarkdownSpan::Code("42".into())]);
        assert_eq!(
            table.rows[1][1],
            vec![MarkdownSpan::Strikethrough("9".into())]
        );

        let lines = parse_markdown(doc);
        assert_eq!(lines.len(), 1);
        assert!(matches!(&lines[0], MarkdownLine::Table(line_table) if line_table == table));

        let located = parse_located_blocks(doc);
        assert_eq!(located.len(), 1);
        assert_eq!(&doc[located[0].source.clone()], doc);
    }

    #[test]
    fn document_blocks_reflow_paragraphs_group_quotes_nest_lists_and_preserve_math() {
        let doc = concat!(
            "First wrapped line\nsecond wrapped line  \nhard tail.\n\n",
            "> Quote line one\n> Quote line two\n\n",
            "- [x] Done task\n  - Nested bullet\n- [ ] Open task\n\n",
            "##### Level Five\n\n",
            "$$\n+ x\n---\n```\n# x\n$$\n\n",
            "![Diagram](chart.png)\n",
        );
        let blocks = parse_blocks(doc);
        assert!(matches!(
            &blocks[0],
            MarkdownDocBlock::Paragraph { spans }
                if spans.contains(&MarkdownSpan::Text("First wrapped line second wrapped line".into()))
                    && spans.contains(&MarkdownSpan::HardBreak)
        ));
        assert!(matches!(
            &blocks[1],
            MarkdownDocBlock::Quote { kind: None, blocks } if blocks.len() == 1
        ));
        let MarkdownDocBlock::List {
            ordered: false,
            items,
            ..
        } = &blocks[2]
        else {
            panic!("expected bullet list: {blocks:?}");
        };
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].task, Some(true));
        assert_eq!(items[1].task, Some(false));
        assert!(
            items[0]
                .blocks
                .iter()
                .any(|b| matches!(b, MarkdownDocBlock::List { .. }))
        );
        assert!(matches!(
            &blocks[3],
            MarkdownDocBlock::Heading { level: 5, text, .. } if text == "Level Five"
        ));
        assert!(matches!(
            &blocks[4],
            MarkdownDocBlock::Paragraph { spans }
                if matches!(spans.as_slice(), [MarkdownSpan::Math { display: true, .. }])
        ));
        assert!(matches!(
            &blocks[5],
            MarkdownDocBlock::Paragraph { spans }
                if matches!(spans.as_slice(), [MarkdownSpan::Image { alt, url }] if alt == "Diagram" && url == "chart.png")
        ));
    }

    #[test]
    fn located_blocks_flag_incomplete_fences_and_math_and_coalesce_lines() {
        let incomplete_fence = "# Title\n\n```mermaid\nflowchart LR\n";
        let blocks = parse_located_blocks(incomplete_fence);
        assert!(blocks.iter().any(|b| b.incomplete_fence));

        let incomplete_math = "Intro\n\n$$\nx + y\n";
        let blocks = parse_located_blocks(incomplete_math);
        assert!(matches!(
            blocks.last().map(|b| &b.block),
            Some(MarkdownDocBlock::IncompleteMath { .. })
        ));

        let lines = parse_markdown("Line one\nLine two\n\n> Q1\n> Q2\n");
        let coalesced = blocks_from_lines(lines);
        assert_eq!(coalesced.len(), 2);
        assert!(matches!(&coalesced[0], MarkdownDocBlock::Paragraph { .. }));
        assert!(
            matches!(&coalesced[1], MarkdownDocBlock::Quote { blocks, .. } if blocks.len() == 1)
        );
    }

    #[test]
    fn inline_and_block_html_with_css_tables_details_and_images_parse_safely() {
        let doc = concat!(
            "<!-- ignored comment -->\n\n",
            "Inline <span style='color: red; text-decoration: underline'>alert</span> ",
            "<kbd>Ctrl</kbd> H<sub>2</sub>O line<br>next.\n\n",
            "<details>\n<summary>Click to expand</summary>\n\n- nested bullet\n\n</details>\n\n",
            "<div style='color: #336699; background: rgba(255, 255, 255, 0.05); border: 1px solid #888; border-radius: 8px; padding: 12px 16px;'>\n\n",
            "Boxed markdown paragraph.\n\n</div>\n\n",
            "<img src=\"/nonfree/interviews/PaulShapera.jpg\" alt=\"Paul Shapera\" style=\"float:right;height:12em;border-radius:25px;\">\n\n",
            "<table><thead><tr><th align=\"center\">Name</th><th>Value</th></tr></thead><tbody><tr><td><b>Alpha</b></td><td><code>42</code></td></tr></tbody></table>\n\n",
            "<div hidden><img src=\"/hidden.png\" alt=\"secret\"></div>\n\n",
            "<script>alert(1)</script><style>.x{color:red}</style>\n",
        );

        let blocks = parse_blocks(doc);
        assert!(
            matches!(
                &blocks[0],
                MarkdownDocBlock::Paragraph { spans }
                    if spans.iter().any(|s| matches!(
                        s,
                        MarkdownSpan::Styled { text, css, .. }
                            if text == "alert" && css.color == Some(RgbaColor { r: 220, g: 40, b: 40, a: 255 }) && css.underline
                    )) && spans.iter().any(|s| matches!(
                        s,
                        MarkdownSpan::Styled { text, css, .. } if text == "Ctrl" && css.kbd && css.monospace
                    )) && spans.iter().any(|s| matches!(
                        s,
                        MarkdownSpan::Styled { text, css, .. } if text == "2" && css.subscript
                    )) && spans.contains(&MarkdownSpan::HardBreak)
            ),
            "inline HTML/CSS spans failed: {blocks:#?}"
        );
        assert!(
            matches!(
                &blocks[1],
                MarkdownDocBlock::Details { summary, open: false, blocks: inner }
                    if plain_text_from_spans(summary) == "Click to expand"
                        && inner.iter().any(|b| matches!(b, MarkdownDocBlock::List { .. }))
            ),
            "details block failed: {blocks:#?}"
        );
        assert!(
            matches!(
                &blocks[2],
                MarkdownDocBlock::StyledContainer { style, blocks: inner }
                    if style.border_width == 1.0
                        && style.border_radius == 8.0
                        && style.padding == [12.0, 16.0, 12.0, 16.0]
                        && style.text_color == Some(RgbaColor { r: 0x33, g: 0x66, b: 0x99, a: 255 })
                        && !inner.is_empty()
            ),
            "styled container failed: {blocks:#?}"
        );
        assert!(
            matches!(
                &blocks[3],
                MarkdownDocBlock::Paragraph { spans }
                    if matches!(
                        spans.as_slice(),
                        [MarkdownSpan::StyledImage { alt, url, style, .. }]
                            if alt == "Paul Shapera"
                                && url == "/nonfree/interviews/PaulShapera.jpg"
                                && style.float == Some(TextAlign::Right)
                                && style.height_px == Some(192.0)
                                && style.border_radius == Some(25.0)
                    )
            ),
            "styled image failed: {blocks:#?}"
        );
        assert!(
            matches!(
                &blocks[4],
                MarkdownDocBlock::Table(table)
                    if table.alignments == vec![TableAlignment::Center, TableAlignment::None]
                        && table.headers.len() == 2
                        && table.rows.len() == 1
                        && table.rows[0][0] == vec![MarkdownSpan::Bold("Alpha".into())]
                        && table.rows[0][1] == vec![MarkdownSpan::Code("42".into())]
            ),
            "HTML table failed: {blocks:#?}"
        );
        assert_eq!(
            blocks.len(),
            5,
            "hidden/comment/script blocks must be omitted: {blocks:#?}"
        );
    }

    #[test]
    fn hugo_shortcodes_parse_blocks_inline_nesting_and_source_ranges() {
        let md = "\
Intro with {{< hl rust >}}let x: Vec<i32> = vec![1, 2];{{< /hl >}} and inline {{< katex >}}\\LaTeX{{< /katex >}} and {{< latex >}}E = mc^2{{< /latex >}}.

{{< katex display >}}

\\begin{aligned}
a &= b + c \\\\
d &= e
\\end{aligned}

{{< /katex >}}

{{< mermaid align=\"center\" >}}

sequenceDiagram
    Alice->>John: Hi

{{< /mermaid >}}

{{< code lang=\"python\" title=\"fib.py\" lines=\"20\" compact=\"true\" numbers=\"false\" >}}
def fib(n):
    return n
{{< /code >}}

- Bullet item:

    {{< code title=\"demo.c\" numbers=\"false\" >}}
int main(void) { return 0; }
    {{< /code >}}

{{< details \"Here for more?\" open >}}

Nested list:
* Item inside

{{< code lang=\"rust\" >}}
fn nested() {}
{{< /code >}}

{{< /details >}}

| Syntax | Sample |
| --- | --- |
| Pipe safe | {{< hl rust >}}a || b{{< /hl >}} |

Literal `{{< code >}}raw{{< /code >}}` and unknown {{< custom_widget >}}stay{{< /custom_widget >}}.

Trailing paragraph.
";

        let located = parse_located_blocks(md);
        let blocks: Vec<&MarkdownDocBlock> = located.iter().map(|b| &b.block).collect();

        // Paragraph 0: inline hl (with <i32> intact), inline katex, inline latex.
        assert!(
            matches!(
                blocks[0],
                MarkdownDocBlock::Paragraph { spans }
                    if spans.iter().any(|s| matches!(
                        s,
                        MarkdownSpan::HighlightedCode { language, code }
                            if language == "rust" && code == "let x: Vec<i32> = vec![1, 2];"
                    )) && spans.iter().any(|s| matches!(
                        s,
                        MarkdownSpan::Math { source, display: false, .. } if source == "\\LaTeX"
                    )) && spans.iter().any(|s| matches!(
                        s,
                        MarkdownSpan::Math { source, display: false, .. } if source == "E = mc^2"
                    ))
            ),
            "inline hl/katex/latex paragraph failed: {blocks:#?}"
        );

        // Block 1: multi-line katex with internal blank lines collapsed cleanly.
        assert!(
            matches!(
                blocks[1],
                MarkdownDocBlock::Paragraph { spans }
                    if matches!(
                        spans.as_slice(),
                        [MarkdownSpan::Math { source, display: true, .. }]
                            if source.contains("\\begin{aligned}") && source.contains("\\end{aligned}")
                    )
            ),
            "display katex block failed: {blocks:#?}"
        );

        // Block 2: mermaid shortcode becomes native mermaid Code block.
        assert!(
            matches!(
                blocks[2],
                MarkdownDocBlock::Code { language, text }
                    if language == "mermaid" && text.starts_with("sequenceDiagram") && text.contains("Alice->>John: Hi")
            ),
            "mermaid shortcode failed: {blocks:#?}"
        );

        // Block 3: {{< code >}} preserves indentation and strips template options.
        assert!(
            matches!(
                blocks[3],
                MarkdownDocBlock::Code { language, text }
                    if language == "python" && text == "def fib(n):\n    return n"
            ),
            "code shortcode failed: {blocks:#?}"
        );

        // Block 4: list item with indented {{< code title="demo.c" >}} infers C language cleanly inside Item.
        assert!(
            matches!(
                blocks[4],
                MarkdownDocBlock::List { items, .. }
                    if items[0].blocks.iter().any(|b| matches!(
                        b,
                        MarkdownDocBlock::Code { language, text }
                            if language == "c" && text == "int main(void) { return 0; }"
                    ))
            ),
            "indented code shortcode inside bullet failed: {blocks:#?}"
        );

        // Block 5: {{< details "Here for more?" open >}} wraps nested list + code block.
        assert!(
            matches!(
                blocks[5],
                MarkdownDocBlock::Details { summary, open: true, blocks: inner }
                    if plain_text_from_spans(summary) == "Here for more?"
                        && inner.iter().any(|b| matches!(b, MarkdownDocBlock::List { .. }))
                        && inner.iter().any(|b| matches!(
                            b,
                            MarkdownDocBlock::Code { language, text }
                                if language == "rust" && text == "fn nested() {}"
                        ))
            ),
            "details shortcode failed: {blocks:#?}"
        );

        // Block 6: pipe inside {{< hl rust >}}a || b{{< /hl >}} stays inside table cell.
        assert!(
            matches!(
                blocks[6],
                MarkdownDocBlock::Table(table)
                    if table.rows.len() == 1
                        && table.rows[0].len() == 2
                        && table.rows[0][1].iter().any(|s| matches!(
                            s,
                            MarkdownSpan::HighlightedCode { language, code }
                                if language == "rust" && code == "a || b"
                        ))
            ),
            "table inline hl with pipe failed: {blocks:#?}"
        );

        // Block 7: backtick-protected shortcode and unknown shortcode remain untouched.
        assert!(
            matches!(
                blocks[7],
                MarkdownDocBlock::Paragraph { spans }
                    if spans.iter().any(|s| matches!(s, MarkdownSpan::Code(c) if c == "{{< code >}}raw{{< /code >}}"))
                        && plain_text_from_spans(spans).contains("{{< custom_widget >}}")
            ),
            "backtick/unknown preservation failed: {blocks:#?}"
        );

        // Block 8: Trailing paragraph preserves EXACT UTF-8 source byte range!
        let trailing = located.last().expect("trailing block");
        assert!(
            matches!(&trailing.block, MarkdownDocBlock::Paragraph { spans } if plain_text_from_spans(spans) == "Trailing paragraph."),
            "trailing block mismatch: {trailing:#?}"
        );
        assert_eq!(md[trailing.source.clone()].trim(), "Trailing paragraph.");

        // Line-oriented card parser also handles block & inline shortcodes.
        let card_lines = parse_markdown(md);
        assert!(
            card_lines.iter().any(|line| matches!(
                line,
                MarkdownLine::CodeFence { language } if language == "mermaid"
            )),
            "card parser should recognize {{< mermaid >}}"
        );
        assert!(
            card_lines.iter().any(|line| matches!(
                line,
                MarkdownLine::CodeFence { language } if language == "python"
            )),
            "card parser should recognize {{< code >}}"
        );
    }

    #[test]
    fn opguides_custom_shortcodes_parse_spoilers_tabs_columns_quotes_hints_and_captions() {
        let md = "\
Before {{< spoiler >}}hidden **secret**{{< /spoiler >}} and {{< smalltext >}}inline note{{< /smalltext >}}.

{{< attribution >}}Art by [Author](https://example.com){{< /attribution >}}

{{< hint info >}}

Info note body.

{{< /hint >}}

{{< hint danger >}}

Danger alert body.

{{< /hint >}}

{{< quote \"David on *[Podcast](https://example.com)*\" >}}

Something worth quoting.

{{< /quote >}}

{{< bigquote \"Source\" >}}

Something huge.

{{< /bigquote >}}

{{< tabs \"Tools\" >}}

{{< tab \"Linux\" >}}
Use JACK or PipeWire.
{{< /tab >}}

{{< tab \"Windows\" >}}
Use ASIO.
{{< /tab >}}

{{< /tabs >}}

{{< columns3 >}}

Column one

<--->

{{< code lang=\"rust\" >}}
// <---> inside code must not split columns
fn a() {}
{{< /code >}}

<--->

{{< attribution >}}Third column caption{{< /attribution >}}

{{< /columns3 >}}

{{< columns2 >}}

A

<--->

B

{{< /columns2 >}}
";

        let blocks = parse_blocks(md);

        assert!(
            matches!(
                &blocks[0],
                MarkdownDocBlock::Paragraph { spans }
                    if spans.iter().any(|s| matches!(
                        s,
                        MarkdownSpan::Spoiler { spans: inner }
                            if plain_text_from_spans(inner) == "hidden secret"
                                && inner.iter().any(|x| matches!(x, MarkdownSpan::Bold(b) if b == "secret"))
                    )) && spans.iter().any(|s| matches!(
                        s,
                        MarkdownSpan::Styled { text, css, .. } if text == "inline note" && css.small
                    ))
            ),
            "spoiler/smalltext failed: {blocks:#?}"
        );

        assert!(
            matches!(
                &blocks[1],
                MarkdownDocBlock::StyledContainer { style, blocks: inner }
                    if style.margin_bottom > 0.0
                        && inner.iter().any(|b| matches!(
                            b,
                            MarkdownDocBlock::Paragraph { spans }
                                if spans.iter().any(|s| matches!(
                                    s,
                                    MarkdownSpan::Styled { text, url: Some(u), css }
                                        if text == "Author" && u == "https://example.com" && css.small
                                ))
                        ))
            ),
            "attribution block failed: {blocks:#?}"
        );

        assert!(matches!(
            &blocks[2],
            MarkdownDocBlock::Quote {
                kind: Some(QuoteKind::Note),
                ..
            }
        ));
        assert!(matches!(
            &blocks[3],
            MarkdownDocBlock::Quote {
                kind: Some(QuoteKind::Caution),
                ..
            }
        ));

        assert!(
            matches!(
                &blocks[4],
                MarkdownDocBlock::PullQuote { big: false, source, .. }
                    if source.iter().any(|s| matches!(
                        s,
                        MarkdownSpan::Styled { text, url: Some(_), css }
                            if text == "Podcast" && css.italic
                    ))
            ),
            "quote shortcode failed: {blocks:#?}"
        );
        assert!(
            matches!(
                &blocks[5],
                MarkdownDocBlock::PullQuote { big: true, source, .. }
                    if plain_text_from_spans(source) == "Source"
            ),
            "bigquote shortcode failed: {blocks:#?}"
        );

        assert!(
            matches!(
                &blocks[6],
                MarkdownDocBlock::Tabs { tabs }
                    if tabs.len() == 2 && tabs[0].title == "Linux" && tabs[1].title == "Windows"
            ),
            "tabs/tab shortcode failed: {blocks:#?}"
        );

        assert!(
            matches!(
                &blocks[7],
                MarkdownDocBlock::Columns { dividers: true, bottom_rule: false, columns }
                    if columns.len() == 3
                        && matches!(&columns[1][0], MarkdownDocBlock::Code { language, .. } if language == "rust")
            ),
            "columns3 shortcode failed: {blocks:#?}"
        );

        assert!(
            matches!(
                &blocks[8],
                MarkdownDocBlock::Columns { dividers: false, bottom_rule: true, columns }
                    if columns.len() == 2
            ),
            "columns2 shortcode failed: {blocks:#?}"
        );
    }

    #[test]
    fn hugo_shortcodes_handle_multibyte_utf8_at_every_scan_boundary() {
        let long_em_dash_tag = format!("{{{{< {}not_closed", "—".repeat(700));
        let input = format!(
            "Escaped \\— char and `inline → café` before {{{{< quote \"David — *[Pod]*\" >}}}}\n\
Inside — body with `code → span` and \\é backslash.\n\
{{{{< /quote >}}}}\n\n\
{{{{< spoiler >}}}}\u{201C}secret\u{201D} — hidden{{{{< /spoiler >}} and {{{{< hl rust >}}}}let α = 'β';{{{{< /hl >}}}}\n\n\
{{{{< tabs \"utf8\" >}}}}\n\
{{{{< tab \"α — Beta\" >}}}}\n\
Tab body — café\n\
{{{{< /tab >}}}}\n\
{{{{< /tabs >}}}}\n\n\
{{{{< columns >}}}}\n\
Left — α\n\
<--->\n\
Right — β\n\
{{{{< /columns >}}}}\n\n\
{{{{< code lang=\"text\" >}}}}\n\
Diagram — box → arrow\n\
{{{{< /code >}}}}\n\n\
{long_em_dash_tag}"
        );

        let blocks = parse_markdown(&input);
        assert!(!blocks.is_empty());

        let (preview, _total) = parse_markdown_preview(&input);
        assert!(!preview.is_empty());

        let located = parse_located_blocks(&input);
        assert!(!located.is_empty());
        for item in &located {
            assert!(
                input.is_char_boundary(item.source.start)
                    && input.is_char_boundary(item.source.end)
                    && item.source.start <= item.source.end,
                "invalid located byte range {:?} for UTF-8 source",
                item.source
            );
            let _ = &input[item.source.clone()];
        }
    }

    #[test]
    fn hugo_ref_relref_and_button_shortcodes_parse_as_links() {
        let md = "\
See [Soatok]({{< relref \"Interviews/soatok\" >}}) and [Guide]({{% ref \"/a/my page#sec\" %}}).

Inline {{< button relref=\"Other/conlangs/tokipona\" color=\"other\" >}}toki **pona**{{< /button >}} and {{< button relref=\"Engineering/WrappingUp/latex\" >}}{{< katex >}}\\LaTeX{{< /katex >}}{{< /button >}}.";

        let located = parse_located_blocks(md);
        assert_eq!(located.len(), 2, "expected 2 paragraphs: {located:#?}");

        let MarkdownDocBlock::Paragraph { spans: p0 } = &located[0].block else {
            panic!("expected paragraph 0");
        };
        assert!(p0.iter().any(|s| matches!(
            s,
            MarkdownSpan::Link { label, url }
                if label == "Soatok" && url == "Interviews/soatok"
        )));
        assert!(p0.iter().any(|s| matches!(
            s,
            MarkdownSpan::Link { label, url }
                if label == "Guide" && url == "/a/my%20page#sec"
        )));

        let MarkdownDocBlock::Paragraph { spans: p1 } = &located[1].block else {
            panic!("expected paragraph 1");
        };
        assert!(p1.iter().any(|s| matches!(
            s,
            MarkdownSpan::Styled { text, url: Some(u), css }
                if text == "pona" && u == "Other/conlangs/tokipona" && css.bold
        )));
        assert!(p1.iter().any(|s| matches!(
            s,
            MarkdownSpan::Math { source, url: Some(u), .. }
                if source == "\\LaTeX" && u == "Engineering/WrappingUp/latex"
        )));
    }

    #[test]
    fn html_container_boundaries_and_blockquote_isolation_preserve_blocks() {
        let md = "\
<div style=\"border: 1px solid red\">
<p>Outer lead</p>
<div style=\"background: yellow\">
Inner line
</div>
</div>
<div style=\"color: blue\">
Next box
</div>

> <div style=\"color: green\">
> Inside quote without close

Outside normal paragraph

<details open title=\"x > y\"><summary>Sum <b>bold</b></summary>Body text</details>";

        let blocks: Vec<MarkdownDocBlock> = parse_located_blocks(md)
            .into_iter()
            .map(|item| item.block)
            .collect();
        let Some(MarkdownDocBlock::StyledContainer {
            blocks: outer_children,
            ..
        }) = blocks.first()
        else {
            panic!("expected outer styled container: {blocks:#?}");
        };
        assert!(
            outer_children.iter().any(|b| matches!(
                b,
                MarkdownDocBlock::Paragraph { spans }
                    if plain_text_from_spans(spans) == "Outer lead"
            )),
            "outer container must preserve intermediate blocks before nested open container: {outer_children:#?}"
        );
        assert!(
            outer_children
                .iter()
                .any(|b| matches!(b, MarkdownDocBlock::StyledContainer { .. })),
            "outer container must contain inner container: {outer_children:#?}"
        );
        let quote = blocks.iter().find_map(|b| match b {
            MarkdownDocBlock::Quote { blocks, .. } => Some(blocks),
            _ => None,
        });
        assert!(
            quote.is_some_and(|inner| !inner.is_empty()),
            "blockquote with unclosed <div> must keep its inner content: {blocks:#?}"
        );
        assert!(
            matches!(
                blocks.last(),
                Some(MarkdownDocBlock::Details { open: true, .. })
            ),
            "details with '>' inside quoted attribute should parse cleanly: {blocks:#?}"
        );
        assert!(blocks.iter().any(|b| matches!(
            b,
            MarkdownDocBlock::Paragraph { spans }
                if plain_text_from_spans(spans) == "Outside normal paragraph"
        )));
    }

    #[test]
    fn deeply_nested_inline_shortcodes_are_bounded_and_button_links_images() {
        let img_btn = parse_markdown_spans(
            "{{< button relref=\"/docs/guide\" >}}<img src=\"/icon.png\" alt=\"Go\">{{< /button >}}",
        );
        assert!(img_btn.iter().any(|s| matches!(
            s,
            MarkdownSpan::StyledImage { url, href: Some(h), .. }
                if url == "/icon.png" && h == "/docs/guide"
        )));

        let nested = format!(
            "{}leaf{}",
            "{{< spoiler >}}".repeat(16),
            "{{< /spoiler >}}".repeat(16)
        );
        let spans = parse_markdown_spans(&nested);
        assert!(!spans.is_empty());
    }

    #[test]
    fn linked_code_hidden_inline_html_and_shortcodes_in_html_and_columns() {
        let linked_code = parse_markdown_spans(
            "Call [`init()`](/docs/api), [Home]({{< relref / >}}), [Docs]({{< relref docs/ >}}) <span hidden>`secret`</span><span style=\"display:none\">gone</span>.",
        );
        assert!(linked_code.iter().any(|s| matches!(
            s,
            MarkdownSpan::Styled { text, url: Some(u), css }
                if text == "init()" && u == "/docs/api" && css.monospace
        )));
        assert!(linked_code.iter().any(|s| matches!(
            s,
            MarkdownSpan::Link { label, url } if label == "Home" && url == "/"
        )));
        assert!(linked_code.iter().any(|s| matches!(
            s,
            MarkdownSpan::Link { label, url } if label == "Docs" && url == "docs/"
        )));
        let plain = plain_text_from_spans(&linked_code);
        assert!(!plain.contains("secret") && !plain.contains("gone"));

        let html_shortcode = parse_located_blocks("<p><b>{{< katex >}}x{{< /katex >}}</b></p>");
        let Some(MarkdownDocBlock::Paragraph { spans }) = html_shortcode.first().map(|b| &b.block)
        else {
            panic!("expected paragraph: {html_shortcode:#?}");
        };
        assert!(spans.iter().any(|s| matches!(
            s,
            MarkdownSpan::Math { source, .. } if source == "x"
        )));

        let cols = parse_located_blocks(
            "{{< columns >}}\nLeft `<--->` split\n<--->\nRight\n{{< /columns >}}",
        );
        let Some(MarkdownDocBlock::Columns { columns, .. }) = cols.first().map(|b| &b.block) else {
            panic!("expected columns: {cols:#?}");
        };
        assert_eq!(columns.len(), 2, "backtick `<--->` should not split column");

        let card_lines = parse_markdown(
            "{{< katex >}}\n\n# Visible Heading\n\n{{< details \"Info\" >}}\nBody\n{{< /details >}}\n\n{{< /quote >}} trailing card words",
        );
        assert!(
            card_lines.iter().any(|line| matches!(
                line,
                MarkdownLine::Heading { level: 1, text, .. } if text == "Visible Heading"
            )),
            "zero-body {{{{< katex >}}}} loader must not consume subsequent blocks: {card_lines:#?}"
        );
        assert!(
            card_lines.iter().any(|line| matches!(
                line,
                MarkdownLine::Paragraph { spans }
                    if plain_text_from_spans(spans).contains("trailing card words")
            )),
            "closing shortcode followed by trailing text must not ignore the whole line: {card_lines:#?}"
        );

        let inline_html = parse_markdown_spans(
            "One</br>Two <svg><text>svg-hidden</text></svg><iframe src=\"x\"/>visible",
        );
        assert!(inline_html.contains(&MarkdownSpan::HardBreak));
        let inline_plain = plain_text_from_spans(&inline_html);
        assert!(!inline_plain.contains("svg-hidden") && inline_plain.contains("visible"));
    }
}
