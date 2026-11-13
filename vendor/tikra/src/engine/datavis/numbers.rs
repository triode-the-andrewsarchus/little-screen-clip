//! Tick texts: `\pgfmathprintnumber` with the `/pgf/number format` in effect (the engine's,
//! plus `frac`, which the polar axes use for multiples of π), and the "typesetting" of a tick's
//! text: its code is run, and what it would put on the page is kept as the text of the tick's node.

use crate::tex::{self, Cat, Meaning, Prim, Token};
use crate::Error;

use super::{key_text, macro_text, Engine};
use crate::engine::{macro_from, tikra_tokens};

/// The `frac` number format's options (pgfmathfloat.code.tex), which the engine's number format
/// leaves out; `frac` itself is a style of the engine's number format.
pub const NUMBER_FORMAT: &str = r"
\pgfkeys{/pgf/number format/.cd,
  frac TeX/.initial=\frac,
  frac denom/.initial=,
  frac whole/.code=\def\tikra@nf@fracwhole{#1},
  frac whole/.default=true,
  frac shift/.initial=4,
  frac warning/.code=,
  assume math mode/.code=\def\tikra@nf@math{#1},
  assume math mode/.default=true,
}
\def\tikra@nf@fracwhole{true}\def\tikra@nf@math{false}
";

impl Engine {
    /// `\pgfmathprintnumber[options]{x}`: the number as text, in math mode.
    pub fn print_number_command(&mut self) -> Result<(), Error> {
        let t = self.print_number_tokens(false)?;
        self.tex.push_tokens(t);
        Ok(())
    }

    /// Reads `[options]{x}` and prints x; in `$…$` unless `math` (or `assume math mode`).
    fn print_number_tokens(&mut self, math: bool) -> Result<Vec<Token>, Error> {
        let o = self.optional()?;
        let a = self.argument()?;
        self.begin_group();
        let r = (|| {
            if let Some(o) = o {
                self.keys_command(o, Some("/pgf/number format"), true)?;
            }
            let s = self.tex.expand_to_string(a)?;
            let v = super::parse_value(self, &s).unwrap_or(0.0);
            let text = if matches!(self.st.number_format.style, crate::engine::pgfplots::number::Style::Frac) {
                let tex = crate::keys::value(&self.tex, "/pgf/number format/frac TeX").map(|v| tex::tokens_to_string(&v)).unwrap_or_else(|| "\\frac".into());
                let denom = key_text(self, "/pgf/number format/frac denom").and_then(|s| s.parse().ok());
                let whole = macro_text(self, "tikra@nf@fracwhole") != "false";
                frac(v, &tex, denom, whole, self.st.number_format.showpos)
            } else {
                self.st.number_format.print(v)
            };
            let wrap = !math && macro_text(self, "tikra@nf@math") != "true";
            Ok(if wrap { format!("${text}$") } else { text })
        })();
        self.end_group();
        Ok(tikra_tokens(&r?))
    }

    /// `\pgfmathapproxequalto{a}{b}`: `\pgfmathresult` is 1.0 when they are about equal, else 0.0.
    pub fn approx_equal_command(&mut self) -> Result<(), Error> {
        let a = self.argument()?;
        let b = self.argument()?;
        let (a, b) = (self.number_of(a)?, self.number_of(b)?);
        let eq = (a - b).abs() < 1e-4 * a.abs().max(b.abs()).max(1.0);
        self.tex.set_meaning(&Token::cs("pgfmathresult"), macro_from(if eq { "1.0" } else { "0.0" }), false);
        Ok(())
    }

    /// The text that `code` would typeset: assignments and pgfmath commands are run, what would be
    /// printed is kept (`\pgfmathprintnumber` already formatted).
    pub fn typeset(&mut self, code: Vec<Token>) -> Result<Vec<Token>, Error> {
        let depth = self.tex.depth();
        self.tex.push_frame(code);
        let mut out = Vec::new();
        let mut math = false;
        let r = (|| {
            while let Some(t) = self.tex.get_x_token_until(depth)? {
                if t.is_cs("pgfmathprintnumber") || t.is_cs("tikra@dv@printnumber") {
                    out.extend(self.print_number_tokens(math)?);
                    continue;
                }
                match self.tex.meaning(&t) {
                    Meaning::Char('$', Cat::Math) => {
                        math = !math;
                        out.push(t);
                    }
                    Meaning::Prim(Prim::Relax) => {}
                    Meaning::Prim(p) if assignment(p) => self.execute(t)?,
                    Meaning::Native(id) if id < super::NATIVE_KEY_BASE && runs_in_text(&t) => self.execute(t)?,
                    _ => out.push(t),
                }
                if out.len() > tex::MAX_TOKENS {
                    break;
                }
            }
            Ok(())
        })();
        self.tex.drain_to(depth);
        r.map(|_| out)
    }
}

fn assignment(p: Prim) -> bool {
    use Prim::*;
    matches!(p, Def | Gdef | Edef | Xdef | Let | Futurelet | Global | Advance | Multiply | Divide | Count | Dimen | Begingroup | Endgroup)
}

/// The commands that compute rather than print.
fn runs_in_text(t: &Token) -> bool {
    let Token::Cs(n) = t else { return false };
    n.starts_with("pgfmath") || n.starts_with("pgfkeys") || n.starts_with("tikra@dv@") || matches!(&**n, "pgfset" | "tikzset" | "pgfqkeys" | "tikzdatavisualizationset")
}

/// `frac`: a whole part and a reduced fraction (`1\frac{1}{3}`), as `\pgfmathfloatgetfrac` gives.
fn frac(v: f64, tex: &str, denom: Option<i64>, whole_part: bool, showpos: bool) -> String {
    let neg = v < 0.0;
    let a = v.abs();
    let mut whole = a.trunc() as i64;
    let rest = a.fract();
    let (mut num, den) = match denom {
        Some(d) if d > 0 => ((rest * d as f64).round() as i64, d),
        _ => best_fraction(rest, 1000),
    };
    if num == den {
        whole += 1;
        num = 0;
    }
    let g = gcd(num, den).max(1);
    let (num, den) = (num / g, den / g);
    let mut out = String::new();
    if whole != 0 || num != 0 {
        if neg {
            out.push('-');
        } else if showpos {
            out.push('+');
        }
    }
    if !whole_part && whole != 0 {
        return format!("{out}{tex}{{{}}}{{{den}}}", num + whole * den);
    }
    if whole != 0 || num == 0 {
        out.push_str(&whole.to_string());
    }
    if num != 0 {
        out.push_str(&format!("{tex}{{{num}}}{{{den}}}"));
    }
    out
}

/// The fraction closest to x in [0, 1) with a denominator up to `max`.
fn best_fraction(x: f64, max: i64) -> (i64, i64) {
    let (mut best, mut err) = ((0, 1), x.abs());
    for d in 1..=max {
        let n = (x * d as f64).round() as i64;
        let e = (x - n as f64 / d as f64).abs();
        if e < err - 1e-12 {
            best = (n, d);
            err = e;
            if e < 1e-9 {
                break;
            }
        }
    }
    best
}

fn gcd(a: i64, b: i64) -> i64 {
    if b == 0 {
        a.abs()
    } else {
        gcd(b, a % b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fractions_as_pgf_prints_them() {
        assert_eq!(frac(1.0 / 6.0, "\\frac", None, true, false), "\\frac{1}{6}");
        assert_eq!(frac(1.5, "\\frac", None, true, false), "1\\frac{1}{2}");
        assert_eq!(frac(-5.0 / 12.0, "\\frac", None, true, false), "-\\frac{5}{12}");
        assert_eq!(frac(2.0, "\\frac", None, true, false), "2");
        assert_eq!(frac(1.5, "\\frac", None, false, false), "\\frac{3}{2}");
    }
}
