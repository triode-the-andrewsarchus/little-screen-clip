//! Bounded Erd schema parsing, retaining the authored token spans.
//!
//! Follows the original Unlicense grammar retained in `vendor/erd-reference`.
//! This is the bracket/entity `.er` language, not Mermaid or rusterd's DSL.
use std::{
    collections::{BTreeMap, HashSet},
    ops::Range,
};

/// An authored identifier or setting, including its complete source token.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Token {
    pub text: String,
    pub span: Range<usize>,
}

/// Explicit settings. Missing entries inherit from the relevant global scope.
pub type Options = BTreeMap<String, Token>;

/// A schema attribute with independent primary/foreign-key flags.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attribute {
    pub name: Token,
    pub primary: bool,
    pub foreign: bool,
    pub options: Options,
}

/// An entity and its attributes, in authored order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entity {
    pub name: Token,
    pub options: Options,
    pub attributes: Vec<Attribute>,
}

/// Participation at one end of a relationship.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cardinality {
    ZeroOrOne,
    One,
    ZeroOrMore,
    OneOrMore,
}
impl Cardinality {
    fn parse(value: char) -> Option<Self> {
        match value {
            '?' => Some(Self::ZeroOrOne),
            '1' => Some(Self::One),
            '*' => Some(Self::ZeroOrMore),
            '+' => Some(Self::OneOrMore),
            _ => None,
        }
    }

    /// Returns the canonical UML cardinality label, preserving minimums.
    pub const fn label(self) -> &'static str {
        match self {
            Self::ZeroOrOne => "{0,1}",
            Self::One => "1",
            Self::ZeroOrMore => "0..N",
            Self::OneOrMore => "1..N",
        }
    }
}

/// A relationship, which may precede either of its entity declarations.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Relation {
    pub left: Token,
    pub right: Token,
    pub left_cardinality: Cardinality,
    pub right_cardinality: Cardinality,
    pub options: Options,
}

/// The complete schema; local options remain separate from inherited defaults.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Document {
    pub entities: Vec<Entity>,
    pub relations: Vec<Relation>,
    pub title: Options,
    pub header: Options,
    pub entity: Options,
    pub relationship: Options,
}

/// Parses the complete schema without resolving files, URLs or configuration.
///
/// Caps source bytes, entity/attribute/relationship counts and setting sizes.
/// Invalid syntax, unknown entities and duplicate entity declarations return
/// line/column diagnostics instead of silently rendering a partial schema.
pub fn parse(source: &str) -> Result<Document, String> {
    if source.len() > 256 * 1024 || source.contains('\0') {
        return Err("Erd source exceeds 256 KiB or contains a NUL byte".into());
    }
    let mut reader = Reader {
        source,
        offset: usize::from(source.starts_with('\u{feff}')) * 3,
    };
    let mut document = Document::default();
    let mut attributes = 0;
    while reader.trivia() {
        reader.statement(&mut document, &mut attributes)?;
        if document.entities.len() > 128 || document.relations.len() > 2048 || attributes > 4096 {
            return Err(reader.error("Schema exceeds the entity, attribute or relationship budget"));
        }
    }
    if document.entities.is_empty() {
        return Err(reader.error("Schema requires at least one entity declaration"));
    }
    let mut names = HashSet::new();
    for entity in &document.entities {
        if !names.insert(entity.name.text.as_str()) {
            reader.offset = entity.name.span.start;
            return Err(reader.error("Duplicate entity declaration"));
        }
    }
    for relation in &document.relations {
        for name in [&relation.left, &relation.right] {
            if !names.contains(name.text.as_str()) {
                reader.offset = name.span.start;
                return Err(reader.error(&format!("Unknown relationship entity: {}", name.text)));
            }
        }
    }
    Ok(document)
}

