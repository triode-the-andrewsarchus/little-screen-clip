//! Enough of pgf's floating point unit to step through a domain the way pgfplots does.
//!
//! A number is a decimal `n * 10^p`. pgf adds two of them by scaling both mantissas to [100, 1000),
//! reading them as TeX dimensions (sp), shifting the one of smaller exponent (`\divide` by ten,
//! cutting), adding and printing the sum as TeX prints a dimension; it divides with the mantissas
//! scaled to [10, 100). So sums keep about eight digits and quotients five or six, and a sampled
//! domain can end a little short of its end (`-5:5` with 25 samples ends at 4.9998401).
//! Its `exp` is pgf's fixed point one, with a relative error near 1e-4.

use crate::tex::dimen::{round_decimals, show, xn_over_d};

const UNITY: i128 = 65536;

/// A decimal number `n * 10^p`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Num {
    n: i128,
    p: i32,
}

impl Num {
    pub const ZERO: Num = Num { n: 0, p: 0 };

    /// The number written as `v` prints (its shortest decimal form).
    pub fn from_f64(v: f64) -> Option<Num> {
        if !v.is_finite() {
            return None;
        }
        let s = format!("{:e}", v);
        let (m, e) = s.split_once('e')?;
        let e: i32 = e.parse().ok()?;
        let neg = m.starts_with('-');
        let m = m.trim_start_matches('-');
        let (int, frac) = m.split_once('.').unwrap_or((m, ""));
        // (at most 17 digits: the shortest form of an f64)
        let digits = format!("{int}{frac}");
        let n: i128 = digits.parse().ok()?;
        Some(Num { n: if neg { -n } else { n }, p: e - frac.len() as i32 }.trimmed())
    }

    pub fn value(self) -> f64 {
        self.n as f64 * 10f64.powi(self.p)
    }

    pub fn negated(self) -> Num {
        Num { n: -self.n, p: self.p }
    }

    pub fn is_zero(self) -> bool {
        self.n == 0
    }

    fn trimmed(mut self) -> Num {
        if self.n == 0 {
            return Num::ZERO;
        }
        while self.n % 10 == 0 {
            self.n /= 10;
            self.p += 1;
        }
        self
    }

    /// The power of ten of the leading digit.
    fn exponent(self) -> i32 {
        let mut d = 0;
        let mut m = self.n.abs();
        while m >= 10 {
            m /= 10;
            d += 1;
        }
        d + self.p
    }

    /// The mantissa scaled to `10^shift` times [1, 10), read as a dimension (sp), and the power of
    /// ten that goes with it.
    fn scaled(self, shift: i32) -> (i128, i32) {
        // (never of zero: `plus` and `over` answer that themselves)
        let e = self.exponent() - shift;
        // the mantissa is n * 10^(p - e): a decimal with k = e - p digits after the point (k is
        // at most the number of digits, a handful)
        let k = e - self.p;
        let sp = if k <= 0 {
            self.n.abs() * 10i128.pow((-k) as u32) * UNITY
        } else {
            // TeX rounds the decimal digits to the nearest sp
            let d = 10i128.pow(k as u32);
            (2 * self.n.abs() * UNITY + d) / (2 * d)
        };
        (self.n.signum() * sp, e)
    }

    /// A dimension (sp) as TeX prints it, times `10^e`.
    fn printed(sp: i128, e: i32) -> Num {
        let neg = sp < 0;
        let mut s = sp.abs();
        let int = s / UNITY;
        // TeX's \the of a dimension: the fewest decimals that read back as the same sp
        let mut frac: i128 = 0;
        let mut k = 0;
        s = 10 * (s % UNITY) + 5;
        let mut delta = 10;
        loop {
            if delta > UNITY {
                s += 0x8000 - 50000;
            }
            frac = 10 * frac + s / UNITY;
            k += 1;
            s = 10 * (s % UNITY);
            delta *= 10;
            if s <= delta {
                break;
            }
        }
        let n = int * 10i128.pow(k) + frac;
        Num { n: if neg { -n } else { n }, p: e - k as i32 }.trimmed()
    }

