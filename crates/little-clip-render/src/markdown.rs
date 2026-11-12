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
    /// Blank line between blocks.
    Empty,
}

/// Parses raw Markdown text into a sequence of [`MarkdownLine`] blocks.
pub fn parse_markdown(input: &str) -> Vec<MarkdownLine> {
    let metadata = little_clip_core::markdown_metadata::parse(input);
    let body = &input[metadata.as_ref().map_or(0, |value| value.body_start)..];
    let mut lines = Vec::new();
    if let Some(metadata) = metadata {
        lines.push(MarkdownLine::FrontMatter(Box::new(metadata)));
    }
    lines.extend(parse_body(body));
    lines
}

fn parse_body(input: &str) -> Vec<MarkdownLine> {
    let input = input.trim_start_matches('\u{feff}');
    let mut lines = Vec::new();
    let mut fence: Option<(char, usize)> = None;

    for raw_line in input.lines() {
        let trimmed_start = raw_line.trim_start();
        let trimmed = raw_line.trim();

        if let Some((marker, minimum)) = fence {
            let run = trimmed.chars().take_while(|value| *value == marker).count();
            if run >= minimum && trimmed.chars().skip(run).all(char::is_whitespace) {
                fence = None;
                lines.push(MarkdownLine::CodeFence {
                    language: String::new(),
                });
            } else {
                lines.push(MarkdownLine::CodeLine {
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
                lines.push(MarkdownLine::CodeFence {
                    language: language.trim().to_string(),
                });
                continue;
            }
        }

        if trimmed.is_empty() {
            lines.push(MarkdownLine::Empty);
            continue;
        }

        // Horizontal rule (---, ***, ___)
        if (trimmed.starts_with("---") && trimmed.chars().all(|c| c == '-'))
            || (trimmed.starts_with("***") && trimmed.chars().all(|c| c == '*'))
            || (trimmed.starts_with("___") && trimmed.chars().all(|c| c == '_'))
        {
            lines.push(MarkdownLine::Rule);
            continue;
        }

        // Headings
        if let Some(rest) = trimmed_start.strip_prefix("### ") {
            let spans = parse_markdown_spans(rest.trim());
            lines.push(MarkdownLine::Heading {
                level: 3,
                text: rest.trim().to_string(),
                spans,
            });
            continue;
        } else if let Some(rest) = trimmed_start.strip_prefix("## ") {
            let spans = parse_markdown_spans(rest.trim());
            lines.push(MarkdownLine::Heading {
                level: 2,
                text: rest.trim().to_string(),
                spans,
            });
            continue;
        } else if let Some(rest) = trimmed_start.strip_prefix("# ") {
            let spans = parse_markdown_spans(rest.trim());
            lines.push(MarkdownLine::Heading {
                level: 1,
                text: rest.trim().to_string(),
                spans,
            });
            continue;
        } else if let Some(rest) = trimmed_start.strip_prefix("#### ") {
            let spans = parse_markdown_spans(rest.trim());
            lines.push(MarkdownLine::Heading {
                level: 3,
                text: rest.trim().to_string(),
                spans,
            });
            continue;
        }

        // Blockquotes
        if let Some(rest) = trimmed_start.strip_prefix("> ") {
            let spans = parse_markdown_spans(rest.trim());
            lines.push(MarkdownLine::Quote { spans });
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
            lines.push(MarkdownLine::Bullet { indent, spans });
            continue;
        }

        // Numbered lists (e.g. "1. ", "10. ")
        if let Some(dot_pos) = trimmed_start.find(". ") {
            let num_part = &trimmed_start[..dot_pos];
            if !num_part.is_empty() && num_part.chars().all(|c| c.is_ascii_digit()) {
                let rest = &trimmed_start[dot_pos + 2..];
                let spans = parse_markdown_spans(rest.trim());
                lines.push(MarkdownLine::Numbered {
                    number: format!("{}.", num_part),
                    spans,
                });
                continue;
            }
        }

        // Normal paragraph
        let spans = parse_markdown_spans(trimmed);
        lines.push(MarkdownLine::Paragraph { spans });
    }

    lines
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
    let mut prepared = parse_body(&body[..end]);
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

/// Parses inline formatting spans (bold, italic, code, links) within a line of text.
pub fn parse_markdown_spans(text: &str) -> Vec<MarkdownSpan> {
    let mut spans = Vec::new();
    let mut i = 0;
    let mut buf = String::new();

    while i < text.len() {
        let rest = &text[i..];

        // Link: [label](url)
        if rest.starts_with('[')
            && let Some(close_bracket) = rest[1..].find(']')
        {
            let after_bracket = 1 + close_bracket + 1;
            if rest[after_bracket..].starts_with('(')
                && let Some(close_paren) = rest[after_bracket + 1..].find(')')
            {
                if !buf.is_empty() {
                    spans.push(MarkdownSpan::Text(std::mem::take(&mut buf)));
                }
                let label = rest[1..1 + close_bracket].to_string();
                let url = rest[after_bracket + 1..after_bracket + 1 + close_paren].to_string();
                spans.push(MarkdownSpan::Link { label, url });
                i += after_bracket + 1 + close_paren + 1;
                continue;
            }
        }

        // Inline code: `code`
        if rest.starts_with('`')
            && let Some(close_tick) = rest[1..].find('`')
        {
            if !buf.is_empty() {
                spans.push(MarkdownSpan::Text(std::mem::take(&mut buf)));
            }
            let code_str = rest[1..1 + close_tick].to_string();
            spans.push(MarkdownSpan::Code(code_str));
            i += 1 + close_tick + 1;
            continue;
        }

        // Bold italic: ***text***
        if rest.starts_with("***")
            && let Some(close_idx) = rest[3..].find("***")
        {
            if !buf.is_empty() {
                spans.push(MarkdownSpan::Text(std::mem::take(&mut buf)));
            }
            let bi_str = rest[3..3 + close_idx].to_string();
            spans.push(MarkdownSpan::BoldItalic(bi_str));
            i += 3 + close_idx + 3;
            continue;
        }

        // Bold: **text** or __text__
        if rest.starts_with("**") || rest.starts_with("__") {
            let delim = &rest[..2];
            if let Some(close_idx) = rest[2..].find(delim) {
                if !buf.is_empty() {
                    spans.push(MarkdownSpan::Text(std::mem::take(&mut buf)));
                }
                let b_str = rest[2..2 + close_idx].to_string();
                spans.push(MarkdownSpan::Bold(b_str));
                i += 2 + close_idx + 2;
                continue;
            }
        }

        // Italic: *text* or _text_
        if rest.starts_with('*') || rest.starts_with('_') {
            let delim = rest.as_bytes()[0] as char;
            if let Some(close_idx) = rest[1..].find(delim) {
                // Ensure not preceded or followed by same delimiter
                if close_idx > 0 {
                    if !buf.is_empty() {
                        spans.push(MarkdownSpan::Text(std::mem::take(&mut buf)));
                    }
                    let it_str = rest[1..1 + close_idx].to_string();
                    spans.push(MarkdownSpan::Italic(it_str));
                    i += 1 + close_idx + 1;
                    continue;
                }
            }
        }

        let ch = rest.chars().next().expect("i < text.len()");
        buf.push(ch);
        i += ch.len_utf8();
    }

    if !buf.is_empty() {
        spans.push(MarkdownSpan::Text(buf));
    }

    spans
}

#[cfg(test)]
mod tests {
    use super::*;

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
