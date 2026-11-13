//! Unclamped numeric color conversions. RGB channels are unit-scaled, XYZ uses
//! Y = 1, and Lab uses D50 with L in 0..100. Extended channels are preserved.
//!
//! Keep the literal constants and arithmetic order: these paths feed byte
//! quantization. The sign-reflected sRGB transfers here intentionally differ
//! from csscolorparser's negative-channel behavior; its f32 conversion routes
//! remain in the caller along with alpha, gamut mapping and CSS policy.

use crate::portable_math::{cbrt, powf};

pub(super) fn decode_srgb(component: f64) -> f64 {
    if component.abs() <= 0.040_45 {
        component / 12.92
    } else {
        component.signum() * powf((component.abs() + 0.055) / 1.055, 2.4)
    }
}

pub(super) fn encode_srgb(component: f64) -> f64 {
    if component.abs() > 0.003_130_8 {
        component.signum() * (1.055 * powf(component.abs(), 1.0 / 2.4) - 0.055)
    } else {
        12.92 * component
    }
}

fn multiply(matrix: [[f64; 3]; 3], vector: [f64; 3]) -> [f64; 3] {
    matrix.map(|row| row[0] * vector[0] + row[1] * vector[1] + row[2] * vector[2])
}

pub(super) fn adapt_d50_to_d65(xyz: [f64; 3]) -> [f64; 3] {
    multiply(
        [
            [0.955_473_4, -0.023_098_5, 0.063_259_3],
            [-0.028_369_7, 1.009_995_5, 0.021_041_4],
            [0.012_314_0, -0.020_507_7, 1.330_365_9],
        ],
        xyz,
    )
}

pub(super) fn adapt_d65_to_d50(xyz: [f64; 3]) -> [f64; 3] {
    multiply(
        [
            [1.047_929_8, 0.022_946_8, -0.050_192_2],
            [0.029_627_8, 0.990_434_5, -0.017_073_8],
            [-0.009_243_0, 0.015_055_2, 0.751_874_3],
        ],
        xyz,
    )
}

pub(super) fn srgb_to_xyz_d65(rgb: [f64; 3]) -> [f64; 3] {
    multiply(
        [
            [0.412_390_8, 0.357_584_34, 0.180_480_79],
            [0.212_639_01, 0.715_168_68, 0.072_192_32],
            [0.019_330_82, 0.119_194_78, 0.950_532_15],
        ],
        rgb.map(decode_srgb),
    )
}

pub(super) fn xyz_d65_to_linear_srgb(xyz: [f64; 3]) -> [f64; 3] {
    multiply(
        [
            [3.240_969_94, -1.537_383_18, -0.498_610_76],
            [-0.969_243_64, 1.875_967_50, 0.041_555_06],
            [0.055_630_08, -0.203_976_96, 1.056_971_51],
        ],
        xyz,
    )
}

pub(super) fn display_p3_to_xyz_d65([first, second, third]: [f64; 3]) -> [f64; 3] {
    multiply(
        [
            [0.486_570_95, 0.265_667_69, 0.198_217_29],
            [0.228_974_56, 0.691_738_52, 0.079_286_91],
            [0.0, 0.045_113_38, 1.043_944_37],
        ],
        [decode_srgb(first), decode_srgb(second), decode_srgb(third)],
    )
}

pub(super) fn linear_display_p3_to_xyz_d65(rgb: [f64; 3]) -> [f64; 3] {
    multiply(
        [
            [0.486_570_95, 0.265_667_69, 0.198_217_29],
            [0.228_974_56, 0.691_738_52, 0.079_286_91],
            [0.0, 0.045_113_38, 1.043_944_37],
        ],
        rgb,
    )
}

pub(super) fn a98_rgb_to_xyz_d65([first, second, third]: [f64; 3]) -> [f64; 3] {
    multiply(
        [
            [0.576_730_9, 0.185_554_0, 0.188_185_2],
            [0.297_376_9, 0.627_349_1, 0.075_274_1],
            [0.027_034_3, 0.070_687_2, 0.991_108_5],
        ],
        [
            first.signum() * powf(first.abs(), 563.0 / 256.0),
            second.signum() * powf(second.abs(), 563.0 / 256.0),
            third.signum() * powf(third.abs(), 563.0 / 256.0),
        ],
    )
}

pub(super) fn prophoto_rgb_to_xyz_d65([first, second, third]: [f64; 3]) -> [f64; 3] {
    let decode = |component: f64| {
        if component.abs() <= 16.0 / 512.0 {
            component / 16.0
        } else {
            component.signum() * powf(component.abs(), 1.8)
        }
    };
    let d50 = multiply(
        [
            [0.797_760_49, 0.135_185_84, 0.031_349_35],
            [0.288_071_13, 0.711_843_22, 0.000_085_65],
            [0.0, 0.0, 0.825_104_60],
        ],
        [decode(first), decode(second), decode(third)],
    );
    adapt_d50_to_d65(d50)
}

