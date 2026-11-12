//! Skia's snapping of an ICC profile to its sRGB color space, which then draws
//! untransformed (`SkColorSpace::Make`/`MakeRGB` at Skia 53348aa, pinned by
//! Chromium 149). A profile snaps when its CICP tag is BT.709 primaries with
//! the sRGB transfer at full range, or when its matrix is within 0.01 of
//! Skia's sRGB gamut and its transfer is sRGB: identical parametric curves
//! within 0.001 of sRGB's seven parameters, or curves whose round trip through
//! the sRGB inverse stays below 1/512. Skia's third route,
//! `skcms_ApproximatelyEqualProfiles`, compares sampled transforms with skcms's
//! own arithmetic and is not replicated.

use moxcms::{ColorProfile, ToneReprCurve};

use crate::portable_math;

/// `powf` through the portable math the renderer uses on every target.
fn pow(base: f32, exponent: f32) -> f32 {
    portable_math::powf(f64::from(base), f64::from(exponent)) as f32
}

const fn fixed(v: u32) -> f32 {
    v as f32 / 65536.0
}

/// `SkNamedGamut::kSRGB`, the ICC 16.16 values Skia keeps in sync with skcms.
const SRGB_GAMUT: [[f32; 3]; 3] = [
    [fixed(0x6FA2), fixed(0x6299), fixed(0x24A0)],
    [fixed(0x38F5), fixed(0xB785), fixed(0x0F84)],
    [fixed(0x0390), fixed(0x18DA), fixed(0xB6CF)],
];

/// `SkNamedTransferFn::kSRGB` as `{g, a, b, c, d, e, f}`.
const SRGB_TRANSFER: [f32; 7] = [
    2.4,
    (1.0 / 1.055) as f32,
    (0.055 / 1.055) as f32,
    (1.0 / 12.92) as f32,
    0.04045,
    0.0,
    0.0,
];

/// skcms's parametric form of a curve, when it has one: a `curv` with no or one
/// entry, or a `para` of any type.
fn parametric(curve: &ToneReprCurve) -> Option<[f32; 7]> {
    match curve {
        ToneReprCurve::Lut(table) if table.is_empty() => Some([1.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0]),
        ToneReprCurve::Lut(table) if table.len() == 1 => {
            Some([f32::from(table[0]) / 256.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0])
        }
        ToneReprCurve::Lut(_) => None,
        ToneReprCurve::Parametric(p) => match *p.as_slice() {
            [g] => Some([g, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0]),
            [g, a, b] => Some([g, a, b, 0.0, -b / a, 0.0, 0.0]),
            [g, a, b, c] => Some([g, a, b, 0.0, -b / a, c, c]),
            [g, a, b, c, d] => Some([g, a, b, c, d, 0.0, 0.0]),
            [g, a, b, c, d, e, f] => Some([g, a, b, c, d, e, f]),
            _ => None,
        },
    }
}

/// skcms `eval_curve` for a table: linear interpolation between 16-bit entries.
fn eval_table(table: &[u16], x: f32) -> f32 {
    let ix = x.clamp(0.0, 1.0) * (table.len() - 1) as f32;
    let lo = ix as usize;
    let hi = (lo + 1).min(table.len() - 1);
    let t = ix - lo as f32;
    let (l, h) = (
        f32::from(table[lo]) / 65535.0,
        f32::from(table[hi]) / 65535.0,
    );
    l + (h - l) * t
}

/// skcms `skcms_TransferFunction_eval` of the sRGB inverse, in sRGBish form.
fn srgb_inverse(y: f32) -> f32 {
    let (g, a, c, d, e) = (
        1.0 / 2.4_f32,
        pow(1.055, 2.4),
        12.92_f32,
        0.003_130_8_f32,
        -0.055_f32,
    );
    let magnitude = y.abs();
    let value = if magnitude < d {
        c * magnitude
    } else {
        pow(a * magnitude, g) + e
    };
    value.copysign(y)
}

