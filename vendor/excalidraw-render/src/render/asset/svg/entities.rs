//! Internal DTD general entities, substituted before the sanitizer parses the
//! document, as the reference's XML parser (libxml2) substitutes them.
//!
//! - An internal entity (`<!ENTITY name "value">`) expands wherever it is
//!   referenced: in content its replacement text is parsed as markup, so an
//!   entity can contribute elements; in an attribute value it is text, where a
//!   `<` is not well-formed and a quote is escaped rather than closing the
//!   value. Character references in the literal are replaced at declaration,
//!   general references at use, recursively. The first declaration wins.
//! - An external entity (`SYSTEM`/`PUBLIC`) is never fetched. Its reference is
//!   left in place, and the later DTD pass removes and reports it.
//! - An undeclared entity rejects the document, as the reference does, unless
//!   the document may declare it where the reference does not look (it has an
//!   external subset or parameter-entity references and is not standalone);
//!   then it is removed and reported like an external entity.
//! - A reference loop, nesting deeper than [`MAX_DEPTH`], or a malformed
//!   declaration rejects the document.
//!
//! Expansion is bounded deterministically, in the spirit of libxml2's
//! amplification check: every expanded reference costs [`FIXED_COST`] plus
//! every byte it produces (nested references included), and the document is
//! rejected once the total exceeds the larger of [`ALLOWED_EXPANSION`] bytes
//! and [`AMPLIFICATION`] times its source length, but never more than the
//! per-asset byte ceiling. The DTD itself is dropped afterwards.
use std::borrow::Cow;
use std::collections::BTreeMap;

use super::super::ResolveError;

/// Expansion admitted regardless of the source length.
pub(super) const ALLOWED_EXPANSION: u64 = 1_000_000;
/// Expansion admitted per source byte beyond [`ALLOWED_EXPANSION`].
pub(super) const AMPLIFICATION: u64 = 5;
/// Cost of each expanded reference, however short its replacement.
pub(super) const FIXED_COST: u64 = 20;
/// Deepest admitted entity nesting.
pub(super) const MAX_DEPTH: u32 = 40;

enum Entity {
    Internal(String),
    External,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// The document itself: markup, including its DOCTYPE.
    Document,
    /// An entity replacement referenced from content: balanced markup.
    Content,
    /// An entity replacement referenced from an attribute value delimited by
    /// this quote.
    Attribute(char),
}

struct Expander {
    entities: BTreeMap<String, Entity>,
    /// Undeclared entities are removed rather than rejected.
    lenient: bool,
    stack: Vec<String>,
    cost: u64,
    cap: u64,
}

/// Expands the internal general entities of `text` (see the module docs).
/// Returns the text unchanged when it references no entity.
pub(super) fn expand(text: &str, max_asset_bytes: u64) -> Result<Cow<'_, str>, ResolveError> {
    if !text.contains('&') {
        return Ok(Cow::Borrowed(text));
    }
    let (entities, lenient) = declarations(text)?;
    let source = u64::try_from(text.len()).unwrap_or(u64::MAX);
    let mut expander = Expander {
        entities,
        lenient,
        stack: Vec::new(),
        cost: 0,
        cap: ALLOWED_EXPANSION
            .max(source.saturating_mul(AMPLIFICATION))
            .min(max_asset_bytes),
    };
    let mut output = String::new();
    output
        .try_reserve(text.len())
        .map_err(|_| ResolveError::Memory)?;
    expander.scan(text, &mut output, Mode::Document, 0)?;
    Ok(Cow::Owned(output))
}

impl Expander {
    fn emit(&mut self, output: &mut String, text: &str, depth: u32) -> Result<(), ResolveError> {
        if depth > 0 {
            self.cost = self
                .cost
                .saturating_add(u64::try_from(text.len()).unwrap_or(u64::MAX));
            if self.cost > self.cap {
                return Err(ResolveError::SvgDecodeFailed);
            }
        }
        output
            .try_reserve(text.len())
            .map_err(|_| ResolveError::Memory)?;
        output.push_str(text);
        Ok(())
    }

