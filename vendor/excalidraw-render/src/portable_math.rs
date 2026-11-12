//! The transcendental functions rendering math may call, each routed through
//! the pure-Rust `libm` crate so that one input gives one double on every
//! supported target (ADR-0007).
//!
//! `f64::sin` and its siblings are not correctly rounded, and which
//! implementation answers them is a property of the target: on
//! `x86_64-unknown-linux-gnu` they resolve to glibc's libm, on
//! `wasm32-unknown-unknown` to the `compiler-builtins` port of this very
//! crate, and the two can differ in the last ulp. Rough path coordinates are
//! quantized to two decimals and colors to eight bits, so most of that
//! difference vanishes, but a value within an ulp of a rounding boundary flips
//! a digit and with it the artifact bytes. Calling one implementation on every
//! target makes artifact byte stability structural for these functions rather
//! than something the cross-target pins observe after the fact.
//!
//! What stays on `std`: `sqrt`, `abs`, `floor`/`ceil`/`round`/`trunc`,
//! `min`/`max`, `copysign`, `rem_euclid`, `mul_add`, and `to_radians` and
//! `to_degrees` (one multiplication each). IEEE 754 requires each of them to
//! be exact or correctly rounded, so every conforming implementation agrees.
//! `powi` is a compiler intrinsic with no specified rounding: at run time
//! LLVM expands a small constant exponent into repeated multiplication and
//! `compiler-builtins`' `__powidf2` does the same, but when the base is a
//! compile-time constant LLVM folds the call with the *host* compiler's
//! `pow`, which is correctly rounded and therefore differs from the
//! multiplication chain — `0.3.powi(3)` folds to `0.026999999999999996` while
//! `0.3 * 0.3 * 0.3` is `0.027`. Every site that used it squared or cubed,
//! so they call [`square`] and [`cube`] here, which are the one or two IEEE
//! multiplications on every target and under every optimizer decision.
//!
//! The workspace `clippy.toml` disallows the `std` methods this module wraps,
//! so a new call site has to come through here.

/// Sine of an angle in radians.
pub(crate) fn sin(radians: f64) -> f64 {
    libm::sin(radians)
}

/// Cosine of an angle in radians.
pub(crate) fn cos(radians: f64) -> f64 {
    libm::cos(radians)
}

/// `(sin, cos)` of an angle in radians, the pair `f64::sin_cos` returns.
pub(crate) fn sin_cos(radians: f64) -> (f64, f64) {
    (sin(radians), cos(radians))
}

/// Arctangent of `value`, in radians within `[-π/2, π/2]`; JavaScript's
/// `Math.atan`, which the Rough.js dashed and zigzag-line fillers take of a
/// hachure line's slope.
pub(crate) fn atan(value: f64) -> f64 {
    libm::atan(value)
}

/// Four-quadrant arctangent of `y / x`, in radians.
pub(crate) fn atan2(y: f64, x: f64) -> f64 {
    libm::atan2(y, x)
}

/// `base` raised to the real power `exponent`.
pub(crate) fn powf(base: f64, exponent: f64) -> f64 {
    libm::pow(base, exponent)
}

/// Natural exponential, used by the HLG inverse transfer function.
pub(crate) fn exp(value: f64) -> f64 {
    libm::exp(value)
}

/// Base-two exponential, used to apply HDR tone-map log gain.
pub(crate) fn exp2(value: f64) -> f64 {
    libm::exp2(value)
}

/// Base-two logarithm, used to construct HDR tone-map gain curves.
pub(crate) fn log2(value: f64) -> f64 {
    libm::log2(value)
}

/// Euclidean length of the vector `(x, y)`.
pub(crate) fn hypot(x: f64, y: f64) -> f64 {
    libm::hypot(x, y)
}

/// Euclidean length of the `f32` vector `(x, y)`, for geometry that follows
/// a dependency's own `f32` arithmetic.
#[cfg(any(feature = "png", feature = "pdf"))]
pub(crate) fn hypotf(x: f32, y: f32) -> f32 {
    libm::hypotf(x, y)
}

