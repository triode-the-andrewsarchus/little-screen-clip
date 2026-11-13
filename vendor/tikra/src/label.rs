//! Node text, typeset by mathra: `{Exit}`, `{$\vec{v}$}`, `{Check every value twice}`.
//! Lines by `\\`, automatic wrapping at `text width`, sizes `\small`, `\Large`.

mod hyphen;
mod paragraph;
mod picture;

use mathra::layout::MathBox;

use crate::color::Color;
use crate::scene::Label;

/// Centimeters per em in a 10pt font.
const EM_CM: f64 = 10.0 * 2.54 / 72.27;

/// The font labels are set in: Latin Modern Math, the Computer Modern that TeX sets the figures in.
pub const FONT: mathra::font::MathFont = mathra::font::MathFont::LatinModern;
/// The font text in Times (`\fontfamily{ptm}`) is laid out in: STIX Two Math, whose letters are a
/// Times; they are drawn in TeX Gyre Termes (`times`).
pub const TIMES: mathra::font::MathFont = mathra::font::MathFont::StixTwo;

/// Size factor of the LaTeX font commands (10pt = 1).
pub fn font_size(text: &str) -> f64 {
    // the last size command in the text is the one in effect
    let mut f = 1.0;
    let mut at = None;
    for (cmd, factor) in [
        ("\\tiny", 0.5),
        ("\\scriptsize", 0.7),
        ("\\footnotesize", 0.8),
        ("\\small", 0.9),
        ("\\normalsize", 1.0),
        ("\\large", 1.2),
        ("\\Large", 1.44),
        ("\\LARGE", 1.728),
        ("\\huge", 2.074),
        ("\\Huge", 2.488),
    ] {
        // \Large contains \large: look for the command followed by something that is not a letter
        let mut from = 0;
        while let Some(i) = text[from..].find(cmd) {
            let i = from + i;
            let after = &text[i + cmd.len()..];
            if !after.chars().next().is_some_and(|c| c.is_ascii_alphabetic()) && at.is_none_or(|a| i > a) {
                f = factor;
                at = Some(i);
            }
            from = i + cmd.len();
        }
    }
    // `\fontsize{6}{8}\selectfont`: that size in points
    let mut from = 0;
    while let Some(i) = text[from..].find("\\fontsize{") {
        let i = from + i;
        let rest = &text[i + 10..];
        if let Some(v) = rest.split('}').next().and_then(|v| v.trim().trim_end_matches("pt").parse::<f64>().ok()) {
            if v > 0.0 && at.is_none_or(|a| i > a) {
                f = v / 10.0;
                at = Some(i);
            }
        }
        from = i + 10;
    }
    f
}

/// Removes font commands that do not change mathra's drawing (family, series, size), and writes
/// `\verb` as `\texttt`.
fn clean(text: &str) -> String {
    const COMMANDS: &[&str] = &[
        "tiny",
        "scriptsize",
        "footnotesize",
        "small",
        "normalsize",
        "large",
        "Large",
        "LARGE",
        "huge",
        "Huge",
        "sffamily",
        "rmfamily",
        "ttfamily",
        "itshape",
        "mdseries",
        "upshape",
        "centering",
        "raggedright",
        "raggedleft",
        "it",
        "sf",
        "tt",
        "rm",
        "selectfont",
    ];
    let c: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < c.len() {
        // `\\` is a line break: the letters after it are text, not a command (`Does\\it`)
        if c[i] == '\\' && c.get(i + 1) == Some(&'\\') {
            out.push_str("\\\\");
            i += 2;
            continue;
        }
        if c[i] == '\\' {
            let name: String = c[i + 1..].iter().take_while(|x| x.is_ascii_alphabetic()).collect();
            // `\verb!…!` (any delimiter): the text as it is, in typewriter
            if name == "verb" {
                if let Some(&d) = c.get(i + 5) {
                    if let Some(len) = c[i + 6..].iter().position(|&x| x == d) {
                        let body: String = c[i + 6..i + 6 + len].iter().collect();
                        out.push_str(&format!("\\texttt{{{body}}}"));
                        i += 7 + len;
                        continue;
                    }
                }
            }
            // `\fontsize{6}{8}`: its two arguments go with it (the size is read before)
            if name == "fontsize" {
                i += 9;
                for _ in 0..2 {
                    while c.get(i) == Some(&' ') {
                        i += 1;
                    }
                    match c.get(i) {
                        Some('{') => i += c[i..].iter().position(|&x| x == '}').map_or(c.len() - i, |p| p + 1),
                        // unbraced, an argument is one token: a control word or a character (`\fontsize{12}9`)
                        Some('\\') => i += 1 + c[i + 1..].iter().take_while(|x| x.is_ascii_alphabetic()).count().max(1),
                        Some(_) => i += 1,
                        None => {}
                    }
                }
                continue;
            }
            // `\hskip0pt` (what `execute at begin node` puts first, to allow a break): no space
            if name == "hskip" {
                i += 6;
                while c.get(i).is_some_and(|x| *x == ' ' || x.is_ascii_digit() || *x == '.' || *x == '-') {
                    i += 1;
                }
                if c.get(i..i + 2).is_some_and(|u| ["pt", "em", "ex", "cm", "mm"].contains(&u.iter().collect::<String>().as_str())) {
                    i += 2;
                }
                continue;
            }
            // `\begingroup … \endgroup` a group, `\relax` nothing: the spaces after them go, as
            // TeX skips those after a control word
            if matches!(name.as_str(), "begingroup" | "endgroup" | "relax") {
                out.push_str(match name.as_str() {
                    "begingroup" => "{",
                    "endgroup" => "}",
                    _ => "",
                });
                i += 1 + name.len();
                while c.get(i) == Some(&' ') {
                    i += 1;
                }
                continue;
            }
            if COMMANDS.contains(&name.as_str()) || name == "bfseries" || name == "bf" {
                i += 1 + name.chars().count();
                while c.get(i) == Some(&' ') {
                    i += 1;
                }
                // a control word kept before keeps its end (`\foo\small x`)
                let tail: String = out.chars().rev().take_while(|x| x.is_ascii_alphabetic()).collect();
                if !tail.is_empty() && out[..out.len() - tail.len()].ends_with('\\') {
                    out.push(' ');
                }
                continue;
            }
        }
        out.push(c[i]);
        i += 1;
    }
    out
}

/// Typewriter text as TeX's `cmtt` sets it, every character half an em and a fortieth wide: letters
/// and digits from the math font's monospace alphabet, the others (which it lacks) centered in that
/// width, spaces too.
fn typewriter(t: &str) -> String {
    const WIDTH: f32 = 0.525;
    // `\textbackslash`, set aside by `families`
    const BACKSLASH: char = '\u{e000}';
    let font = FONT.get();
    let mut out = String::from("$");
    let mut run = String::new();
    let flush = |run: &mut String, out: &mut String| {
        if !run.is_empty() {
            out.push_str(&format!("\\mathtt{{{run}}}"));
            run.clear();
        }
    };
    // the text has no backslash: its braces only group
    for c in t.chars().filter(|c| !matches!(c, '{' | '}')) {
        if c.is_ascii_alphanumeric() {
            run.push(c);
            continue;
        }
        flush(&mut run, &mut out);
        // cmtt's parentheses and brackets are shorter than the roman ones, from 1/12 em below the
        // baseline to 25/36 above: the roman glyph shrunk to that height, in the box of cmtt's
        // (bottom, top)
        let short = match c {
            '(' | ')' | '[' | ']' => Some((7.0 / 9.0, -1.0 / 12.0, 25.0 / 36.0)),
            _ => None,
        };
        let advance = if c == ' ' { 0.0 } else { font.glyph(c).map_or(WIDTH, |g| font.advance(g) / font.units_per_em()) };
        let advance = advance * short.map_or(1.0, |s: (f32, f32, f32)| s.0);
        let side = ((WIDTH - advance) / 2.0).max(0.0);
        let glyph = match (c, short) {
            (' ', _) => String::new(),
            ('_' | '#' | '%' | '&' | '$', _) => format!("\\text{{\\{c}}}"),
            (BACKSLASH, _) => "\\backslash".to_string(),
            (c, Some((scale, bottom, top))) => format!("\\smash{{\\scalebox{{{scale:.4}}}{{{c}}}}}\\rule[{bottom:.4}em]{{0em}}{{{:.4}em}}", top - bottom),
            (c, None) => format!("\\text{{{c}}}"),
        };
        out.push_str(&format!("\\hspace{{{side}em}}{glyph}\\hspace{{{side}em}}"));
    }
    flush(&mut run, &mut out);
    out.push('$');
    if out == "$$" {
        String::new()
    } else {
        out
    }
}

