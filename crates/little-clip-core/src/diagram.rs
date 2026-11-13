//! Diagram syntax identities shared by Markdown and standalone file viewers.
use std::path::Path;

/// Bounded WaveJSON parsing shared by rendering and graphical editing.
pub mod wavejson;

/// Preserving scene snapshots and transactional native drawing edits.
pub mod drawing;

/// Bounded harness data shared by WireViz previews and direct editing.
pub mod wireviz;

/// Erd schema syntax and authored token locations.
pub mod erd;

/// Maximum decoded UTF-8 source size for an explicit native drawing.
pub const MAX_DRAWING_SOURCE_BYTES: usize = 2 * 1024 * 1024;

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
    Wavedrom,
    Excalidraw,
    Wireviz,
    Nomnoml,
    Dbml,
    Goat,
    Bytefield,
    Erd,
    Plantuml,
    Drawio,
    Structurizr,
    Vega,
    VegaLite,
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
            "wavedrom" | "wavejson" => Some(Self::Wavedrom),
            "excalidraw" => Some(Self::Excalidraw),
            "wireviz" => Some(Self::Wireviz),
            "nomnoml" | "noml" => Some(Self::Nomnoml),
            "dbml" => Some(Self::Dbml),
            "goat" => Some(Self::Goat),
            "bytefield" => Some(Self::Bytefield),
            "erd" | "er" => Some(Self::Erd),
            "plantuml" | "puml" | "c4plantuml" => Some(Self::Plantuml),
            "drawio" | "diagramsnet" => Some(Self::Drawio),
            "structurizr" => Some(Self::Structurizr),
            "vega" => Some(Self::Vega),
            "vegalite" | "vega-lite" => Some(Self::VegaLite),
            _ => None,
        }
    }

    /// Recognizes standalone extension candidates.
    ///
    /// DBML and Erd candidates still need XML disambiguation once source is available.
    ///
    /// Ambiguous `.bob` files need content classification and are not claimed here.
    pub fn from_path(path: &Path) -> Option<Self> {
        let name = path.file_name()?.as_encoded_bytes();
        for (suffix, format) in [
            (b".excalidraw.json".as_slice(), Self::Excalidraw),
            (b".vega.json".as_slice(), Self::Vega),
            (b".vg.json".as_slice(), Self::Vega),
            (b".vega-lite.json".as_slice(), Self::VegaLite),
            (b".vegalite.json".as_slice(), Self::VegaLite),
            (b".vl.json".as_slice(), Self::VegaLite),
            (b".drawio.xml".as_slice(), Self::Drawio),
            (b".dio.xml".as_slice(), Self::Drawio),
            (b".wire.yaml".as_slice(), Self::Wireviz),
            (b".wire.yml".as_slice(), Self::Wireviz),
        ] {
            if name
                .get(name.len().saturating_sub(suffix.len())..)
                .is_some_and(|tail| tail.eq_ignore_ascii_case(suffix))
            {
                return Some(format);
            }
        }
        Self::from_extension(path.extension()?.to_str()?)
    }

    /// Keeps compound drawing names identifiable when a text viewer stores only
    /// an extension hint. Other formats retain their original suffix and aliases.
    pub fn source_extension_hint(path: &Path) -> Option<&str> {
        match Self::from_path(path) {
            Some(Self::Excalidraw) => Some("excalidraw"),
            Some(Self::Wireviz) => Some("wireviz"),
            Some(Self::Vega) => Some("vega"),
            Some(Self::VegaLite) => Some("vegalite"),
            Some(Self::Drawio) => Some(if path.extension()?.eq_ignore_ascii_case("dio") {
                "dio"
            } else {
                "drawio"
            }),
            _ => path.extension()?.to_str(),
        }
    }

    /// Recognizes extension candidates without treating fence aliases as paths.
    pub fn from_extension(extension: &str) -> Option<Self> {
        match extension.to_ascii_lowercase().as_str() {
            "pikchr" => Some(Self::Pikchr),
            "svgbob" => Some(Self::Svgbob),
            "mermaid" => Some(Self::Mermaid),
            "lini" => Some(Self::Lini),
            "gv" | "graphviz" => Some(Self::Graphviz),
            "d2" => Some(Self::D2),
            "bpmn" | "bpmn2" => Some(Self::Bpmn),
            "wavedrom" | "wavejson" => Some(Self::Wavedrom),
            "excalidraw" => Some(Self::Excalidraw),
            "wireviz" => Some(Self::Wireviz),
            "nomnoml" | "noml" => Some(Self::Nomnoml),
            "dbml" => Some(Self::Dbml),
            "goat" => Some(Self::Goat),
            "bytefield" => Some(Self::Bytefield),
            "erd" | "er" => Some(Self::Erd),
            "plantuml" | "puml" | "pu" | "iuml" => Some(Self::Plantuml),
            "drawio" | "dio" => Some(Self::Drawio),
            "structurizr" => Some(Self::Structurizr),
            "vega" => Some(Self::Vega),
            "vegalite" => Some(Self::VegaLite),
            _ => None,
        }
    }

    /// Uses content signatures for extensions shared with ordinary Markdown.
    pub fn from_source_path(path: &Path, source: &str) -> Option<Self> {
        if let Some(extension) = path.extension().and_then(|ext| ext.to_str())
            && matches!(
                extension.to_ascii_lowercase().as_str(),
                "dbml" | "erd" | "er" | "dio"
            )
        {
            return Self::from_source_extension(extension, source);
        }
        Self::from_path(path)
            .or_else(|| Self::from_source_extension(path.extension()?.to_str()?, source))
    }

    /// Combines unambiguous extensions with conservative content classification.
    pub fn from_source_extension(extension: &str, source: &str) -> Option<Self> {
        Self::from_extension(extension)
            .filter(|_| !extension.eq_ignore_ascii_case("dio") || drawio_signature(source))
            .filter(|format| {
                // Microsoft LINQ-to-SQL and ERMaster share DBML/Erd suffixes.
                // Incomplete XML drafts also remain in the ordinary XML viewer.
                !matches!(format, Self::Dbml | Self::Erd)
                    || !source
                        .trim_start_matches('\u{feff}')
                        .trim_start()
                        .starts_with('<')
            })
            .or_else(|| match extension.to_ascii_lowercase().as_str() {
                "dsl" => structurizr_signature(source).then_some(Self::Structurizr),
                "edn" => bytefield_signature(source).then_some(Self::Bytefield),
                "mmd" => mermaid_signature(source).then_some(Self::Mermaid),
                "dot" => dot_signature(source).then_some(Self::Graphviz),
                "xml" => {
                    if bpmn_signature(source) {
                        Some(Self::Bpmn)
                    } else {
                        drawio_signature(source).then_some(Self::Drawio)
                    }
                }
                "json" if excalidraw_signature(source) => Some(Self::Excalidraw),
                "json" => vega_format(source)
                    .or_else(|| wavejson::signature(source).then_some(Self::Wavedrom)),
                "json5" => wavejson::signature(source).then_some(Self::Wavedrom),
                "yaml" | "yml" => wireviz::signature(source).then_some(Self::Wireviz),
                _ => None,
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
            (Self::Wavedrom, wavejson::signature),
            (Self::Excalidraw, excalidraw_signature),
            (Self::Wireviz, wireviz::signature),
            (Self::Nomnoml, nomnoml_signature),
            (Self::Dbml, dbml_signature),
            (Self::Pikchr, pikchr_signature),
            (Self::Lini, lini_signature),
            (Self::D2, d2_signature),
            (Self::Svgbob, svgbob_signature),
            (Self::Goat, goat_signature),
            (Self::Bytefield, bytefield_signature),
            (Self::Erd, erd_signature),
            (Self::Plantuml, plantuml_signature),
            (Self::Drawio, drawio_signature),
            (Self::Structurizr, structurizr_signature),
        ];
        signatures
            .iter()
            .find_map(|(format, signature)| signature(source).then_some(*format))
            .or_else(|| vega_format(source))
    }

    /// Returns the preferred source extension, excluding inline math.
    ///
    /// Generic extensions such as JSON5 require content classification on reopen.
    pub const fn source_extension(self) -> Option<&'static str> {
        match self {
            Self::Pikchr => Some("pikchr"),
            Self::Svgbob => Some("svgbob"),
            Self::Mermaid => Some("mermaid"),
            Self::Lini => Some("lini"),
            Self::Graphviz => Some("gv"),
            Self::D2 => Some("d2"),
            Self::Bpmn => Some("bpmn"),
            Self::Wavedrom => Some("json5"),
            Self::Excalidraw => Some("excalidraw"),
            Self::Wireviz => Some("wireviz"),
            Self::Nomnoml => Some("nomnoml"),
            Self::Dbml => Some("dbml"),
            Self::Goat => Some("goat"),
            Self::Bytefield => Some("bytefield"),
            Self::Erd => Some("er"),
            Self::Plantuml => Some("puml"),
            Self::Drawio => Some("drawio"),
            Self::Structurizr => Some("structurizr"),
            Self::Vega => Some("vega"),
            Self::VegaLite => Some("vegalite"),
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
            Self::Wavedrom => "WaveDrom",
            Self::Excalidraw => "Excalidraw",
            Self::Wireviz => "WireViz",
            Self::Nomnoml => "Nomnoml",
            Self::Dbml => "DBML",
            Self::Goat => "GoAT",
            Self::Bytefield => "Bytefield",
            Self::Erd => "Erd",
            Self::Plantuml => "PlantUML",
            Self::Drawio => "draw.io",
            Self::Structurizr => "Structurizr",
            Self::Vega => "Vega",
            Self::VegaLite => "Vega-Lite",
            Self::Math | Self::MathInline => "LaTeX math",
        }
    }
}