struct Reader<'a> {
    source: &'a str,
    offset: usize,
}
impl Reader<'_> {
    fn rest(&self) -> &str {
        &self.source[self.offset..]
    }
    fn peek(&self) -> Option<char> {
        self.rest().chars().next()
    }
    fn take(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.offset += c.len_utf8();
        Some(c)
    }
    fn error(&self, message: &str) -> String {
        // Erd accepts LF, CRLF and classic CR line endings. Normalize only
        // this bounded diagnostic copy; authored bytes and token spans stay exact.
        let normalized = self.source[..self.offset]
            .replace("\r\n", "\n")
            .replace('\r', "\n");
        let before = normalized.as_str();
        let line = before.bytes().filter(|c| *c == b'\n').count() + 1;
        let column = before
            .rsplit_once('\n')
            .map_or(before, |(_, tail)| tail)
            .chars()
            .count()
            + 1;
        format!("Erd line {line}, column {column}: {message}")
    }
    fn expect(&mut self, expected: char) -> Result<(), String> {
        if self.peek() != Some(expected) {
            return Err(self.error(&format!("Expected '{expected}'")));
        }
        self.take();
        Ok(())
    }
    fn horizontal(&mut self) {
        while self
            .peek()
            .is_some_and(|c| c.is_whitespace() && !matches!(c, '\n' | '\r'))
        {
            self.take();
        }
    }
    fn trivia(&mut self) -> bool {
        loop {
            while self.peek().is_some_and(char::is_whitespace) {
                self.take();
            }
            if self.peek() != Some('#') {
                return self.peek().is_some();
            }
            while self.peek().is_some_and(|c| !matches!(c, '\n' | '\r')) {
                self.take();
            }
        }
    }
    fn end_line(&mut self) -> Result<(), String> {
        self.horizontal();
        if self.peek() == Some('#') {
            while self.peek().is_some_and(|c| !matches!(c, '\n' | '\r')) {
                self.take();
            }
        }
        if self.peek().is_some_and(|c| !matches!(c, '\n' | '\r')) {
            return Err(self.error("Unexpected text after the declaration"));
        }
        Ok(())
    }
    fn quoted(&mut self, quote: char, multiline: bool) -> Result<Token, String> {
        let start = self.offset;
        self.expect(quote)?;
        let contents = self.offset;
        while self.peek().is_some_and(|c| c != quote) {
            if !multiline && self.peek().is_some_and(char::is_control) {
                return Err(self.error("Quoted names cannot contain control characters"));
            }
            self.take();
        }
        let text = self.source[contents..self.offset].to_owned();
        self.expect(quote)?;
        Ok(Token {
            text,
            span: start..self.offset,
        })
    }
    fn name(&mut self) -> Result<Token, String> {
        self.horizontal();
        let start = self.offset;
        let token = match self.peek() {
            Some(quote @ ('\'' | '"' | '`')) => self.quoted(quote, false)?,
            _ => {
                while self.peek().is_some_and(|c| c.is_alphanumeric() || c == '_') {
                    self.take();
                }
                Token {
                    text: self.source[start..self.offset].into(),
                    span: start..self.offset,
                }
            }
        };
        if token.text.is_empty() || token.text.len() > 4096 {
            return Err(self.error("Expected a nonempty name of at most 4096 bytes"));
        }
        self.horizontal();
        Ok(token)
    }
    fn options(&mut self) -> Result<Options, String> {
        self.horizontal();
        let mut options = Options::new();
        if self.peek() != Some('{') {
            return Ok(options);
        }
        self.take();
        self.trivia();
        while self.peek() != Some('}') {
            let start = self.offset;
            while self.peek().is_some_and(|c| c.is_alphabetic() || c == '-') {
                self.take();
            }
            let key = self.source[start..self.offset].to_owned();
            self.horizontal();
            self.expect(':')?;
            self.trivia();
            let value = self.quoted('"', true)?;
            validate_option(&key, &value.text).map_err(|message| self.error(&message))?;
            options.insert(key, value);
            self.trivia();
            if self.peek() == Some('}') {
                break;
            }
            self.expect(',')?;
            self.trivia();
        }
        self.expect('}')?;
        Ok(options)
    }
    fn statement(&mut self, doc: &mut Document, attributes: &mut usize) -> Result<(), String> {
        if self.peek() == Some('[') {
            self.take();
            let name = self.name()?;
            self.expect(']')?;
            let options = self.options()?;
            doc.entities.push(Entity {
                name,
                options,
                attributes: Vec::new(),
            });
        } else {
            self.item(doc, attributes)?;
        }
        self.end_line()
    }
    fn item(&mut self, doc: &mut Document, attributes: &mut usize) -> Result<(), String> {
        let (mut primary, mut foreign) = (false, false);
        while matches!(self.peek(), Some('*' | '+')) {
            primary |= self.peek() == Some('*');
            foreign |= self.peek() == Some('+');
            self.take();
            self.horizontal();
        }
        let name = self.name()?;
        if !primary && !foreign {
            if let Some(cardinality) = self.peek().and_then(Cardinality::parse) {
                self.take();
                self.expect('-')?;
                self.expect('-')?;
                let other = self
                    .peek()
                    .and_then(Cardinality::parse)
                    .ok_or_else(|| self.error("Expected a relationship cardinality (? 1 * +)"))?;
                self.take();
                let right = self.name()?;
                let options = self.options()?;
                doc.relations.push(Relation {
                    left: name,
                    right,
                    left_cardinality: cardinality,
                    right_cardinality: other,
                    options,
                });
                return Ok(());
            }
            if doc.entities.is_empty() && doc.relations.is_empty() {
                let target = match name.text.as_str() {
                    "title" => Some(&mut doc.title),
                    "header" => Some(&mut doc.header),
                    "entity" => Some(&mut doc.entity),
                    "relationship" => Some(&mut doc.relationship),
                    _ => None,
                };
                if let Some(target) = target {
                    // Repeated global directives replace their scope, as upstream.
                    *target = self.options()?;
                    return Ok(());
                }
            }
        }
        let options = self.options()?;
        let entity = doc
            .entities
            .last_mut()
            .ok_or_else(|| self.error("Attribute precedes its entity declaration"))?;
        entity.attributes.push(Attribute {
            name,
            primary,
            foreign,
            options,
        });
        *attributes += 1;
        Ok(())
    }
}

