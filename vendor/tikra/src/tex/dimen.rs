//! Lengths as TeX keeps them: integers in scaled points (65536 sp = 1 pt).

/// The largest dimension TeX allows (16383.99999 pt).
pub const MAX_DIMEN: i64 = (1 << 30) - 1;

pub const SP_PER_PT: f64 = 65536.0;

/// Units and their size in sp. `em` and `ex` are those of Computer Modern at 10 pt, TeX's default font.
pub const UNITS: [(&str, f64); 12] = [
    ("pt", 65536.0),
    ("pc", 786432.0),
    ("in", 4736286.72),
    ("bp", 65781.76),
    ("cm", 1864679.811),
    ("mm", 186467.981),
    ("dd", 70124.086),
    ("cc", 841489.04),
    ("sp", 1.0),
    ("em", 655360.0),
    ("ex", 282168.0),
    ("px", 65781.76),
];

/// TeX's `round_decimals`: decimal digits after the point as a fraction of 65536.
pub fn round_decimals(digits: &[u8]) -> i64 {
    let mut a: i64 = 0;
    for &d in digits.iter().take(17).rev() {
        a = (a + d as i64 * 131072) / 10;
    }
    (a + 1) / 2
}

/// TeX's `xn_over_d`: x·n/d for x in sp, with the remainder, without overflow.
pub fn xn_over_d(x: i64, n: i64, d: i64) -> (i64, i64) {
    let neg = x < 0;
    let x = x.abs();
    let t = (x % 32768) * n;
    let mut u = (x / 32768) * n + t / 32768;
    let v = (u % d) * 32768 + t % 32768;
    u = 32768 * (u / d) + v / d;
    let rem = v % d;
    if neg {
        (-u, -rem)
    } else {
        (u, rem)
    }
}

/// TeX's `nx_plus_y` with y = 0: n·x.
pub fn mult(n: i64, x: i64) -> i64 {
    n.saturating_mul(x)
}

/// Units with an exact ratio to points (TeX §458): numerator, denominator.
pub fn ratio(unit: &str) -> Option<(i64, i64)> {
    Some(match unit {
        "pt" => (1, 1),
        "in" => (7227, 100),
        "pc" => (12, 1),
        "cm" => (7227, 254),
        "mm" => (7227, 2540),
        "bp" | "px" => (7227, 7200),
        "dd" => (1238, 1157),
        "cc" => (14856, 1157),
        _ => return None,
    })
}

/// `integer.digits` of a unit, in sp, as TeX computes it.
pub fn exact(integer: i64, digits: &[u8], unit: &str) -> Option<i64> {
    let mut f = round_decimals(digits);
    let mut v = integer;
    if unit == "sp" {
        return Some(v);
    }
    if let Some(sp) = match unit {
        "em" => Some(655360),
        "ex" => Some(282168),
        _ => None,
    } {
        // a font unit: v·unit + f·unit/65536
        let (frac, _) = xn_over_d(sp, f, 65536);
        return Some(mult(v, sp) + frac);
    }
    let (n, d) = ratio(unit)?;
    if (n, d) != (1, 1) {
        let (q, r) = xn_over_d(v, n, d);
        v = q;
        f = (n * f + 65536 * r) / d;
        v += f / 65536;
        f %= 65536;
    }
    Some(v * 65536 + f)
}

/// A dimension as `\the` writes it: `12.0pt`, `-0.39998pt` (TeX's `print_scaled`).
pub fn show(sp: i64) -> String {
    let mut s = String::new();
    let mut v = sp;
    if v < 0 {
        s.push('-');
        v = -v;
    }
    s.push_str(&(v / 65536).to_string());
    s.push('.');
    let mut f = 10 * (v % 65536) + 5;
    let mut delta = 10;
    loop {
        if delta > 65536 {
            // round the last digit
            f += 0x8000 - 50000;
        }
        s.push(char::from(b'0' + (f / 65536) as u8));
        f = 10 * (f % 65536);
        delta *= 10;
        if f <= delta {
            break;
        }
    }
    s.push_str("pt");
    s
}

/// Scaled points to centimeters.
pub fn to_cm(sp: i64) -> f64 {
    sp as f64 / SP_PER_PT * 2.54 / 72.27
}

/// Centimeters to scaled points.
pub fn from_cm(cm: f64) -> i64 {
    (cm / 2.54 * 72.27 * SP_PER_PT).round().clamp(-(MAX_DIMEN as f64), MAX_DIMEN as f64) as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shown_as_tex_shows_them() {
        assert_eq!(show(65536 * 12), "12.0pt");
        assert_eq!(show(-26214), "-0.4pt");
        assert_eq!(show(1), "0.00002pt");
        assert_eq!(show(32768), "0.5pt");
        assert_eq!(show(exact(1, &[], "cm").unwrap()), "28.45274pt");
        assert_eq!(show(exact(1, &[5], "pt").unwrap()), "1.5pt");
        assert_eq!(show(exact(0, &[4], "pt").unwrap()), "0.4pt");
    }
}