fn vega_format(source: &str) -> Option<Format> {
    if source.len() > 256 * 1024 {
        return None;
    }
    let value: serde_json::Value =
        serde_json::from_str(source.trim_start_matches('\u{feff}')).ok()?;
    let object = value.as_object()?;
    if let Some(schema) = object.get("$schema").and_then(serde_json::Value::as_str) {
        let schema = schema
            .strip_prefix("https://")
            .or_else(|| schema.strip_prefix("http://"))?;
        return if schema.starts_with("vega.github.io/schema/vega-lite/") {
            Some(Format::VegaLite)
        } else if schema.starts_with("vega.github.io/schema/vega/") {
            Some(Format::Vega)
        } else {
            None
        };
    }
    if lite_shape(&value) {
        return Some(Format::VegaLite);
    }
    const MARKS: &[&str] = &[
        "arc", "area", "group", "image", "line", "path", "rect", "rule", "shape", "symbol", "text",
        "trail",
    ];
    object
        .get("marks")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|marks| {
            marks.iter().any(|mark| {
                mark.get("type")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|kind| MARKS.contains(&kind))
                    && mark.get("encode").is_some_and(serde_json::Value::is_object)
            })
        })
        .then_some(Format::Vega)
}

fn lite_shape(value: &serde_json::Value) -> bool {
    let encoding = value
        .get("encoding")
        .is_some_and(serde_json::Value::is_object);
    if encoding && value.get("mark").is_some_and(lite_mark) {
        return true;
    }
    // Classification is a bounded hint, not recursive schema validation.
    value
        .get("layer")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|layers| {
            layers.iter().any(|layer| {
                layer.get("mark").is_some_and(lite_mark)
                    && (encoding
                        || layer
                            .get("encoding")
                            .is_some_and(serde_json::Value::is_object))
            })
        })
}

