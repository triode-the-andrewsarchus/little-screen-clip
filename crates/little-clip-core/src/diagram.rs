//! Diagram syntax identities shared by Markdown and standalone file viewers.
use std::path::Path;

/// A diagram syntax with an integrated native renderer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    Pikchr,
    Svgbob,
    Mermaid,
    Lini,
    Graphviz,
    D2,
    Bpmn,
    Math,
    MathInline,
}
impl Format {
    /// Recognizes a Markdown fence's language, without executing its attributes.
    pub fn from_language(language: &str) -> Option<Self> {
        match language
            .split_whitespace()
            .next()?
            .to_ascii_lowercase()
            .as_str()
        {
            "pikchr" => Some(Self::Pikchr),
            "svgbob" | "bob" => Some(Self::Svgbob),
            "mermaid" | "mmd" => Some(Self::Mermaid),
            "lini" => Some(Self::Lini),
            "graphviz" | "dot" => Some(Self::Graphviz),
            "d2" => Some(Self::D2),
            "bpmn" | "bpmn2" => Some(Self::Bpmn),
            _ => None,
        }
    }

    /// Recognizes unambiguous standalone extensions.
    ///
    /// Ambiguous `.bob` files need content classification and are not claimed here.
    pub fn from_path(path: &Path) -> Option<Self> {
        Self::from_extension(path.extension()?.to_str()?)
    }

    /// Recognizes an unambiguous extension without treating fence aliases as paths.
    pub fn from_extension(extension: &str) -> Option<Self> {
        match extension.to_ascii_lowercase().as_str() {
            "pikchr" => Some(Self::Pikchr),
            "svgbob" => Some(Self::Svgbob),
            "mermaid" => Some(Self::Mermaid),
            "lini" => Some(Self::Lini),
            "gv" | "graphviz" => Some(Self::Graphviz),
            "d2" => Some(Self::D2),
            "bpmn" | "bpmn2" => Some(Self::Bpmn),
            _ => None,
        }
    }

    /// Uses content signatures for extensions shared with ordinary Markdown.
    pub fn from_source_path(path: &Path, source: &str) -> Option<Self> {
        Self::from_source_extension(path.extension()?.to_str()?, source)
    }

    /// Combines unambiguous extensions with conservative content classification.
    pub fn from_source_extension(extension: &str, source: &str) -> Option<Self> {
        Self::from_extension(extension).or_else(|| {
            if extension.eq_ignore_ascii_case("mmd") && mermaid_signature(source) {
                Some(Self::Mermaid)
            } else if extension.eq_ignore_ascii_case("dot") && dot_signature(source) {
                Some(Self::Graphviz)
            } else if extension.eq_ignore_ascii_case("xml") && bpmn_signature(source) {
                Some(Self::Bpmn)
            } else {
                None
            }
        })
    }

    /// Recognizes distinctive diagram syntax in extensionless clipboard text.
    ///
    /// This is a bounded hint, not validation. Callers must fall back to text
    /// when native rendering fails. Ambiguous edge-only D2 and bare math are not
    /// claimed; explicit files and Markdown fences remain available for them.
    pub fn from_snippet(source: &str) -> Option<Self> {
        if source.len() > 256 * 1024 || source.contains('\0') {
            return None;
        }
        let source = source.trim_start_matches('\u{feff}').trim();
        let signatures = [
            (Self::Mermaid, mermaid_signature as fn(&str) -> bool),
            (Self::Graphviz, dot_signature),
            (Self::Bpmn, bpmn_signature),
            (Self::Pikchr, pikchr_signature),
            (Self::Lini, lini_signature),
            (Self::D2, d2_signature),
            (Self::Svgbob, svgbob_signature),
        ];
        signatures
            .iter()
            .find_map(|(format, signature)| signature(source).then_some(*format))
    }

