//! Erd's native table/relationship presentation, using the existing DOT engine.
use super::graphviz::label;
use little_clip_core::diagram::erd::{self, Document, Options};
use std::{collections::HashMap, fmt::Write};

struct Style<'a> {
    local: &'a Options,
    global: &'a Options,
}
impl Style<'_> {
    fn get(&self, key: &str) -> Option<&str> {
        self.local
            .get(key)
            .or_else(|| self.global.get(key))
            .map(|v| v.text.as_str())
    }
    fn attributes(&self, attributes: &[(&str, &str, &str)]) -> String {
        let mut result = String::new();
        for (key, attribute, default) in attributes {
            let value = self.get(key).unwrap_or(default);
            if !value.is_empty() {
                write!(
                    result,
                    " {attribute}=\"{}\"",
                    quick_xml::escape::escape(value)
                )
                .unwrap();
            }
        }
        result
    }
    fn text(&self, text: &str) -> String {
        format!(
            "<FONT{}>{}</FONT>",
            self.attributes(&[
                ("font", "FACE", ""),
                ("color", "COLOR", ""),
                ("size", "POINT-SIZE", "")
            ]),
            label(text)
        )
    }
    fn suffix(&self) -> String {
        self.get("label")
            .map(|text| self.text(&format!(" [{text}]")))
            .unwrap_or_default()
    }
}

pub(super) fn svg(source: &str) -> Result<(String, Vec<String>), String> {
    let document = erd::parse(source)?;
    super::graphviz::svg(&dot(&document)?).map_err(|error| format!("Erd layout: {error}"))
}

fn dot(document: &Document) -> Result<String, String> {
    let empty = Options::new();
    let mut dot = String::from(
        "digraph Erd { graph [rankdir=LR bgcolor=white]; node [shape=plaintext]; edge [color=gray50 minlen=2 style=dashed dir=none];\n",
    );
    if let Some(title) = document.title.get("label") {
        let style = Style {
            local: &document.title,
            global: &empty,
        };
        let font = style.attributes(&[
            ("font", "FACE", ""),
            ("color", "COLOR", ""),
            ("size", "POINT-SIZE", "30"),
        ]);
        writeln!(
            dot,
            "graph [label=<<FONT{font}>{}</FONT>> labelloc=t labeljust=l];",
            label(&title.text)
        )
        .unwrap();
    }
    let mut names = HashMap::new();
    for (index, entity) in document.entities.iter().enumerate() {
        names.insert(entity.name.text.as_str(), index);
        entity_node(&mut dot, document, index, entity);
    }
    for (index, relation) in document.relations.iter().enumerate() {
        let a = names
            .get(relation.left.text.as_str())
            .ok_or("Unknown Erd relationship source")?;
        let b = names
            .get(relation.right.text.as_str())
            .ok_or("Unknown Erd relationship target")?;
        let style = Style {
            local: &relation.options,
            global: &document.relationship,
        };
        write!(
            dot,
            "e{a} -> e{b} [id=\"erd-relation-{index}\" taillabel=<{}> headlabel=<{}>",
            style.text(relation.left_cardinality.label()),
            style.text(relation.right_cardinality.label())
        )
        .unwrap();
        if let Some(text) = style.get("label") {
            write!(dot, " label=<{}>", style.text(text)).unwrap();
        }
        dot.push_str("];\n");
    }
    dot.push('}');
    if dot.len() > 1024 * 1024 {
        return Err("Erd generated diagram exceeds the byte budget".into());
    }
    Ok(dot)
}