    /// `\pgfmathfloatadd@`.
    pub fn plus(self, o: Num) -> Num {
        if self.n == 0 {
            return o;
        }
        if o.n == 0 {
            return self;
        }
        let ((mut a, ea), (mut b, eb)) = (self.scaled(2), o.scaled(2));
        let e = ea.max(eb);
        for (m, x) in [(&mut a, ea), (&mut b, eb)] {
            let d = e - x;
            // `\divide` cuts toward zero
            *m = if d > 8 { 0 } else { *m / 10i128.pow(d as u32) };
        }
        Num::printed(a + b, e)
    }

    /// `\pgfmathfloatdivide@`: the mantissas scaled to [10, 100) and divided as pgfmath divides
    /// dimensions (by a whole number: `\divide`, cutting).
    pub fn over(self, o: Num) -> Num {
        if o.n == 0 || self.n == 0 {
            return Num::ZERO;
        }
        let ((a, ea), (b, eb)) = (self.scaled(1), o.scaled(1));
        let q = if b % UNITY == 0 { a / (b / UNITY) } else { a * UNITY / b };
        Num::printed(q, ea - eb)
    }
}

/// A decimal `n * 10^p` read as a TeX dimension, in sp: the whole part times 2^16 plus the first
/// 17 digits of the fraction rounded as TeX's `round_decimals` does.
fn dimen_sp(n: i128, p: i32) -> i128 {
    let n = n.abs();
    if p >= 0 {
        return n * 10i128.pow(p as u32) * UNITY;
    }
    // (at most five decimals: the square root's digits)
    let k = (-p) as u32;
    let d = 10i128.pow(k);
    let (whole, frac) = (n / d, n % d);
    let digits: Vec<i128> = (0..k).map(|i| frac / 10i128.pow(k - 1 - i) % 10).take(17).collect();
    let mut a = 0;
    for &dig in digits.iter().rev() {
        a = (a + dig * 2 * UNITY) / 10;
    }
    whole * UNITY + (a + 1) / 2
}

/// `\pgfmathfloatsqrt@`: the mantissa scaled to [1000, 10000) is read as a dimension and given to
/// pgfmath's digit-by-digit square root, which cuts after five decimals; an odd power of ten
/// multiplies that by sqrt(10) or sqrt(0.1) as dimensions multiply (`3.16228` resp. `0.31622`
/// times it). So sqrt(1) is 0.999985 and sqrt(0.25) is exactly 0.5.
pub fn sqrt(v: f64) -> f64 {
    if v.is_nan() || v < 0.0 {
        return f64::NAN;
    }
    if v == 0.0 || v.is_infinite() {
        return v;
    }
    // the fpu's numbers keep about nine digits (1 - 1e-12 is 1 to it)
    let Some(num) = format!("{v:.8e}").parse().ok().and_then(Num::from_f64) else { return v.sqrt() };
    let e = num.exponent() - 3;
    let x = Num::printed(dimen_sp(num.n, num.p - e), 0);
    // the digits of x in pairs from its whole part: floor(sqrt(x * 10^10)) / 10^5
    let big = x.n * 10i128.pow((x.p + 10).max(0) as u32);
    // big is below 2^53 (x below 10^4, five decimals): f64's square root floors exactly
    let root = (big as f64).sqrt() as i128;
    let r = if e % 2 != 0 {
        let sp = dimen_sp(root, -5);
        let (whole, fraction) = if e > 0 { (3, 10635) } else { (0, 20724) };
        Num::printed(whole * sp + sp * fraction / UNITY, 0)
    } else {
        Num { n: root, p: -5 }.trimmed()
    };
    // `\divide` cuts toward zero
    Num { n: r.n, p: r.p + e / 2 }.value()
}

