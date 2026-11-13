//! pgfkeys: the key tree TikZ options live in, stored the way pgfkeys stores it: a key `K` keeps
//! its value in the macro `\pgfk@K`, its code in `\pgfk@K/.@cmd` and its default in `\pgfk@K/.@def`.
//! Being macros, keys are saved and restored with groups, so `\begin{scope}[thick]` ends where the
//! scope ends. Running the keys is done by the engine, which knows what each built-in key draws.

use crate::tex::{Cat, Meaning, Tex, Token};

/// The macro holding a key's value, code (`/.@cmd`) or default (`/.@def`).
pub fn macro_name(path: &str) -> String {
    format!("pgfk@{path}")
}

/// Splits text at top-level commas, respecting `{}`, `()` and `[]`; trims the parts and drops the
/// empty ones (for lists read as text: `by={a, b}`, coordinate lists, plot arguments).
pub fn split_top_level(text: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut level = 0i32;
    for c in text.chars() {
        match c {
            '{' | '(' | '[' => level += 1,
            '}' | ')' | ']' => level -= 1,
            ',' if level == 0 => {
                parts.push(std::mem::take(&mut current));
                continue;
            }
            _ => {}
        }
        current.push(c);
    }
    parts.push(current);
    parts.into_iter().map(|p| p.trim().to_string()).filter(|p| !p.is_empty()).collect()
}

/// Strips one pair of outer braces: `{a, b}` → `a, b` (not `{a}{b}`, whose first brace closes early).
pub fn strip_braces(v: &str) -> &str {
    let t = v.trim();
    if t.starts_with('{') && t.ends_with('}') && t.len() >= 2 {
        let inner = &t[1..t.len() - 1];
        // only when the opening brace closes at the end
        let mut level = 0;
        for (i, c) in inner.char_indices() {
            match c {
                '{' => level += 1,
                '}' => {
                    level -= 1;
                    if level < 0 && i < inner.len() {
                        return t;
                    }
                }
                _ => {}
            }
        }
        return inner.trim();
    }
    t
}

/// One item of a key list: the key and, if `=` was given, the value (one pair of braces removed).
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub key: String,
    pub value: Option<Vec<Token>>,
}

/// Splits `a, b=c, d={e, f}` at top-level commas and the first top-level `=`, as pgfkeys does.
pub fn split_list(tokens: &[Token]) -> Vec<Item> {
    let mut items = Vec::new();
    let mut current: Vec<Token> = Vec::new();
    let mut depth = 0usize;
    let mut push = |current: &mut Vec<Token>| {
        let part = std::mem::take(current);
        if let Some(item) = split_item(&part) {
            items.push(item);
        }
    };
    for t in tokens {
        match t {
            Token::Char(_, Cat::Begin) => depth += 1,
            Token::Char(_, Cat::End) => depth = depth.saturating_sub(1),
            Token::Char(',', _) if depth == 0 => {
                push(&mut current);
                continue;
            }
            _ => {}
        }
        current.push(t.clone());
    }
    push(&mut current);
    items
}

fn split_item(part: &[Token]) -> Option<Item> {
    let part = trim(part);
    if part.is_empty() {
        return None;
    }
    // a whole item in braces is unbraced first: {draw=red}
    let part = crate::tex::strip_outer_braces(part.to_vec());
    let mut depth = 0usize;
    let mut eq = None;
    for (i, t) in part.iter().enumerate() {
        match t {
            Token::Char(_, Cat::Begin) => depth += 1,
            Token::Char(_, Cat::End) => depth = depth.saturating_sub(1),
            Token::Char('=', _) if depth == 0 => {
                eq = Some(i);
                break;
            }
            _ => {}
        }
    }
    let (key, value) = match eq {
        Some(i) => (&part[..i], Some(&part[i + 1..])),
        None => (&part[..], None),
    };
    let key = crate::tex::tokens_to_string(trim(key)).trim().to_string();
    let value = value.map(|v| crate::tex::strip_outer_braces(trim(v).to_vec()));
    if key.is_empty() && value.is_none() {
        return None;
    }
    Some(Item { key, value })
}