    /// Returns the preferred standalone source extension, excluding inline math.
    pub const fn source_extension(self) -> Option<&'static str> {
        match self {
            Self::Pikchr => Some("pikchr"),
            Self::Svgbob => Some("svgbob"),
            Self::Mermaid => Some("mermaid"),
            Self::Lini => Some("lini"),
            Self::Graphviz => Some("gv"),
            Self::D2 => Some("d2"),
            Self::Bpmn => Some("bpmn"),
            Self::Math | Self::MathInline => None,
        }
    }

    /// Returns the syntax's display name.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Pikchr => "Pikchr",
            Self::Svgbob => "SvgBob",
            Self::Mermaid => "Mermaid",
            Self::Lini => "Lini",
            Self::Graphviz => "Graphviz",
            Self::D2 => "D2",
            Self::Bpmn => "BPMN",
            Self::Math | Self::MathInline => "LaTeX math",
        }
    }
}

fn pikchr_signature(source: &str) -> bool {
    let first = source.lines().next().unwrap_or_default();
    let (word, rest) = first.split_once(char::is_whitespace).unwrap_or((first, ""));
    matches!(word, "box" | "circle" | "ellipse" | "oval" | "cylinder")
        && rest.trim_start().starts_with('"')
}

fn lini_signature(source: &str) -> bool {
    (source.starts_with('|') || source.starts_with('{'))
        && source.lines().take(16).any(lini_declaration)
}

fn d2_signature(source: &str) -> bool {
    source
        .lines()
        .next()
        .unwrap_or_default()
        .strip_prefix("direction:")
        .is_some_and(|value| matches!(value.trim(), "right" | "left" | "up" | "down"))
        && source
            .lines()
            .skip(1)
            .any(|line| line.contains("->") || line.contains("shape:"))
}

fn svgbob_signature(source: &str) -> bool {
    let first = source.lines().next().unwrap_or_default();
    first.starts_with('+')
        && first.ends_with('+')
        && first.bytes().all(|byte| matches!(byte, b'+' | b'-' | b' '))
        && first.contains("--")
        && source.lines().skip(1).any(|line| line.trim() == first)
        && source.lines().any(|line| line.trim().starts_with('|'))
}

fn lini_declaration(line: &str) -> bool {
    let Some(rest) = line.trim_start().strip_prefix('|') else {
        return false;
    };
    let Some((component, label)) = rest.split_once('|') else {
        return false;
    };
    let kind = component.split(['#', ' ', '.']).next().unwrap_or_default();
    matches!(kind, "box" | "text" | "row" | "column" | "circle")
        && label.trim_start().starts_with('"')
}

// Generic XML is claimed only by its actual, namespace-resolved root. A bounded
// prefix avoids materializing the document or reading beyond the source buffer.
fn bpmn_signature(source: &str) -> bool {
    use quick_xml::{events::Event, name::ResolveResult};
    let source = &source[..source.floor_char_boundary(source.len().min(32 * 1024))];
    let mut reader = quick_xml::NsReader::from_str(source.trim_start_matches('\u{feff}'));
    loop {
        match reader.read_resolved_event() {
            Ok((ResolveResult::Bound(namespace), Event::Start(tag) | Event::Empty(tag))) => {
                return namespace.as_ref() == b"http://www.omg.org/spec/BPMN/20100524/MODEL"
                    && tag.local_name().as_ref() == b"definitions";
            }
            Ok((_, Event::Decl(_) | Event::Comment(_) | Event::PI(_))) => (),
            Ok((_, Event::Text(text))) if text.iter().all(u8::is_ascii_whitespace) => (),
            _ => return false,
        }
    }
}