/// Text in another family (`\texttt{…}`, `\textit{…}`, `\emph{…}`, `\textsf{…}`, or all of it
/// under `\ttfamily`, `\itshape`, `\sffamily`) as the math font's letters of that family, the
/// closest it has to TeX's typewriter, italic and sans serif fonts; its spaces kept. In `bold`
/// text (`\bfseries`), italic is bold italic, as TeX's `\emph` there. In Times, the words are apart by
/// the text's space (a quarter em), not math's `\ ` (a third).
fn families(text: &str, all: Option<&str>, bold: bool, face: mathra::font::MathFont) -> String {
    let set = |t: &str, family: &str| -> String {
        if family == "mathtt" {
            return typewriter(t);
        }
        let family = if bold && family == "mathit" { "mathbfit" } else { family };
        // digits in sans serif as the math font's sans-serif digits (`\mathsf` keeps them roman);
        // only sans serif uses it: typewriter and italic are set otherwise
        let digits = |p: &str| -> String { p.chars().map(|c| if c.is_ascii_digit() { char::from_u32(0x1D7E2 + (c as u32 - '0' as u32)).unwrap_or(c) } else { c }).collect() };
        // italic as mathra's italic text: no italic correction between the letters nor after the
        // words, as in TeX's text (math would put one after each letter that ends a list)
        let run = |r: &str| -> String {
            match family {
                "mathit" => format!("\\text{{\\itshape {r}}}"),
                "mathbfit" => format!("\\text{{\\bfseries\\itshape {r}}}"),
                _ => format!("\\{family}{{{}}}", digits(r)),
            }
        };
        // letters and digits in the family; the rest (`-`, `,`, `.`) as text, or math would make a
        // hyphen a minus sign with space around it
        let word = |p: &str| -> String {
            let mut out = String::new();
            let mut letters = String::new();
            for c in p.chars() {
                if c.is_alphanumeric() {
                    letters.push(c);
                    continue;
                }
                if !letters.is_empty() {
                    out.push_str(&run(&letters));
                    letters.clear();
                }
                out.push_str(&format!("\\text{{{c}}}"));
            }
            if !letters.is_empty() {
                out.push_str(&run(&letters));
            }
            out
        };
        let parts: Vec<String> = t.split(' ').filter(|p| !p.is_empty()).map(word).collect();
        if parts.is_empty() {
            String::new()
        } else if face == TIMES {
            parts.iter().map(|p| format!("${p}$")).collect::<Vec<_>>().join(" ")
        } else {
            format!("${}$", parts.join("\\ "))
        }
    };
    if let Some(family) = all {
        if !text.contains('$') && !text.contains('\\') {
            return set(text, family);
        }
    }
    let mut out = text.to_string();
    for (command, family) in [("texttt", "mathtt"), ("textit", "mathit"), ("emph", "mathit"), ("textsf", "mathsf")] {
        let open = format!("\\{command}{{");
        let mut done = String::new();
        let mut rest = out.as_str();
        while let Some(i) = rest.find(&open) {
            done.push_str(&rest[..i]);
            let body = &rest[i + open.len()..];
            // to the matching brace
            let mut depth = 1;
            let mut end = body.len();
            for (k, c) in body.char_indices() {
                match c {
                    '{' => depth += 1,
                    '}' => {
                        depth -= 1;
                        if depth == 0 {
                            end = k;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            let inner = &body[..end];
            // `\textbackslash` (and the space it ends with) is a character of typewriter text
            let plain = if family == "mathtt" { inner.replace("\\textbackslash ", "\u{e000}").replace("\\textbackslash", "\u{e000}") } else { inner.to_string() };
            done.push_str(&if plain.contains('$') || plain.contains('\\') { inner.to_string() } else { set(&plain, family) });
            rest = body.get(end + 1..).unwrap_or("");
        }
        done.push_str(rest);
        out = done;
    }
    out
}

/// A TeX dimension at the start of `s` (`2cm`, `1 pt`, `.5ex`): its value in points and the rest.
fn dimension(s: &str, size: f64) -> Option<(f64, &str)> {
    let s = s.trim_start();
    let end = s.find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-' || c == '+')).unwrap_or(s.len());
    let v: f64 = s[..end].parse().ok()?;
    let rest = s[end..].trim_start();
    for (unit, pt) in
        [("pt", 1.0), ("cm", 72.27 / 2.54), ("mm", 72.27 / 25.4), ("in", 72.27), ("bp", 72.27 / 72.0), ("em", 10.0 * size * quad(10.0 * size, false)), ("ex", 4.3 * size)]
    {
        if let Some(r) = rest.strip_prefix(unit) {
            return Some((v * pt, r));
        }
    }
    None
}

/// The ligatures of TeX's text fonts: `--` and `---` as dashes, ``` `` ``` and `''` as quotes; not
/// in math, nor in typewriter text (whose font has none).
fn ligatures(text: &str) -> String {
    if !text.contains("--") && !text.contains("``") && !text.contains("''") {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut math = false;
    let mut rest = text;
    while let Some(c) = rest.chars().next() {
        if !math && rest.starts_with("\\texttt{") {
            // to the matching brace, as it is
            let mut depth = 0;
            let end = rest.char_indices().find(|&(_, ch)| {
                match ch {
                    '{' => depth += 1,
                    '}' => depth -= 1,
                    _ => {}
                }
                ch == '}' && depth == 0
            });
            let end = end.map_or(rest.len(), |(i, _)| i + 1);
            out.push_str(&rest[..end]);
            rest = &rest[end..];
            continue;
        }
        if c == '\\' {
            // a control symbol (`\$`) or word as it is
            let n = rest[1..].chars().next().map_or(0, |d| {
                if d.is_ascii_alphabetic() {
                    rest[1..].find(|x: char| !x.is_ascii_alphabetic()).unwrap_or(rest.len() - 1)
                } else {
                    d.len_utf8()
                }
            });
            out.push_str(&rest[..1 + n]);
            rest = &rest[1 + n..];
            continue;
        }
        if c == '$' {
            math = !math;
        } else if !math {
            let swap = [("---", "\\textemdash{}"), ("--", "\\textendash{}"), ("``", "\u{201c}"), ("''", "\u{201d}")].into_iter().find(|(l, _)| rest.starts_with(l));
            if let Some((l, r)) = swap {
                out.push_str(r);
                rest = &rest[l.len()..];
                continue;
            }
        }
        out.push(c);
        rest = &rest[c.len_utf8()..];
    }
    out
}

/// `\vrule width 1pt height 2cm depth 0pt` as mathra's `$\rule[raise]{width}{height}$`, in ems of
/// the label's font (so that the label's scaling gives the rule its size in points). A height or
/// depth not given runs to the line's, `running` (points); without it the rules are left out.
fn rules(text: &str, size: f64, running: Option<(f64, f64)>) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find("\\vrule") {
        let after = &rest[i + 6..];
        if after.chars().next().is_some_and(|c| c.is_ascii_alphabetic()) {
            out.push_str(&rest[..i + 6]);
            rest = after;
            continue;
        }
        out.push_str(&rest[..i]);
        let (mut w, mut h, mut d) = (0.4, None, None);
        let mut r = after;
        loop {
            let t = r.trim_start();
            let Some((key, more)) = ["width", "height", "depth"].iter().find_map(|k| t.strip_prefix(k).map(|m| (*k, m))) else { break };
            let Some((v, more)) = dimension(more, size) else { break };
            match key {
                "width" => w = v,
                "height" => h = Some(v),
                _ => d = Some(v),
            }
            r = more;
        }
        rest = r;
        let Some((line_h, line_d)) = running else { continue };
        let em = 10.0 * size;
        let (h, d) = (h.unwrap_or(line_h), d.unwrap_or(line_d));
        out.push_str(&format!("$\\rule[{:.4}em]{{{:.4}em}}{{{:.4}em}}$", -d / em, w / em, (h + d) / em));
    }
    out.push_str(rest);
    out
}

/// `\hbox to 2cm{Text\hss}`: the text in a box of that width, to the left of it (`\hss` after),
/// to the right (`\hss` before) or centered (both); `stretch` is how much the label widens its
/// boxes afterwards.
fn hboxes(text: &str, size: f64, stretch: f64) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find("\\hbox") {
        out.push_str(&rest[..i]);
        let after = &rest[i + 5..];
        let parsed = (|| {
            let a = after.trim_start().strip_prefix("to")?;
            let (w, a) = dimension(a, size)?;
            let body = a.trim_start().strip_prefix('{')?;
            let mut depth = 1;
            let end = body.char_indices().find(|&(_, c)| {
                match c {
                    '{' => depth += 1,
                    '}' => depth -= 1,
                    _ => {}
                }
                depth == 0
            })?;
            Some((w, &body[..end.0], &body[end.0 + 1..]))
        })();
        let Some((w, inner, after)) = parsed else {
            out.push_str("\\hbox");
            rest = after;
            continue;
        };
        let inner = inner.trim();
        let (left, inner) = inner.strip_prefix("\\hss").map_or((false, inner), |r| (true, r));
        let (right, inner) = inner.strip_suffix("\\hss").map_or((false, inner), |r| (true, r));
        let inner = inner.trim();
        let em = w / (10.0 * size * stretch);
        if inner.contains('$') || !(left || right) {
            out.push_str(inner);
        } else if left && right {
            out.push_str(&format!("$\\hspace{{{:.4}em}}\\clap{{{inner}}}\\hspace{{{:.4}em}}$", em / 2.0, em / 2.0));
        } else if right {
            out.push_str(&format!("$\\rlap{{{inner}}}\\hspace{{{em:.4}em}}$"));
        } else {
            out.push_str(&format!("$\\hspace{{{em:.4}em}}\\llap{{{inner}}}$"));
        }
        rest = after;
    }
    out.push_str(rest);
    out
}

/// Whether each character of `c` is at the text's top level: outside braces, math (`$…$`, `$$…$$`,
/// `\(…\)`, `\[…\]`) and environments, as there a `\\` is a row of the array or the alignment, not
/// a line of the node. Lists and the centering ones (`itemize`, `center`) are text: their lines
/// are the node's. A control symbol (`\\`, `\$`) is one with its backslash.
fn top_level(c: &[char]) -> Vec<bool> {
    scan(c).0
}

/// Whether the end of `text` is in math (`$…`, `\(…`, `\[…`): what is put there is read as math.
pub fn in_math(text: &str) -> bool {
    scan(&text.chars().collect::<Vec<_>>()).1
}

/// `top_level`, and whether math is open at the end.
fn scan(c: &[char]) -> (Vec<bool>, bool) {
    const TEXT: &[&str] = &["itemize", "enumerate", "description", "center", "flushleft", "flushright"];
    let mut top = vec![true; c.len()];
    let (mut level, mut envs) = (0, 0);
    // what closes the math the text is in
    let mut math: Option<&str> = None;
    let mut i = 0;
    while i < c.len() {
        let here = level == 0 && envs == 0 && math.is_none();
        let mut step = 1;
        match (c[i], c.get(i + 1).copied()) {
            ('{', _) => level += 1,
            ('}', _) => level -= 1,
            ('$', next) => {
                let close = if next == Some('$') { "$$" } else { "$" };
                step = close.len();
                math = match math {
                    None => Some(close),
                    Some(m) if m == close => None,
                    m => m,
                };
            }
            ('\\', Some(o @ ('(' | '['))) if math.is_none() => {
                math = Some(if o == '(' { "\\)" } else { "\\]" });
                step = 2;
            }
            ('\\', Some(x)) if x.is_ascii_alphabetic() => {
                let name: String = c[i + 1..].iter().take_while(|x| x.is_ascii_alphabetic()).collect();
                step = 1 + name.len();
                if name == "begin" || name == "end" {
                    let rest: String = c[i + step..].iter().collect();
                    if let Some(env) = rest.strip_prefix('{').and_then(|r| r.split_once('}')).map(|(e, _)| e.trim().trim_end_matches('*')) {
                        if !TEXT.contains(&env) {
                            envs += if name == "begin" { 1 } else { -1 };
                        }
                    }
                }
            }
            ('\\', Some(x)) => {
                if math.is_some_and(|m| m == format!("\\{x}")) {
                    math = None;
                }
                step = 2;
            }
            _ => {}
        }
        for t in &mut top[i..(i + step).min(c.len())] {
            *t = here;
        }
        i += step;
    }
    (top, math.is_some())
}

/// The displayed formulas of a paragraph (`\[…\]`, `$$…$$` at its top level), each a line of its
/// own, centered in the paragraph's width (`width`, pt) as TeX centers a display, with
/// \abovedisplayskip and \belowdisplayskip (10pt at 10pt) around it.
fn displays(text: &str, width: f64, size: f64, stretch: f64) -> String {
    let c: Vec<char> = text.chars().collect();
    let top = top_level(&c);
    let mut out = String::new();
    let mut i = 0;
    let skip = 10.0 * size;
    while i < c.len() {
        let open = match (c[i], c.get(i + 1)) {
            ('\\', Some('[')) | ('$', Some('$')) if top[i] => 2,
            _ => 0,
        };
        if open == 0 {
            // a control symbol (`\\` of `\\[2pt]`) whole, so its second character opens nothing
            let n = if c[i] == '\\' && c.get(i + 1).is_some_and(|x| !x.is_ascii_alphabetic()) { 2 } else { 1 };
            out.extend(&c[i..(i + n).min(c.len())]);
            i += n;
            continue;
        }
        // the display runs to where the text is at its top level again, less its closing `\]` or `$$`
        let end = (i + open..c.len()).find(|&k| top[k]).unwrap_or(c.len());
        let inner: String = c[i + open..end.saturating_sub(2).max(i + open)].iter().collect();
        if !out.trim().is_empty() {
            out.push_str(&format!(" \\\\[{skip:.2}pt] "));
        }
        let half = width / (10.0 * size * stretch) / 2.0;
        out.push_str(&format!("$\\hspace{{{half:.4}em}}\\mathclap{{\\displaystyle {}}}\\hspace{{{half:.4}em}}$", inner.trim()));
        let rest: String = c[end..].iter().collect();
        if !rest.trim().is_empty() {
            out.push_str(&format!(" \\\\[{skip:.2}pt] "));
        }
        i = end;
    }
    out
}

/// Splits into lines at top-level `\\`, `\par` and, in a `paragraph` (a `text width`), `\newline`,
/// each with the space (pt) added after it: `\\[2pt]`, or the `\vskip 1em` after a `\par`.
fn manual_lines(text: &str, size: f64, paragraph: bool) -> Vec<(String, f64)> {
    let c: Vec<char> = text.chars().collect();
    let top = top_level(&c);
    let mut lines = vec![(String::new(), 0.0)];
    let mut i = 0;
    // a dimension from `i`: up to the char `end`, or a number and a unit of two letters
    let skip = |i: usize, end: Option<char>| -> (f64, usize) {
        let rest: String = c[i.min(c.len())..].iter().collect();
        let len = match end {
            Some(e) => rest.find(e).unwrap_or(rest.len()),
            None => {
                let t = rest.trim_start();
                let n = t.find(|x: char| !(x.is_ascii_digit() || x == '.' || x == '-')).unwrap_or(t.len());
                rest.len() - t.len() + (n + 2).min(t.len())
            }
        };
        let v = dimension(&rest[..len], size).map_or(0.0, |(v, _)| v);
        (v, i + rest[..len].chars().count())
    };
    let word = |i: usize, name: &str| {
        let n = name.chars().count();
        c[i + 1..].iter().take(n).copied().eq(name.chars()) && !c.get(i + 1 + n).is_some_and(|x| x.is_ascii_alphabetic())
    };
    while i < c.len() {
        match c[i] {
            // `\par` ends a paragraph, and a line
            '\\' if top[i] && word(i, "par") => {
                i += 4;
                let mut j = i;
                while c.get(j).is_some_and(|x| x.is_whitespace()) {
                    j += 1;
                }
                if c[j..].starts_with(&['\\', 'v', 's', 'k', 'i', 'p']) {
                    let (v, next) = skip(j + 6, None);
                    lines.last_mut().unwrap().1 += v;
                    i = next;
                }
                lines.push((String::new(), 0.0));
                continue;
            }
            // `\newline` breaks a paragraph's line (in a `text width`) as `\\` does; out of one,
            // in TikZ's alignment of lines, it does nothing (and the spaces after it go, as after
            // any control word)
            '\\' if top[i] && word(i, "newline") => {
                i += 8;
                if paragraph {
                    lines.push((String::new(), 0.0));
                } else {
                    while c.get(i) == Some(&' ') {
                        i += 1;
                    }
                }
                continue;
            }
            '\\' if top[i] && c.get(i + 1) == Some(&'\\') => {
                i += 2;
                if c.get(i) == Some(&'[') {
                    let (v, next) = skip(i + 1, Some(']'));
                    lines.last_mut().unwrap().1 += v;
                    i = next + 1;
                }
                lines.push((String::new(), 0.0));
                continue;
            }
            _ => {}
        }
        lines.last_mut().unwrap().0.push(c[i]);
        i += 1;
    }
    lines.into_iter().map(|(l, v)| (l.trim().to_string(), v)).collect()
}

/// Words separated by spaces at the top level (outside braces, math and environments).
/// The pieces a line may break between, each with whether a space comes before it: the words,
/// and the parts of a hyphenated word (`Fixed-` `Parameter`), as TeX may break after a hyphen.
fn words(line: &str) -> Vec<(String, bool)> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut space = false;
    let c: Vec<char> = line.chars().collect();
    let top = top_level(&c);
    for (i, &ch) in c.iter().enumerate() {
        match ch {
            ' ' if top[i] => {
                if !current.is_empty() {
                    out.push((std::mem::take(&mut current), space));
                }
                space = true;
                continue;
            }
            '-' if top[i] && i > 0 && c[i - 1].is_alphabetic() && c.get(i + 1).is_some_and(|x| x.is_alphabetic()) => {
                current.push(ch);
                out.push((std::mem::take(&mut current), space));
                space = false;
                continue;
            }
            _ => {}
        }
        current.push(ch);
    }
    if !current.is_empty() {
        out.push((current, space));
    }
    out
}

/// Characters of a height and a depth (em).
type Class = (f32, f32, &'static str);

/// The heights and depths (em) that TeX's fonts give the characters, by the first character of
/// each alphabet the label font draws them from (roman, math italic, typewriter, bold, sans serif)
/// and its characters of that height and depth: from the fonts' metrics (`cmr10`, `cmmi10`,
/// `cmtt10`, `cmbx10`, `cmss10`). TeX sizes a box by these, not by the ink of the letters, which
/// overshoots them (round letters dip below the baseline and rise above the x-height).
const TEX_METRICS: &[(char, &[Class])] = &[
    (
        'A',
        &[
            (0.1056, 0.0, "."),
            (0.1056, 0.1944, ","),
            (0.3669, 0.0, "="),
            (0.4306, 0.0, "-:acemnorsuvwxz"),
            (0.4306, 0.1944, ";gpqy"),
            (0.5833, 0.0833, "+"),
            (0.6151, 0.0, "t"),
            (0.6444, 0.0, "0123456789"),
            (0.6679, 0.0, "i"),
            (0.6679, 0.1944, "j"),
            (0.6833, 0.0, "ABCDEFGHIJKLMNOPRSTUVWXYZ"),
            (0.6833, 0.1944, "Q"),
            (0.6944, 0.0, "!&'?@bdfhkl"),
            (0.6944, 0.1944, "#"),
            (0.75, 0.0, "*"),
            (0.75, 0.0556, "$%"),
            (0.75, 0.25, "()/[]"),
        ],
    ),
    (
        '\u{1D434}',
        &[
            (0.4306, 0.0, "acemnorsuvwxz"),
            (0.4306, 0.1944, "gpqy"),
            (0.6151, 0.0, "t"),
            (0.6595, 0.0, "i"),
            (0.6595, 0.1944, "j"),
            (0.6833, 0.0, "ABCDEFGHIJKLMNOPRSTUVWXYZ"),
            (0.6833, 0.1944, "Q"),
            (0.6944, 0.0, "bdhkl"),
            (0.6944, 0.1944, "f"),
        ],
    ),
    (
        '\u{1D670}',
        &[
            (0.4306, 0.0, "acemnorsuvwxz"),
            (0.4306, 0.2222, "gpqy"),
            (0.5536, 0.0, "t"),
            (0.6111, 0.0, "0123456789ABCDEFGHIJKLMNOPRSTUVWXYZbdfhikl"),
            (0.6111, 0.1389, "Q"),
            (0.6111, 0.2222, "j"),
        ],
    ),
    (
        '\u{1D400}',
        &[
            (0.4444, 0.0, "acemnorsuvwxz"),
            (0.4444, 0.1944, "gpqy"),
            (0.6349, 0.0, "t"),
            (0.6444, 0.0, "0123456789"),
            (0.6861, 0.0, "ABCDEFGHIJKLMNOPRSTUVWXYZ"),
            (0.6861, 0.1944, "Q"),
            (0.6944, 0.0, "bdfhikl"),
            (0.6944, 0.1944, "j"),
        ],
    ),
    (
        '\u{1D5A0}',
        &[
            (0.4444, 0.0, "acemnorsuvwxz"),
            (0.4444, 0.1944, "gpqy"),
            (0.5714, 0.0, "t"),
            (0.6556, 0.0, "0123456789"),
            (0.6794, 0.0, "i"),
            (0.6794, 0.1944, "j"),
            (0.6944, 0.0, "ABCDEFGHIJKLMNOPRSTUVWXYZbdfhkl"),
            (0.6944, 0.125, "Q"),
        ],
    ),
];

/// The character of the alphabet that starts at `first` (a mathematical alphanumeric one, or the
/// ASCII roman for `A`) standing for `c`, as the label font has it.
fn in_alphabet(first: char, c: char) -> Option<char> {
    if first == 'A' {
        return Some(c);
    }
    let offset = match c {
        'A'..='Z' => c as u32 - 'A' as u32,
        'a'..='z' => c as u32 - 'a' as u32 + 26,
        // a digit (the tables list letters and digits only, the digits of the alphabets that
        // have them in Unicode: typewriter, bold, sans serif)
        _ => {
            let zero = match first {
                '\u{1D670}' => 0x1D7F6,
                '\u{1D400}' => 0x1D7CE,
                _ => 0x1D7E2,
            };
            return char::from_u32(zero + (c as u32 - '0' as u32));
        }
    };
    // the italic h is Planck's constant, out of the block
    if first == '\u{1D434}' && c == 'h' {
        return Some('\u{210E}');
    }
    char::from_u32(first as u32 + offset)
}

/// TeX's height and depth of the label font's glyphs that stand for Computer Modern characters, in
/// font units.
fn tex_metrics() -> &'static std::collections::HashMap<u16, (f32, f32)> {
    static METRICS: std::sync::OnceLock<std::collections::HashMap<u16, (f32, f32)>> = std::sync::OnceLock::new();
    METRICS.get_or_init(|| {
        let font = FONT.get();
        let em = font.units_per_em();
        let mut m = std::collections::HashMap::new();
        for (first, classes) in TEX_METRICS {
            for (h, d, chars) in *classes {
                for c in chars.chars() {
                    if let Some(g) = in_alphabet(*first, c).and_then(|c| font.glyph(c)) {
                        m.insert(g.0, (h * em, d * em));
                    }
                }
            }
        }
        m
    })
}

/// The box as tall and deep as TeX makes it: where its height (or depth) is that of a glyph's ink,
/// that of the glyphs' metrics in TeX's font instead. A height set by a rule, a strut or `\smash`
/// stays.
fn tex_extents(mut b: MathBox) -> MathBox {
    let font = FONT.get();
    let metrics = tex_metrics();
    let (mut ink_top, mut ink_bottom, mut top, mut bottom) = (f32::MIN, f32::MIN, f32::MIN, f32::MIN);
    for item in &b.items {
        if let mathra::layout::Item::Glyph { id, y, scale, .. } = item {
            let (_, y0, _, y1) = font.limits(*id);
            let (h, d) = metrics.get(&id.0).copied().unwrap_or((y1.max(0.0), (-y0).max(0.0)));
            ink_top = ink_top.max(y + y1.max(0.0) * scale);
            ink_bottom = ink_bottom.max(-y + (-y0).max(0.0) * scale);
            top = top.max(y + h * scale);
            bottom = bottom.max(-y + d * scale);
        }
    }
    // (font units: a thousandth of an em)
    if top > f32::MIN && (b.height - ink_top).abs() < 0.5 {
        b.height = top.max(0.0);
    }
    if bottom > f32::MIN && (b.depth - ink_bottom).abs() < 0.5 {
        b.depth = bottom.max(0.0);
    }
    b
}

thread_local! {
    /// How text is kerned: as TeX's OT1 fonts, or T1 ones after `\usepackage[T1]{fontenc}`.
    static KERNING: std::cell::Cell<mathra::layout::kerning::TextKerning> = const { std::cell::Cell::new(mathra::layout::kerning::TextKerning::Ot1) };
}

/// Text from now on kerned as `kerning` says (a figure starts with OT1).
pub fn set_kerning(kerning: mathra::layout::kerning::TextKerning) {
    KERNING.with(|k| k.set(kerning));
}

/// `sc`: in small capitals (the node font's `\scshape`).
fn typeset(text: &str, bold: bool, sc: bool, face: mathra::font::MathFont, warnings: &mut Vec<String>) -> MathBox {
    // (Times is kerned by `times`, as TeX's `ptm`)
    let kerning = if face == FONT { KERNING.with(|k| k.get()) } else { mathra::layout::kerning::TextKerning::None };
    // no fallback fonts, whatever mathra's features: every glyph is the face's, which the figure's defs draw
    let options = mathra::Options { display: false, text_kerning: kerning, fallback_fonts: Vec::new(), ..Default::default() };
    let text = if sc { format!("\\textsc{{{text}}}") } else { text.to_string() };
    let tex = if bold { format!("\\text{{\\textbf{{{text}}}}}") } else { format!("\\text{{{text}}}") };
    match mathra::typeset_in_font(&tex, &options, face) {
        Ok((c, applied)) => {
            warnings.extend(applied.into_iter().map(|a| format!("{} {}", a.rule, a.description)));
            // (TeX's metrics are Computer Modern's; Times' letters are drawn in Termes)
            if face == FONT {
                tex_extents(c)
            } else {
                crate::times::retarget(c, bold)
            }
        }
        // even in tolerant mode mathra refuses some input (nesting past its limit)
        Err(e) => {
            warnings.push(format!("label '{text}' not typeset: {e}"));
            MathBox::empty()
        }
    }
}

/// How much wider than its 10pt design TeX's font of `pt` points sets text, in the family the text
/// is in: LaTeX picks the Computer Modern of the design size at or below it (`cmr7` for
/// `\scriptsize`, `cmr17` for `\Huge`), whose letters are proportionally wider at small sizes and
/// narrower at large ones, while the label font has only the 10pt design. From the fonts' metrics.
fn design_width(pt: f64, family: char) -> f64 {
    let table: &[(f64, f64)] = match family {
        // cmtt8, cmtt9, cmtt10 and cmtt12
        't' => &[(0.0, 1.0119), (9.0, 1.0), (10.0, 1.0), (12.0, 0.9802)],
        // cmmi5 … cmmi12
        'm' => &[(0.0, 1.4283), (6.0, 1.2526), (7.0, 1.1559), (8.0, 1.0584), (9.0, 1.026), (10.0, 1.0), (12.0, 0.9784)],
        // cmss8 (from 5pt) … cmss17
        's' => &[(0.0, 1.0625), (9.0, 1.0278), (10.0, 1.0), (12.0, 0.9792), (17.0, 0.9404)],
        // cmbx5 … cmbx12
        'b' => &[(0.0, 1.3083), (6.0, 1.1996), (7.0, 1.1221), (8.0, 1.064), (9.0, 1.0285), (10.0, 1.0), (12.0, 0.9774)],
        // cmr5 … cmr17
        _ => &[(0.0, 1.3611), (6.0, 1.2222), (7.0, 1.1389), (8.0, 1.0625), (9.0, 1.0278), (10.0, 1.0), (12.0, 0.9791), (17.0, 0.9172)],
    };
    table.iter().rev().find(|(from, _)| pt + 0.01 >= *from).map_or(1.0, |(_, w)| *w)
}

/// The quad (`1em`) of TeX's roman font of `pt` points, or its sans serif one, over `pt`.
pub fn quad(pt: f64, sans: bool) -> f64 {
    design_width(pt, if sans { 's' } else { 'r' })
}

/// The x-height (`1ex`) of TeX's roman font, or its sans serif one, over `pt`.
pub fn x_height(sans: bool) -> f64 {
    if sans {
        0.4444
    } else {
        0.4306
    }
}

/// The family most of the text is set in: typewriter (`t`) when all of it is `\texttt`, math (`m`)
/// when all of it is between dollars, else bold (`b`) or roman (`r`).
fn family(text: &str, font: &str, bold: bool) -> char {
    if font.contains("\\ttfamily") || text.contains("\\ttfamily") {
        return 't';
    }
    if font.contains("\\sffamily") || text.contains("\\sffamily") {
        return 's';
    }
    // the text outside the groups that `open` starts and `close` ends
    let outside = |t: &str, open: &str, close: char| -> String {
        let mut out = String::new();
        let mut rest = t;
        while let Some(i) = rest.find(open) {
            out.push_str(&rest[..i]);
            rest = &rest[i + open.len()..];
            rest = rest.find(close).map_or("", |j| &rest[j + 1..]);
        }
        out.push_str(rest);
        out
    };
    // letters or digits, not counting the names of control words
    let letters = |t: &str| {
        t.split('\\').enumerate().any(|(k, w)| {
            let w = if k == 0 { w } else { w.trim_start_matches(|c: char| c.is_ascii_alphabetic()) };
            w.chars().any(|c| c.is_alphanumeric())
        })
    };
    if !letters(text) {
        return 'r';
    }
    if !letters(&outside(text, "\\texttt{", '}')) {
        return 't';
    }
    if !letters(&outside(text, "$", '$')) {
        return 'm';
    }
    if bold {
        'b'
    } else {
        'r'
    }
}

/// The box `f` times as wide, its items moved with it (the glyphs are widened when drawn).
fn widen(mut b: MathBox, f: f32) -> MathBox {
    use mathra::layout::Item;
    for item in &mut b.items {
        match item {
            Item::Glyph { x, .. } => *x *= f,
            Item::Rule { x, width, .. } => {
                *x *= f;
                *width *= f;
            }
            Item::Line { x1, x2, .. } => {
                *x1 *= f;
                *x2 *= f;
            }
            Item::Ellipse { cx, rx, .. } => {
                *cx *= f;
                *rx *= f;
            }
            // (moved with the box; what it holds is widened when drawn, as glyphs are)
            Item::Transformed { matrix, .. } => matrix[4] *= f,
            // a group with HTML attributes is drawn where its items are: they are widened as the box's
            Item::Group { items, bounds, .. } => {
                *items = widen(MathBox { items: std::mem::take(items), ..MathBox::empty() }, f).items;
                bounds[0] *= f;
                bounds[2] *= f;
            }
        }
    }
    b.width *= f;
    b
}

/// A text that is a `tabular` (`\begin{tabular}{lc} a & b \\ c & d \end{tabular}`): its rows,
/// the columns as wide as their widest cell, aligned as the preamble says, `\tabcolsep` (6pt)
/// on both sides of each, and each row a strut high (its baselines a `\baselineskip` apart).
fn tabular(text: &str, size: f64, stretch: f64, cell: &mut dyn FnMut(&str, &mut Vec<String>) -> MathBox, warnings: &mut Vec<String>) -> Option<Vec<MathBox>> {
    let body = text.trim().strip_prefix("\\begin{tabular}")?.strip_suffix("\\end{tabular}")?;
    let body = body.trim_start().strip_prefix('{')?;
    // the preamble runs to its matching brace
    let mut depth = 0;
    let close = body
        .char_indices()
        .find(|&(_, c)| {
            match c {
                '{' => depth += 1,
                '}' if depth == 0 => return true,
                '}' => depth -= 1,
                _ => {}
            }
            false
        })?
        .0;
    // the columns, and whether an `@{…}` next to one takes its \tabcolsep away on that side
    let mut items = Vec::new();
    let mut level = 0;
    for c in body[..close].chars() {
        match c {
            '{' => level += 1,
            '}' => level -= 1,
            '@' | 'l' | 'c' | 'r' if level == 0 => items.push(c),
            _ => {}
        }
    }
    let columns: Vec<char> = items.iter().copied().filter(|c| *c != '@').collect();
    let mut bare_left = Vec::new();
    let mut bare_right = Vec::new();
    for (k, c) in items.iter().enumerate() {
        if *c != '@' {
            bare_left.push(k > 0 && items[k - 1] == '@');
            bare_right.push(items.get(k + 1) == Some(&'@'));
        }
    }
    let body = &body[close + 1..];
    // rows at the top-level `\\`, cells at the top-level `&`
    let mut rows: Vec<Vec<String>> = vec![vec![String::new()]];
    let mut level = 0;
    let c: Vec<char> = body.chars().collect();
    let mut i = 0;
    while i < c.len() {
        match c[i] {
            '{' => level += 1,
            '}' => level -= 1,
            '&' if level == 0 => {
                rows.last_mut().unwrap().push(String::new());
                i += 1;
                continue;
            }
            '\\' if level == 0 && c.get(i + 1) == Some(&'\\') => {
                rows.push(vec![String::new()]);
                i += 2;
                continue;
            }
            _ => {}
        }
        rows.last_mut().unwrap().last_mut().unwrap().push(c[i]);
        i += 1;
    }
    let rows: Vec<Vec<String>> =
        rows.into_iter().map(|r| r.into_iter().map(|s| s.replace("\\hline", "").trim().to_string()).collect::<Vec<_>>()).filter(|r| r.iter().any(|s| !s.is_empty())).collect();
    let boxes: Vec<Vec<MathBox>> = rows.iter().map(|r| r.iter().map(|s| if s.is_empty() { MathBox::empty() } else { cell(s, warnings) }).collect()).collect();
    let n = boxes.iter().map(Vec::len).max().unwrap_or(0).max(columns.len());
    let widths: Vec<f32> = (0..n).map(|k| boxes.iter().filter_map(|r| r.get(k)).map(|b| b.width).fold(0.0, f32::max)).collect();
    // font units of the label (an em is 1000), the label widening its boxes afterwards
    let units = |pt: f64| (pt / (10.0 * size) * 1000.0 / stretch) as f32;
    let sep = units(6.0);
    let strut = cell(&format!("$\\rule[{:.4}em]{{0em}}{{{:.4}em}}$", -0.36, 1.2), warnings);
    let mut out = Vec::new();
    for row in boxes {
        let mut line = strut.clone();
        for (k, w) in widths.iter().enumerate() {
            let b = row.get(k).cloned().unwrap_or_else(MathBox::empty);
            let room = w - b.width;
            let left = match columns.get(k) {
                Some('r') => room,
                Some('c') | None => room / 2.0,
                _ => 0.0,
            };
            let before = if bare_left.get(k) == Some(&true) { 0.0 } else { sep };
            let after = if bare_right.get(k) == Some(&true) { 0.0 } else { sep };
            line.append(&MathBox::space(before + left));
            line.append(&b);
            line.append(&MathBox::space(room - left + after));
        }
        out.push(line);
    }
    Some(out)
}

/// TeX's space factor after `word`, from `sf` before it: 3000 after the end of a sentence (`.`, `?`,
/// `!`, but not after a capital), 2000 after a colon, 1500 after a semicolon, 1250 after a comma;
/// closing parentheses and quotes keep it.
fn space_factor(word: &str, mut sf: f32) -> f32 {
    for c in word.chars() {
        let code = match c {
            'A'..='Z' => 999.0,
            '.' | '?' | '!' => 3000.0,
            ':' => 2000.0,
            ';' => 1500.0,
            ',' => 1250.0,
            ')' | ']' | '\'' | '"' => continue,
            _ => 1000.0,
        };
        sf = if code > 1000.0 && sf < 1000.0 { 1000.0 } else { code };
    }
    sf
}

/// A line of text broken into lines `width` wide (label font units, a thousand to the em) as TeX
/// breaks the paragraph of a node with a `text width`, by `align`, TikZ's alignment: `l` (left,
/// and no alignment), `r`, `c`, their flush variants `L`, `R`, `C`, and `j` (justify). The ragged
/// ones (`text ragged` and the like) have spaces of a third of an em (half after a sentence) and
/// lines that may fall 2 em short; the flush ones and justified text the font's spaces, which
/// stretch and shrink, with lines that may fall short by any amount, or none. Words after a space
/// may be hyphenated, as in English, and the first one when `after_glue`.
fn paragraph_lines(line: &str, width: f32, align: char, after_glue: bool, typeset: &mut dyn FnMut(&str, &mut Vec<String>) -> MathBox, warnings: &mut Vec<String>) -> Vec<MathBox> {
    use paragraph::{Item, Skip};
    let pieces = words(line);
    if pieces.is_empty() {
        return vec![MathBox::empty()];
    }
    let em = 1000.0;
    let two = Skip { width: 0.0, stretch: 2.0 * em, fil: 0.0 };
    let fill = Skip { width: 0.0, stretch: 0.0, fil: 1.0 };
    // the skips at both sides, and whether the last line may end short (`\parfillskip`)
    let (left, right, short_last) = match align {
        'l' => (Skip::default(), two, true),
        'L' => (Skip::default(), fill, true),
        'r' => (two, Skip::default(), false),
        'R' => (fill, Skip::default(), false),
        'c' => (two, two, false),
        'C' => (fill, fill, false),
        _ => (Skip::default(), Skip::default(), true),
    };
    let ragged = matches!(align, 'l' | 'r' | 'c');
    let hyphen_width = typeset("-", &mut Vec::new()).width;
    let mut items = Vec::new();
    // the text of each box item
    let mut texts: Vec<String> = Vec::new();
    let mut sf = 1000.0;
    for (k, (w, spaced)) in pieces.iter().enumerate() {
        if k > 0 {
            items.push(if *spaced {
                let (mut width, mut stretch, mut shrink) = if ragged { (em / 3.0, 0.0, 0.0) } else { (em / 3.0, em / 6.0, em / 9.0) };
                if ragged && sf >= 2000.0 {
                    width = em / 2.0;
                } else if !ragged {
                    if sf >= 2000.0 {
                        width += em / 9.0;
                    }
                    stretch *= sf / 1000.0;
                    shrink *= 1000.0 / sf;
                }
                Item::Glue { width, stretch, fil: 0.0, shrink }
            } else {
                // after an explicit hyphen (`Fixed-Parameter`)
                Item::Penalty { width: 0.0, penalty: 50.0, flagged: true, hyphen: false }
            });
            texts.push(String::new());
        }
        // a word after a space, of letters and maybe punctuation after them, may be hyphenated
        let end = w.find(|c: char| !c.is_ascii_alphabetic()).unwrap_or(w.len());
        let breaks = if (*spaced || (k == 0 && after_glue)) && w[end..].chars().all(|c| ".,;:!?)'-".contains(c)) { hyphen::hyphenate(&w[..end], 2, 3) } else { Vec::new() };
        let mut from = 0;
        for b in breaks {
            items.push(Item::Box(typeset(&w[from..b], &mut Vec::new()).width));
            texts.push(w[from..b].to_string());
            items.push(Item::Penalty { width: hyphen_width, penalty: 50.0, flagged: true, hyphen: true });
            texts.push(String::new());
            from = b;
        }
        items.push(Item::Box(typeset(&w[from..], &mut Vec::new()).width));
        texts.push(w[from..].to_string());
        sf = space_factor(w, sf);
    }
    items.push(Item::Penalty { width: 0.0, penalty: 10000.0, flagged: false, hyphen: false });
    items.push(Item::Glue { width: 0.0, stretch: 0.0, fil: if short_last { 1.0 } else { 0.0 }, shrink: 0.0 });
    items.push(Item::Penalty { width: 0.0, penalty: -10000.0, flagged: false, hyphen: false });
    // (the last pass of the line breaking always breaks: it takes overfull lines when it must)
    let lines = paragraph::break_lines(&items, width, left, right, 3.0 * em);
    let mut out = Vec::new();
    for l in lines {
        let mut b = MathBox::empty();
        let mut word = String::new();
        let mut flush = |word: &mut String, b: &mut MathBox, warnings: &mut Vec<String>| {
            if !word.is_empty() {
                b.append(&typeset(word, warnings));
                word.clear();
            }
        };
        for k in l.start..l.end.min(items.len()) {
            match items[k] {
                Item::Box(_) => word.push_str(&texts[k]),
                Item::Glue { width, stretch, shrink, .. } => {
                    flush(&mut word, &mut b, warnings);
                    let w = if l.ratio > 0.0 { width + l.ratio * stretch } else { width + l.ratio * shrink };
                    if w > 0.0 {
                        b.append(&MathBox::space(w));
                    }
                }
                Item::Penalty { .. } => {}
            }
        }
        if l.hyphen {
            word.push('-');
        }
        flush(&mut word, &mut b, warnings);
        out.push(b);
    }
    out
}

/// A paragraph broken into lines as `paragraph_lines` does, with the lists in it (`itemize`,
/// `enumerate`) set as LaTeX sets them in a minipage: each item's lines 2.5 em (25 pt) in, its
/// label ending half an em (5 pt) before them, `\topsep` around the list (not at the text's end)
/// and `\itemsep` and `\parsep` between items, as the font size has them. Each line with the space (pt) after it
/// besides the baselines'. `units` turns pt into the lines' units.
#[allow(clippy::too_many_arguments)]
fn list_lines(
    text: &str,
    width: f32,
    align: char,
    after_glue: bool,
    size: f64,
    units: &dyn Fn(f64) -> f32,
    typeset: &mut dyn FnMut(&str, &mut Vec<String>) -> MathBox,
    warnings: &mut Vec<String>,
) -> Vec<(MathBox, f64)> {
    let start = ["itemize", "enumerate"].iter().filter_map(|env| text.find(&format!("\\begin{{{env}}}")).map(|i| (i, *env))).min();
    let Some((at, env)) = start else {
        return paragraph_lines(text, width, align, after_glue, typeset, warnings).into_iter().map(|b| (b, 0.0)).collect();
    };
    let open = format!("\\begin{{{env}}}");
    let close = format!("\\end{{{env}}}");
    let body_start = at + open.len();
    let body_end = text[body_start..].find(&close).map_or(text.len(), |e| body_start + e);
    let (before, body, after) = (&text[..at], &text[body_start..body_end], text.get(body_end + close.len()..).unwrap_or(""));
    // LaTeX's \@listi of the size: \topsep, \parsep (and \itemsep, the same); only \small and
    // \footnotesize set their own, the smaller sizes keep \normalsize's
    let (topsep, parsep) = if (0.85..0.95).contains(&size) {
        (4.0, 2.0)
    } else if (0.75..0.85).contains(&size) {
        (3.0, 2.0)
    } else {
        (8.0, 4.0)
    };
    let mut out: Vec<(MathBox, f64)> = Vec::new();
    if !before.trim().is_empty() {
        out.extend(paragraph_lines(before.trim(), width, align, after_glue, typeset, warnings).into_iter().map(|b| (b, 0.0)));
    }
    let (margin, label_end) = (units(25.0), units(20.0));
    for (n, item) in body.split("\\item").skip(1).enumerate() {
        // the gap before the item: \topsep before the first, \itemsep and \parsep between them
        if let Some(last) = out.last_mut() {
            last.1 += if n == 0 { topsep } else { 2.0 * parsep };
        }
        let mut item = item.trim();
        let label = match item.strip_prefix('[').and_then(|r| r.split_once(']')) {
            Some((l, rest)) => {
                item = rest.trim();
                l.to_string()
            }
            None if env == "enumerate" => format!("{}.", n + 1),
            None => "$\\bullet$".to_string(),
        };
        let label = typeset(&label, warnings);
        // (a list sets `\rightskip` to LaTeX's `\@rightskip`: ragged right text is flush there)
        let item_align = if align == 'l' { 'L' } else { align };
        for (k, line) in paragraph_lines(item, width - margin, item_align, false, typeset, warnings).into_iter().enumerate() {
            let mut b = MathBox::empty();
            if k == 0 {
                let left = (label_end - label.width).max(0.0);
                b.append(&MathBox::space(left));
                b.append(&label);
                b.append(&MathBox::space(margin - left - label.width));
            } else {
                b.append(&MathBox::space(margin));
            }
            b.append(&line);
            out.push((b, 0.0));
        }
    }
    // (at the end of the text the minipage takes the last space away)
    if let Some(last) = out.last_mut().filter(|_| !after.trim().is_empty()) {
        last.1 += topsep;
    }
    if !after.trim().is_empty() {
        out.extend(list_lines(after.trim(), width, align, false, size, units, typeset, warnings));
    }
    out
}

/// A text that is all a paragraph box, `\parbox[t]{4cm}{…}` or `\begin{minipage}{4cm}…\end{minipage}`
/// (its position and height options aside): its width, and its text.
pub fn paragraph_box(text: &str) -> Option<(String, String)> {
    // a `{…}` group at the start (spaces skipped): what it holds and what follows
    fn group(s: &str) -> Option<(&str, &str)> {
        let s = s.trim_start().strip_prefix('{')?;
        let mut level = 0;
        for (i, c) in s.char_indices() {
            match c {
                '{' => level += 1,
                '}' if level == 0 => return Some((&s[..i], &s[i + 1..])),
                '}' => level -= 1,
                _ => {}
            }
        }
        None
    }
    // the `[t]`, `[3cm]`, `[b]` options
    fn options(mut s: &str) -> &str {
        while let Some((_, rest)) = s.trim_start().strip_prefix('[').and_then(|r| r.split_once(']')) {
            s = rest;
        }
        s
    }
    let t = text.trim();
    if let Some(rest) = t.strip_prefix("\\parbox") {
        let (width, rest) = group(options(rest))?;
        let (body, rest) = group(rest)?;
        return rest.trim().is_empty().then(|| (width.to_string(), body.to_string()));
    }
    let rest = t.strip_prefix("\\begin{minipage}")?.strip_suffix("\\end{minipage}")?;
    let (width, body) = group(options(rest))?;
    // (one minipage, not two side by side)
    (!body.contains("\\begin{minipage}")).then(|| (width.to_string(), body.to_string()))
}

/// amsmath's `\implies`, `\impliedby` and `\iff`: long arrows with thick spaces around them.
fn long_arrows(text: &str) -> String {
    let mut out = text.to_string();
    for (name, arrow) in [("implies", "Longrightarrow"), ("impliedby", "Longleftarrow"), ("iff", "Longleftrightarrow")] {
        let cs = format!("\\{name}");
        let mut done = String::new();
        let mut rest = out.as_str();
        while let Some(i) = rest.find(&cs) {
            let after = &rest[i + cs.len()..];
            done.push_str(&rest[..i]);
            if after.starts_with(|c: char| c.is_ascii_alphabetic()) {
                done.push_str(&cs);
            } else {
                done.push_str(&format!("\\;\\{arrow}\\;"));
            }
            rest = after;
        }
        done.push_str(rest);
        out = done;
    }
    out
}

/// Typesets a node's text. `text_width` (cm) turns on automatic wrapping.
pub fn compose_label(text: &str, font: &str, text_width: Option<f64>, align: char, color: Color, warnings: &mut Vec<String>) -> Label {
    // the lines of a paragraph sit as the alignment says, a justified one to the left
    let drawn = if align == 'j' { 'l' } else { align.to_ascii_lowercase() };
    let size = font_size(&format!("{font} {text}"));
    let bold = font.contains("\\bfseries") || font.contains("\\bf ") || font.trim_end().ends_with("\\bf") || text.contains("\\bfseries") || text.contains("\\bf ");
    let scale = EM_CM * size / 1000.0;
    let sc = font.contains("\\scshape");
    let all = [("\\ttfamily", "mathtt"), ("\\itshape", "mathit"), ("\\sffamily", "mathsf")].into_iter().find(|(c, _)| font.contains(c) || text.contains(c)).map(|(_, f)| f);
    // Times is one design scaled to every size, as STIX Two is: Computer Modern's widths of its
    // other design sizes do not apply
    let face = if [font, text].iter().any(|t| t.contains("\\fontfamily{ptm}")) { TIMES } else { FONT };
    let design_width = |pt: f64, family: char| if face == FONT { design_width(pt, family) } else { 1.0 };
    let mut stretch = design_width(10.0 * size, family(text, font, bold));
    if sc && face == FONT {
        // cmcsc10 is an extended design: its capitals are about 9% wider than cmr10's, so a word in
        // small capitals is 1.079 times as wide as the label font sets it (59.17pt for
        // "Complexity" in TeX against 54.83)
        stretch *= 1.079;
    }
    if all == Some("mathit") && face == FONT {
        // text italic (cmti) is narrower than the math italic it is drawn with: by 0.943 on the
        // lowercase letters of English text, weighted by how often they come
        stretch *= 0.943;
    }
    let mut boxes = Vec::new();
    // a text that is one group (`{upper line \\ lower line}`): what it groups
    let text = crate::keys::strip_braces(text);
    // pictures (`\begin{picture}…`) as `\tikrapicture{k}`, set as boxes of their size below
    let (text, pictures) = if text.contains("\\begin{picture}") { picture::extract(text, warnings) } else { (text.to_string(), Vec::new()) };
    let text = text.as_str();
    let text_raw = text;
    // the text after glue (`\hskip0pt`, as mindmap concepts begin): its first word may be hyphenated
    let lead_glue = text.trim_start().starts_with("\\hskip");
    let text = if all == Some("mathtt") { clean(text) } else { ligatures(&clean(text)) };
    let text = long_arrows(&text);
    if let Some(rows) = tabular(&text, size, stretch, &mut |cell: &str, w: &mut Vec<String>| typeset(&families(cell, all, bold, face), bold, sc, face, w), warnings) {
        let lines = rows.into_iter().map(|b| widen(b, stretch as f32)).collect();
        let mut label = Label { lines, scale, color, align: drawn, line_height: 1.2 * EM_CM * size, stretch, skips: Vec::new(), font: face, lift: 0.0 };
        // a `tabular` is centered on the math axis (2.5 pt at 10 pt)
        let n = label.lines.len();
        if n > 0 {
            let top = label.lines[0].height as f64 * scale;
            let bottom = label.baseline(n - 1) - label.lines[n - 1].depth as f64 * scale;
            label.lift = 0.25 * EM_CM * size - (top + bottom) / 2.0;
        }
        return label;
    }
    // the space after each line besides the baselines' (cm)
    let mut skips: Vec<f64> = Vec::new();
    let text = match text_width {
        Some(w) => displays(&text, w / 2.54 * 72.27, size, stretch),
        None => text,
    };
    let manual = manual_lines(&text, size, text_width.is_some());
    // lines aligned without a text width are the cells of an `\halign`, each a group: a size
    // command in one is that line's alone, and the baselines are the node font's apart
    // (from the text before its size commands go)
    let raw = manual_lines(text_raw, size, text_width.is_some());
    let sizes: Vec<f64> = if raw.len() == manual.len() { raw.iter().map(|(l, _)| font_size(&format!("{font} {l}"))).collect() } else { vec![size; manual.len()] };
    let per_line = text_width.is_none() && sizes.windows(2).any(|w| w[0] != w[1]);
    let fam = family(text_raw, font, bold);
    let (size, scale, line_height) = if per_line { (sizes[0], EM_CM * sizes[0] / 1000.0, 1.2 * EM_CM * font_size(font)) } else { (size, scale, 1.2 * EM_CM * size) };
    if per_line {
        stretch *= design_width(10.0 * size, fam) / design_width(10.0 * font_size(&format!("{font} {text_raw}")), fam);
    }
    for (k, (line, after)) in manual.into_iter().enumerate() {
        let line = if line.contains("\\hbox") { hboxes(&line, size, stretch) } else { line };
        let line = if pictures.is_empty() { line } else { picture_boxes(&line, &pictures, size, stretch) };
        let line = if line.contains("\\vrule") {
            // the rules run to the height and depth of the rest of the line
            let bare = typeset(&families(&rules(&line, size, None), all, bold, face), bold, sc, face, &mut Vec::new());
            let pt = 10.0 * size / 1000.0;
            rules(&line, size, Some((bare.height as f64 * pt, bare.depth as f64 * pt)))
        } else {
            line
        };
        match text_width {
            None => {
                // (a line in another size is in that size's design width too)
                let b = scaled(typeset(&families(&line, all, bold, face), bold, sc, face, warnings), (sizes[k] / size) as f32);
                boxes.push(widen(b, (design_width(10.0 * sizes[k], fam) / design_width(10.0 * size, fam)) as f32));
            }
            Some(max) => {
                let units = |pt: f64| (pt / (10.0 * size) * 1000.0 / stretch) as f32;
                let lines = list_lines(
                    &line,
                    (max / scale / stretch) as f32,
                    align,
                    k == 0 && lead_glue,
                    size,
                    &units,
                    // (the family word by word: a line in it is one formula, which would not break)
                    &mut |w: &str, warnings: &mut Vec<String>| typeset(&families(w, all, bold, face), bold, sc, face, warnings),
                    warnings,
                );
                for (b, skip) in lines {
                    boxes.push(b);
                    skips.resize(boxes.len(), 0.0);
                    *skips.last_mut().unwrap() = skip * 2.54 / 72.27;
                }
            }
        }
        skips.resize(boxes.len(), 0.0);
        if let Some(s) = skips.last_mut() {
            *s += after * 2.54 / 72.27;
        }
    }
    let mut lines: Vec<mathra::layout::MathBox> = boxes.into_iter().map(|b| widen(b, stretch as f32)).collect();
    if !pictures.is_empty() {
        let unit = (2.54 / 72.27 / scale) as f32;
        lines.iter_mut().for_each(|b| draw_pictures(&mut b.items, &pictures, unit));
    }
    // TeX's interline glue: lines that would come closer than \lineskiplimit (0pt) at their
    // baselines' distance are set \lineskip (1pt) apart instead (a tall formula, a molecule)
    skips.resize(lines.len(), 0.0);
    for k in 0..lines.len().saturating_sub(1) {
        let gap = line_height - (lines[k].depth + lines[k + 1].height) as f64 * scale;
        if gap < 0.0 {
            skips[k] += 2.54 / 72.27 - gap;
        }
    }
    let mut label = Label { lines, scale, color, align: drawn, line_height, stretch, skips, font: face, lift: 0.0 };
    // lines of their own (`align=`) are an \halign in a \vbox, on the last one's baseline; in a
    // text width, a \vtop on the first's
    if text_width.is_none() {
        label.lift = -label.baseline(label.lines.len().saturating_sub(1));
    }
    label
}

/// The color that marks the box of picture `k` in a typeset line.
fn picture_mark(k: usize) -> String {
    format!("tikrapicture{k}")
}

/// `\tikrapicture{k}` as a box of picture `k`'s size, in ems of the label's font (`stretch`, how
/// much the label widens its boxes afterwards), marked by its color for `draw_pictures`.
fn picture_boxes(line: &str, pictures: &[picture::Picture], size: f64, stretch: f64) -> String {
    let mut out = line.to_string();
    let em = 10.0 * size;
    for (k, p) in pictures.iter().enumerate() {
        let b = format!("$\\textcolor{{{}}}{{\\rule{{{:.5}em}}{{{:.5}em}}}}$", picture_mark(k), p.width / em / stretch, p.height / em);
        out = out.replace(&format!("\\tikrapicture{{{k}}}"), &b);
    }
    out
}

/// The pictures in place of their boxes: what they draw, from their lower left corners, `unit`
/// label units per point.
fn draw_pictures(items: &mut Vec<mathra::layout::Item>, pictures: &[picture::Picture], unit: f32) {
    use mathra::layout::Item;
    use picture::Mark;
    let mut out = Vec::with_capacity(items.len());
    for item in items.drain(..) {
        let k = match &item {
            Item::Rule { color: Some(c), .. } => c.strip_prefix("tikrapicture").and_then(|k| k.parse::<usize>().ok()).filter(|k| *k < pictures.len()),
            _ => None,
        };
        let (Some(k), Item::Rule { x: ox, y: oy, .. }) = (k, &item) else {
            let mut item = item;
            if let Item::Transformed { items, .. } = &mut item {
                draw_pictures(items, pictures, unit);
            }
            out.push(item);
            continue;
        };
        let at = |x: f64, y: f64| (ox + x as f32 * unit, oy + y as f32 * unit);
        for m in &pictures[k].marks {
            out.push(match *m {
                Mark::Rule { x, y, width, height } => {
                    let (x, y) = at(x, y);
                    Item::Rule { x, y, width: width as f32 * unit, height: height as f32 * unit, color: None }
                }
                Mark::Line { x1, y1, x2, y2, thickness } => {
                    let ((x1, y1), (x2, y2)) = (at(x1, y1), at(x2, y2));
                    Item::Line { x1, y1, x2, y2, thickness: thickness as f32 * unit, color: None }
                }
                // a disk: a ring as thick as its radius
                Mark::Disk { x, y, diameter } => {
                    let (cx, cy) = at(x, y);
                    let r = diameter as f32 * unit / 4.0;
                    Item::Ellipse { cx, cy, rx: r, ry: r, thickness: 2.0 * r, color: None }
                }
                Mark::Ring { x, y, diameter, thickness } => {
                    let (cx, cy) = at(x, y);
                    let r = diameter as f32 * unit / 2.0;
                    Item::Ellipse { cx, cy, rx: r, ry: r, thickness: thickness as f32 * unit, color: None }
                }
            });
        }
    }
    *items = out;
}

/// The box `f` times as large, its items too.
fn scaled(mut b: MathBox, f: f32) -> MathBox {
    use mathra::layout::Item;
    if f == 1.0 {
        return b;
    }
    for item in &mut b.items {
        match item {
            Item::Glyph { x, y, scale, .. } => {
                *x *= f;
                *y *= f;
                *scale *= f;
            }
            Item::Rule { x, y, width, height, .. } => {
                *x *= f;
                *y *= f;
                *width *= f;
                *height *= f;
            }
            Item::Line { x1, y1, x2, y2, thickness, .. } => {
                *x1 *= f;
                *y1 *= f;
                *x2 *= f;
                *y2 *= f;
                *thickness *= f;
            }
            Item::Ellipse { cx, cy, rx, ry, thickness, .. } => {
                *cx *= f;
                *cy *= f;
                *rx *= f;
                *ry *= f;
                *thickness *= f;
            }
            // what it holds, mapped f times as large
            Item::Transformed { matrix, .. } => matrix.iter_mut().for_each(|m| *m *= f),
            Item::Group { items, bounds, .. } => {
                *items = scaled(MathBox { items: std::mem::take(items), ..MathBox::empty() }, f).items;
                bounds.iter_mut().for_each(|v| *v *= f);
            }
        }
    }
    b.width *= f;
    b.height *= f;
    b.depth *= f;
    b
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_line_break_before_letters_is_not_a_font_command() {
        // `\\it` is a line break and "it", not `\it`
        assert_eq!(clean("Does\\\\it do"), "Does\\\\it do");
        assert_eq!(clean("\\it x"), "x");
    }

    use super::*;

    #[test]
    fn rules_and_the_last_size() {
        let mut w = Vec::new();
        let r = compose_label("A\\vrule width 1pt height 2cm", "\\Huge", None, 'c', Color::BLACK, &mut w);
        assert!(w.is_empty(), "{w:?}");
        // 2cm high whatever the size
        assert!((r.height() - 2.0).abs() < 0.02, "{}", r.height());
        assert!((font_size("\\Huge \\scriptsize x") - 0.7).abs() < 1e-9);
    }

    #[test]
    fn wrapping_and_size() {
        let mut w = Vec::new();
        let r = compose_label("Check every value twice", "", Some(2.5), 'c', Color::BLACK, &mut w);
        assert!(r.lines.len() >= 2, "{}", r.lines.len());
        assert!(w.is_empty(), "{w:?}");
        assert!((font_size("\\Large x") - 1.44).abs() < 1e-9);
        assert!((font_size("\\large x") - 1.2).abs() < 1e-9);
        let r = compose_label("$m_1$ \\\\ second", "", None, 'c', Color::BLACK, &mut w);
        assert_eq!(r.lines.len(), 2);
    }

    #[test]
    fn lists() {
        let mut w = Vec::new();
        let r = compose_label("Steps: \\begin{enumerate} \\item one \\item two \\end{enumerate}", "", Some(4.0), 'l', Color::BLACK, &mut w);
        assert!(w.is_empty(), "{w:?}");
        assert_eq!(r.lines.len(), 3);
        // the items 2.5 em in, and space around them
        assert!(r.lines[1].width > r.lines[0].width, "{} {}", r.lines[1].width, r.lines[0].width);
        assert!(r.skips[0] > 0.0 && r.skips[1] > 0.0);
    }

    #[test]
    fn lines_break_at_the_top_level_only() {
        let lines = |t: &str, paragraph: bool| manual_lines(t, 1.0, paragraph).into_iter().map(|(l, _)| l).collect::<Vec<_>>();
        // the `\\` of an array, a display, an alignment and a matrix are their rows
        for t in [
            r"$\begin{array}{c} a \\ b \end{array}$",
            r"\[ a \\ b \]",
            r"\( a \\ b \)",
            r"$$ a \\ b $$",
            r"\begin{align*} a &= b \\ c &= d \end{align*}",
            r"\begin{tabular}{c} a \\ b \end{tabular}",
            r"{a \\ b}",
        ] {
            assert_eq!(lines(t, false).len(), 1, "{t}");
        }
        // a list and a centered text are lines of the node; `\$` is no math
        assert_eq!(lines(r"x \begin{center} a \\ b \end{center}", false).len(), 2);
        assert_eq!(lines(r"\$5 \\ \$6", false), ["\\$5", "\\$6"]);
        // a `$` inside `$$…$$`, a `\]` inside `$…$` and a `\begin` with no name do not end them
        assert_eq!(lines(r"$$ a \text{$b$} \\ c $$ \\ d", false).len(), 2);
        assert_eq!(lines(r"$a \] \\ b$ \\ c", false).len(), 2);
        assert_eq!(lines(r"\begin x \\ y", false).len(), 2);
        // `\newline` breaks a paragraph's line; elsewhere it is nothing, nor the spaces after it
        assert_eq!(lines(r"one\newline two", true), ["one", "two"]);
        assert_eq!(lines(r"one\newline two", false), ["onetwo"]);
        assert_eq!(lines(r"one\newlines", true).len(), 1);
        assert!(in_math("a $b") && in_math(r"\[ b") && !in_math(r"\[ b \]") && !in_math(r"a \$"));
    }

    #[test]
    fn words_keep_math_and_environments_whole() {
        let words = |t: &str| words(t).into_iter().map(|(w, _)| w).collect::<Vec<_>>();
        assert_eq!(words(r"see \[ a + b \] and $c d$"), ["see", r"\[ a + b \]", "and", "$c d$"]);
        assert_eq!(words(r"x \begin{matrix} a & b \end{matrix} y").len(), 3);
    }

    #[test]
    fn paragraph_boxes() {
        let pb = |t: &str| paragraph_box(t).map(|(w, b)| (w, b.trim().to_string()));
        assert_eq!(pb(r"\parbox[t]{2cm}{\centering a b}"), Some(("2cm".into(), r"\centering a b".into())));
        assert_eq!(pb(r"\parbox{3cm}{a {b} c}"), Some(("3cm".into(), "a {b} c".into())));
        assert_eq!(pb(r"\begin{minipage}[b][1cm]{4cm} a b \end{minipage}"), Some(("4cm".into(), "a b".into())));
        // text after the box, a box not closed, two minipages, or no box: not a paragraph box
        for t in [r"\parbox{2cm}{a} b", r"\parbox{2cm}{a", r"\begin{minipage}{1cm}a\end{minipage}\begin{minipage}{1cm}b\end{minipage}", "a b"] {
            assert_eq!(pb(t), None, "{t}");
        }
    }

    #[test]
    fn a_text_in_another_family_wraps() {
        let mut w = Vec::new();
        for font in ["\\sffamily", "\\ttfamily", "\\itshape"] {
            let r = compose_label("No flow from the first component", font, Some(2.0), 'c', Color::BLACK, &mut w);
            assert!(r.lines.len() >= 3, "{font}: {}", r.lines.len());
        }
        assert!(w.is_empty(), "{w:?}");
    }

    #[test]
    fn displays_are_lines_of_their_own() {
        let d = displays(r"is \[ x = 1 \] so", 100.0, 1.0, 1.0);
        assert_eq!(d, r"is  \\[10.00pt] $\hspace{5.0000em}\mathclap{\displaystyle x = 1}\hspace{5.0000em}$ \\[10.00pt]  so");
        // at the start or the end, no empty line; `$$` too; a `\\[2pt]` is no display
        assert_eq!(displays(r"$$y$$", 100.0, 1.0, 1.0), r"$\hspace{5.0000em}\mathclap{\displaystyle y}\hspace{5.0000em}$");
        assert_eq!(displays(r"a \\[2pt] b $x$", 100.0, 1.0, 1.0), r"a \\[2pt] b $x$");
        assert_eq!(manual_lines(&d, 1.0, true).len(), 3);
    }
}