fn lite_mark(value: &serde_json::Value) -> bool {
    const MARKS: &[&str] = &[
        "arc",
        "area",
        "bar",
        "boxplot",
        "circle",
        "errorband",
        "errorbar",
        "geoshape",
        "image",
        "line",
        "point",
        "rect",
        "rule",
        "square",
        "text",
        "tick",
        "trail",
    ];
    value
        .as_str()
        .or_else(|| value.get("type")?.as_str())
        .is_some_and(|kind| MARKS.contains(&kind))
}

fn structurizr_signature(source: &str) -> bool {
    static HEADER: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r#"(?s)^workspace(?:\s+"(?:\\.|[^"\\])*"){0,2}\s*\{"#)
            .expect("static Structurizr signature")
    });
    let Some(source) = dot_trivia(source.trim_start_matches('\u{feff}')) else {
        return false;
    };
    let Some(header) = HEADER.find(source) else {
        return false;
    };
    let Some(body) = dot_trivia(&source[header.end()..]) else {
        return false;
    };
    ["model", "views"].iter().any(|keyword| {
        body.strip_prefix(keyword)
            .and_then(dot_trivia)
            .is_some_and(|body| body.starts_with('{'))
    })
}

fn excalidraw_signature(source: &str) -> bool {
    if source.len() > 32 * 1024 {
        return false;
    }
    serde_json::from_str::<serde_json::Value>(source.trim_start_matches('\u{feff}')).is_ok_and(
        |value| {
            value.get("type").and_then(serde_json::Value::as_str) == Some("excalidraw")
                && value
                    .get("elements")
                    .is_some_and(serde_json::Value::is_array)
        },
    )
}