/// A decimal as TeX reads `<decimal>pt`: in sp, rounded to 2^-16.
fn scan_pt(text: &str) -> i64 {
    let (neg, t) = match text.strip_prefix('-') {
        Some(t) => (true, t),
        None => (false, text),
    };
    let (i, f) = t.split_once('.').unwrap_or((t, ""));
    let digits: Vec<u8> = f.bytes().filter(u8::is_ascii_digit).map(|b| b - b'0').collect();
    let v = i.parse::<i64>().unwrap_or(0) * 65536 + round_decimals(&digits);
    if neg {
        -v
    } else {
        v
    }
}

/// `\dimen=<decimal>\dimen`: TeX's product of a dimension by a decimal factor.
fn times(factor: &str, sp: i64) -> i64 {
    // (the factors here are never negative)
    let (i, f) = factor.split_once('.').unwrap_or((factor, ""));
    let digits: Vec<u8> = f.bytes().filter(u8::is_ascii_digit).map(|b| b - b'0').collect();
    i.parse::<i64>().unwrap_or(0) * sp + xn_over_d(sp, round_decimals(&digits), 65536).0
}

/// `\pgfmath@tonumber`: a dimension as `\the` writes it, without `pt`.
fn number(sp: i64) -> String {
    show(sp).trim_end_matches("pt").to_string()
}

/// pgf's `reciprocal` (`\pgfmathreciprocal@`) of a number as `\pgfmath@tonumber` writes it: 10^9
/// over its first six digits, in sp.
fn reciprocal(text: &str) -> i64 {
    let (a, b) = text.split_once('.').unwrap_or((text, "0"));
    let b = format!("{b}0000000");
    let int: i64 = a.parse().unwrap_or(0);
    if b[..5].parse::<i64>().unwrap_or(0) == 0 {
        // an integer: TeX's division
        return 65536 / int.max(1);
    }
    // (the numbers here are below 10: pgf's branch for those above 100 is not needed)
    let digits = format!("{a}{}", &b[..5]);
    let mut cb = 1_000_000_000 / digits.parse::<i64>().unwrap_or(1).max(1);
    let ca = cb / 10000;
    let x = ca * 65536;
    cb -= ca * 10000;
    let mut y = cb * 65536;
    for _ in 0..4 {
        y = times(".1", y);
    }
    x + y
}

/// pgf's fixed point `exp` (`\pgfmathexp@`): the series added in sp until a term adds nothing,
/// and the reciprocal for a negative argument.
fn basic_exp(text: &str) -> i64 {
    // (from above -ln 10: `exp` takes the powers of ten out first)
    let arg = scan_pt(text);
    let factor = number(arg.abs());
    let (mut x, mut term, mut last, mut n) = (65536i64, 65536i64, 65536i64, 1i64);
    loop {
        term = times(&factor, term / n);
        x += term;
        if x == last {
            break;
        }
        last = x;
        n += 1;
    }
    if arg < 0 {
        x = reciprocal(&number(x));
    }
    x
}

/// `exp` as pgf's floating point unit computes it (`\pgfmathfloatexp@`): 10^k exp(x - k ln 10),
/// the second factor by the fixed point `exp`, all in TeX dimensions. exp(-0.00008) is 1.00014.
pub fn exp(v: f64) -> f64 {
    if v == 0.0 || !v.is_finite() || v.abs() > 16000.0 {
        return v.exp();
    }
    let fixed = format!("{v}");
    let t = number(times("0.434294481", scan_pt(&fixed)));
    let (i, f) = t.split_once('.').unwrap_or((&t, "0"));
    // `\pgfmathfloatexp@@toint`: "-0.3" is 0 (not negative), "-1.5" is -3
    let mut k: i64 = i.parse().unwrap_or(0);
    if k < 0 {
        k -= 1;
        if f.parse::<i64>().unwrap_or(0) > 0 {
            k -= 1;
        }
    }
    let r = basic_exp(&number(scan_pt("2.302585092") * -k + scan_pt(&fixed)));
    number(r).parse::<f64>().unwrap_or(f64::NAN) * 10f64.powi(k as i32)
}