    fn scan(
        &mut self,
        text: &str,
        output: &mut String,
        mode: Mode,
        depth: u32,
    ) -> Result<(), ResolveError> {
        let (mut cursor, mut in_tag, mut quote) = (0, false, None);
        while cursor < text.len() {
            let rest = &text[cursor..];
            if let Mode::Attribute(delimiter) = mode {
                let ch = rest.chars().next().ok_or(ResolveError::SvgDecodeFailed)?;
                if ch == '&' {
                    if let Some(end) = reference_end(rest) {
                        self.reference(&rest[1..end], output, mode, depth)?;
                        cursor += end + 1;
                        continue;
                    }
                }
                match ch {
                    // Not well-formed in an attribute value.
                    '<' => return Err(ResolveError::SvgDecodeFailed),
                    '"' if delimiter == '"' => self.emit(output, "&quot;", depth)?,
                    '\'' if delimiter == '\'' => self.emit(output, "&apos;", depth)?,
                    _ => self.emit(output, &rest[..ch.len_utf8()], depth)?,
                }
                cursor += ch.len_utf8();
                continue;
            }
            if !in_tag {
                let opaque = [("<!--", "-->"), ("<![CDATA[", "]]>"), ("<?", "?>")]
                    .into_iter()
                    .find(|(open, _)| rest.starts_with(open));
                if let Some((open, close)) = opaque {
                    let Some(end) = rest[open.len()..].find(close) else {
                        if mode == Mode::Document {
                            // Left for the parser to reject.
                            self.emit(output, rest, depth)?;
                            return Ok(());
                        }
                        return Err(ResolveError::SvgDecodeFailed);
                    };
                    let end = open.len() + end + close.len();
                    self.emit(output, &rest[..end], depth)?;
                    cursor += end;
                    continue;
                }
                if mode == Mode::Document && rest.starts_with("<!DOCTYPE") {
                    let end = super::doctype_end(rest).ok_or(ResolveError::SvgDecodeFailed)?;
                    self.emit(output, &rest[..end], depth)?;
                    cursor += end;
                    continue;
                }
            }
            if rest.starts_with('&') && (!in_tag || quote.is_some()) {
                if let Some(end) = reference_end(rest) {
                    let mode = match quote {
                        Some(delimiter) if in_tag => Mode::Attribute(delimiter),
                        _ => Mode::Content,
                    };
                    self.reference(&rest[1..end], output, mode, depth)?;
                    cursor += end + 1;
                    continue;
                }
            }
            let ch = rest.chars().next().ok_or(ResolveError::SvgDecodeFailed)?;
            if in_tag {
                if quote == Some(ch) {
                    quote = None;
                } else if quote.is_none() && matches!(ch, '\'' | '"') {
                    quote = Some(ch);
                } else if quote.is_none() && ch == '>' {
                    in_tag = false;
                }
            } else if ch == '<' {
                in_tag = true;
            }
            self.emit(output, &rest[..ch.len_utf8()], depth)?;
            cursor += ch.len_utf8();
        }
        // Replacement text referenced from content must be balanced markup.
        if mode == Mode::Content && in_tag {
            return Err(ResolveError::SvgDecodeFailed);
        }
        Ok(())
    }

    fn reference(
        &mut self,
        name: &str,
        output: &mut String,
        mode: Mode,
        depth: u32,
    ) -> Result<(), ResolveError> {
        let predefined = matches!(name, "amp" | "lt" | "gt" | "quot" | "apos")
            || name
                .strip_prefix('#')
                .is_some_and(super::valid_numeric_entity);
        let replacement = match self.entities.get(name) {
            _ if predefined => None,
            Some(Entity::Internal(replacement)) => Some(super::super::try_owned(replacement)?),
            Some(Entity::External) => None,
            None if self.lenient => None,
            None => return Err(ResolveError::SvgDecodeFailed),
        };
        let Some(replacement) = replacement else {
            // Left in place: character references for the parser, external
            // and tolerated undeclared entities for the DTD pass to remove.
            output
                .try_reserve(name.len().saturating_add(2))
                .map_err(|_| ResolveError::Memory)?;
            output.push('&');
            output.push_str(name);
            output.push(';');
            return Ok(());
        };
        if depth >= MAX_DEPTH || self.stack.iter().any(|open| open == name) {
            return Err(ResolveError::SvgDecodeFailed);
        }
        self.cost = self.cost.saturating_add(FIXED_COST);
        if self.cost > self.cap {
            return Err(ResolveError::SvgDecodeFailed);
        }
        self.stack
            .try_reserve(1)
            .map_err(|_| ResolveError::Memory)?;
        self.stack.push(super::super::try_owned(name)?);
        let mode = if mode == Mode::Document {
            Mode::Content
        } else {
            mode
        };
        self.scan(&replacement, output, mode, depth + 1)?;
        self.stack.pop();
        Ok(())
    }
}

