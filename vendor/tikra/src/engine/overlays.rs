//! Beamer's overlays in a picture: `\pause`, `\only<2->{…}`, `\uncover<…>{…}`, `\visible`,
//! `\invisible`, `\onslide`, `\alt` and `\draw<2->`. A figure with them is run once per slide; on
//! each, what is off is left out (`\only`, which takes no room) or drawn with no opacity
//! (`\uncover`, which keeps it). Written from the Beamer manual's account of how they behave.

use crate::canvas::Item;
use crate::tex::{self, Meaning, Token};
use crate::Error;

use super::{Cmd, Engine};

/// The slides of a specification, as ranges with both ends included.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Slides(Vec<(usize, usize)>);

impl Slides {
    pub fn has(&self, slide: usize) -> bool {
        self.0.iter().any(|&(a, b)| a <= slide && slide <= b)
    }
}

/// A number of a specification: `3`, `+`, `.`, `+(1)`, `.(-1)`. `pauses` is Beamer's counter of the
/// same name; `plus` says a `+` was used, which moves it on when the specification ends.
fn number(s: &str, pauses: usize, plus: &mut bool) -> Option<usize> {
    let s = s.trim();
    let (base, rest) = if let Some(r) = s.strip_prefix('+') {
        *plus = true;
        (pauses as i64, r)
    } else if let Some(r) = s.strip_prefix('.') {
        (pauses as i64 - 1, r)
    } else {
        return s.parse().ok();
    };
    let offset = rest.trim().strip_prefix('(').and_then(|r| r.strip_suffix(')')).map_or(Some(0), |r| r.trim().parse::<i64>().ok())?;
    usize::try_from((base + offset).max(1)).ok()
}

/// Reads `1`, `2-`, `-3`, `1,3-4`, `+-`, `beamer:2`, `alert@2`. The last slide it names, for how many
/// there are, comes with it.
pub fn parse(spec: &str, pauses: &mut usize) -> Option<(Slides, usize)> {
    let spec = spec.rsplit(':').next().unwrap_or(spec);
    let mut plus = false;
    let mut ranges = Vec::new();
    let mut last = 1;
    for term in spec.split(['|', ',']) {
        let term = term.rsplit('@').next().unwrap_or(term).trim();
        if term.is_empty() {
            continue;
        }
        // a `-` inside `(…)` is an offset's sign
        let mut depth = 0;
        let dash = term.char_indices().find(|&(i, c)| {
            match c {
                '(' => depth += 1,
                ')' => depth -= 1,
                _ => {}
            }
            c == '-' && depth == 0 && i < term.len()
        });
        let range = match dash {
            None => {
                let n = number(term, *pauses, &mut plus)?;
                (n, n)
            }
            Some((i, _)) => {
                let (a, b) = (term[..i].trim(), term[i + 1..].trim());
                let from = if a.is_empty() { 1 } else { number(a, *pauses, &mut plus)? };
                (from, if b.is_empty() { usize::MAX } else { number(b, *pauses, &mut plus)? })
            }
        };
        last = last.max(if range.1 == usize::MAX { range.0 } else { range.1 });
        ranges.push(range);
    }
    if plus {
        *pauses += 1;
    }
    (!ranges.is_empty()).then_some((Slides(ranges), last))
}

/// The overlay command `t` is, if the code has not given its name a meaning. They are not defined
/// as LaTeX sees it, so `\\providecommand{\\alt}` and `\\ifdefined\\alt` work as without Beamer:
/// the names are free, and only a command nobody defined is read as an overlay.
pub fn named(tex: &tex::Tex, t: &Token) -> Option<Cmd> {
    let Token::Cs(name) = t else { return None };
    if !matches!(tex.meaning(t), Meaning::Undefined) {
        return None;
    }
    match &**name {
        "only" => Some(Cmd::Only),
        "uncover" => Some(Cmd::Uncover),
        "visible" => Some(Cmd::Visible),
        "invisible" => Some(Cmd::Invisible),
        "onslide" => Some(Cmd::Onslide),
        "alt" => Some(Cmd::Alt),
        "pause" => Some(Cmd::Pause),
        _ => None,
    }
}

/// Whether a command starts a path, and takes `<…>` after its name: `\draw<2->`, `\node<1>`.
pub fn takes_a_specification(id: u16) -> bool {
    [Cmd::Path, Cmd::Draw, Cmd::Fill, Cmd::Filldraw, Cmd::Clip, Cmd::Shade, Cmd::Shadedraw, Cmd::Pattern, Cmd::Node, Cmd::Coordinate, Cmd::Matrix, Cmd::Pic]
        .iter()
        .any(|c| *c as u16 == id)
}

/// Whether a command can run while the rest of the slide is hidden with nothing around it: it
/// draws nothing itself, or opens what stays open after it.
pub fn stays_out(id: u16) -> bool {
    [Cmd::Begin, Cmd::End].iter().any(|c| *c as u16 == id)
}

impl Engine {
    /// The `<…>` right here, if there is one.
    pub fn overlay_specification(&mut self) -> Result<Option<Slides>, Error> {
        match self.tex.next_nonspace() {
            Some(t) if t.is_char('<') => self.read_specification().map(Some),
            Some(t) => {
                self.tex.back(t);
                Ok(None)
            }
            None => Ok(None),
        }
    }