pub(super) fn rec2020_to_xyz_d65([first, second, third]: [f64; 3]) -> [f64; 3] {
    let decode = |component: f64| {
        const ALPHA: f64 = 1.099_296_826_809_44;
        const BETA: f64 = 0.018_053_968_510_807;
        if component.abs() < BETA * 4.5 {
            component / 4.5
        } else {
            component.signum() * powf((component.abs() + ALPHA - 1.0) / ALPHA, 1.0 / 0.45)
        }
    };
    multiply(
        [
            [0.636_958_05, 0.144_616_90, 0.168_880_98],
            [0.262_700_21, 0.677_998_07, 0.059_301_72],
            [0.0, 0.028_072_69, 1.060_985_06],
        ],
        [decode(first), decode(second), decode(third)],
    )
}

pub(super) fn srgb_to_display_p3(rgb: [f64; 3]) -> [f64; 3] {
    let linear = multiply(
        [
            [2.493_496_91, -0.931_383_62, -0.402_710_78],
            [-0.829_488_97, 1.762_664_06, 0.023_624_69],
            [0.035_845_83, -0.076_172_39, 0.956_884_52],
        ],
        srgb_to_xyz_d65(rgb),
    );
    linear.map(encode_srgb)
}

pub(super) fn srgb_to_a98_rgb(rgb: [f64; 3]) -> [f64; 3] {
    multiply(
        [
            [2.041_587_90, -0.565_006_97, -0.344_731_35],
            [-0.969_243_64, 1.875_967_50, 0.041_555_06],
            [0.013_444_28, -0.118_362_39, 1.015_174_99],
        ],
        srgb_to_xyz_d65(rgb),
    )
    .map(|component| component.signum() * powf(component.abs(), 256.0 / 563.0))
}

pub(super) fn srgb_to_prophoto_rgb(rgb: [f64; 3]) -> [f64; 3] {
    multiply(
        [
            [1.345_798_97, -0.255_580_10, -0.051_106_29],
            [-0.544_622_49, 1.508_232_74, 0.020_536_03],
            [0.0, 0.0, 1.211_967_55],
        ],
        adapt_d65_to_d50(srgb_to_xyz_d65(rgb)),
    )
    .map(|component| {
        if component.abs() <= 1.0 / 512.0 {
            component * 16.0
        } else {
            component.signum() * powf(component.abs(), 1.0 / 1.8)
        }
    })
}

pub(super) fn srgb_to_rec2020(rgb: [f64; 3]) -> [f64; 3] {
    multiply(
        [
            [1.716_651_19, -0.355_670_78, -0.253_366_28],
            [-0.666_684_35, 1.616_481_24, 0.015_768_55],
            [0.017_639_86, -0.042_770_61, 0.942_103_12],
        ],
        srgb_to_xyz_d65(rgb),
    )
    .map(encode_rec2020)
}

fn encode_rec2020(component: f64) -> f64 {
    const ALPHA: f64 = 1.099_296_826_809_44;
    const BETA: f64 = 0.018_053_968_510_807;
    if component.abs() < BETA {
        component * 4.5
    } else {
        component.signum() * (ALPHA * powf(component.abs(), 0.45) - (ALPHA - 1.0))
    }
}

pub(super) fn srgb_to_lab_d50(rgb: [f64; 3]) -> [f64; 3] {
    let xyz = adapt_d65_to_d50(srgb_to_xyz_d65(rgb));
    let f = |value: f64| {
        const EPSILON: f64 = 216.0 / 24_389.0;
        const KAPPA: f64 = 24_389.0 / 27.0;
        if value > EPSILON {
            cbrt(value)
        } else {
            (KAPPA * value + 16.0) / 116.0
        }
    };
    let x = f(xyz[0] / 0.964_22);
    let y = f(xyz[1]);
    let z = f(xyz[2] / 0.825_21);
    [116.0 * y - 16.0, 500.0 * (x - y), 200.0 * (y - z)]
}

pub(super) fn lab_d50_to_srgb([lightness, a, b]: [f64; 3]) -> [f64; 3] {
    let f1 = (lightness + 16.0) / 116.0;
    let f0 = a / 500.0 + f1;
    let f2 = f1 - b / 200.0;
    let inverse = |value: f64| {
        const EPSILON: f64 = 216.0 / 24_389.0;
        const KAPPA: f64 = 24_389.0 / 27.0;
        let cube = value * value * value;
        if cube > EPSILON {
            cube
        } else {
            (116.0 * value - 16.0) / KAPPA
        }
    };
    let xyz_d50 = [inverse(f0) * 0.964_22, inverse(f1), inverse(f2) * 0.825_21];
    let xyz = adapt_d50_to_d65(xyz_d50);
    let linear = multiply(
        [
            [3.240_969_94, -1.537_383_18, -0.498_610_76],
            [-0.969_243_64, 1.875_967_50, 0.041_555_06],
            [0.055_630_08, -0.203_976_96, 1.056_971_51],
        ],
        xyz,
    );
    linear.map(encode_srgb)
}