/// The byte index of the `;` closing a reference at the start of `text`, when
/// it is one: the same boundary the DTD pass uses.
fn reference_end(text: &str) -> Option<usize> {
    let end = text.as_bytes()[1..]
        .iter()
        .position(|b| {
            b.is_ascii_whitespace() || matches!(b, b';' | b'<' | b'>' | b'&' | b'"' | b'\'')
        })
        .map(|index| index + 1)
        .filter(|index| text.as_bytes()[*index] == b';')?;
    (end > 1).then_some(end)
}

/// The general entities the internal subset declares, and whether an
/// undeclared entity is tolerated.
fn declarations(text: &str) -> Result<(BTreeMap<String, Entity>, bool), ResolveError> {
    let mut entities = BTreeMap::new();
    let mut standalone = false;
    let mut rest = text.trim_start_matches('\u{feff}');
    loop {
        rest = rest.trim_start_matches(is_space);
        if rest.starts_with("<?") {
            let end = rest.find("?>").ok_or(ResolveError::SvgDecodeFailed)? + 2;
            if rest.starts_with("<?xml") && rest[5..].starts_with(is_space) {
                standalone = standalone_declared(&rest[..end]);
            }
            rest = &rest[end..];
        } else if rest.starts_with("<!--") {
            let end = rest.find("-->").ok_or(ResolveError::SvgDecodeFailed)? + 3;
            rest = &rest[end..];
        } else {
            break;
        }
    }
    if !rest.starts_with("<!DOCTYPE") {
        // Without a DTD, every entity must be predefined.
        return Ok((entities, false));
    }
    let doctype = &rest[..super::doctype_end(rest).ok_or(ResolveError::SvgDecodeFailed)?];
    let mut cursor = Cursor(&doctype["<!DOCTYPE".len()..doctype.len() - 1]);
    cursor.space();
    cursor.name()?;
    cursor.space();
    let external_subset = cursor.keyword("SYSTEM") || cursor.keyword("PUBLIC");
    let mut parameter_references = false;
    if external_subset {
        cursor.skip_until('[');
    }
    cursor.space();
    if cursor.eat("[") {
        loop {
            cursor.space();
            if cursor.eat("]") || cursor.0.is_empty() {
                break;
            }
            if cursor.eat("<!ENTITY") {
                entity_declaration(&mut cursor, &mut entities)?;
            } else if cursor.eat("<!--") {
                cursor.skip_past("-->")?;
            } else if cursor.eat("<?") {
                cursor.skip_past("?>")?;
            } else if cursor.eat("<!") {
                cursor.skip_declaration()?;
            } else if cursor.eat("%") {
                cursor.name()?;
                if !cursor.eat(";") {
                    return Err(ResolveError::SvgDecodeFailed);
                }
                parameter_references = true;
            } else {
                return Err(ResolveError::SvgDecodeFailed);
            }
        }
    }
    Ok((
        entities,
        !standalone && (external_subset || parameter_references),
    ))
}

fn entity_declaration(
    cursor: &mut Cursor<'_>,
    entities: &mut BTreeMap<String, Entity>,
) -> Result<(), ResolveError> {
    cursor.required_space()?;
    let parameter = cursor.eat("%");
    if parameter {
        cursor.required_space()?;
    }
    let name = cursor.name()?;
    cursor.required_space()?;
    let entity = if cursor.keyword("SYSTEM") {
        cursor.required_space()?;
        cursor.literal()?;
        Entity::External
    } else if cursor.keyword("PUBLIC") {
        cursor.required_space()?;
        cursor.literal()?;
        cursor.required_space()?;
        cursor.literal()?;
        Entity::External
    } else {
        Entity::Internal(replacement_text(cursor.literal()?)?)
    };
    cursor.space();
    if matches!(entity, Entity::External) && cursor.keyword("NDATA") {
        cursor.required_space()?;
        cursor.name()?;
        cursor.space();
    }
    if !cursor.eat(">") {
        return Err(ResolveError::SvgDecodeFailed);
    }
    let predefined = matches!(name, "amp" | "lt" | "gt" | "quot" | "apos");
    if !parameter && !predefined && !entities.contains_key(name) {
        entities.insert(super::super::try_owned(name)?, entity);
    }
    Ok(())
}