/// `ln` as pgf's floating point unit computes it (`\pgfmathlog@float`): ln(M) + E·2.302585pt for
/// v = M·10^E, in TeX dimensions (ln(M) is pgf's fixed point one, here exact to the sp). ln(1e5)
/// is 11.51291.
pub fn ln(v: f64) -> f64 {
    if !(v.is_finite() && v > 0.0) {
        return v.ln();
    }
    let e = v.log10().floor() as i32;
    let (m, e) = match v / 10f64.powi(e) {
        m if m >= 10.0 => (m / 10.0, e + 1),
        m if m < 1.0 => (m * 10.0, e - 1),
        m => (m, e),
    };
    let sp = (m.ln() * 65536.0).round() as i64 + scan_pt("2.302585") * e as i64;
    number(sp).parse().unwrap_or(f64::NAN)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(v: f64) -> Num {
        Num::from_f64(v).unwrap()
    }

    fn assert_near(a: Num, b: f64) {
        assert!((a.value() - b).abs() < 1e-12, "{} != {b}", a.value());
    }

    #[test]
    fn sums_and_quotients_as_pgf() {
        // the values pgf's fpu gives (\pgfmathfloatadd@ and \pgfmathfloatdivide@)
        assert_near(n(-5.0).plus(n(0.41666)), -4.58334);
        assert_near(n(0.208333).plus(n(0.208333)), 0.41666599);
        assert_near(n(4.7916585).plus(n(0.208333)), 4.9999915);
        assert_near(n(10.0).over(n(24.0)), 0.41666);
        assert_near(n(5.0).over(n(24.0)), 0.208333);
        assert_near(n(-4.58334).plus(n(5.0)), 0.41666);
    }

    #[test]
    fn exp_as_pgf() {
        // the values pgf's fpu gives (`\pgfmathparse{exp(…)}` with /pgf/fpu)
        let tex = [
            (-7.999e-5, 1.00014),
            (8e-5, 1.00008),
            (-0.001, 0.99924),
            (-0.1, 0.90501),
            (-1.5, 0.22314),
            (-3.0, 4.978603e-2),
            (2.5, 12.1822),
            (-4.99, 6.805405e-3),
            (10.0, 22026.0),
            (-10.0, 4.539816e-5),
        ];
        for (v, t) in tex {
            assert!((exp(v) - t).abs() < 1e-9 * t, "exp({v}) = {} != {t}", exp(v));
        }
    }

    #[test]
    fn square_roots_as_pgf() {
        // \pgfmathparse{sqrt(…)} with /pgf/fpu, as pdflatex prints them
        for (x, r) in [
            (1.0, 9.99985e-1),
            (0.99723, 9.98614e-1),
            (2.0, 1.414192),
            (0.25, 0.5),
            (3.0, 1.732025),
            (10.0, 3.162277),
            (0.1, 3.162277e-1),
            (0.001, 3.162277e-2),
            (123.456, 1.11109e1),
            (9.99, 3.160649),
            (0.0001, 9.99985e-3),
            (7.0, 2.645712),
            (1e-5, 3.162277e-3),
            (0.75, 8.660254e-1),
            (0.36, 0.6),
            (12345.0, 1.1110803e2),
            (0.84, 9.165151e-1),
            (0.96, 9.797958e-1),
            (0.0025, 5.0e-2),
            (0.9375, 9.682458e-1),
            (1.0 - 1e-14, 9.99985e-1),
        ] {
            assert!((sqrt(x) - r).abs() < 1e-12 * r, "sqrt({x}) = {} != {r}", sqrt(x));
        }
    }
}