fn entity_node(dot: &mut String, document: &Document, index: usize, entity: &erd::Entity) {
    let style = Style {
        local: &entity.options,
        global: &document.entity,
    };
    let header = Style {
        local: &entity.options,
        global: &document.header,
    };
    let font = style.attributes(&[
        ("font", "FACE", "Helvetica"),
        ("color", "COLOR", ""),
        ("size", "POINT-SIZE", ""),
    ]);
    let table = style.attributes(&[
        ("bgcolor", "BGCOLOR", ""),
        ("border-color", "COLOR", ""),
        ("border", "BORDER", "0"),
        ("cellborder", "CELLBORDER", "1"),
        ("cellspacing", "CELLSPACING", "0"),
        ("cellpadding", "CELLPADDING", "4"),
        ("text-alignment", "ALIGN", ""),
    ]);
    let header_font = header.attributes(&[
        ("font", "FACE", ""),
        ("color", "COLOR", ""),
        ("size", "POINT-SIZE", "16"),
    ]);
    write!(dot, "e{index} [id=\"erd-entity-{index}\" label=<<FONT{font}><TABLE{table}><TR><TD><B><FONT{header_font}>{}</FONT></B>{}</TD></TR>", label(&entity.name.text), header.suffix()).unwrap();
    let empty = Options::new();
    for attribute in &entity.attributes {
        let field = Style {
            local: &attribute.options,
            global: &empty,
        };
        let cell = field.attributes(&[
            ("bgcolor", "BGCOLOR", ""),
            ("border-color", "COLOR", ""),
            ("border", "BORDER", ""),
            ("cellpadding", "CELLPADDING", ""),
            ("text-alignment", "ALIGN", "LEFT"),
        ]);
        let mut name = field.text(&attribute.name.text);
        if attribute.primary {
            name = format!("<U>{name}</U>");
        }
        if attribute.foreign {
            name = format!("<I>{name}</I>");
        }
        write!(dot, "<TR><TD{cell}>{name}{}</TD></TR>", field.suffix()).unwrap();
    }
    dot.push_str("</TABLE></FONT>>];\n");
}

#[cfg(test)]
mod tests {
    use super::*;
    use little_clip_core::diagram::Format;

    #[test]
    fn original_erd_fixtures_render_as_outlined_tables_and_relationships() {
        for source in [
            include_str!("../../../../vendor/erd-reference/examples/simple.er"),
            include_str!("../../../../vendor/erd-reference/examples/nfldb.er"),
        ] {
            let rendered = super::super::render_inline(Format::Erd, source, None).unwrap();
            assert!(rendered.png.len() > 500);
            assert!(rendered.svg.contains("<path"));
            assert!(rendered.svg.contains("erd-entity-0"));
            assert!(rendered.svg.contains("erd-relation-0"));
            assert!(!rendered.svg.contains("<text"));
            assert!(rendered.warnings.is_empty(), "{:?}", rendered.warnings);
        }
    }

    #[test]
    fn styles_key_formatting_and_all_cardinality_minimums_survive_native_layout() {
        let source = "entity {bgcolor: \"red\", size: \"12.5\"}\nheader {color: \"blue\"}\n[A] {bgcolor: \"#b8d8ec\"}\n*+id {label: \"int <&>\", border-color: \"green\"}\n[B]\nid\nA ?--+ B {label: \"owns\", color: \"purple\"}\n";
        let dot = dot(&erd::parse(source).unwrap()).unwrap();
        assert!(dot.contains("BGCOLOR=\"#b8d8ec\""));
        assert!(dot.contains("BGCOLOR=\"red\""));
        assert!(dot.contains("<I><U><FONT>id</FONT></U></I>"));
        assert!(dot.contains("int &lt;&amp;&gt;"));
        assert!(dot.contains("{0,1}"));
        assert!(dot.contains("1..N"));
        let (svg, warnings) = svg(source).unwrap();
        assert!(svg.contains("#b8d8ec"));
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    #[test]
    fn authored_names_and_styles_cannot_inject_dot_html_or_external_resources() {
        let source = "[`<IMG SRC='/etc/passwd'/>`]\n\"</FONT><IMG SRC='/etc/passwd'/>\"\n";
        let dot = dot(&erd::parse(source).unwrap()).unwrap();
        assert!(!dot.contains("<IMG"));
        assert!(dot.contains("&lt;IMG"));
        let (rendered, _) = svg(source).unwrap();
        assert!(!rendered.contains("<image"));
        assert!(!rendered.contains("/etc/passwd"));
        // A malformed family can also make the upstream SVG writer reject
        // its output. The shared SVG validator must refuse that artifact.
        assert!(svg("[A] {font: \"x'> <IMG SRC='/etc/passwd'>\"}\nid").is_err());
        assert!(svg("[A]\nA 1--1 Missing").unwrap_err().contains("Unknown"));
    }
}