fn dbml_signature(source: &str) -> bool {
    if source.len() > 32 * 1024 {
        return false;
    }
    // The document itself must start as DBML, not merely contain a table in a
    // Markdown fence or quoted example. Generic enum declarations remain text.
    let first = source
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with("//"));
    let starts_dbml = first.is_some_and(|line| {
        let keyword = line.split_whitespace().next().unwrap_or_default();
        matches!(
            keyword.to_ascii_lowercase().as_str(),
            "table" | "project" | "enum" | "tablegroup" | "tablepartial"
        ) && line.contains('{')
    });
    starts_dbml
        && source.lines().take(32).any(|line| {
            let Some((keyword, rest)) = line.trim_start().split_once(char::is_whitespace) else {
                return false;
            };
            keyword.eq_ignore_ascii_case("table") && rest.contains('{')
        })
}

fn nomnoml_signature(source: &str) -> bool {
    if source.len() > 32 * 1024 {
        return false;
    }
    // Skip only source comments and compact #key: directives, preserving
    // ordinary Markdown headings and fences as document framing.
    let first = source
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with("//"))
        .find(|line| !line.starts_with('#') || line.starts_with("# ") || !line.contains(':'));
    first.is_some_and(|line| {
        line.starts_with('[')
            && (nomnoml_relation(line) || line.ends_with(']') || line.ends_with('|') || line == "[")
    }) && source.lines().any(nomnoml_relation)
}

fn nomnoml_relation(line: &str) -> bool {
    let Some((left, rest)) = line.split_once(']') else {
        return false;
    };
    if !left.trim_start().starts_with('[') {
        return false;
    }
    let Some((relation, right)) = rest.split_once('[') else {
        return false;
    };
    let relation = relation.trim();
    !relation.is_empty()
        && relation.contains('-')
        && relation.chars().all(|c| {
            matches!(
                c,
                '-' | '<' | '>' | ':' | 'o' | '+' | '_' | '(' | ')' | ' ' | '\t'
            )
        })
        && right.contains(']')
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

fn plantuml_signature(source: &str) -> bool {
    if source.len() > 32 * 1024 {
        return false;
    }
    let Some(first) = source
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with('\''))
    else {
        return false;
    };
    let start = first.split_whitespace().next().unwrap_or_default();
    let Some(kind) = start.strip_prefix("@start") else {
        return false;
    };
    matches!(
        kind,
        "uml"
            | "mindmap"
            | "wbs"
            | "gantt"
            | "json"
            | "yaml"
            | "salt"
            | "chen"
            | "ebnf"
            | "timing"
            | "chart"
            | "packet"
            | "hcl"
    ) && source
        .lines()
        .any(|line| line.trim() == format!("@end{kind}"))
}

fn erd_signature(source: &str) -> bool {
    // Bracket headings alone overlap INI and prose. A complete canonical
    // cardinality relationship provides a distinctive extensionless hint.
    source.len() <= 32 * 1024
        && source.contains("--")
        && erd::parse(source).is_ok_and(|document| !document.relations.is_empty())
}