/// Cube root.
pub(crate) fn cbrt(value: f64) -> f64 {
    libm::cbrt(value)
}

/// `value * value`, the one multiplication `powi(2)` performs.
pub(crate) fn square<T: Copy + std::ops::Mul<Output = T>>(value: T) -> T {
    value * value
}

/// `value * value * value`, the two multiplications `powi(3)` performs. IEEE
/// multiplication is commutative, so `(v * v) * v` and `v * (v * v)` are the
/// same double and the association here is not a choice.
pub(crate) fn cube(value: f64) -> f64 {
    value * value * value
}

/// `value³` rounded once, from the exact product of the significands: the
/// value ECMAScript's `Math.pow(value, 3)` returns in the pinned reference
/// profile, whose V8 defers `Math.pow` to the platform `pow` (glibc's, correctly
/// rounded in all but its rare hard cases), where [`cube`] rounds twice and
/// lands an ulp away about a quarter of the time. The reference's cubic
/// extremum (`getBezierValueForT`) is the one place the renderer needs
/// `Math.pow(x, 3)` at full precision: the scene extent of a curve (#173).
/// A cube that would overflow, or whose result would be subnormal, falls back
/// to the twice-rounded product, which is exact or infinite there anyway.
pub(crate) fn cube_rounded_once(value: f64) -> f64 {
    if value == 0.0 || !value.is_finite() {
        return cube(value);
    }
    let bits = value.to_bits();
    let sign = bits >> 63;
    let biased_exponent = ((bits >> 52) & 0x7ff) as i64;
    let fraction = bits & ((1u64 << 52) - 1);
    let (significand, exponent) = if biased_exponent == 0 {
        (fraction, -1074)
    } else {
        (fraction | (1u64 << 52), biased_exponent - 1075)
    };
    // `significand³` needs up to 159 bits: `square` fits a `u128`, and the
    // last multiplication is carried in two 64-bit limbs weighted `2^64`
    // (`high`) and `2^0` (`low_low`), the middle limb folded into `high`.
    let squared = u128::from(significand) * u128::from(significand);
    let low = (squared & ((1u128 << 64) - 1)) * u128::from(significand);
    let high = (squared >> 64) * u128::from(significand) + (low >> 64);
    let low_low = low & ((1u128 << 64) - 1);
    let total_bits = if high > 0 {
        i64::from(128 - high.leading_zeros()) + 64
    } else {
        i64::from(128 - low_low.leading_zeros())
    };
    let shift = total_bits - 53;
    // Round the product to 53 bits, half to even, from the dropped bits.
    let (quotient, dropped_versus_half) = if shift <= 0 {
        (
            ((high << 64) | low_low) << (-shift),
            core::cmp::Ordering::Less,
        )
    } else if shift >= 64 {
        // Every bit of `low_low` is dropped along with `high_shift` bits of
        // `high`; the half-way mark is bit `shift - 1` of the whole product.
        let high_shift = shift - 64;
        let quotient = high >> high_shift;
        let ordering = if high_shift == 0 {
            low_low.cmp(&(1u128 << 63))
        } else {
            let dropped_high = high & ((1u128 << high_shift) - 1);
            match dropped_high.cmp(&(1u128 << (high_shift - 1))) {
                core::cmp::Ordering::Equal if low_low != 0 => core::cmp::Ordering::Greater,
                ordering => ordering,
            }
        };
        (quotient, ordering)
    } else {
        let quotient = (high << (64 - shift)) | (low_low >> shift);
        let dropped = low_low & ((1u128 << shift) - 1);
        (quotient, dropped.cmp(&(1u128 << (shift - 1))))
    };
    let mut quotient = quotient as u64;
    match dropped_versus_half {
        core::cmp::Ordering::Greater => quotient += 1,
        core::cmp::Ordering::Equal if quotient & 1 == 1 => quotient += 1,
        _ => {}
    }
    let mut result_exponent = 3 * exponent + shift;
    if quotient == 1u64 << 53 {
        quotient >>= 1;
        result_exponent += 1;
    }
    let biased = result_exponent + 52 + 1023;
    if biased <= 0 || biased >= 0x7ff {
        return cube(value);
    }
    f64::from_bits((sign << 63) | ((biased as u64) << 52) | (quotient & ((1u64 << 52) - 1)))
}

