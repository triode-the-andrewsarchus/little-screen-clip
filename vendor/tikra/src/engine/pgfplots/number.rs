//! `\pgfmathprintnumber`, as `pgfmathfloat.code.tex` writes numbers: the `std`, `fixed`, `sci`,
//! `int detect` and `int trunc` styles, `precision`, `zerofill`, the thousands separator (`{,}`
//! by default, from 1000 on) and `\cdot 10^{e}` for scientific notation.
//!
//! Rounding works on the decimal digits, as pgf's `\pgfmathroundto` does (half up), not on the
//! binary value: `0.125` with two digits is `0.13`.

/// How numbers are written: the keys of `/pgf/number format`.
#[derive(Debug, Clone, PartialEq)]
pub struct NumberFormat {
    pub style: Style,
    /// Digits after the period (`precision`, initially 2).
    pub precision: usize,
    /// `sci precision`, if set apart from `precision`.
    pub sci_precision: Option<usize>,
    pub fixed_zerofill: bool,
    pub sci_zerofill: bool,
    /// `set decimal separator` (`.`) and `set thousands separator` (`{,}`).
    pub dec_sep: String,
    pub thousands_sep: String,
    /// `min exponent for 1000 sep` (0: from 1000 on).
    pub min_exponent_1000: i32,
    /// `showpos`
    pub showpos: bool,
    /// `skip 0.`: `.5` instead of `0.5`
    pub skip_zero: bool,
    pub sci_style: SciStyle,
    /// `retain unit mantissa` (true): `1\cdot 10^{5}`, not `10^{5}`.
    pub unit_mantissa: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Style {
    /// `std=lower:upper`: fixed for exponents in [lower, upper], else sci. `lower` None means
    /// `-precision/2`.
    Std {
        lower: Option<i32>,
        upper: i32,
    },
    Fixed,
    Sci,
    IntDetect,
    IntTrunc,
    /// `relative=<exponent>`: rounds to 10^exponent units.
    Relative(i32),
    FixedRelative,
    /// `frac`: a (mixed) fraction, `1\frac{1}{2}`.
    Frac,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SciStyle {
    /// `1.2\cdot 10^{3}`
    TenE,
    /// `1.2e{+}3`
    LowerE,
    /// `1.2E{+}3`
    UpperE,
    /// `1.2_{3}`
    Subscript,
    /// `1.2^{3}`
    Superscript,
}

impl Default for NumberFormat {
    fn default() -> Self {
        NumberFormat {
            style: Style::Std { lower: None, upper: 4 },
            precision: 2,
            sci_precision: None,
            fixed_zerofill: false,
            sci_zerofill: false,
            dec_sep: ".".into(),
            thousands_sep: "{,}".into(),
            min_exponent_1000: 0,
            showpos: false,
            skip_zero: false,
            sci_style: SciStyle::TenE,
            unit_mantissa: true,
        }
    }
}

/// A number as pgf's float parser sees it: sign, decimal digits (the first one is the units
/// digit) and the exponent of that first digit.
#[derive(Debug, Clone, PartialEq)]
struct Decimal {
    negative: bool,
    /// Significant digits, no leading zeros; empty for zero.
    digits: Vec<u8>,
    exponent: i32,
}

impl Decimal {
    fn parse(v: f64) -> Decimal {
        let negative = v < 0.0;
        if v == 0.0 {
            return Decimal { negative: false, digits: Vec::new(), exponent: 0 };
        }
        // the shortest representation that reads back as the same f64: what was written
        let s = format!("{:e}", v.abs());
        let (mantissa, exp) = s.split_once('e').unwrap_or((&s, "0"));
        let exponent: i32 = exp.parse().unwrap_or(0);
        let digits: Vec<u8> = mantissa.bytes().filter(u8::is_ascii_digit).map(|b| b - b'0').collect();
        let mut d = Decimal { negative, digits, exponent };
        d.trim();
        d
    }

    fn trim(&mut self) {
        // (no leading zeros: the digits come from `{:e}` and rounding carries into a new 1)
        while self.digits.last() == Some(&0) {
            self.digits.pop();
        }
        if self.digits.is_empty() {
            self.exponent = 0;
        }
    }

    fn is_zero(&self) -> bool {
        self.digits.is_empty()
    }

    /// Rounds (half up) to keep digits down to 10^`place`.
    fn round_at(&mut self, place: i32) {
        if self.is_zero() {
            return;
        }
        // index of the first digit dropped
        let keep = self.exponent - place + 1;
        if keep < 0 {
            self.digits.clear();
            self.trim();
            return;
        }
        let keep = keep as usize;
        if keep >= self.digits.len() {
            return;
        }
        let up = self.digits[keep] >= 5;
        self.digits.truncate(keep);
        if up {
            let mut i = keep;
            loop {
                if i == 0 {
                    self.digits.insert(0, 1);
                    self.exponent += 1;
                    break;
                }
                i -= 1;
                if self.digits[i] == 9 {
                    self.digits[i] = 0;
                } else {
                    self.digits[i] += 1;
                    break;
                }
            }
        }
        self.trim();
    }

    /// The integer part's digits and the fractional digits.
    fn split(&self) -> (String, String) {
        if self.is_zero() {
            return ("0".into(), String::new());
        }
        let mut int = String::new();
        let mut frac = String::new();
        if self.exponent < 0 {
            int.push('0');
            for _ in 0..(-self.exponent - 1) {
                frac.push('0');
            }
            for d in &self.digits {
                frac.push((b'0' + d) as char);
            }
        } else {
            for k in 0..=self.exponent as usize {
                int.push((b'0' + self.digits.get(k).copied().unwrap_or(0)) as char);
            }
            for d in self.digits.iter().skip(self.exponent as usize + 1) {
                frac.push((b'0' + d) as char);
            }
        }
        (int, frac)
    }
}

/// `v` as a fraction of the smallest denominator that gives it (to 10^-4): `2`, `\frac{3}{4}`,
/// `1\frac{1}{2}` (its whole part apart), with its sign before.
fn frac(v: f64) -> String {
    let sign = if v < 0.0 { "-" } else { "" };
    let a = v.abs();
    let den = (1..=10_000u64).find(|d| ((a * *d as f64) - (a * *d as f64).round()).abs() < 1e-4 * *d as f64).unwrap_or(10_000);
    let num = (a * den as f64).round() as u64;
    let (whole, rest) = (num / den, num % den);
    match (whole, rest) {
        (w, 0) => format!("{sign}{w}"),
        (0, r) => format!("{sign}\\frac{{{r}}}{{{den}}}"),
        (w, r) => format!("{sign}{w}\\frac{{{r}}}{{{den}}}"),
    }
}

impl NumberFormat {
    /// The TeX text of `v` (to be read in math mode).
    pub fn print(&self, v: f64) -> String {
        if v.is_nan() {
            return "\\hbox{NaN}".into();
        }
        if v.is_infinite() {
            return if v < 0.0 {
                "-\\infty".into()
            } else if self.showpos {
                "+\\infty".into()
            } else {
                "\\infty".into()
            };
        }
        let d = Decimal::parse(v);
        match &self.style {
            Style::Std { lower, upper } => {
                let lower = lower.unwrap_or(-(self.precision as i32) / 2);
                if !d.is_zero() && (d.exponent > *upper || d.exponent < lower) {
                    self.sci(d)
                } else {
                    self.fixed(d)
                }
            }
            Style::Fixed => self.fixed(d),
            Style::Sci => self.sci(d),
            Style::IntTrunc => {
                let t = v.trunc();
                self.fixed(Decimal::parse(if t == 0.0 { 0.0 } else { t }))
            }
            Style::IntDetect => {
                let mut r = d.clone();
                r.round_at(-6);
                let (_, frac) = r.split();
                if frac.is_empty() {
                    self.fixed(r)
                } else {
                    self.sci(d)
                }
            }
            Style::Relative(e) => {
                let mut r = d.clone();
                r.round_at(*e - self.precision as i32);
                let f = NumberFormat { style: Style::Std { lower: None, upper: 4 }, precision: 9999, ..self.clone() };
                if r.is_zero() {
                    return f.fixed(r);
                }
                f.print_decimal(r)
            }
            Style::FixedRelative => {
                let mut r = d.clone();
                r.round_at(d.exponent - self.precision as i32 + 1);
                let f = NumberFormat { fixed_zerofill: false, precision: 9999, ..self.clone() };
                f.fixed(r)
            }
            Style::Frac => frac(v),
        }
    }

    fn print_decimal(&self, d: Decimal) -> String {
        let Style::Std { lower, upper } = &self.style else { unreachable!("`relative` prints with a std format") };
        let lower = lower.unwrap_or(-2);
        if d.exponent > *upper || d.exponent < lower {
            self.sci(d)
        } else {
            self.fixed(d)
        }
    }

    /// Fixed point, rounded to `precision` digits after the period.
    fn fixed(&self, mut d: Decimal) -> String {
        d.round_at(-(self.precision as i32));
        let (int, mut frac) = d.split();
        if self.fixed_zerofill {
            while frac.len() < self.precision {
                frac.push('0');
            }
        }
        let negative = d.negative && !d.is_zero();
        let mut out = String::new();
        if negative {
            out.push('-');
        } else if self.showpos {
            out.push('+');
        }
        let skip = self.skip_zero && int == "0" && !frac.is_empty();
        if !skip {
            out.push_str(&self.group_thousands(&int, d.exponent));
        }
        if !frac.is_empty() {
            out.push_str(&self.dec_sep);
            out.push_str(&frac);
        }
        out
    }

    fn group_thousands(&self, int: &str, exponent: i32) -> String {
        if self.thousands_sep.is_empty() || exponent < self.min_exponent_1000.max(0) || int.len() <= 3 {
            return int.to_string();
        }
        let mut out = String::new();
        let n = int.len();
        for (i, c) in int.chars().enumerate() {
            if i > 0 && (n - i).is_multiple_of(3) {
                out.push_str(&self.thousands_sep);
            }
            out.push(c);
        }
        out
    }

    /// Scientific: the mantissa rounded to `sci precision` digits, then the exponent.
    fn sci(&self, d: Decimal) -> String {
        if d.is_zero() {
            return self.fixed(d);
        }
        let precision = self.sci_precision.unwrap_or(self.precision);
        let exponent = d.exponent;
        let mut m = Decimal { negative: d.negative, digits: d.digits.clone(), exponent: 0 };
        m.round_at(-(precision as i32));
        let mut exponent = exponent;
        if m.exponent > 0 {
            // 9.999 rounded up to 10
            m.exponent -= 1;
            exponent += 1;
        }
        let mantissa_one = m.digits == [1] && m.exponent == 0;
        let mut mf = self.clone();
        mf.thousands_sep = String::new();
        mf.fixed_zerofill = self.sci_zerofill;
        mf.precision = precision;
        let mantissa = mf.fixed(m.clone());
        let (sep, exp) = match self.sci_style {
            SciStyle::TenE => ("\\cdot ", format!("10^{{{exponent}}}")),
            SciStyle::LowerE => ("", if exponent < 0 { format!("e{{-}}{}", -exponent) } else { format!("e{{+}}{exponent}") }),
            SciStyle::UpperE => ("", if exponent < 0 { format!("E{{-}}{}", -exponent) } else { format!("E{{+}}{exponent}") }),
            SciStyle::Subscript => ("", format!("_{{{exponent}}}")),
            SciStyle::Superscript => ("", format!("^{{{exponent}}}")),
        };
        if mantissa_one && !self.unit_mantissa && self.sci_style == SciStyle::TenE {
            return format!("{}{exp}", if m.negative { "-" } else { "" });
        }
        format!("{mantissa}{sep}{exp}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn std(v: f64) -> String {
        NumberFormat::default().print(v)
    }

    #[test]
    fn std_style_like_pgf() {
        assert_eq!(std(0.0), "0");
        assert_eq!(std(2.0), "2");
        assert_eq!(std(-6.0), "-6");
        assert_eq!(std(0.5), "0.5");
        assert_eq!(std(0.125), "0.13");
        assert_eq!(std(1.0 / 3.0), "0.33");
        assert_eq!(std(0.1 * 3.0), "0.3");
        assert_eq!(std(1000.0), "1{,}000");
        assert_eq!(std(12345.0), "12{,}345");
        assert_eq!(std(123456.0), "1.23\\cdot 10^{5}");
        assert_eq!(std(100000.0), "1\\cdot 10^{5}");
        assert_eq!(std(0.05), "5\\cdot 10^{-2}");
        assert_eq!(std(0.1), "0.1");
        assert_eq!(std(-0.001), "-1\\cdot 10^{-3}");
        assert_eq!(std(9.999), "10");
        assert_eq!(std(-0.0001 + 0.0001), "0");
    }

    #[test]
    fn fixed_sci_and_zerofill() {
        let f = NumberFormat { style: Style::Fixed, precision: 3, ..Default::default() };
        assert_eq!(f.print(1.23456), "1.235");
        assert_eq!(f.print(123456.0), "123{,}456");
        let f = NumberFormat { style: Style::Fixed, fixed_zerofill: true, ..Default::default() };
        assert_eq!(f.print(2.5), "2.50");
        assert_eq!(f.print(-0.001), "0.00");
        let f = NumberFormat { style: Style::Sci, ..Default::default() };
        assert_eq!(f.print(9.996e3), "1\\cdot 10^{4}");
        assert_eq!(f.print(0.0), "0");
        let f = NumberFormat { style: Style::IntTrunc, ..Default::default() };
        assert_eq!(f.print(-3.7), "-3");
        let f = NumberFormat { thousands_sep: String::new(), ..Default::default() };
        assert_eq!(f.print(2000.0), "2000");
    }
}
