//! Qualification guard for cssparser 0.37.0's nested-block APIs.
use cssparser::{Parser, Token};

/// cssparser's automatic error cleanup has a 16-entry inline block stack, then
/// spills via infallible allocation. Inspect tokens without entering that
/// cleanup path before calling nested-block APIs on untrusted input. A fresh
/// parser per token deliberately prevents `next()` from auto-skipping a block.
/// This is a linear, allocation-free pass with a fixed stack, not CSS admission.
pub(crate) fn fits_inline_block_stack(mut source: &str) -> bool {
    // Escapes and NUL replacement can allocate decoded token strings. Leave
    // these spellings on the caller's historical compatibility path as well.
    if source.contains(['\\', '\0']) {
        return false;
    }
    let mut stack = [0_u8; 16];
    let mut depth = 0;
    while !source.is_empty() {
        let mut parser = Parser::new(source);
        let Ok(token) = parser.next_including_whitespace_and_comments() else {
            break;
        };
        let (open, close) = match token {
            Token::Function(_) | Token::ParenthesisBlock => (b')', 0),
            Token::SquareBracketBlock => (b']', 0),
            Token::CurlyBracketBlock => (b'}', 0),
            Token::CloseParenthesis => (0, b')'),
            Token::CloseSquareBracket => (0, b']'),
            Token::CloseCurlyBracket => (0, b'}'),
            _ => (0, 0),
        };
        if open != 0 {
            let Some(slot) = stack.get_mut(depth) else {
                return false;
            };
            *slot = open;
            depth += 1;
        } else if close != 0 && depth != 0 && stack[depth - 1] == close {
            depth -= 1;
        }
        source = &source[parser.position().byte_index()..];
    }
    true
}