fn bytefield_signature(source: &str) -> bool {
    if source.len() > 32 * 1024 {
        return false;
    }
    let source = source.trim_start_matches('\u{feff}').trim_start();
    let first = source
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with(';'));
    if !first.is_some_and(|line| line.starts_with('(')) {
        return false;
    }
    // Only actual list heads are hints. Mentions in strings, comments, prose,
    // Markdown or generic EDN maps must not claim a diagram preview.
    let (mut quoted, mut escaped, mut comment, mut drawing) = (false, false, false, false);
    let mut chars = source.chars().peekable();
    while let Some(c) = chars.next() {
        if comment {
            comment = c != '\n';
        } else if escaped {
            escaped = false;
        } else if quoted {
            match c {
                '\\' => escaped = true,
                '"' => quoted = false,
                _ => {}
            }
        } else {
            match c {
                ';' => comment = true,
                '"' => quoted = true,
                '(' => {
                    let name: String = chars
                        .by_ref()
                        .take_while(|c| !c.is_whitespace() && *c != ')')
                        .collect();
                    drawing |= matches!(
                        name.as_str(),
                        "draw-box"
                            | "draw-boxes"
                            | "draw-related-boxes"
                            | "draw-column-headers"
                            | "draw-gap"
                    );
                }
                _ => {}
            }
        }
    }
    drawing && !quoted
}

fn goat_signature(source: &str) -> bool {
    if source.len() > 32 * 1024 {
        return false;
    }
    // Shared square boxes retain the SvgBob hint. Closed rounded/Unicode
    // frames provide a conservative GoAT hint without claiming prose or fences.
    let lines = source
        .trim()
        .lines()
        .take(129)
        .map(str::trim)
        .collect::<Vec<_>>();
    if lines.len() < 3 || lines.len() > 128 {
        return false;
    }
    let top = lines[0];
    let bottom = lines[lines.len() - 1];
    let corners = (
        top.chars().next(),
        top.chars().last(),
        bottom.chars().next(),
        bottom.chars().last(),
    );
    let frame = match corners {
        (Some('.'), Some('.'), Some('\''), Some('\'')) => top.contains('-') && bottom.contains('-'),
        (Some('╭' | '┌'), Some('╮' | '┐'), Some('╰' | '└'), Some('╯' | '┘')) => {
            top.contains('─') && bottom.contains('─')
        }
        _ => false,
    };
    frame
        && lines
            .iter()
            .skip(1)
            .take(lines.len() - 2)
            .any(|line| line.contains('|') || line.contains('│'))
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
    xml_root_matches(
        source,
        Some(b"http://www.omg.org/spec/BPMN/20100524/MODEL"),
        &[b"definitions"],
    )
}

fn drawio_signature(source: &str) -> bool {
    xml_root_matches(source, None, &[b"mxfile", b"mxGraphModel"])
}

