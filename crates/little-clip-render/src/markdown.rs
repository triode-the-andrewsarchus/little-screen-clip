//! Lightweight Markdown parsing for ambient screen previews.
//!
//! Supports:
//! - Headings (# H1, ## H2, ### H3)
//! - Bulleted lists (- item, * item, + item)
//! - Numbered lists (1. item, 2. item)
//! - Quotes (> quote)
//! - Horizontal rules (---, ***)
//! - Code blocks (```)
//! - Inline bold (**bold**, __bold__)
//! - Inline italics (*italic*, _italic_)
//! - Inline bold-italics (***bold italic***)
//! - Inline code (`code`)
//! - Links (\[label](url))

/// A styled inline segment within a Markdown line.
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
    /// Inline code span (`` `code` ``).
    Code(String),
    /// Hyperlink with label and URL (`[label](url)`).
    Link { label: String, url: String },
    /// Native equation; a link around it remains attached to the rendered image.
    Math {
        source: String,
        display: bool,
        url: Option<String>,
    },
}

/// A parsed line of Markdown content for ambient preview rendering.
#[derive(Debug, Clone, PartialEq)]
pub enum MarkdownLine {
    /// Parsed document metadata, or an explicit framing/parse diagnostic.
    FrontMatter(Box<little_clip_core::markdown_metadata::FrontMatter>),
    /// Section heading with level 1..=3.
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
    /// Numbered list item with number prefix.
    Numbered {
        number: String,
        spans: Vec<MarkdownSpan>,
    },
    /// Blockquote line.
    Quote { spans: Vec<MarkdownSpan> },
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

fn parse_body_into(input: &str, mut sink: impl FnMut(MarkdownLine, std::ops::Range<usize>)) {
    let original = input;
    let input = input.trim_start_matches('\u{feff}');
    let bom = original.len() - input.len();
    let mut fence: Option<(char, usize)> = None;
    let mut display = None;
    for range in logical_lines(input) {
        let mut source_range = range.clone();
        let raw_line = input[range.clone()].trim_end_matches(['\r', '\n']);
        let tail;
        let raw_line = if fence.is_none() {
            match display_math_line(raw_line, range.start, &mut display) {
                DisplayLine::Ordinary => raw_line,
                DisplayLine::Pending => continue,
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
                continue;
            }
        }

        if trimmed.is_empty() {
            emit(MarkdownLine::Empty);
            continue;
        }

        // Horizontal rules and headings share one framing path each; adding
        // inline content must not create another copy for every heading level.
        if let Some(marker @ (b'-' | b'*' | b'_')) = trimmed.as_bytes().first()
            && trimmed.len() >= 3
            && trimmed.bytes().all(|value| value == *marker)
        {
            emit(MarkdownLine::Rule);
            continue;
        }
        if let Some((marker, rest)) = trimmed_start.split_once(' ')
            && matches!(marker, "#" | "##" | "###" | "####")
        {
            emit(MarkdownLine::Heading {
                level: marker.len().min(3),
                text: rest.trim().to_owned(),
                spans: parse_markdown_spans(rest.trim()),
            });
            continue;
        }

        // Blockquotes
        if let Some(rest) = trimmed_start.strip_prefix("> ") {
            let spans = parse_markdown_spans(rest.trim());
            emit(MarkdownLine::Quote { spans });
            continue;
        }

        // Bulleted lists
        let indent = raw_line.len() - trimmed_start.len();
        if let Some(rest) = trimmed_start
            .strip_prefix("- ")
            .or_else(|| trimmed_start.strip_prefix("* "))
            .or_else(|| trimmed_start.strip_prefix("+ "))
        {
            let spans = parse_markdown_spans(rest.trim());
            emit(MarkdownLine::Bullet { indent, spans });
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
                continue;
            }
        }

        // Normal paragraph
        let spans = parse_markdown_spans(trimmed);
        emit(MarkdownLine::Paragraph { spans });
    }

    if let Some((start, source)) = display {
        sink(
            MarkdownLine::IncompleteMath { source },
            start + bom..original.len(),
        );
    }
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

/// Parses inline formatting and native math with CommonMark delimiter semantics.
pub fn parse_markdown_spans(text: &str) -> Vec<MarkdownSpan> {
    use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
    let mut spans = Vec::new();
    let (mut strong, mut emphasis) = (0usize, 0usize);
    let mut url: Option<String> = None;
    for event in Parser::new_ext(text, Options::ENABLE_MATH) {
        match event {
            Event::Start(Tag::Strong) => strong += 1,
            Event::End(TagEnd::Strong) => strong = strong.saturating_sub(1),
            Event::Start(Tag::Emphasis) => emphasis += 1,
            Event::End(TagEnd::Emphasis) => emphasis = emphasis.saturating_sub(1),
            Event::Start(Tag::Link { dest_url, .. }) => url = Some(dest_url.into_string()),
            Event::End(TagEnd::Link) => url = None,
            Event::InlineMath(source) => spans.push(MarkdownSpan::Math {
                source: source.into_string(),
                display: false,
                url: url.clone(),
            }),
            Event::DisplayMath(source) => spans.push(MarkdownSpan::Math {
                source: source.into_string(),
                display: true,
                url: url.clone(),
            }),
            Event::Code(source) => spans.push(MarkdownSpan::Code(source.into_string())),
            Event::Text(value) | Event::InlineHtml(value) | Event::Html(value) => {
                let span = if let Some(url) = &url {
                    MarkdownSpan::Link {
                        label: value.into_string(),
                        url: url.clone(),
                    }
                } else {
                    match (strong > 0, emphasis > 0) {
                        (true, true) => MarkdownSpan::BoldItalic(value.into_string()),
                        (true, false) => MarkdownSpan::Bold(value.into_string()),
                        (false, true) => MarkdownSpan::Italic(value.into_string()),
                        (false, false) => MarkdownSpan::Text(value.into_string()),
                    }
                };
                push_span(&mut spans, span);
            }
            Event::SoftBreak | Event::HardBreak => {
                push_span(&mut spans, MarkdownSpan::Text(" ".into()))
            }
            _ => {}
        }
    }
    spans
}

fn push_span(spans: &mut Vec<MarkdownSpan>, span: MarkdownSpan) {
    // Entities and escaped punctuation arrive as separate native text events.
    // Coalesce equal styles so those scanner details do not create layout gaps.
    let target = match (spans.last_mut(), &span) {
        (Some(MarkdownSpan::Text(old)), MarkdownSpan::Text(value))
        | (Some(MarkdownSpan::Bold(old)), MarkdownSpan::Bold(value))
        | (Some(MarkdownSpan::Italic(old)), MarkdownSpan::Italic(value))
        | (Some(MarkdownSpan::BoldItalic(old)), MarkdownSpan::BoldItalic(value)) => {
            Some((old, value))
        }
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
}
