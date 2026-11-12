// SPDX-License-Identifier: MPL-2.0
//! code_shape.go: syntax-highlighted code shapes and code labels over the
//! Chroma translation in d2-rs-text.

use d2_rs_scene::{
    Bounds, Box as SceneBox, Node, Nrgba, Paint, Point, Primitive, Rect, Stroke, TextAnchor,
    TextRun,
};
use d2_rs_target::{Shape, Text};
use d2_rs_text::code::{self, Style, Token};

use crate::Builder;
use crate::golib::geo::go_fmadd;
use crate::util::*;

const MAX_CODE_SOURCE_BYTES: usize = 1 << 20;
const MAX_CODE_SOURCE_RUNES: usize = 100_000;
const MAX_CODE_RAW_TOKENS: i64 = 100_000;
const MAX_CODE_EXPANDED_TOKEN_PIECES: i64 = 100_000;
/// textmeasure.CODE_LINE_HEIGHT.
const CODE_LINE_HEIGHT: f64 = 1.3;

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct CodeTokenBudget {
    max_raw_tokens: i64,
    max_expanded_pieces: i64,
    raw_tokens: i64,
    expanded_pieces: i64,
}

impl CodeTokenBudget {
    pub(crate) fn new(max_nodes: i64) -> Self {
        let (mut raw, mut expanded) = (MAX_CODE_RAW_TOKENS, MAX_CODE_EXPANDED_TOKEN_PIECES);
        if max_nodes > 0 {
            raw = raw.min(max_nodes);
            expanded = expanded.min(max_nodes);
        }
        Self {
            max_raw_tokens: raw,
            max_expanded_pieces: expanded,
            raw_tokens: 0,
            expanded_pieces: 0,
        }
    }
}

/// validateCodeShape.
pub(crate) fn validate_code_shape(s: &Shape) -> Result<()> {
    validate_code_text(&format!("shape {}", q(&s.id)), &s.text)
}

/// validateCodeText.
pub(crate) fn validate_code_text(object: &str, text: &Text) -> Result<()> {
    if text.label.is_empty() || text.language.is_empty() {
        return Ok(());
    }
    if text.label.len() > MAX_CODE_SOURCE_BYTES {
        return Err(invalid_field(
            object,
            "label",
            Some(&text.label.len().to_string()),
            &format!("code source bytes must not exceed {MAX_CODE_SOURCE_BYTES}"),
        ));
    }
    let runes = text.label.chars().count();
    if runes > MAX_CODE_SOURCE_RUNES {
        return Err(invalid_field(
            object,
            "label",
            Some(&runes.to_string()),
            &format!("code source runes must not exceed {MAX_CODE_SOURCE_RUNES}"),
        ));
    }
    if text.font_size <= 0 {
        return Err(invalid_field(
            object,
            "fontSize",
            Some(&text.font_size.to_string()),
            "must be positive for a syntax-highlighted label",
        ));
    }
    if text.font_size > i64::from(i32::MAX) / 64 {
        return Err(invalid_field(
            object,
            "fontSize",
            Some(&text.font_size.to_string()),
            "must fit raster font scaling",
        ));
    }
    Ok(())
}

fn code_token_text(value: &str) -> String {
    let value = value.strip_suffix('\n').unwrap_or(value);
    let value = value.strip_suffix('\r').unwrap_or(value);
    value.replace('\t', "    ")
}

fn code_whitespace_only(value: &str) -> bool {
    value.chars().all(char::is_whitespace)
}

fn parse_hex_colour(value: &str) -> Option<Nrgba> {
    let hex = value.strip_prefix('#')?;
    let digits: Vec<u8> = hex
        .bytes()
        .map(|b| (b as char).to_digit(16).map(|d| d as u8))
        .collect::<Option<_>>()?;
    let (r, g, b) = match digits.as_slice() {
        [r, g, b] => (r * 17, g * 17, b * 17),
        [r1, r2, g1, g2, b1, b2] => (r1 * 16 + r2, g1 * 16 + g2, b1 * 16 + b2),
        _ => return None,
    };
    Some(Nrgba::new(r, g, b, 0xff))
}

/// codeColourPaint / codeForegroundPaint: an unset foreground is SVG black.
fn code_foreground_paint(style: &Style) -> Option<Paint> {
    let color = style
        .color
        .as_deref()
        .and_then(parse_hex_colour)
        .unwrap_or(Nrgba::new(0, 0, 0, 0xff));
    Some(Paint::solid(color))
}

/// countCodeTokenPieces.
fn count_code_token_pieces(
    value: &str,
    remaining: i64,
    limit: i64,
) -> std::result::Result<i64, String> {
    let mut pieces = 1;
    if remaining < pieces {
        return Err(format!(
            "newline-expanded token piece count exceeds limit {limit}"
        ));
    }
    for b in value.bytes() {
        if b != b'\n' {
            continue;
        }
        pieces += 1;
        if pieces > remaining {
            return Err(format!(
                "newline-expanded token piece count exceeds limit {limit}"
            ));
        }
    }
    Ok(pieces)
}

