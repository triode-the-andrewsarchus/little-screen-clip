//! Natural ordering shared by file lists and archive reading order.
use std::cmp::Ordering;

/// Compare digit runs by value without parsing integers or imposing a digit limit.
/// Equal numbers with fewer leading zeroes come first. Other text is literal.
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (mut a, mut b) = (a.as_bytes(), b.as_bytes());
    while let (Some(&left), Some(&right)) = (a.first(), b.first()) {
        if left.is_ascii_digit() && right.is_ascii_digit() {
            let (ad, ar) = digits(a);
            let (bd, br) = digits(b);
            let av = &ad[ad.iter().position(|v| *v != b'0').unwrap_or(ad.len())..];
            let bv = &bd[bd.iter().position(|v| *v != b'0').unwrap_or(bd.len())..];
            let order = av
                .len()
                .cmp(&bv.len())
                .then_with(|| av.cmp(bv))
                .then(ad.len().cmp(&bd.len()));
            if !order.is_eq() {
                return order;
            }
            (a, b) = (ar, br);
        } else {
            let order = left.cmp(&right);
            if !order.is_eq() {
                return order;
            }
            a = &a[1..];
            b = &b[1..];
        }
    }
    a.len().cmp(&b.len())
}

fn digits(bytes: &[u8]) -> (&[u8], &[u8]) {
    bytes.split_at(
        bytes
            .iter()
            .take_while(|byte| byte.is_ascii_digit())
            .count(),
    )
}