fn validate_option(key: &str, value: &str) -> Result<(), String> {
    if value.len() > 8192 {
        return Err("Setting exceeds the 8192-byte budget".into());
    }
    let valid = match key {
        "label" | "color" | "bgcolor" | "font" | "border-color" => true,
        "size" => value
            .parse::<f64>()
            .is_ok_and(|v| v.is_finite() && v > 0.0 && v <= 512.0),
        "border" | "cellspacing" | "cellborder" | "cellpadding" => value.parse::<u8>().is_ok(),
        "text-alignment" => matches!(
            value.to_ascii_uppercase().as_str(),
            "LEFT" | "RIGHT" | "CENTER" | "TEXT"
        ),
        _ => return Err(format!("Unknown setting: {key}")),
    };
    if !valid {
        return Err(format!("Invalid or excessive value for {key}: {value}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn original_examples_preserve_entities_keys_and_relations() {
        let simple = parse(include_str!(
            "../../../../vendor/erd-reference/examples/simple.er"
        ))
        .unwrap();
        assert_eq!(simple.entities.len(), 2);
        assert_eq!(simple.relations.len(), 1);
        assert_eq!(simple.entities[0].attributes.len(), 5);
        assert!(simple.entities[0].attributes[0].primary);
        assert!(simple.entities[0].attributes[4].foreign);
        assert_eq!(simple.entities[1].name.text, "Birth Place");
        let large = parse(include_str!(
            "../../../../vendor/erd-reference/examples/nfldb.er"
        ))
        .unwrap();
        assert_eq!(large.entities.len(), 7);
        assert_eq!(large.relations.len(), 13);
    }

    #[test]
    fn preserves_quoted_unicode_names_multiline_options_source_ranges_and_local_settings() {
        let source = "entity {bgcolor: \"red\"}\r\n[`人 <&>`] {bgcolor: \"blue\", border-color: \"green\"}\r\n*+\"key field\" {size: \"12.5\",\ntext-alignment: \"RIGHT\", # trailing comma\n}\n";
        let doc = parse(source).unwrap();
        let entity = &doc.entities[0];
        assert_eq!(&source[entity.name.span.clone()], "`人 <&>`");
        assert_eq!(entity.options["bgcolor"].text, "blue");
        assert_eq!(doc.entity["bgcolor"].text, "red");
        let field = &entity.attributes[0];
        assert!(field.primary && field.foreign);
        assert_eq!(&source[field.options["size"].span.clone()], "\"12.5\"");
    }

    #[test]
    fn relations_may_precede_entities_or_separate_attributes_and_keep_all_cardinalities() {
        for (op, card) in [
            ('?', Cardinality::ZeroOrOne),
            ('1', Cardinality::One),
            ('*', Cardinality::ZeroOrMore),
            ('+', Cardinality::OneOrMore),
        ] {
            let doc = parse(&format!("A {op}--1 A\n[A]\na\nA 1--{op} A\nb\n")).unwrap();
            assert_eq!(doc.entities[0].attributes.len(), 2);
            assert_eq!(doc.relations[0].left_cardinality, card);
            assert_eq!(doc.relations[1].right_cardinality, card);
        }
    }

    #[test]
    fn parse_diagnostics_count_all_supported_line_endings() {
        for ending in ["\n", "\r\n", "\r"] {
            let error = parse(&format!("[A]{ending}unquoted field")).unwrap_err();
            assert!(error.contains("line 2, column 10"), "{error}");
        }
    }

    #[test]
    fn rejects_partial_unknown_ambiguous_or_excessive_documents_with_locations() {
        for (source, message) in [
            ("[A", "Expected ']'"),
            ("orphan\n[A]", "precedes"),
            ("[A]\n[A]", "Duplicate"),
            ("[A]\nA 1--1 Missing", "Unknown relationship"),
            ("[A] {size: \"NaN\"}", "Invalid"),
            ("[A] {size: \"999999\"}", "Invalid"),
            ("[A] {image: \"/etc/passwd\"}", "Unknown setting"),
            ("[A] ignored", "Unexpected text"),
            ("[A]\n\"unterminated", "Expected"),
        ] {
            let error = parse(source).unwrap_err();
            assert!(
                error.contains(message) && error.contains("line") && error.contains("column"),
                "{source}: {error}"
            );
        }
        assert!(parse(&"[A]\n".repeat(129)).unwrap_err().contains("budget"));
        assert!(parse(&format!("[A]\n{}", "x".repeat(256 * 1024))).is_err());
    }
}