impl Builder<'_> {
    /// buildCodeShape.
    pub(crate) fn build_code_shape(
        &mut self,
        s: &Shape,
        stroke: Option<&Stroke>,
    ) -> Result<Vec<Node>> {
        validate_code_shape(s)?;
        if s.text.label.is_empty() {
            return Ok(Vec::new());
        }
        if s.text.language.is_empty() {
            return self.build_shape_text(s);
        }
        let object = format!("shape {}", q(&s.id));
        let dark = self.theme_is_dark();
        let background = code::style("Background", dark).background;
        let Some(background) = background.as_deref().and_then(parse_hex_colour) else {
            return err(format!(
                "scene: {object} code snippet style has no background"
            ));
        };
        let b = SceneBox {
            x: s.pos.x as f64,
            y: s.pos.y as f64,
            width: s.width as f64,
            height: s.height as f64,
        };
        let code_stroke = stroke.map(|stroke| Stroke {
            dashes: None,
            dash_offset: 0.0,
            ..stroke.clone()
        });
        let mut nodes = vec![node(
            format!("{}:code-background", s.id),
            Some(Primitive::Rect(Rect {
                r#box: b,
                fill: Some(Paint::solid(background)),
                stroke: code_stroke,
                ..Default::default()
            })),
        )];
        let padding = s.text.font_size as f64 / 2.0;
        nodes.extend(self.build_code_text_runs(
            &object,
            &format!("{}:code", s.id),
            &s.text,
            dark,
            Point::new(b.x + padding, b.y + padding),
        )?);
        Ok(nodes)
    }

    fn collect_code_tokens(&mut self, text: &Text) -> std::result::Result<Vec<Token>, String> {
        let tokens = match code::tokenize(&text.language, &text.label) {
            Some(tokens) => tokens,
            None if code::registered(&text.language) => {
                return Err(format!(
                    "the Chroma lexer {} is not translated in the native port",
                    q(code::lexer_name(&text.language).unwrap_or(&text.language))
                ));
            }
            // lexers.Fallback: plain text.
            None => code::plain(&text.label.replace("\r\n", "\n").replace('\r', "\n")),
        };
        let budget = &mut self.code_tokens;
        for token in &tokens {
            if budget.raw_tokens >= budget.max_raw_tokens {
                return Err(format!(
                    "raw lexer token count exceeds limit {}",
                    budget.max_raw_tokens
                ));
            }
            let pieces = count_code_token_pieces(
                &token.value,
                budget.max_expanded_pieces - budget.expanded_pieces,
                budget.max_expanded_pieces,
            )?;
            budget.raw_tokens += 1;
            budget.expanded_pieces += pieces;
        }
        Ok(tokens)
    }

    /// buildCodeTextRuns.
    pub(crate) fn build_code_text_runs(
        &mut self,
        object: &str,
        id_prefix: &str,
        text: &Text,
        dark: bool,
        origin: Point,
    ) -> Result<Vec<Node>> {
        let tokens = self.collect_code_tokens(text).map_err(|e| {
            Error(format!(
                "scene: {object} tokenize {} code: {e}",
                text.language
            ))
        })?;
        let lines = code::split_lines(tokens);
        let line_advance = text.font_size as f64 * CODE_LINE_HEIGHT;
        let mut nodes = Vec::new();
        for (line_index, tokens) in lines.iter().enumerate() {
            // ARM64 code_shape.go:141: FMADDD.
            let line_top = go_fmadd(line_index as f64, line_advance, origin.y);
            let baseline = line_top + text.font_size as f64;
            let mut cursor = origin.x;
            for (token_index, token) in tokens.iter().enumerate() {
                let token_text = code_token_text(&token.value);
                if token_text.is_empty() {
                    continue;
                }
                let entry = code::style(token.kind, dark);
                let mut font_text = text.clone();
                font_text.font_family = "mono".into();
                font_text.italic = entry.italic == Some(true);
                font_text.bold = entry.bold == Some(true) && !font_text.italic;
                let code_font = self.font(&font_text).map_err(|e| {
                    Error(format!(
                        "scene: {object} code token {line_index}:{token_index}: {e}"
                    ))
                })?;
                let advance =
                    crate::shaping::code_advance(self, &code_font, &token_text).map_err(|e| {
                        Error(format!(
                            "scene: {object} code token {line_index}:{token_index}: {e}"
                        ))
                    })?;
                let underline = entry.underline == Some(true);
                if !code_whitespace_only(&token_text) || underline {
                    let run = TextRun {
                        text: token_text,
                        origin: Point::new(cursor, baseline),
                        anchor: TextAnchor::START,
                        font: code_font,
                        fill: code_foreground_paint(&entry),
                        underline,
                        // ARM64 code_shape.go:182: lineAdvance is the SSA
                        // product FontSize*CODE_LINE_HEIGHT, fused here.
                        ink: Bounds::new(
                            cursor,
                            line_top,
                            cursor + advance,
                            go_fmadd(text.font_size as f64, CODE_LINE_HEIGHT, line_top),
                        ),
                        ..Default::default()
                    };
                    nodes.push(node(
                        format!("{id_prefix}-line:{line_index}:token:{token_index}"),
                        Some(Primitive::TextRun(run)),
                    ));
                }
                cursor += advance;
            }
        }
        Ok(nodes)
    }
}