/// ECMAScript `Math.hypot(x, y)` as the pinned V8 evaluates it, which is not
/// the correctly rounded [`hypot`]: the arguments are normalized to the larger
/// magnitude, their squares Kahan-summed and the root scaled back,
/// `sqrt(1 + (min / max)²) * max` for two finite arguments (the running
/// compensation never feeds back with two terms). The reference's ellipse
/// bounds take it of the rotated half extents (#172), where the last ulp is
/// what the SVG `viewBox` prints.
pub(crate) fn ecmascript_hypot(x: f64, y: f64) -> f64 {
    let (x, y) = (x.abs(), y.abs());
    if x.is_infinite() || y.is_infinite() {
        return f64::INFINITY;
    }
    if x.is_nan() || y.is_nan() {
        return f64::NAN;
    }
    let max = x.max(y);
    if max == 0.0 {
        return 0.0;
    }
    let first = x / max;
    let second = y / max;
    (first * first + second * second).sqrt() * max
}

#[cfg(test)]
mod tests {
    use super::{cube, sin_cos, square};

    // The module's claim about the run-time `powi` expansion is checked
    // against the intrinsic itself, which nothing else in the crate may call.
    // `black_box` keeps the base out of the optimizer's hands: a constant base
    // is folded with the host's `pow` instead, which is the hazard the module
    // documents, and the call sites this replaced never had a constant base.
    #[allow(clippy::disallowed_methods)]
    #[test]
    fn square_and_cube_match_the_run_time_powi_expansion_bit_for_bit() {
        let samples = [
            0.0_f64,
            -0.0,
            1.0,
            -1.0,
            0.3,
            0.7,
            1e-300,
            1e300,
            123.456,
            -987.654,
            0.1 + 0.2,
        ];
        for value in samples.map(std::hint::black_box) {
            assert_eq!(square(value).to_bits(), value.powi(2).to_bits(), "{value}");
            assert_eq!(cube(value).to_bits(), value.powi(3).to_bits(), "{value}");
            let single = value as f32;
            assert_eq!(
                square(single).to_bits(),
                single.powi(2).to_bits(),
                "{single}"
            );
        }
    }

    #[test]
    fn sin_cos_is_the_pair_of_sin_and_cos() {
        for radians in [0.0, 0.5, 1.0, -2.5, std::f64::consts::PI, 1e6] {
            let (sin, cos) = sin_cos(radians);
            assert_eq!(sin.to_bits(), super::sin(radians).to_bits());
            assert_eq!(cos.to_bits(), super::cos(radians).to_bits());
        }
    }