/// An entity literal's replacement text: character references replaced,
/// general references kept for expansion at use.
fn replacement_text(literal: &str) -> Result<String, ResolveError> {
    let mut text = String::new();
    text.try_reserve(literal.len())
        .map_err(|_| ResolveError::Memory)?;
    let mut rest = literal;
    while let Some(index) = rest.find(['&', '%']) {
        text.push_str(&rest[..index]);
        rest = &rest[index..];
        // A parameter reference inside a literal is not well-formed in the
        // internal subset.
        if rest.starts_with('%') {
            return Err(ResolveError::SvgDecodeFailed);
        }
        let end = reference_end(rest).ok_or(ResolveError::SvgDecodeFailed)?;
        let name = &rest[1..end];
        if let Some(number) = name.strip_prefix('#') {
            let value = if let Some(hex) = number.strip_prefix('x') {
                u32::from_str_radix(hex, 16).ok()
            } else {
                number.parse::<u32>().ok()
            };
            let character = value
                .filter(|_| super::valid_numeric_entity(number))
                .and_then(char::from_u32)
                .filter(|c| xml_char(*c))
                .ok_or(ResolveError::SvgDecodeFailed)?;
            text.push(character);
        } else {
            text.push_str(&rest[..=end]);
        }
        rest = &rest[end + 1..];
    }
    text.push_str(rest);
    Ok(text)
}

fn xml_char(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\r' | '\u{20}'..='\u{d7ff}' | '\u{e000}'..='\u{fffd}' | '\u{10000}'..='\u{10ffff}')
}

fn is_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\r' | '\n')
}

fn standalone_declared(declaration: &str) -> bool {
    let Some((_, after)) = declaration.split_once("standalone") else {
        return false;
    };
    let after = after.trim_start_matches(is_space);
    let Some(after) = after.strip_prefix('=') else {
        return false;
    };
    let after = after.trim_start_matches(is_space);
    after.starts_with("\"yes\"") || after.starts_with("'yes'")
}

struct Cursor<'a>(&'a str);

impl<'a> Cursor<'a> {
    fn space(&mut self) -> bool {
        let before = self.0.len();
        self.0 = self.0.trim_start_matches(is_space);
        self.0.len() != before
    }

    fn required_space(&mut self) -> Result<(), ResolveError> {
        self.space()
            .then_some(())
            .ok_or(ResolveError::SvgDecodeFailed)
    }

    fn eat(&mut self, token: &str) -> bool {
        if let Some(rest) = self.0.strip_prefix(token) {
            self.0 = rest;
            true
        } else {
            false
        }
    }

    /// A keyword followed by a space.
    fn keyword(&mut self, keyword: &str) -> bool {
        match self.0.strip_prefix(keyword) {
            Some(rest) if rest.starts_with(is_space) => {
                self.0 = rest;
                true
            }
            _ => false,
        }
    }

    fn name(&mut self) -> Result<&'a str, ResolveError> {
        let end = self
            .0
            .find(|c: char| {
                is_space(c) || matches!(c, ';' | '>' | '[' | '"' | '\'' | '%' | '&' | '<')
            })
            .unwrap_or(self.0.len());
        if end == 0 {
            return Err(ResolveError::SvgDecodeFailed);
        }
        let (name, rest) = self.0.split_at(end);
        self.0 = rest;
        Ok(name)
    }

    fn literal(&mut self) -> Result<&'a str, ResolveError> {
        let quote = self
            .0
            .chars()
            .next()
            .filter(|c| matches!(c, '"' | '\''))
            .ok_or(ResolveError::SvgDecodeFailed)?;
        let body = &self.0[1..];
        let end = body.find(quote).ok_or(ResolveError::SvgDecodeFailed)?;
        self.0 = &body[end + 1..];
        Ok(&body[..end])
    }

    fn skip_past(&mut self, close: &str) -> Result<(), ResolveError> {
        let end = self.0.find(close).ok_or(ResolveError::SvgDecodeFailed)?;
        self.0 = &self.0[end + close.len()..];
        Ok(())
    }

    /// Skips a markup declaration other than an entity, honouring literals.
    fn skip_declaration(&mut self) -> Result<(), ResolveError> {
        let mut quote = None;
        for (index, c) in self.0.char_indices() {
            match quote {
                Some(open) if c == open => quote = None,
                Some(_) => {}
                None if matches!(c, '"' | '\'') => quote = Some(c),
                None if c == '>' => {
                    self.0 = &self.0[index + 1..];
                    return Ok(());
                }
                None => {}
            }
        }
        Err(ResolveError::SvgDecodeFailed)
    }

    /// Skips the external identifier's literals up to the internal subset.
    fn skip_until(&mut self, open: char) {
        let mut quote = None;
        for (index, c) in self.0.char_indices() {
            match quote {
                Some(q) if c == q => quote = None,
                Some(_) => {}
                None if matches!(c, '"' | '\'') => quote = Some(c),
                None if c == open => {
                    self.0 = &self.0[index..];
                    return;
                }
                None => {}
            }
        }
        self.0 = "";
    }
}