/// Without spaces at either end.
pub fn trim(v: &[Token]) -> &[Token] {
    let mut a = 0;
    let mut b = v.len();
    while a < b && v[a].is_space() {
        a += 1;
    }
    while b > a && v[b - 1].is_space() {
        b -= 1;
    }
    &v[a..b]
}

/// `/tikz/draw` from the default path `/tikz` and the key `draw`; an absolute key stays as it is.
pub fn full_path(default: &str, key: &str) -> String {
    let key = key.trim();
    if key.starts_with('/') {
        return normalize(key);
    }
    if default.is_empty() || default == "/" {
        return normalize(&format!("/{key}"));
    }
    normalize(&format!("{}/{key}", default.trim_end_matches('/')))
}

/// Collapses runs of spaces, as pgfkeys compares keys after `\pgfkeys@spdef`.
fn normalize(k: &str) -> String {
    let mut out = String::with_capacity(k.len());
    let mut space = false;
    for c in k.chars() {
        if c.is_whitespace() {
            space = true;
            continue;
        }
        if space && !out.is_empty() && !out.ends_with('/') {
            out.push(' ');
        }
        space = false;
        out.push(c);
    }
    out
}

/// `/tikz/foo/.style` → (`/tikz/foo`, `.style`): the handler, if the last part starts with a dot.
pub fn handler(path: &str) -> Option<(&str, &str)> {
    let i = path.rfind("/.")?;
    // `/.cd` at the root, or `key/.style`
    Some((&path[..i], &path[i + 1..]))
}

/// The last part of a path: `/tikz/draw` → `draw`.
pub fn name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// The path without its last part: `/tikz/draw` → `/tikz`.
pub fn parent(path: &str) -> &str {
    match path.rfind('/') {
        Some(0) => "/",
        Some(i) => &path[..i],
        None => "",
    }
}

/// The stored value of a key, as tokens, if it has one.
pub fn value(tex: &Tex, path: &str) -> Option<Vec<Token>> {
    match tex.meaning_of(&macro_name(path)) {
        Meaning::Macro(m) if m.params.is_empty() => Some(crate::tex::substitute(&m.body, &[])),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokens(s: &str) -> Vec<Token> {
        let mut t = Tex::new();
        t.push_text(s);
        let mut v = Vec::new();
        while let Some(x) = t.get_token() {
            v.push(x);
        }
        v
    }

    #[test]
    fn lists_split_like_pgfkeys() {
        let items = split_list(&tokens("draw, line width = 2pt, label={above:{a, b}}, {fill=red}, ,"));
        let keys: Vec<&str> = items.iter().map(|i| i.key.as_str()).collect();
        assert_eq!(keys, ["draw", "line width", "label", "fill"]);
        assert_eq!(crate::tex::tokens_to_string(items[1].value.as_ref().unwrap()), "2pt");
        assert_eq!(crate::tex::tokens_to_string(items[2].value.as_ref().unwrap()), "above:{a, b}");
        assert!(items[0].value.is_none());
    }

    #[test]
    fn text_lists_and_braces() {
        assert_eq!(split_top_level("a, {b, c}, (1,2), [x, y],, d "), ["a", "{b, c}", "(1,2)", "[x, y]", "d"]);
        assert!(split_top_level(" ").is_empty());
        assert_eq!(strip_braces(" {a, b} "), "a, b");
        assert_eq!(strip_braces("{a}{b}"), "{a}{b}");
        assert_eq!(strip_braces("{{a}}"), "{a}");
        assert_eq!(strip_braces("a"), "a");
    }

    #[test]
    fn paths() {
        assert_eq!(full_path("/tikz", "draw"), "/tikz/draw");
        assert_eq!(full_path("/tikz/", "every  node/.style"), "/tikz/every node/.style");
        assert_eq!(full_path("/tikz", "/pgf/x"), "/pgf/x");
        assert_eq!(full_path("/", "a"), "/a");
        assert_eq!(handler("/tikz/foo/.style"), Some(("/tikz/foo", ".style")));
        assert_eq!(handler("/tikz/.cd"), Some(("/tikz", ".cd")));
        assert_eq!(handler("/tikz/draw"), None);
        assert_eq!(name("/tikz/draw"), "draw");
        assert_eq!(parent("/tikz/draw"), "/tikz");
        assert_eq!(parent("/tikz"), "/");
    }
}