    #[test]
    fn cube_rounded_once_is_the_reference_math_pow_where_two_roundings_miss_it() {
        use super::cube_rounded_once;
        // `Math.pow(x, 3)` under Node 24.19.0 (V8 deferring to glibc's `pow`),
        // beside `x * x * x` in the same engine, for values where the two
        // differ: the correctly rounded cube is the reference's.
        let differing: [(f64, f64, f64); 6] = [
            (
                0.616_205_673_688_851_3,
                0.233_979_106_528_009_88,
                0.233_979_106_528_009_85,
            ),
            (
                0.292_803_224_645_716_2,
                0.025_103_112_125_565_72,
                0.025_103_112_125_565_725,
            ),
            (
                0.703_117_954_469_567_3,
                0.347_603_838_826_051,
                0.347_603_838_826_051_06,
            ),
            (
                0.883_273_516_906_646_3,
                0.689_105_359_566_399_5,
                0.689_105_359_566_399_6,
            ),
            (
                0.257_553_106_503_737_53,
                0.017_084_425_433_172_436,
                0.017_084_425_433_172_44,
            ),
            (
                0.712_343_658_038_061_7,
                0.361_467_026_445_337_1,
                0.361_467_026_445_337_2,
            ),
        ];
        for (value, pow, twice) in differing {
            assert_eq!(cube(value).to_bits(), f64::to_bits(twice), "{value} twice");
            assert_eq!(
                cube_rounded_once(value).to_bits(),
                f64::to_bits(pow),
                "{value} once"
            );
        }
        // And where they agree, including the sign, a value whose cube is
        // exact, and the overflowing and vanishing ends that fall back.
        let agreeing: [(f64, f64); 12] = [
            (0.3, 0.026_999_999_999_999_996),
            (0.7, 0.342_999_999_999_999_9),
            (0.5, 0.125),
            (-0.152_134_363_308_487_29, -0.003_521_129_224_458_991),
            (123.456, 1_881_640.295_202_816_1),
            (1e100, 1e300),
            (1e103, f64::INFINITY),
            (1e-300, 0.0),
            (2e-108, 1e-323),
            (0.0, 0.0),
            (1.0, 1.0),
            (-1.0, -1.0),
        ];
        for (value, pow) in agreeing {
            assert_eq!(
                cube_rounded_once(value).to_bits(),
                f64::to_bits(pow),
                "{value}"
            );
        }
        assert!(cube_rounded_once(f64::NAN).is_nan());
        assert_eq!(cube_rounded_once(f64::NEG_INFINITY), f64::NEG_INFINITY);
    }

    #[test]
    fn ecmascript_hypot_normalizes_like_v8_where_the_naive_root_misses() {
        use super::{ecmascript_hypot, hypot};
        // `Math.hypot(a, b)` under Node 24.19.0 beside `Math.sqrt(a*a + b*b)`
        // for pairs where the two differ.
        let pairs: [(f64, f64, f64, f64); 5] = [
            (
                4.436_023_699_646_386,
                -5.331_564_516_784_604,
                6.935_696_537_512_369,
                6.935_696_537_512_368,
            ),
            (
                -7.675_428_604_318_679,
                -21.211_718_603_191_01,
                22.557_686_281_198_1,
                22.557_686_281_198_098,
            ),
            (
                -6.635_509_993_519_029,
                -1.917_027_456_322_562,
                6.906_879_696_533_41,
                6.906_879_696_533_410_5,
            ),
            (
                -2.754_105_021_405_473,
                -2.831_362_128_856_252,
                3.949_899_489_056_936,
                3.949_899_489_056_936_6,
            ),
            (
                19.576_852_310_314_674,
                48.882_048_037_118_59,
                52.656_507_353_631_45,
                52.656_507_353_631_44,
            ),
        ];
        for (a, b, reference, naive) in pairs {
            assert_eq!(
                (a * a + b * b).sqrt().to_bits(),
                f64::to_bits(naive),
                "{a} {b} naive"
            );
            assert_eq!(
                ecmascript_hypot(a, b).to_bits(),
                f64::to_bits(reference),
                "{a} {b}"
            );
        }
        // The rotated half extents of the 100x60 ellipse of
        // `focused.rotated-shape-bounds.v1` at 1.2 rad, as `Math.hypot` takes
        // them; the correctly rounded `hypot` agrees on these two.
        let (sin, cos) = sin_cos(1.2);
        assert_eq!(
            ecmascript_hypot(50.0 * cos, 30.0 * sin).to_bits(),
            f64::to_bits(33.317_938_525_169_94)
        );
        assert_eq!(
            ecmascript_hypot(30.0 * cos, 50.0 * sin).to_bits(),
            f64::to_bits(47.853_056_040_685_594)
        );
        assert_eq!(
            hypot(50.0 * cos, 30.0 * sin).to_bits(),
            f64::to_bits(33.317_938_525_169_94)
        );
        assert_eq!(ecmascript_hypot(0.0, 0.0), 0.0);
        assert_eq!(ecmascript_hypot(-3.0, 0.0), 3.0);
        assert_eq!(ecmascript_hypot(f64::INFINITY, f64::NAN), f64::INFINITY);
        assert!(ecmascript_hypot(1.0, f64::NAN).is_nan());
    }
}
