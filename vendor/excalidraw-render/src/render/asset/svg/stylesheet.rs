//! Source-preserving boundaries for the admitted static stylesheet subset.
//!
//! cssparser owns normal block and declaration tokenization. Its CSS error
//! recovery and opaque strings/functions differ from our historical admission
//! policy, which even admits some malformed values for usvg to ignore. Only
//! those cases use the compatibility fallback below; see docs/development/css-tokenization.md.
use cssparser::{Delimiter, ParseError, Parser, Token};

pub(super) struct Rule<'a> {
    pub source: &'a str,
    selector: &'a str,
    declarations: &'a str,
    tokenized: bool,
}

pub(super) struct Rules<'a> {
    remaining: &'a str,
    compatibility: bool,
}

impl<'a> Rules<'a> {
    pub fn new(source: &'a str) -> Self {
        Self {
            remaining: source,
            compatibility: !crate::css_syntax::fits_inline_block_stack(source),
        }
    }
}

impl<'a> Iterator for Rules<'a> {
    type Item = Result<Rule<'a>, ()>;

    fn next(&mut self) -> Option<Self::Item> {
        let source = self.remaining;
        if source.trim_start().is_empty() {
            return None;
        }
        let result = if self.compatibility {
            compatibility_rule(source)
        } else {
            tokenized_rule(source).or_else(|_| {
                // A malformed block can make CSS recovery consume the entire
                // remaining sheet. Never retry that suffix for every raw rule:
                // one switch keeps total boundary work linear in source bytes.
                self.compatibility = true;
                compatibility_rule(source)
            })
        };
        self.remaining = result
            .as_ref()
            .map_or("", |rule| &source[rule.source.len()..]);
        Some(result)
    }
}

fn tokenized_rule(source: &str) -> Result<Rule<'_>, ()> {
    let parser = &mut Parser::new(source);
    let start = parser.position();
    let selector = parser
        .parse_until_before(Delimiter::CurlyBracketBlock, |p| component_slice(p, 0))
        .map_err(|_| ())?;
    parser.expect_curly_bracket_block().map_err(|_| ())?;
    let (declarations, end) = parser
        .parse_nested_block(|p| Ok((component_slice(p, 0)?, p.position())))
        .map_err(|_: ParseError<()>| ())?;
    // No EOF-closed rule, and no token may hide a historical brace boundary.
    if parser.slice(end..parser.position()) != "}"
        || selector.contains(['{', '}'])
        || declarations.contains(['{', '}'])
    {
        return Err(());
    }
    Ok(Rule {
        source: parser.slice_from(start),
        selector,
        declarations,
        tokenized: true,
    })
}

impl Rule<'_> {
    pub fn admitted(&self) -> bool {
        !self.selector.trim().is_empty()
            && !self.selector.contains([':', '}'])
            && !self.declarations.contains('{')
            && self
                .tokenized
                .then(|| tokenized_declarations(self.declarations).ok())
                .flatten()
                .unwrap_or_else(|| {
                    // Preserve old admission for delimiters in opaque tokens and
                    // malformed values, rather than silently widening CSS support.
                    self.declarations.split(';').all(compatibility_declaration)
                })
    }
}

fn tokenized_declarations(source: &str) -> Result<bool, ()> {
    let parser = &mut Parser::new(source);
    while !parser.is_exhausted() {
        let admitted = parser
            .parse_until_before(Delimiter::Semicolon, |declaration| {
                let start = declaration.position();
                if declaration.is_exhausted() {
                    return Ok(true);
                }
                declaration.expect_ident()?;
                let property = declaration.slice_from(start);
                declaration.expect_colon()?;
                let value = component_slice(declaration, 0)?;
                if declaration.slice_from(start).contains(';') || property.contains(':') {
                    return Err(ParseError::custom(()));
                }
                Ok(admitted_declaration(property, value))
            })
            .map_err(|_: ParseError<()>| ())?;
        if !admitted {
            return Ok(false);
        }
        if !parser.is_exhausted() {
            parser.expect_semicolon().map_err(|_| ())?;
        }
    }
    Ok(true)
}

fn admitted_declaration(property: &str, value: &str) -> bool {
    let property = property.trim();
    !property.is_empty()
        && property
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        && super::allowed_static_style_property(property)
        && !value.trim().is_empty()
}