// Classify only the DOT header, not a guessed filename or arbitrary prose.
fn dot_signature(source: &str) -> bool {
    let source = &source[..source.floor_char_boundary(source.len().min(64 * 1024))];
    let Some(mut rest) = dot_trivia(source.trim_start_matches('\u{feff}')) else {
        return false;
    };
    for expected in ["strict", "graph"] {
        let end = rest
            .find(|value: char| !value.is_ascii_alphabetic())
            .unwrap_or(rest.len());
        let word = &rest[..end];
        if expected == "strict" && !word.eq_ignore_ascii_case("strict") {
            continue;
        }
        if expected == "graph"
            && !word.eq_ignore_ascii_case("graph")
            && !word.eq_ignore_ascii_case("digraph")
        {
            return false;
        }
        if rest[end..].starts_with(|value: char| value.is_alphanumeric() || value == '_') {
            return false;
        }
        let Some(next) = dot_trivia(&rest[end..]) else {
            return false;
        };
        rest = next;
    }
    if rest.starts_with('{') {
        return true;
    }
    // Quoted graph names may contain braces and escaped quotes, or concatenate
    // quoted parts. An opening brace inside a name is not a graph body.
    if rest.starts_with('"') {
        loop {
            let mut escaped = false;
            let mut after = None;
            for (index, value) in rest[1..].char_indices() {
                if escaped {
                    escaped = false;
                } else if value == '\\' {
                    escaped = true;
                } else if value == '"' {
                    after = Some(&rest[index + 2..]);
                    break;
                }
            }
            let Some(next) = after.and_then(dot_trivia) else {
                return false;
            };
            rest = next;
            if !rest.starts_with('+') {
                break;
            }
            let Some(next) = dot_trivia(&rest[1..]) else {
                return false;
            };
            rest = next;
            if !rest.starts_with('"') {
                return false;
            }
        }
    } else {
        let end = rest
            .find(|value: char| value.is_whitespace() || matches!(value, '{' | '/' | '#'))
            .unwrap_or(rest.len());
        if end == 0
            || !rest[..end].chars().all(|value| {
                value.is_alphanumeric() || matches!(value, '_' | '-' | '.') || !value.is_ascii()
            })
        {
            return false;
        }
        let Some(next) = dot_trivia(&rest[end..]) else {
            return false;
        };
        rest = next;
    }
    rest.starts_with('{')
}

fn dot_trivia(mut source: &str) -> Option<&str> {
    loop {
        source = source.trim_start();
        if let Some(comment) = source.strip_prefix("/*") {
            source = &comment[comment.find("*/")? + 2..];
        } else if source.starts_with("//") || source.starts_with('#') {
            source = source.split_once('\n')?.1;
        } else {
            return Some(source);
        }
    }
}

