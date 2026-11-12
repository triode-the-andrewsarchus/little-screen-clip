//! Parity vectors from the #198 reference model (a Python port of Chromium
//! 149's canvas `drawImage`, Skia 53348aa and cc's decode cache that
//! reproduces the pinned browser with no differing pixel on every probe).
//! Pixel outputs are pinned as FNV-1a 64 digests of the premultiplied bytes.

use super::*;

/// A deterministic premultiplied test image (the model's `image(w, h, alpha)`).
fn image(width: u32, height: u32, alpha: bool) -> Vec<u8> {
    let mut pixels = Vec::new();
    for i in 0..(width * height) {
        let a = if alpha { (i * 29 + 64) & 255 } else { 255 };
        let rgb = [
            (i * 37 + 11) & 255,
            ((i * 91 + 7) & 255) ^ ((i >> 3) & 255),
            (i * 53 + (i >> 2)) & 255,
        ];
        pixels.extend(rgb.map(|c| ((c * a + 127) / 255) as u8));
        pixels.push(a as u8);
    }
    pixels
}

fn fnv(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// The model's `draw_image`: one canvas `drawImage` onto a transparent
/// `canvas` under the scale-translate `ctm` `(a, d, e, f)`, through `cache`.
#[allow(clippy::too_many_arguments)]
fn draw_image(
    cache: &mut (DecodeCachePlanner, DecodeCache),
    pixels: &[u8],
    size: (u32, u32),
    canvas: (u32, u32),
    src: [f32; 4],
    dst: [f32; 4],
    ctm: [f32; 4],
    alpha: f64,
    output: &mut [u8],
) -> Option<LevelSource> {
    let draw = CanvasImageDraw {
        ctm: AffineF {
            a: ctm[0],
            b: 0.0,
            c: 0.0,
            d: ctm[1],
            e: ctm[2],
            f: ctm[3],
        },
        src: RectF {
            x: src[0],
            y: src[1],
            width: src[2],
            height: src[3],
        },
        dst: RectF {
            x: dst[0],
            y: dst[1],
            width: dst[2],
            height: dst[3],
        },
        alpha8: paint_alpha8(alpha),
    };
    let image = PremulView::new(size.0, size.1, pixels);
    let decode = draw.cc_decode(size.0, size.1).unwrap();
    let (painted, source) = if decode.level > 0 {
        let key = ([7; 32], decode.rect);
        let source = cache.0.plan(key, decode.level_width, decode.level_height);
        let level = cache.1.level(key, image, &decode, source).unwrap();
        (
            device_draw_image_rect(level.view(), &draw.for_level(&decode), canvas.0, canvas.1),
            Some(source),
        )
    } else {
        (
            device_draw_image_rect(image, &draw, canvas.0, canvas.1),
            None,
        )
    };
    if let Some(painted) = painted.unwrap() {
        let width = (painted.right - painted.left) as usize;
        for (row, line) in painted.pixels.chunks_exact(width * 4).enumerate() {
            let start =
                ((painted.top as usize + row) * canvas.0 as usize + painted.left as usize) * 4;
            output[start..start + width * 4].copy_from_slice(line);
        }
    }
    source
}

fn draw_digest(
    size: (u32, u32),
    alpha_image: bool,
    canvas: (u32, u32),
    src_dst: &[f32],
    ctm: [f32; 4],
    alpha: f64,
) -> u64 {
    let pixels = image(size.0, size.1, alpha_image);
    let (src, dst) = if src_dst.len() == 8 {
        (
            [src_dst[0], src_dst[1], src_dst[2], src_dst[3]],
            [src_dst[4], src_dst[5], src_dst[6], src_dst[7]],
        )
    } else {
        (
            [0.0, 0.0, size.0 as f32, size.1 as f32],
            [src_dst[0], src_dst[1], src_dst[2], src_dst[3]],
        )
    };
    let mut output = vec![0; (canvas.0 * canvas.1 * 4) as usize];
    draw_image(
        &mut Default::default(),
        &pixels,
        size,
        canvas,
        src,
        dst,
        ctm,
        alpha,
        &mut output,
    );
    fnv(&output)
}

const IDENTITY: [f32; 4] = [1.0, 1.0, 0.0, 0.0];

#[test]
fn cc_level_selection_matches_get_level_for_size() {
    for (width, height, target_width, target_height, level) in [
        (25, 25, 12, 12, 1),
        (25, 25, 13, 13, 1),
        (25, 25, 7, 7, 2),
        (25, 25, 6, 6, 2),
        (25, 25, 1, 1, 5),
        (257, 193, 18, 14, 3),
        (257, 193, 103, 77, 1),
        (257, 193, 129, 97, 1),
        (64, 3, 16, 1, 2),
        (64, 64, 64, 6, 0),
        (100, 37, 50, 18, 1),
        (100, 37, 49, 18, 1),
        (1, 1, 1, 1, 0),
    ] {
        assert_eq!(
            cc_level_for_size(width, height, (target_width, target_height)),
            level,
            "{width}x{height} -> {target_width}x{target_height}"
        );
    }
    assert_eq!(cc_level_size(25, 1), 13);
    assert_eq!(cc_level_size(25, 3), 4);
    assert_eq!(cc_level_size(3, 7), 1);
}

#[test]
fn skia_level_matches_compute_level() {
    for (scale_x, scale_y, level) in [
        (0.5, 0.5, 1),
        (0.51, 0.51, 0),
        (0.25, 0.25, 2),
        (0.36, 0.28, 1),
        (0.7, 0.7, 0),
        (1.0, 0.2, 1),
        (0.125, 0.125, 3),
        (0.0625, 0.0625, 4),
        (0.03, 0.04, 4),
    ] {
        assert_eq!(
            skia_compute_level(scale_x, scale_y),
            level,
            "{scale_x}x{scale_y}"
        );
    }
}

#[test]
fn skia_mip_halves_odd_axes_with_one_two_one_taps() {
    for (width, height, expected) in [
        (5, 3, vec![100, 63, 31, 142, 53, 41, 72, 104]),
        (
            4,
            4,
            vec![
                73, 72, 44, 136, 82, 37, 52, 130, 81, 54, 74, 112, 48, 66, 67, 170,
            ],
        ),
        (1, 5, vec![20, 41, 22, 93, 74, 49, 97, 151]),
        (
            7,
            1,
            vec![20, 41, 22, 93, 74, 49, 97, 151, 162, 114, 56, 209],
        ),
    ] {
        let pixels = image(width, height, true);
        let half = skia_half(PremulView::new(width, height, &pixels)).unwrap();
        assert_eq!(half.data, expected, "{width}x{height}");
    }
}

#[test]
fn lowp_bilinear_matches_the_raster_pipeline() {
    let pixels = image(7, 5, true);
    let output = lowp_bilinear(
        PremulView::new(7, 5, &pixels),
        3,
        2,
        (7.0_f64 / 3.0) as f32,
        2.5,
    )
    .unwrap();
    assert_eq!(
        output.data,
        [
            8, 25, 18, 44, 54, 46, 30, 111, 149, 84, 78, 179, 30, 117, 78, 167, 121, 105, 207, 235,
            75, 52, 59, 89
        ]
    );
}

#[test]
fn scale_pixels_samples_skias_own_mip_level() {
    for (width, height, level_width, level_height, digest) in [
        (25, 25, 13, 13, 0x6594_f757_7f67_afc4_u64),
        (25, 25, 7, 7, 0x5b96_e593_38c5_ac5a),
        (257, 193, 65, 49, 0x823f_a1bc_32f3_9a22),
        (37, 23, 10, 6, 0x6509_7380_7675_5941),
    ] {
        let pixels = image(width, height, true);
        let output = scale_pixels(
            PremulView::new(width, height, &pixels),
            level_width,
            level_height,
        )
        .unwrap();
        assert_eq!(
            fnv(&output.data),
            digest,
            "{width}x{height} -> {level_width}x{level_height}"
        );
    }
}

#[test]
fn legacy_sampler_restarts_its_fixed_point_step_every_127_pixels() {
    assert_eq!(
        draw_digest(
            (97, 5),
            true,
            (300, 4),
            &[0.25, 0.5, 299.3, 3.2],
            IDENTITY,
            1.0
        ),
        0x62db_49d8_241a_9e9a
    );
    assert_eq!(
        draw_digest(
            (64, 3),
            false,
            (260, 3),
            &[1.0, 0.0, 256.6, 3.0],
            IDENTITY,
            1.0
        ),
        0xab6f_fae1_9a2c_62a9
    );
    assert_eq!(
        draw_digest(
            (5, 4),
            true,
            (23, 17),
            &[0.5, 0.25, 22.3, 16.1],
            IDENTITY,
            1.0
        ),
        0x82e5_2fee_fe06_f0ab
    );
}

#[test]
fn legacy_sampler_downgrades_to_sprite_translate_and_nearest() {
    let sprite = 0xe361_02f2_14ed_4f83;
    // SkTreatAsSprite: an integer translate, from the destination or the CTM,
    // and a scale whose mapped bounds still round to the bitmap's size.
    assert_eq!(
        draw_digest((9, 7), true, (20, 10), &[3.0, 2.0, 9.0, 7.0], IDENTITY, 1.0),
        sprite
    );
    assert_eq!(
        draw_digest(
            (9, 7),
            true,
            (20, 10),
            &[1.5, 1.0, 9.0, 7.0],
            [1.0, 1.0, 1.5, 1.0],
            1.0
        ),
        sprite
    );
    assert_eq!(
        draw_digest(
            (9, 7),
            true,
            (20, 10),
            &[3.0, 2.0, 9.0002, 7.0001],
            IDENTITY,
            1.0
        ),
        sprite
    );
    // just_trans_integral: a translate within 1/256 of whole pixels.
    assert_eq!(
        draw_digest(
            (9, 7),
            true,
            (20, 10),
            &[3.001, 2.002, 9.0, 7.0],
            IDENTITY,
            1.0
        ),
        sprite
    );
    // A fractional translate filters; so does a scale within 1/32768 of 1
    // (just_trans_general) once its translate is fractional.
    assert_eq!(
        draw_digest(
            (9, 7),
            true,
            (20, 10),
            &[3.25, 2.0, 9.0, 7.0],
            IDENTITY,
            1.0
        ),
        0x45e0_2ea4_d971_0a9d
    );
    assert_eq!(
        draw_digest(
            (9, 7),
            true,
            (20, 10),
            &[3.25, 2.5, 9.0002, 7.0001],
            IDENTITY,
            1.0
        ),
        0x7b8e_7995_a298_d026
    );
}

#[test]
fn minified_draws_sample_the_cc_decode_level() {
    type Case<'a> = ((u32, u32), (u32, u32), &'a [f32], [f32; 4], f64, u64);
    let cases: [Case<'_>; 6] = [
        (
            (25, 25),
            (9, 7),
            &[0.0, 0.0, 9.0, 7.0],
            IDENTITY,
            1.0,
            0xed64_492d_096f_669c,
        ),
        (
            (25, 25),
            (12, 12),
            &[0.0, 0.0, 12.0, 12.0],
            IDENTITY,
            0.37,
            0x6461_e0aa_2f52_6173,
        ),
        (
            (40, 30),
            (12, 9),
            &[5.0, 3.0, 20.0, 15.0, 0.0, 0.0, 12.0, 9.0],
            IDENTITY,
            1.0,
            0x4cdc_6be4_e8fa_1888,
        ),
        (
            (40, 30),
            (7, 5),
            &[5.5, 3.25, 20.0, 15.0, 0.0, 0.0, 6.5, 4.5],
            IDENTITY,
            1.0,
            0x4bf8_831a_7225_ecd5,
        ),
        (
            (40, 30),
            (12, 9),
            &[0.0, 0.0, 12.0, 9.0],
            [-1.0, 1.0, 12.0, 0.0],
            1.0,
            0x426c_34e2_0a1c_b505,
        ),
        (
            (64, 48),
            (20, 16),
            &[1.0, 1.0, 30.0, 22.0],
            [0.5, 0.5, 0.5, 0.25],
            1.0,
            0xc707_2b00_b089_e572,
        ),
    ];
    for (size, canvas, src_dst, ctm, alpha, digest) in cases {
        assert_eq!(
            draw_digest(size, true, canvas, src_dst, ctm, alpha),
            digest,
            "{size:?} {src_dst:?} {ctm:?}"
        );
    }
}