/// Bounded recursive descent; no list allocations. Each pass consumes at least
/// one source byte per token. The 128-level cap limits *this adapter's* stack,
/// not admission: deeper uninterpreted values take the flat compatibility path.
/// The whole-sheet inline-stack guard runs before any nested parser can enter
/// cssparser's automatic error cleanup.
fn component_slice<'i>(parser: &mut Parser<'i>, depth: u32) -> Result<&'i str, ParseError<()>> {
    if depth == 128 {
        return Err(ParseError::custom(()));
    }
    let start = parser.position();
    while let Ok(token) = parser.next_including_whitespace_and_comments() {
        let close = match token {
            Token::Function(_) | Token::ParenthesisBlock => ")",
            Token::SquareBracketBlock => "]",
            Token::CurlyBracketBlock => "}",
            token if token.is_parse_error() => return Err(ParseError::custom(())),
            _ => continue,
        };
        let end = parser.parse_nested_block(|p| {
            component_slice(p, depth + 1)?;
            Ok(p.position())
        })?;
        if parser.slice(end..parser.position()) != close {
            return Err(ParseError::custom(()));
        }
    }
    Ok(parser.slice_from(start))
}

// These raw boundaries are deliberately confined to the compatibility path.
// For example `rect {fill:rgb(1,2,3}` used to be admitted, and a quoted `}`
// used to end a rule for both admission and PDF source ordinals.
fn compatibility_rule(source: &str) -> Result<Rule<'_>, ()> {
    let (selector, after_selector) = source.split_once('{').ok_or(())?;
    let (declarations, remaining) = after_selector.split_once('}').ok_or(())?;
    Ok(Rule {
        source: &source[..source.len() - remaining.len()],
        selector,
        declarations,
        tokenized: false,
    })
}

fn compatibility_declaration(source: &str) -> bool {
    source.trim().is_empty()
        || source
            .split_once(':')
            .is_some_and(|(p, v)| admitted_declaration(p, v))
}

/// The stylesheet without its top-level at-rules (`@import`, `@font-face`,
/// `@keyframes`, `@media`, ...), none of which the static subset admits, and
/// whether any was removed. Every other rule is kept for admission. `None`
/// when an at-keyword does not start a rule or the sheet does not tokenize:
/// the caller then refuses the whole sheet, as before.
pub(super) fn without_at_rules(source: &str) -> Option<(std::borrow::Cow<'_, str>, bool)> {
    if !source.contains('@') {
        return Some((std::borrow::Cow::Borrowed(source), false));
    }
    if !crate::css_syntax::fits_inline_block_stack(source) {
        return None;
    }
    let parser = &mut Parser::new(source);
    let mut removed = Vec::new();
    let mut boundary = true;
    loop {
        let start = parser.position().byte_index();
        let Ok(token) = parser.next_including_whitespace_and_comments() else {
            break;
        };
        match token {
            Token::WhiteSpace(_) | Token::CDO | Token::CDC => {}
            Token::AtKeyword(_) if boundary => {
                loop {
                    match parser.next_including_whitespace_and_comments() {
                        Ok(Token::Semicolon) | Err(_) => break,
                        Ok(Token::CurlyBracketBlock) => {
                            parser
                                .parse_nested_block(|block| {
                                    while block.next_including_whitespace_and_comments().is_ok() {}
                                    Ok::<(), ParseError<()>>(())
                                })
                                .ok()?;
                            break;
                        }
                        Ok(token) if token.is_parse_error() => return None,
                        Ok(_) => {}
                    }
                }
                removed.push(start..parser.position().byte_index());
            }
            Token::CurlyBracketBlock => boundary = true,
            token if token.is_parse_error() => return None,
            _ => boundary = false,
        }
    }
    if removed.is_empty() {
        return Some((std::borrow::Cow::Borrowed(source), false));
    }
    let mut kept = String::new();
    kept.try_reserve(source.len()).ok()?;
    let mut copied = 0;
    for range in removed {
        kept.push_str(source.get(copied..range.start)?);
        copied = range.end;
    }
    kept.push_str(source.get(copied..)?);
    Some((std::borrow::Cow::Owned(kept), true))
}