fn mermaid_signature(source: &str) -> bool {
    let mut end = source.len().min(64 * 1024);
    while !source.is_char_boundary(end) {
        end -= 1;
    }
    let prefix = &source[..end];
    let metadata = crate::markdown_metadata::parse(prefix);
    let mut body = &prefix[metadata.as_ref().map_or(0, |metadata| metadata.body_start)..];
    body = body.trim_start_matches('\u{feff}');
    loop {
        body = body.trim_start();
        if body.starts_with("%%{") {
            let Some(end) = body.find("}%%") else {
                return false;
            };
            body = &body[end + 3..];
        } else if body.starts_with("%%") {
            let Some((_, remaining)) = body.split_once('\n') else {
                return false;
            };
            body = remaining;
        } else {
            break;
        }
    }
    let line = body.lines().next().unwrap_or_default();
    let word = line
        .split(|value: char| value.is_whitespace() || value == ';')
        .next()
        .unwrap_or_default();
    let rest = line[word.len()..].trim();
    match word {
        "graph" | "flowchart" => rest
            .split(|value: char| value.is_whitespace() || value == ';')
            .next()
            .is_some_and(|direction| matches!(direction, "LR" | "RL" | "TD" | "TB" | "BT")),
        "pie" => rest.is_empty() || rest.starts_with("title ") || rest.starts_with("showData"),
        "sequenceDiagram" | "classDiagram" | "stateDiagram" | "stateDiagram-v2" | "erDiagram"
        | "requirementDiagram" | "quadrantChart" | "C4Context" | "C4Container" | "C4Component"
        | "C4Dynamic" | "C4Deployment" | "gitGraph" => true,
        "gantt" | "timeline" | "journey" | "mindmap" | "kanban" | "xychart" | "xychart-beta"
        | "block-beta" | "block" | "packet-beta" | "packet" | "architecture-beta"
        | "sankey-beta" | "radar-beta" | "treemap-beta" | "zenuml" => {
            rest.is_empty() || rest.starts_with(';')
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn clipboard_diagrams_use_distinctive_bounded_signatures() {
        for (source, format) in [
            ("flowchart LR\nA[Source] --> B[Preview]", Format::Mermaid),
            ("digraph { a -> b }", Format::Graphviz),
            (
                "<definitions xmlns='http://www.omg.org/spec/BPMN/20100524/MODEL'/>",
                Format::Bpmn,
            ),
            ("box \"Read\"; arrow; circle \"Think\"", Format::Pikchr),
            ("|box#read| \"Read\"\nread -> next", Format::Lini),
            ("direction: right\na -> b", Format::D2),
            ("+----+\n|Hi! |\n+----+", Format::Svgbob),
        ] {
            assert_eq!(Format::from_snippet(source), Some(format), "{source}");
            assert_eq!(
                Format::from_snippet(&format!("\u{feff}\n{source}\n")),
                Some(format)
            );
            assert_eq!(
                Format::from_extension(format.source_extension().unwrap()),
                Some(format)
            );
        }
        for source in [
            "a -> b",
            "A graph of the results.",
            "direction: right",
            "box(1, 2)",
            "| Name | Value |\n| --- | --- |\n| box | 3 |",
            "<definitions xmlns='urn:other'/>",
            "```mermaid\nflowchart LR\nA-->B\n```",
            "fn main() { println!(\"a -> b\"); }",
        ] {
            assert_eq!(Format::from_snippet(source), None, "{source}");
        }
        assert_eq!(Format::from_snippet(&"x".repeat(256 * 1024 + 1)), None);
        assert_eq!(Format::from_snippet("digraph { a -> b }\0"), None);
    }

    #[test]
    fn bpmn_xml_detection_requires_the_resolved_root_namespace() {
        let namespace = "http://www.omg.org/spec/BPMN/20100524/MODEL";
        for source in [
            format!("<definitions xmlns='{namespace}'/>"),
            format!("<?xml version='1.0'?><!-- hi --><p:definitions xmlns:p='{namespace}'/>"),
        ] {
            assert_eq!(
                Format::from_source_extension("xml", &source),
                Some(Format::Bpmn)
            );
        }
        for source in [
            "<definitions/>",
            "<definitions xmlns='urn:other'/>",
            "<root><definitions xmlns='http://www.omg.org/spec/BPMN/20100524/MODEL'/></root>",
            "<!DOCTYPE definitions><definitions/>",
        ] {
            assert_eq!(Format::from_source_extension("xml", source), None);
        }
        assert_eq!(Format::from_extension("bpmn2"), Some(Format::Bpmn));
        assert_eq!(Format::from_language("bpmn"), Some(Format::Bpmn));
    }

    #[test]
    fn dot_files_require_graph_headers_instead_of_claiming_word_templates() {
        for source in [
            "digraph { a -> b }",
            "GRAPH network { a -- b }",
            "\u{feff}/* note */ strict // note\ndigraph G { a -> b }",
            "# 1 \"graph.dot\"\ndigraph \"Name { brace }\" + \"More\" { a -> b }",
            "graph .25 { a -- b }",
        ] {
            assert_eq!(
                Format::from_source_path(Path::new("graph.dot"), source),
                Some(Format::Graphviz),
                "{source}"
            );
        }
        for source in [
            "Word template",
            "graph theory notes",
            "digraphical text {sample}",
            "digraph \"a { b\"",
            "/* unclosed",
            "graph name is ordinary prose { text }",
        ] {
            assert_eq!(
                Format::from_source_path(Path::new("template.dot"), source),
                None,
                "{source}"
            );
        }
        assert_eq!(Format::from_extension("dot"), None);
        assert_eq!(Format::from_extension("graphviz"), Some(Format::Graphviz));
    }
    #[test]
    fn mmd_files_need_diagram_headers_instead_of_extension_only_detection() {
        for source in [
            "flowchart LR; A-->B",
            "sequenceDiagram\nAlice->>Bob: Hi",
            "---\ntitle: Example\n---\n%% comment\nflowchart TD\nA-->B",
            "%%{init: {\n 'theme': 'dark'\n}}%%\npie title Shares\n\"A\" : 20",
        ] {
            assert_eq!(
                Format::from_source_path(Path::new("example.mmd"), source),
                Some(Format::Mermaid)
            );
        }
        for source in [
            "# Ordinary Markdown\n\nC4Container is a diagram type.",
            "pie recipes\n========",
            "graph theory introduction",
            "```mermaid\nflowchart LR\nA-->B\n```",
        ] {
            assert_eq!(
                Format::from_source_path(Path::new("example.mmd"), source),
                None
            );
        }
    }
}