    /// What follows a `<`, up to its `>`.
    pub fn read_specification(&mut self) -> Result<Slides, Error> {
        let spec = self.tex.read_until_char('>', false)?;
        let text = self.tex.expand_to_string(spec)?;
        let Some((slides, last)) = parse(&text, &mut self.pauses) else {
            self.warn(format!("overlay specification <{text}> not understood: shown on every slide"));
            return Ok(Slides(vec![(1, usize::MAX)]));
        };
        self.slides = self.slides.max(last);
        Ok(slides)
    }

    /// `\draw<2->`: on a slide it is off, the statement is skipped.
    pub fn statement_is_off(&mut self) -> Result<bool, Error> {
        let Some(t) = self.tex.get_token() else { return Ok(false) };
        if !t.is_char('<') {
            self.tex.back(t);
            return Ok(false);
        }
        let on = self.read_specification()?.has(self.slide);
        if !on {
            self.skip_statement()?;
        }
        Ok(!on)
    }

    /// Runs `code` with what it draws there, with no opacity: it keeps its room.
    pub fn run_hidden(&mut self, code: Vec<Token>) -> Result<(), Error> {
        if self.picture_depth == 0 {
            return Ok(());
        }
        self.hiding_around(|e| e.run_tokens(code))
    }

    /// Runs `f`, what it draws hidden; nothing is left around it if it draws nothing.
    pub fn hiding_around(&mut self, f: impl FnOnce(&mut Engine) -> Result<(), Error>) -> Result<(), Error> {
        let start = self.canvas.items.len();
        self.canvas.push(Item::Group { opacity: 0.0 });
        self.hiding += 1;
        let r = f(self);
        self.hiding -= 1;
        if self.canvas.items.len() == start + 1 {
            self.canvas.items.pop();
        } else {
            self.canvas.push(Item::End);
        }
        r
    }

    /// The commands of overlays.
    pub fn overlay_command(&mut self, cmd: Cmd) -> Result<(), Error> {
        match cmd {
            Cmd::Pause => {
                self.pauses = match self.optional()? {
                    Some(n) => tex::tokens_to_string(&n).trim().parse().unwrap_or(self.pauses + 1),
                    None => self.pauses + 1,
                };
                self.slides = self.slides.max(self.pauses);
                self.slide_hidden = self.slide < self.pauses;
                Ok(())
            }
            Cmd::Alt => {
                let spec = self.overlay_specification()?;
                let (yes, no) = (self.argument()?, self.argument()?);
                let on = spec.is_none_or(|s| s.has(self.slide));
                self.run_tokens(if on { yes } else { no })
            }
            Cmd::Onslide => {
                let spec = self.overlay_specification()?;
                let on = spec.as_ref().is_none_or(|s| s.has(self.slide));
                match self.tex.next_nonspace() {
                    Some(t) if t.is_char('{') => {
                        self.tex.back(t);
                        let code = self.argument()?;
                        if on {
                            self.run_tokens(code)
                        } else {
                            self.run_hidden(code)
                        }
                    }
                    // until the next one, which may be none at all: back to the pauses
                    other => {
                        if let Some(t) = other {
                            self.tex.back(t);
                        }
                        self.slide_hidden = if spec.is_some() { !on } else { self.slide < self.pauses };
                        Ok(())
                    }
                }
            }
            _ => {
                let spec = self.overlay_specification()?;
                let on = spec.is_none_or(|s| s.has(self.slide));
                let on = if cmd == Cmd::Invisible { !on } else { on };
                let code = self.argument()?;
                match (on, cmd) {
                    (true, _) => self.run_tokens(code),
                    (false, Cmd::Only) => Ok(()),
                    (false, _) => self.run_hidden(code),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn on(spec: &str, pauses: usize) -> Vec<usize> {
        let mut p = pauses;
        let (s, _) = parse(spec, &mut p).unwrap();
        (1..=6).filter(|&k| s.has(k)).collect()
    }

    #[test]
    fn specifications() {
        assert_eq!(on("2", 1), [2]);
        assert_eq!(on("2-", 1), [2, 3, 4, 5, 6]);
        assert_eq!(on("-3", 1), [1, 2, 3]);
        assert_eq!(on("1,3-4", 1), [1, 3, 4]);
        assert_eq!(on("1-2,5-", 1), [1, 2, 5, 6]);
        assert_eq!(on("beamer:2", 1), [2]);
        assert_eq!(on("alert@2", 1), [2]);
    }

    #[test]
    fn the_counter_of_pauses() {
        assert_eq!(on("+", 3), [3]);
        assert_eq!(on("+-", 2), [2, 3, 4, 5, 6]);
        assert_eq!(on(".", 3), [2]);
        assert_eq!(on("+(1)", 2), [3]);
        assert_eq!(on(".(-1)-", 4), [2, 3, 4, 5, 6]);
        let mut pauses = 1;
        parse("+-", &mut pauses);
        assert_eq!(pauses, 2);
        parse("3", &mut pauses);
        assert_eq!(pauses, 2);
    }

    #[test]
    fn how_far_a_specification_goes() {
        assert_eq!(parse("1,4", &mut 1).unwrap().1, 4);
        assert_eq!(parse("3-", &mut 1).unwrap().1, 3);
        assert!(parse("x", &mut 1).is_none());
    }
}