#[test]
fn the_decode_cache_scales_each_level_from_the_smallest_cached_one() {
    let pixels = image(25, 25, true);
    let mut cache = Default::default();
    let mut output = vec![0; 42 * 12 * 4];
    let sources = [
        [0.0, 0.0, 12.0, 12.0],
        [20.0, 0.0, 6.0, 6.0],
        [30.0, 0.0, 3.0, 3.0],
        [40.0, 0.0, 2.0, 2.0],
    ]
    .map(|dst| {
        draw_image(
            &mut cache,
            &pixels,
            (25, 25),
            (42, 12),
            [0.0, 0.0, 25.0, 25.0],
            dst,
            IDENTITY,
            1.0,
            &mut output,
        )
    });
    assert_eq!(
        sources,
        [
            Some(LevelSource::Original),
            Some(LevelSource::Cached(13, 13)),
            Some(LevelSource::Cached(7, 7)),
            Some(LevelSource::Cached(4, 4)),
        ]
    );
    assert_eq!(fnv(&output), 0x7407_fe68_1204_9488);

    let mut planner = DecodeCachePlanner::default();
    let key = ([1; 32], (0, 0, 100, 37));
    assert_eq!(planner.plan(key, 50, 19), LevelSource::Original);
    assert_eq!(planner.plan(key, 25, 10), LevelSource::Cached(50, 19));
    assert_eq!(planner.plan(key, 50, 19), LevelSource::Hit);
    assert_eq!(
        planner.plan(([1; 32], (0, 0, 100, 36)), 25, 9),
        LevelSource::Original
    );
}

#[test]
fn paint_alpha_rounds_in_binary32() {
    assert_eq!(paint_alpha8(1.0), 255);
    assert_eq!(paint_alpha8(0.5), 128);
    assert_eq!(paint_alpha8(0.37), 94);
    // 0.3 * 255 rounds to 76.49999999999999 in binary64 but to 76.5 in
    // binary32, where SkPaint::getAlpha computes it.
    assert_eq!(paint_alpha8(0.3), 77);
}
