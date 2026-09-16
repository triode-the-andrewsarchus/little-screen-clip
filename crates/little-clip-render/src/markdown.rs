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
//! - Links ([label](url))

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
    let mut lines = Vec::new();
    let mut in_code_fence = false;

    for raw_line in input.lines() {
        let trimmed_start = raw_line.trim_start();
        let trimmed = raw_line.trim();

        if trimmed.starts_with("```") {
            in_code_fence = !in_code_fence;
            let lang = trimmed.trim_start_matches('`').trim().to_string();
            lines.push(MarkdownLine::CodeFence { language: lang });
            continue;
        }

        if in_code_fence {
            lines.push(MarkdownLine::CodeLine {
                line: raw_line.to_string(),
            });
            continue;
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

/// Parses inline formatting spans (bold, italic, code, links) within a line of text.
pub fn parse_markdown_spans(text: &str) -> Vec<MarkdownSpan> {
    let mut spans = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    let len = chars.len();
    let mut i = 0;
    let mut buf = String::new();

    while i < len {
        // Link: [label](url)
        if chars[i] == '[' {
            if let Some(close_bracket) = text[i..].find(']') {
                let after_bracket = i + close_bracket + 1;
                if after_bracket < len && chars[after_bracket] == '(' {
                    if let Some(close_paren) = text[after_bracket..].find(')') {
                        if !buf.is_empty() {
                            spans.push(MarkdownSpan::Text(std::mem::take(&mut buf)));
                        }
                        let label: String = chars[i + 1..i + close_bracket].iter().collect();
                        let url: String = chars[after_bracket + 1..after_bracket + close_paren]
                            .iter()
                            .collect();
                        spans.push(MarkdownSpan::Link { label, url });
                        i = after_bracket + close_paren + 1;
                        continue;
                    }
                }
            }
        }

        // Inline code: `code`
        if chars[i] == '`' {
            if let Some(close_tick) = text[i + 1..].find('`') {
                if !buf.is_empty() {
                    spans.push(MarkdownSpan::Text(std::mem::take(&mut buf)));
                }
                let code_str: String = chars[i + 1..i + 1 + close_tick].iter().collect();
                spans.push(MarkdownSpan::Code(code_str));
                i = i + 1 + close_tick + 1;
                continue;
            }
        }

        // Bold italic: ***text***
        if i + 2 < len && chars[i] == '*' && chars[i + 1] == '*' && chars[i + 2] == '*' {
            if let Some(close_idx) = text[i + 3..].find("***") {
                if !buf.is_empty() {
                    spans.push(MarkdownSpan::Text(std::mem::take(&mut buf)));
                }
                let bi_str: String = chars[i + 3..i + 3 + close_idx].iter().collect();
                spans.push(MarkdownSpan::BoldItalic(bi_str));
                i = i + 3 + close_idx + 3;
                continue;
            }
        }

        // Bold: **text** or __text__
        if i + 1 < len
            && ((chars[i] == '*' && chars[i + 1] == '*')
                || (chars[i] == '_' && chars[i + 1] == '_'))
        {
            let delim = if chars[i] == '*' { "**" } else { "__" };
            if let Some(close_idx) = text[i + 2..].find(delim) {
                if !buf.is_empty() {
                    spans.push(MarkdownSpan::Text(std::mem::take(&mut buf)));
                }
                let b_str: String = chars[i + 2..i + 2 + close_idx].iter().collect();
                spans.push(MarkdownSpan::Bold(b_str));
                i = i + 2 + close_idx + 2;
                continue;
            }
        }

        // Italic: *text* or _text_
        if chars[i] == '*' || chars[i] == '_' {
            let delim = chars[i];
            if let Some(close_idx) = text[i + 1..].find(delim) {
                // Ensure not preceded or followed by same delimiter
                if close_idx > 0 {
                    if !buf.is_empty() {
                        spans.push(MarkdownSpan::Text(std::mem::take(&mut buf)));
                    }
                    let it_str: String = chars[i + 1..i + 1 + close_idx].iter().collect();
                    spans.push(MarkdownSpan::Italic(it_str));
                    i = i + 1 + close_idx + 1;
                    continue;
                }
            }
        }

        buf.push(chars[i]);
        i += 1;
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
