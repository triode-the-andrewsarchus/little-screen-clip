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
            } else {
                None
            }
        })
    }

    /// Returns the syntax's display name.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Pikchr => "Pikchr",
            Self::Svgbob => "SvgBob",
            Self::Mermaid => "Mermaid",
            Self::Lini => "Lini",
            Self::Graphviz => "Graphviz",
            Self::Math | Self::MathInline => "LaTeX math",
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