/// `skcms_AreApproximateInverses` against the sRGB inverse.
fn approximately_srgb(curve: &ToneReprCurve) -> bool {
    let curve_at: Box<dyn Fn(f32) -> f32> = match curve {
        ToneReprCurve::Lut(table) if table.len() > 1 => Box::new(move |x| eval_table(table, x)),
        _ => match parametric(curve) {
            Some([g, a, b, c, d, e, f]) => Box::new(move |x: f32| {
                let m = x.abs();
                let v = if m < d {
                    c * m + f
                } else {
                    pow(a * m + b, g) + e
                };
                v.copysign(x)
            }),
            None => return false,
        },
    };
    let n = match curve {
        ToneReprCurve::Lut(table) => table.len().max(256),
        ToneReprCurve::Parametric(_) => 256,
    };
    let dx = 1.0 / (n - 1) as f32;
    (0..n)
        .map(|i| {
            let x = i as f32 * dx;
            (x - srgb_inverse(curve_at(x))).abs()
        })
        .fold(0.0_f32, f32::max)
        < 1.0 / 512.0
}

/// Whether Skia maps this RGB profile to its sRGB color space.
pub(super) fn snaps_to_srgb(profile: &ColorProfile) -> bool {
    if let Some(cicp) = &profile.cicp {
        if cicp.matrix_coefficients as u8 == 0 && cicp.full_range {
            return cicp.color_primaries as u8 == 1 && cicp.transfer_characteristics as u8 == 13;
        }
    }
    let columns = [
        profile.red_colorant,
        profile.green_colorant,
        profile.blue_colorant,
    ];
    let matrix_is_srgb = (0..3).all(|row| {
        (0..3).all(|column| {
            let value = [columns[column].x, columns[column].y, columns[column].z][row] as f32;
            (value - SRGB_GAMUT[row][column]).abs() < 0.01
        })
    });
    let (Some(r), Some(g), Some(b)) = (&profile.red_trc, &profile.green_trc, &profile.blue_trc)
    else {
        return false;
    };
    let transfer_is_srgb = match (parametric(r), parametric(g), parametric(b)) {
        (Some(pr), Some(pg), Some(pb)) if pr == pg && pr == pb => pr
            .iter()
            .zip(SRGB_TRANSFER)
            .all(|(v, s)| (v - s).abs() < 0.001),
        _ => [r, g, b].into_iter().all(approximately_srgb),
    };
    matrix_is_srgb && transfer_is_srgb
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_parametric_srgb_transfer_and_gamut_snap_and_others_do_not() {
        let mut profile = ColorProfile::new_srgb();
        for trc in [
            &mut profile.red_trc,
            &mut profile.green_trc,
            &mut profile.blue_trc,
        ] {
            *trc = Some(ToneReprCurve::Parametric(SRGB_TRANSFER.to_vec()));
        }
        assert!(snaps_to_srgb(&profile));
        let mut linear = profile.clone();
        for trc in [
            &mut linear.red_trc,
            &mut linear.green_trc,
            &mut linear.blue_trc,
        ] {
            *trc = Some(ToneReprCurve::Lut(vec![]));
        }
        assert!(!snaps_to_srgb(&linear));
        let mut shifted = profile.clone();
        shifted.red_colorant.x += 0.02;
        assert!(!snaps_to_srgb(&shifted));
    }

    #[test]
    fn a_tabulated_srgb_curve_snaps_within_the_round_trip_bound() {
        let table: Vec<u16> = (0..1024)
            .map(|i| {
                let x = i as f64 / 1023.0;
                let y = if x < 0.04045 {
                    x / 12.92
                } else {
                    portable_math::powf((x + 0.055) / 1.055, 2.4)
                };
                (y * 65535.0).round() as u16
            })
            .collect();
        let mut profile = ColorProfile::new_srgb();
        for trc in [
            &mut profile.red_trc,
            &mut profile.green_trc,
            &mut profile.blue_trc,
        ] {
            *trc = Some(ToneReprCurve::Lut(table.clone()));
        }
        assert!(snaps_to_srgb(&profile));
        let gamma: Vec<u16> = (0..1024)
            .map(|i| (portable_math::powf(i as f64 / 1023.0, 1.8) * 65535.0).round() as u16)
            .collect();
        for trc in [
            &mut profile.red_trc,
            &mut profile.green_trc,
            &mut profile.blue_trc,
        ] {
            *trc = Some(ToneReprCurve::Lut(gamma.clone()));
        }
        assert!(!snaps_to_srgb(&profile));
    }
}