// Check the first real root only; a quoted/nested diagram is ordinary XML.
fn xml_root_matches(source: &str, namespace: Option<&[u8]>, names: &[&[u8]]) -> bool {
    use quick_xml::{events::Event, name::ResolveResult};
    let source = &source[..source.floor_char_boundary(source.len().min(32 * 1024))];
    let mut reader = quick_xml::NsReader::from_str(source.trim_start_matches('\u{feff}'));
    loop {
        match reader.read_resolved_event() {
            Ok((resolved, Event::Start(tag) | Event::Empty(tag))) => {
                let namespace_matches = match (namespace, resolved) {
                    (Some(expected), ResolveResult::Bound(actual)) => actual.as_ref() == expected,
                    (None, ResolveResult::Unbound) => true,
                    _ => false,
                };
                return namespace_matches && names.contains(&tag.local_name().as_ref());
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
    fn chart_schemas_compound_names_and_distinctive_clipboard_specs_share_routes() {
        let vega = include_str!("../../../docs/examples/diagrams/native-vega.vega.json");
        let lite = include_str!("../../../docs/examples/diagrams/native-vega-lite.vl.json");
        for (source, format, names) in [
            (
                vega,
                Format::Vega,
                vec!["chart.vega", "chart.vg.json", "chart.vega.json"],
            ),
            (
                lite,
                Format::VegaLite,
                vec!["chart.vegalite", "chart.vl.json", "chart.vega-lite.json"],
            ),
        ] {
            assert_eq!(Format::from_source_extension("json", source), Some(format));
            assert_eq!(Format::from_snippet(source), Some(format));
            for name in names {
                let path = Path::new(name);
                assert_eq!(Format::from_path(path), Some(format));
                assert_eq!(Format::from_source_path(path, source), Some(format));
                assert_eq!(
                    Format::from_source_extension(
                        Format::source_extension_hint(path).unwrap(),
                        "{"
                    ),
                    Some(format),
                    "malformed explicit charts retain editor identity"
                );
            }
        }
        assert_eq!(Format::from_language("vega-lite"), Some(Format::VegaLite));
        let implicit = r#"{"mark":"bar","encoding":{"x":{"field":"name","type":"nominal"}}}"#;
        assert_eq!(Format::from_snippet(implicit), Some(Format::VegaLite));
        for ordinary in [
            r#"{"mark":"John","encoding":"utf8"}"#,
            r#"{"layer":[{"mark":"reviewed","encoding":{}}]}"#,
            r#"{"$schema":"https://json-schema.org/schema","mark":"bar","encoding":{}}"#,
            "{",
        ] {
            assert_eq!(
                Format::from_source_extension("json", ordinary),
                None,
                "{ordinary}"
            );
        }
    }

    #[test]
    fn architecture_dsl_uses_content_hints_without_claiming_unrelated_dsl_or_json() {
        let source = include_str!("../../../docs/examples/diagrams/architecture.structurizr");
        for name in ["architecture.structurizr", "architecture.dsl"] {
            assert_eq!(
                Format::from_source_path(Path::new(name), source),
                Some(Format::Structurizr)
            );
        }
        assert_eq!(
            Format::from_language("structurizr"),
            Some(Format::Structurizr)
        );
        assert_eq!(Format::from_snippet(source), Some(Format::Structurizr));
        assert_eq!(
            Format::from_snippet(&format!("/* authored */\n{source}")),
            Some(Format::Structurizr)
        );
        assert_eq!(Format::from_extension("dsl"), None);
        assert_eq!(
            Format::from_source_extension("dsl", "workspace { projects { alpha } }"),
            None
        );
        assert_eq!(
            Format::from_snippet("workspace { projects { alpha } }"),
            None
        );
        assert_eq!(
            Format::from_source_extension("json", r#"{"workspace": {"model": {}}}"#),
            None
        );
        assert_eq!(Format::Structurizr.source_extension(), Some("structurizr"));
        assert_eq!(Format::Structurizr.name(), "Structurizr");
    }

    #[test]
    fn drawio_routes_explicit_files_and_only_distinctive_xml_roots() {
        let source =
            include_str!("../../../vendor/document-svg-drawio/tests/fixtures/basic.drawio");
        for name in [
            "drawing.drawio",
            "drawing.dio",
            "drawing.DRAWIO.XML",
            "drawing.dio.xml",
        ] {
            let path = std::path::Path::new(name);
            assert_eq!(Format::from_source_path(path, source), Some(Format::Drawio));
            let hint = if name == "drawing.dio" {
                "dio"
            } else {
                "drawio"
            };
            assert_eq!(Format::source_extension_hint(path), Some(hint));
        }
        for language in ["drawio", "diagramsnet"] {
            assert_eq!(Format::from_language(language), Some(Format::Drawio));
        }
        for source in [
            source,
            "<?xml version=\"1.0\"?><!-- test --><mxGraphModel><root/></mxGraphModel>",
        ] {
            assert_eq!(
                Format::from_source_extension("xml", source),
                Some(Format::Drawio)
            );
            assert_eq!(Format::from_snippet(source), Some(Format::Drawio));
        }
        for source in [
            "<ordinary><mxfile/></ordinary>",
            "text <mxfile/>",
            "<mxlibrary/>",
            "<mxfile xmlns=\"urn:unrelated\"/>",
            "<svg/>",
        ] {
            assert_eq!(Format::from_source_extension("xml", source), None);
            assert_eq!(Format::from_source_extension("dio", source), None);
            assert_eq!(Format::from_snippet(source), None);
        }
    }

    #[test]
    fn plantuml_aliases_and_complete_clipboard_blocks_share_the_native_format() {
        for language in ["plantuml", "puml", "c4plantuml"] {
            assert_eq!(Format::from_language(language), Some(Format::Plantuml));
        }
        for extension in ["plantuml", "puml", "pu", "iuml"] {
            assert_eq!(Format::from_extension(extension), Some(Format::Plantuml));
        }
        for kind in ["uml", "json", "mindmap", "salt"] {
            let source = format!("@start{kind}\ncontent\n@end{kind}");
            assert_eq!(Format::from_snippet(&source), Some(Format::Plantuml));
            assert_eq!(
                Format::from_snippet(&format!("```plantuml\n{source}\n```")),
                None
            );
        }
        for source in [
            "@startuml\nAlice -> Bob",
            "Alice -> Bob",
            "A prose note mentions @startuml",
            "@startumlSuffix\n@endumlSuffix",
        ] {
            assert_eq!(Format::from_snippet(source), None, "{source}");
        }
    }

    #[test]
    fn erd_routes_canonical_schemas_and_keeps_er_master_xml_and_markdown_separate() {
        let schema = "[Person]\n*name\n[Place]\n*id\nPerson *--1 Place\n";
        assert_eq!(Format::from_snippet(schema), Some(Format::Erd));
        assert_eq!(Format::from_language("erd"), Some(Format::Erd));
        for extension in ["er", "erd"] {
            assert_eq!(
                Format::from_source_extension(extension, schema),
                Some(Format::Erd)
            );
            assert_eq!(
                Format::from_source_extension(extension, "<?xml version=\"1.0\"?><diagram/>"),
                None
            );
            assert_eq!(
                Format::from_source_path(Path::new(&format!("eclipse.{extension}")), "<diagram>"),
                None
            );
        }
        for source in [
            "[Person]\nname",
            "[Section]\nkey=value",
            "# Title\n```erd\n[Person]\n```",
            "[A]\nA 1--1 Missing",
        ] {
            assert_eq!(Format::from_snippet(source), None, "{source}");
        }
    }

    #[test]
    fn bytefield_signatures_disambiguate_clojure_data_without_claiming_quoted_examples() {
        assert_eq!(Format::from_extension("bytefield"), Some(Format::Bytefield));
        assert_eq!(Format::from_extension("edn"), None);
        assert_eq!(Format::from_source_extension("clj", "(draw-box 0)"), None);
        assert_eq!(Format::from_language("bytefield"), Some(Format::Bytefield));
        for source in [
            "(draw-box 0)",
            ";; packet\n(def boxes-per-row 4)\n(draw-column-headers)",
            "(doseq [v (range 4)] (draw-box v))",
        ] {
            assert_eq!(
                Format::from_snippet(source),
                Some(Format::Bytefield),
                "{source}"
            );
            assert_eq!(
                Format::from_source_extension("edn", source),
                Some(Format::Bytefield)
            );
            assert_eq!(
                Format::from_source_path(Path::new("packet.bytefield"), source),
                Some(Format::Bytefield)
            );
        }
        for source in [
            "{:message \"(draw-box 0)\"}",
            ";; (draw-box 0)",
            "(println \"(draw-box 0)\")",
            "(def x 42)",
            "# Packet\n```bytefield\n(draw-box 0)\n```",
        ] {
            assert_eq!(Format::from_snippet(source), None, "{source}");
            assert_eq!(
                Format::from_source_extension("edn", source),
                None,
                "{source}"
            );
        }
    }

    #[test]
    fn goat_signatures_use_complete_frames_without_stealing_square_boxes_or_markdown() {
        assert_eq!(Format::from_extension("GOAT"), Some(Format::Goat));
        assert_eq!(Format::from_language("goat"), Some(Format::Goat));
        assert_eq!(Format::Goat.source_extension(), Some("goat"));
        for source in [".----.\n| hi |\n'----'", "╭────╮\n│ hi │\n╰────╯"]
        {
            assert_eq!(Format::from_snippet(source), Some(Format::Goat));
            assert_eq!(
                Format::from_snippet(&format!("```goat\n{source}\n```")),
                None
            );
        }
        assert_eq!(
            Format::from_snippet("+----+\n| hi |\n+----+"),
            Some(Format::Svgbob)
        );
        for source in [".----.", "...\nwords\n'---'", "ordinary text"] {
            assert_eq!(Format::from_snippet(source), None);
        }
    }

    #[test]
    fn fenced_markdown_keeps_its_document_route_instead_of_a_raw_diagram_hint() {
        for (language, body) in [
            ("nomnoml", "[Source] -> [Preview]"),
            ("dbml", "Table users { id int [pk] }"),
        ] {
            for prefix in [
                "",
                "# Schema overview\n\n",
                "Here is the design.\n\n",
                "[Reference](https://example.com)\n\n",
                "---\ntitle: Schema\n---\n\n",
            ] {
                let source = format!("{prefix}```{language}\n{body}\n```\n");
                assert_eq!(Format::from_snippet(&source), None, "{source}");
                assert!(
                    matches!(crate::ClipItem::from_text(source.clone()).payload, crate::ClipPayload::Markdown(text) if text == source)
                );
            }
        }
    }

    #[test]
    fn dbml_files_fences_and_table_snippets_do_not_claim_generic_enum_source() {
        assert_eq!(Format::from_extension("DBML"), Some(Format::Dbml));
        assert_eq!(Format::from_language("dbml"), Some(Format::Dbml));
        assert_eq!(Format::Dbml.source_extension(), Some("dbml"));
        assert_eq!(
            Format::from_snippet(
                "// Schema\nProject sample { database_type: 'PostgreSQL' }\nTable users { id int }"
            ),
            Some(Format::Dbml)
        );

        let mapping = "\u{feff} <?xml version='1.0'?><Database xmlns='http://schemas.microsoft.com/linqtosql/dbml/2007'/>";
        assert_eq!(
            Format::from_source_path(Path::new("legacy.DBML"), mapping),
            None
        );
        assert_eq!(Format::from_source_extension("dbml", "<Database"), None);
        assert_eq!(crate::syntax::detect_language(mapping, Some("dbml")), "xml");
        assert_eq!(
            Format::from_source_path(Path::new("draft.dbml"), "Table broken {"),
            Some(Format::Dbml)
        );

        assert_eq!(
            Format::from_snippet("// schema\nTable users { id int [pk] }"),
            Some(Format::Dbml)
        );
        for text in [
            "enum Color { Red }",
            "Table of contents",
            "table users",
            "CREATE TABLE users (id int)",
        ] {
            assert_eq!(Format::from_snippet(text), None, "{text}");
        }
    }

    #[test]
    fn nomnoml_names_and_distinctive_relations_route_without_claiming_markdown_links() {
        for extension in ["nomnoml", "noml", "NOMNOML"] {
            assert_eq!(Format::from_extension(extension), Some(Format::Nomnoml));
            assert_eq!(Format::from_language(extension), Some(Format::Nomnoml));
        }
        assert_eq!(Format::Nomnoml.source_extension(), Some("nomnoml"));
        assert_eq!(
            Format::from_snippet("// Figure\n#direction: right\n[Source] -> [Preview]"),
            Some(Format::Nomnoml)
        );

        for source in [
            "[Source] -> [Preview]",
            "// example\n[<actor>User] -:> [System]",
        ] {
            assert_eq!(Format::from_snippet(source), Some(Format::Nomnoml));
        }
        for source in [
            "[text](url) -> [other](url)",
            "- [x] a task",
            "[single label]",
            "// [A] -> [B]",
            "#direction: right",
        ] {
            assert_eq!(Format::from_snippet(source), None, "{source}");
        }
    }

    #[test]
    fn drawing_signatures_do_not_claim_ordinary_json_or_incomplete_clipboard_data() {
        let source = r#"{"type":"excalidraw","version":2,"elements":[]}"#;
        assert_eq!(
            Format::from_source_extension("json", source),
            Some(Format::Excalidraw)
        );
        assert_eq!(Format::from_snippet(source), Some(Format::Excalidraw));
        for name in [
            "drawing.excalidraw",
            "drawing.excalidraw.json",
            "drawing.EXCALIDRAW.JSON",
        ] {
            let path = Path::new(name);
            assert_eq!(Format::source_extension_hint(path), Some("excalidraw"));
            assert_eq!(
                Format::from_source_path(path, "invalid draft"),
                Some(Format::Excalidraw)
            );
        }
        assert_eq!(
            Format::source_extension_hint(Path::new("plain.json")),
            Some("json")
        );
        assert_eq!(
            Format::source_extension_hint(Path::new("scope.wavejson")),
            Some("wavejson")
        );

        assert_eq!(
            Format::from_extension("excalidraw"),
            Some(Format::Excalidraw)
        );
        assert_eq!(
            Format::from_language("excalidraw"),
            Some(Format::Excalidraw)
        );
        for text in [
            r#"{"type":"excalidraw","elements":"ordinary value"}"#,
            r#"{"type":"excalidraw""#,
            r#"{"elements":[]}"#,
        ] {
            assert_eq!(Format::from_snippet(text), None);
        }
    }

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
