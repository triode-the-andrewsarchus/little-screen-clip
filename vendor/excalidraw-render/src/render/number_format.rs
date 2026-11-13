//! The three policies by which an `f64` becomes artifact text, kept together
//! so a reviewer can compare them. PDF numbers are Krilla's spelling
//! (ADR-0021); PDF Rough coordinates still take the Rough policy's value.
//!
//! - `push_rough_number`: Rough.js's path serialization, a fixed two-decimal
//!   spelling with trailing zeros and a vanishing sign dropped, using the
//!   pinned ECMAScript formatter's `toFixed(2)` rounding.
//!   The SVG backend spells Rough path coordinates with it; the PDF backend
//!   draws the number it spells.
//! - `ecmascript_number`: ECMAScript `Number#toString`, the shortest
//!   round-trip digits with exponent notation from 1e21 and below 1e-6 and an
//!   explicit plus sign on positive exponents. The freedraw port reduces its
//!   coordinates through it the way the reference's `toFixed`-free path
//!   builder does.
//! - `push_svg_number`: Rust `Display`, which never uses exponent notation,
//!   with negative zero canonicalized to zero.
//!
//! `ecmascript_round` (`Math.round`) stays in `geometry`: it is a rounding
//! policy applied to coordinates, not a spelling.

use super::FormatEncodeError;

pub(super) fn push_rough_number(
    output: &mut (impl std::fmt::Write + ?Sized),
    value: f64,
) -> Result<(), FormatEncodeError> {
    if !value.is_finite() || value.abs() >= 1e21 {
        return Err(FormatEncodeError::Encoding(
            "Rough path number is outside fixed-decimal range".to_owned(),
        ));
    }
    let mut buffer = ryu_js::Buffer::new();
    // Below 1e21 toFixed always includes the two fractional digits, so trimming
    // stops at the decimal point and cannot consume zeros in the integer part.
    let token = buffer.format_to_fixed(value, 2);
    let token = token.trim_end_matches('0').trim_end_matches('.');
    let token = if token == "-0" { "0" } else { token };
    output
        .write_str(token)
        .map_err(|_| FormatEncodeError::Encoding("failed to write Rough path number".to_owned()))
}

pub(super) fn ecmascript_number(value: f64) -> Result<String, FormatEncodeError> {
    if !value.is_finite() {
        return Err(FormatEncodeError::Encoding(
            "ECMAScript number must be finite".to_owned(),
        ));
    }
    Ok(ryu_js::Buffer::new().format_finite(value).to_owned())
}

pub(super) fn push_svg_number<W: std::fmt::Write + ?Sized>(
    output: &mut W,
    value: f64,
    subject: &str,
) -> Result<(), FormatEncodeError> {
    if !value.is_finite() {
        return Err(FormatEncodeError::Encoding(format!(
            "{subject} must be finite"
        )));
    }
    let value = if value == 0.0 { 0.0 } else { value };
    write!(output, "{value}")
        .map_err(|_| FormatEncodeError::Encoding(format!("failed to write {subject}")))
}

#[cfg(test)]
mod tests {
    use super::{ecmascript_number, push_rough_number, push_svg_number};

    fn rough(value: f64) -> String {
        let mut output = String::new();
        push_rough_number(&mut output, value).expect("finite Rough coordinate");
        output
    }

    fn svg(value: f64) -> String {
        let mut output = String::new();
        push_svg_number(&mut output, value, "test number").expect("finite SVG number");
        output
    }

    #[test]
    fn rough_numbers_carry_at_most_two_decimals_and_round_ties_up() {
        assert_eq!(rough(1.0), "1");
        assert_eq!(rough(1.5), "1.5");
        assert_eq!(rough(1.25), "1.25");
        assert_eq!(rough(-2.5), "-2.5");
        // `toFixed(2)` on the exact tie 60.125 rounds upward.
        assert_eq!(rough(60.125), "60.13");
        // A negative value that rounds to zero loses its sign.
        assert_eq!(rough(-0.001), "0");
        assert!(push_rough_number(&mut String::new(), f64::NAN).is_err());
        assert!(push_rough_number(&mut String::new(), 1e21).is_err());
    }

    #[test]
    fn ecmascript_numbers_spell_like_number_to_string() {
        assert_eq!(ecmascript_number(0.0).unwrap(), "0");
        assert_eq!(ecmascript_number(-0.0).unwrap(), "0");
        assert_eq!(ecmascript_number(1.5).unwrap(), "1.5");
        assert_eq!(ecmascript_number(0.1 + 0.2).unwrap(), "0.30000000000000004");
        // Exponent notation begins at 1e21 and below 1e-6, with an explicit
        // plus sign on positive exponents.
        assert_eq!(ecmascript_number(1e21).unwrap(), "1e+21");
        assert_eq!(
            ecmascript_number(123_456_789_012_345_680_000.0).unwrap(),
            "123456789012345680000"
        );
        assert_eq!(ecmascript_number(1e-7).unwrap(), "1e-7");
        assert_eq!(ecmascript_number(0.000_001).unwrap(), "0.000001");
    }

    #[test]
    fn ecmascript_spelling_matches_pinned_chromium_vectors() {
        let corpus: serde_json::Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/number-format-vectors.json"
        ))
        .unwrap();
        for vector in corpus["vectors"].as_array().unwrap() {
            let bits = u64::from_str_radix(vector["bits"].as_str().unwrap(), 16).unwrap();
            let value = f64::from_bits(bits);
            if let Some(expected) = vector["shortest"].as_str() {
                assert_eq!(ecmascript_number(value).unwrap(), expected, "{vector}");
            } else {
                assert!(ecmascript_number(value).is_err(), "{vector}");
            }
        }
    }

    #[test]
    fn rough_fixed_decimals_match_pinned_chromium_vectors() {
        let corpus: serde_json::Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/number-format-vectors.json"
        ))
        .unwrap();
        for vector in corpus["vectors"].as_array().unwrap() {
            let bits = u64::from_str_radix(vector["bits"].as_str().unwrap(), 16).unwrap();
            let value = f64::from_bits(bits);
            if let Some(expected) = vector["rough"].as_str() {
                assert_eq!(rough(value), expected, "{vector}");
                assert_eq!(
                    ryu_js::Buffer::new().format_to_fixed(value, 2),
                    vector["fixed"].as_str().unwrap(),
                    "{vector}"
                );
            } else {
                let mut output = String::from("unchanged");
                assert!(push_rough_number(&mut output, value).is_err(), "{vector}");
                assert_eq!(output, "unchanged");
            }
        }
    }

    #[cfg(target_arch = "wasm32")]
    mod wasm_tests {
        #[wasm_bindgen_test::wasm_bindgen_test]
        fn number_formatting_matches_pinned_chromium_vectors() {
            super::ecmascript_spelling_matches_pinned_chromium_vectors();
            super::rough_fixed_decimals_match_pinned_chromium_vectors();
        }
    }

    #[test]
    fn display_numbers_canonicalize_negative_zero_and_reject_non_finite_values() {
        assert_eq!(svg(-0.0), "0");
        assert_eq!(svg(0.5), "0.5");
        assert_eq!(svg(1e-7), "0.0000001");
        assert!(push_svg_number(&mut String::new(), f64::INFINITY, "test number").is_err());
    }
}
