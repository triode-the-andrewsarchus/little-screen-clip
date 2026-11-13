//! The raster codecs at the asset cache's own seam: `Cache::resolve` over one
//! scene file record with a transaction sink that admits everything, read off
//! the `Resolution` it returns — decoded RGBA, oriented dimensions, the
//! animation flag, the sniffed media kind and the typed `ResolveError` — rather
//! than off a rendered PNG's pixels or the diagnostics a session later maps
//! them to. The fixture builders are the ones the integration binaries use,
//! included by path from `tests/common`.

use std::sync::Arc;

use base64::Engine as _;

use super::*;

// The codec fixture builders live with the integration plumbing so the six
// `tests/fast/static_*_asset_render.rs` modules and this suite encode the same
// bytes; they are pure encoders with no dependency on either crate spelling,
// which is what lets one file serve both compilation units.
use crate::codec_fixtures as fixtures;

use fixtures::*;

const RED: [u8; 4] = [255, 0, 0, 255];
const GREEN: [u8; 4] = [0, 255, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];
const YELLOW: [u8; 4] = [255, 255, 0, 255];
const GREY_128: [u8; 4] = [128, 128, 128, 255];

/// A scene with no elements and one file record `asset` whose `mimeType` is
/// `mime_type` and whose data URL declares `data_url_media_type` over
/// `payload`; `Cache::resolve` reads only the record.
fn file_record_scene_with_claims(
    mime_type: &str,
    data_url_media_type: &str,
    payload: &[u8],
) -> SceneDocument {
    let encoded = base64::engine::general_purpose::STANDARD.encode(payload);
    let json = format!(
        r#"{{
            "type":"excalidraw","version":2,"elements":[],"appState":{{}},
            "files":{{"asset":{{
                "id":"asset","mimeType":"{mime_type}",
                "dataURL":"data:{data_url_media_type};base64,{encoded}","created":1
            }}}}
        }}"#
    );
    SceneDocument::parse(json.as_bytes()).expect("file record scene is admitted")
}

/// [`file_record_scene_with_claims`] with both claims naming `media_type`.
fn file_record_scene(media_type: &str, payload: &[u8]) -> SceneDocument {
    file_record_scene_with_claims(media_type, media_type, payload)
}

/// A scene with no elements and no file records.
fn empty_scene() -> SceneDocument {
    SceneDocument::parse(
        br#"{"type":"excalidraw","version":2,"elements":[],"appState":{},"files":{}}"#,
    )
    .expect("empty scene is admitted")
}

/// Resolves the `asset` record of `scene` under `authority` for `format`
/// through `cache`, at revision 1 under default limits, sending every
/// transaction to `transact`.
fn resolve_with<F>(
    cache: &mut Cache,
    scene: &SceneDocument,
    supplemental: &SupplementalAssets,
    format: OutputFormat,
    authority: Authority,
    transact: F,
) -> Result<Resolution, ResolutionFailure>
where
    F: FnMut(IntrinsicTransaction) -> Result<(), RenderError>,
{
    cache.resolve(
        scene,
        supplemental,
        CompatibilityRevision::REVISION_1,
        format,
        &ResourceLimits::default(),
        "asset",
        authority,
        transact,
    )
}

/// Resolves the scene's own `asset` record for `format` from a fresh cache,
/// admitting every transaction.
fn resolve_scene(
    scene: &SceneDocument,
    format: OutputFormat,
) -> Result<Resolution, ResolutionFailure> {
    resolve_with(
        &mut Cache::default(),
        scene,
        &SupplementalAssets::default(),
        format,
        Authority::Scene,
        |_| Ok(()),
    )
}

/// Resolves `payload` declared as `media_type` for the PNG target, which
/// derives the pixels.
fn resolve_for_png(media_type: &str, payload: &[u8]) -> Resolution {
    resolve_scene(&file_record_scene(media_type, payload), OutputFormat::Png)
        .expect("the payload decodes")
}

/// Resolves `payload` for the direct-SVG target, which validates without
/// deriving pixels.
fn resolve_for_svg(media_type: &str, payload: &[u8]) -> Resolution {
    resolve_scene(&file_record_scene(media_type, payload), OutputFormat::Svg)
        .expect("the payload validates")
}

/// The decoded asset of `payload` declared as `media_type`.
fn decode(media_type: &str, payload: &[u8]) -> Arc<ResolvedImageAsset> {
    resolve_for_png(media_type, payload).asset
}

/// The typed failure in `result`, which must be the asset's own refusal and
/// not the meter's.
fn asset_error(result: Result<Resolution, ResolutionFailure>) -> ResolveError {
    match result {
        Err(ResolutionFailure::Asset(error)) => error,
        Ok(_) => panic!("the payload is refused"),
        Err(other) => panic!("the payload is refused by the asset, not the meter: {other:?}"),
    }
}

/// The typed failure resolving `payload` for `format` produces.
fn failure_for(media_type: &str, payload: &[u8], format: OutputFormat) -> ResolveError {
    asset_error(resolve_scene(
        &file_record_scene(media_type, payload),
        format,
    ))
}

/// The typed failure resolving `payload` for the PNG target produces.
fn failure(media_type: &str, payload: &[u8]) -> ResolveError {
    failure_for(media_type, payload, OutputFormat::Png)
}

/// The straight-alpha pixel at (`x`, `y`) of a decoded asset.
fn pixel(asset: &ResolvedImageAsset, x: u32, y: u32) -> [u8; 4] {
    assert!(
        x < asset.width && y < asset.height,
        "({x}, {y}) is inside {}x{}",
        asset.width,
        asset.height
    );
    let rgba = asset.rgba.as_ref().expect("the PNG target derives pixels");
    let offset = usize::try_from((y * asset.width + x) * 4).expect("offset fits usize");
    rgba[offset..offset + 4].try_into().expect("four channels")
}

/// The only pixel of a one-pixel decoded asset.
fn only_pixel(asset: &ResolvedImageAsset) -> [u8; 4] {
    assert_eq!((asset.width, asset.height), (1, 1));
    pixel(asset, 0, 0)
}

/// Every pixel of a decoded asset, row-major.
fn pixels(asset: &ResolvedImageAsset) -> Vec<[u8; 4]> {
    (0..asset.height)
        .flat_map(|y| (0..asset.width).map(move |x| (x, y)))
        .map(|(x, y)| pixel(asset, x, y))
        .collect()
}

/// Asserts an opaque neutral grey within `tolerance` of `expected`, the shape
/// every colour-managed lossy or tagged fixture is read back in.
fn assert_grey_within(pixel: [u8; 4], expected: u8, tolerance: u8) {
    assert!(
        pixel[0].abs_diff(expected) <= tolerance,
        "grey {expected}: {pixel:?}"
    );
    assert_eq!(pixel[0], pixel[1], "grey has equal channels: {pixel:?}");
    assert_eq!(pixel[1], pixel[2], "grey has equal channels: {pixel:?}");
    assert_eq!(pixel[3], 255);
}

// --- PNG and APNG ----------------------------------------------------------

mod png_codec {
    use super::*;

    #[test]
    fn a_static_png_decodes_to_its_straight_rgba_and_reports_no_animation() {
        let resolution = resolve_for_png("image/png", &png_rgba(2, 1, &[RED, BLUE]));

        assert_eq!(resolution.media_kind, MediaKind::Png);
        assert!(!resolution.animated);
        assert_eq!(resolution.asset.media_type, "image/png");
        assert_eq!((resolution.asset.width, resolution.asset.height), (2, 1));
        assert_eq!(pixels(&resolution.asset), [RED, BLUE]);
    }

    #[test]
    fn partial_alpha_is_kept_straight_and_premultiplied_side_by_side() {
        let asset = decode("image/png", &png_rgba(1, 1, &[[200, 100, 50, 128]]));

        assert_eq!(only_pixel(&asset), [200, 100, 50, 128]);
        assert_eq!(
            asset.premultiplied_rgba.as_deref().map(Vec::as_slice),
            Some(&[100, 50, 25, 128][..])
        );
    }

    #[test]
    fn fully_transparent_pixels_lose_their_colour() {
        let asset = decode("image/png", &png_rgba(1, 1, &[[255, 17, 33, 0]]));

        assert_eq!(only_pixel(&asset), [0, 0, 0, 0]);
    }

    #[test]
    fn gama_normalizes_to_srgb_unless_srgb_signaling_is_present() {
        let source = png_rgba(1, 1, &[GREY_128]);
        let linear =
            png_with_chunks_before_idat(&source, &[(*b"gAMA", 100_000_u32.to_be_bytes().to_vec())]);
        let srgb = png_with_chunks_before_idat(
            &source,
            &[
                (*b"gAMA", 100_000_u32.to_be_bytes().to_vec()),
                (*b"sRGB", vec![0]),
            ],
        );

        assert_eq!(
            only_pixel(&decode("image/png", &linear)),
            [188, 188, 188, 255]
        );
        assert_eq!(only_pixel(&decode("image/png", &srgb)), GREY_128);
    }

    #[test]
    fn cicp_takes_precedence_over_legacy_color_chunks() {
        let tagged = png_with_chunks_before_idat(
            &png_rgba(1, 1, &[GREY_128]),
            &[
                (*b"gAMA", 100_000_u32.to_be_bytes().to_vec()),
                (*b"cICP", vec![1, 13, 0, 1]),
            ],
        );

        assert_eq!(only_pixel(&decode("image/png", &tagged)), GREY_128);
    }

    #[test]
    fn hdr_cicp_is_tone_mapped_rather_than_read_as_srgb() {
        let pq = png_with_chunks_before_idat(
            &png_rgba(1, 1, &[GREY_128]),
            &[(*b"cICP", vec![9, 16, 0, 1])],
        );

        assert_eq!(only_pixel(&decode("image/png", &pq)), [132, 132, 132, 255]);
    }

    #[test]
    fn the_gain_map_base_image_is_the_standard_sdr_projection() {
        let tagged = png_with_chunks_before_idat(
            &png_rgba(1, 1, &[[64, 128, 192, 255]]),
            &[
                (*b"gmAP", iso_gain_map_metadata()),
                (*b"gdAT", png_rgba(1, 1, &[[255, 255, 255, 255]])),
            ],
        );

        assert_eq!(
            only_pixel(&decode("image/png", &tagged)),
            [64, 128, 192, 255]
        );
    }

    #[test]
    fn exif_orientation_is_applied_from_either_chunk_position() {
        let source = png_rgba(2, 1, &[RED, BLUE]);
        let exif = exif_orientation_tiff(6);
        for oriented in [
            png_with_chunks_before_idat(&source, &[(*b"eXIf", exif.clone())]),
            png_with_chunk_before_iend(&source, *b"eXIf", &exif),
        ] {
            let asset = decode("image/png", &oriented);
            assert_eq!((asset.width, asset.height), (1, 2));
            assert_eq!(pixels(&asset), [RED, BLUE]);
        }
    }

    #[test]
    fn the_direct_svg_target_keeps_source_dimensions_and_derives_no_pixels() {
        let oriented = png_with_chunks_before_idat(
            &png_rgba(2, 1, &[RED, BLUE]),
            &[(*b"eXIf", exif_orientation_tiff(6))],
        );

        let resolution = resolve_for_svg("image/png", &oriented);

        assert_eq!((resolution.asset.width, resolution.asset.height), (2, 1));
        assert!(resolution.asset.rgba.is_none());
        assert!(resolution.asset.premultiplied_rgba.is_none());
        assert_eq!(*resolution.asset.source_bytes, oriented);
    }

    #[test]
    fn every_exif_orientation_maps_the_source_quadrants() {
        // Row-major 2x2 source: RED GREEN / BLUE YELLOW.
        let source = png_rgba(2, 2, &[RED, GREEN, BLUE, YELLOW]);
        let expected: [[[u8; 4]; 4]; 8] = [
            [RED, GREEN, BLUE, YELLOW],
            [GREEN, RED, YELLOW, BLUE],
            [YELLOW, BLUE, GREEN, RED],
            [BLUE, YELLOW, RED, GREEN],
            [RED, BLUE, GREEN, YELLOW],
            [BLUE, RED, YELLOW, GREEN],
            [YELLOW, GREEN, BLUE, RED],
            [GREEN, YELLOW, RED, BLUE],
        ];
        for (orientation, expected) in (1..=8).zip(expected) {
            let oriented = png_with_chunks_before_idat(
                &source,
                &[(*b"eXIf", exif_orientation_tiff(orientation))],
            );
            let asset = decode("image/png", &oriented);
            assert_eq!(pixels(&asset), expected, "orientation {orientation}");
        }
    }

    #[test]
    fn an_apng_flattens_to_frame_zero_and_reports_animation() {
        let resolution = resolve_for_png("image/png", &apng(&[RED], &[BLUE]));

        assert!(resolution.animated);
        assert_eq!(only_pixel(&resolution.asset), RED);
    }

    #[test]
    fn a_one_frame_actl_is_not_an_animation() {
        let apng = png_with_chunks_before_idat(
            &png_rgba(1, 1, &[RED]),
            &[
                (
                    *b"acTL",
                    [1_u32.to_be_bytes(), 0_u32.to_be_bytes()].concat(),
                ),
                (*b"fcTL", apng_frame_control(0, 1, 1)),
            ],
        );

        assert!(!resolve_for_png("image/png", &apng).animated);
    }

    #[test]
    fn animation_frame_zero_wins_over_a_separate_default_image() {
        let resolution = resolve_for_png("image/png", &apng_with_separate_default(false));

        assert!(!resolution.animated);
        assert_eq!(only_pixel(&resolution.asset), BLUE);
    }

    #[test]
    fn a_play_count_beyond_the_apng_range_still_draws_animation_frame_zero() {
        let mut png = apng_with_separate_default(false);
        mutate_first_png_chunk(&mut png, *b"acTL", |data| {
            data[4..8].copy_from_slice(&u32::MAX.to_be_bytes());
        });

        assert_eq!(only_pixel(&decode("image/png", &png)), BLUE);
    }

    /// The strict metadata policy png 0.17 enforced and png 0.18 no longer
    /// does: each of these fails the image in every format.
    #[test]
    fn malformed_late_or_duplicate_decoder_metadata_fails_the_image() {
        let source = png_rgba(1, 1, &[GREY_128]);
        let gama = 45_455_u32.to_be_bytes().to_vec();
        let chrm: Vec<u8> = [
            31_270_u32, 32_900, 64_000, 33_000, 30_000, 60_000, 15_000, 6_000,
        ]
        .iter()
        .flat_map(|value| value.to_be_bytes())
        .collect();
        let phys = [2835_u32.to_be_bytes(), 2835_u32.to_be_bytes()].concat();
        let before = |chunks: &[([u8; 4], Vec<u8>)]| png_with_chunks_before_idat(&source, chunks);
        let late = |kind: [u8; 4], data: &[u8]| png_with_chunk_before_iend(&source, kind, data);
        let cases = [
            ("late gAMA", late(*b"gAMA", &gama)),
            ("late cHRM", late(*b"cHRM", &chrm)),
            ("late sRGB", late(*b"sRGB", &[0])),
            (
                "late pHYs",
                late(*b"pHYs", &[phys.clone(), vec![0]].concat()),
            ),
            ("late iCCP", late(*b"iCCP", b"p\0\0x")),
            (
                "duplicate gAMA",
                before(&[(*b"gAMA", gama.clone()), (*b"gAMA", gama.clone())]),
            ),
            (
                "duplicate cHRM",
                before(&[(*b"cHRM", chrm.clone()), (*b"cHRM", chrm.clone())]),
            ),
            (
                "duplicate sRGB",
                before(&[(*b"sRGB", vec![0]), (*b"sRGB", vec![0])]),
            ),
            ("short gAMA", before(&[(*b"gAMA", vec![0, 1, 2])])),
            ("short cHRM", before(&[(*b"cHRM", chrm[..31].to_vec())])),
            ("short pHYs", before(&[(*b"pHYs", phys.clone())])),
            ("sRGB intent 4", before(&[(*b"sRGB", vec![4])])),
            (
                "pHYs unit 2",
                before(&[(*b"pHYs", [phys.clone(), vec![2]].concat())]),
            ),
            ("tEXt without NUL", before(&[(*b"tEXt", b"Title".to_vec())])),
            ("tEXt empty keyword", before(&[(*b"tEXt", b"\0v".to_vec())])),
            (
                "tEXt 80-byte keyword",
                before(&[(*b"tEXt", [vec![b'k'; 80], b"\0v".to_vec()].concat())]),
            ),
            (
                "zTXt method 1",
                before(&[(*b"zTXt", b"Title\0\x01x".to_vec())]),
            ),
            (
                "iTXt flag 2",
                before(&[(*b"iTXt", b"Title\0\x02\0en\0T\0v".to_vec())]),
            ),
            (
                "iTXt invalid UTF-8",
                before(&[(*b"iTXt", b"Title\0\0\0en\0T\0\xff".to_vec())]),
            ),
        ];
        for (case, png) in cases {
            for format in [OutputFormat::Svg, OutputFormat::Png] {
                assert_eq!(
                    failure_for("image/png", &png, format),
                    ResolveError::DecodeFailed,
                    "{case} {format:?}"
                );
            }
        }
    }

    /// What png 0.17 tolerated stays tolerated: empty ancillary chunks are never
    /// read, and an over-long gAMA applies its leading value.
    #[test]
    fn empty_and_over_long_decoder_metadata_keep_png_0_17_semantics() {
        let source = png_rgba(1, 1, &[GREY_128]);
        for kind in [*b"gAMA", *b"sRGB", *b"cHRM", *b"iCCP", *b"tEXt"] {
            let png = png_with_chunks_before_idat(&source, &[(kind, vec![])]);
            assert_eq!(only_pixel(&decode("image/png", &png)), GREY_128, "{kind:?}");
        }
        let linear = |gama: Vec<u8>| {
            only_pixel(&decode(
                "image/png",
                &png_with_chunks_before_idat(&source, &[(*b"gAMA", gama)]),
            ))
        };
        assert_eq!(
            linear([100_000_u32.to_be_bytes().as_slice(), &[9, 9]].concat()),
            linear(100_000_u32.to_be_bytes().to_vec())
        );
        let garbage = (
            *b"iCCP",
            [b"p\0\0".as_slice(), &[120, 156, 3, 0, 0, 0, 0, 1]].concat(),
        );
        assert_eq!(
            failure(
                "image/png",
                &png_with_chunks_before_idat(&source, &[(*b"iCCP", vec![]), garbage.clone()])
            ),
            failure(
                "image/png",
                &png_with_chunks_before_idat(&source, &[garbage])
            ),
            "an empty iCCP does not shadow the profile after it"
        );
    }

    #[test]
    fn corrupt_image_data_behind_a_valid_crc_is_a_decode_failure() {
        let mut png = png_rgba(1, 1, &[RED]);
        mutate_first_png_chunk(&mut png, *b"IDAT", |data| data[0] ^= 0xff);

        assert_eq!(failure("image/png", &png), ResolveError::DecodeFailed);
        assert_eq!(
            failure_for("image/png", &png, OutputFormat::Svg),
            ResolveError::DecodeFailed
        );
    }

    #[test]
    fn every_apng_frame_is_validated_before_svg_passthrough() {
        let mut apng = apng(&[RED], &[BLUE]);
        mutate_first_png_chunk(&mut apng, *b"fdAT", |data| data[4] ^= 0xff);

        assert_eq!(
            failure_for("image/png", &apng, OutputFormat::Svg),
            ResolveError::DecodeFailed
        );
        assert_eq!(
            failure_for(
                "image/png",
                &apng_with_separate_default(true),
                OutputFormat::Svg
            ),
            ResolveError::DecodeFailed
        );
    }

    #[test]
    fn ihdr_must_be_the_first_chunk() {
        let malformed = png_with_chunk_before_ihdr(&png_rgba(1, 1, &[RED]));

        assert_eq!(failure("image/png", &malformed), ResolveError::DecodeFailed);
    }

    #[test]
    fn color_chunks_may_neither_split_nor_follow_image_data() {
        let source = png_rgba(1, 1, &[RED]);
        let split = png_with_chunk_splitting_idat(&source, *b"cICP", &[1, 13, 0, 1]);
        let after = png_with_chunk_before_iend(&source, *b"cICP", &[1, 13, 0, 1]);

        assert_eq!(failure("image/png", &split), ResolveError::DecodeFailed);
        assert_eq!(failure("image/png", &after), ResolveError::DecodeFailed);
    }
}

// --- JPEG ------------------------------------------------------------------

mod jpeg_codec {
    use super::*;

    fn assert_close(pixel: [u8; 4], expected: [u8; 3], tolerance: u8) {
        for (channel, (actual, wanted)) in pixel.iter().zip(expected).enumerate() {
            assert!(
                actual.abs_diff(wanted) <= tolerance,
                "channel {channel} of {pixel:?} is within {tolerance} of {expected:?}"
            );
        }
        assert_eq!(pixel[3], 255);
    }

    #[test]
    fn a_baseline_jpeg_decodes_to_opaque_srgb() {
        let resolution = resolve_for_png("image/jpeg", &jpeg_rgb(1, 1, &[JFIF_BLUE]));

        assert_eq!(resolution.media_kind, MediaKind::Jpeg);
        assert!(!resolution.animated);
        assert_eq!(resolution.asset.media_type, "image/jpeg");
        assert_close(only_pixel(&resolution.asset), JFIF_BLUE, 3);
    }

    #[test]
    fn the_sniffed_kind_wins_over_both_claims_which_are_kept_for_the_session() {
        let jpeg = jpeg_rgb(1, 1, &[JFIF_BLUE]);
        let resolution = resolve_scene(
            &file_record_scene_with_claims("image/png", "image/jfif", &jpeg),
            OutputFormat::Svg,
        )
        .expect("the JPEG validates whatever it is called");

        assert_eq!(resolution.media_kind, MediaKind::Jpeg);
        assert_eq!(resolution.asset.media_type, "image/jpeg");
        assert_eq!(resolution.claims.mime_type.as_deref(), Some("image/png"));
        assert_eq!(
            resolution.claims.data_url_media_type.as_deref(),
            Some("image/jfif")
        );
    }

    #[test]
    fn baseline_progressive_grayscale_and_raw_streams_decode_alike() {
        let baseline = jpeg_rgb(1, 1, &[[64, 64, 64]]);
        let raw = jpeg_without_app0(&baseline);

        for jpeg in [baseline, progressive_gray_jpeg(64), raw] {
            assert_grey_within(only_pixel(&decode("image/jpeg", &jpeg)), 64, 2);
        }
    }

    #[test]
    fn a_cmyk_jpeg_decodes_to_opaque_srgb() {
        // Full cyan and no other ink is cyan; full key alone is black.
        assert_eq!(
            only_pixel(&decode("image/jpeg", &cmyk_jpeg([255, 0, 0, 0]))),
            [0, 255, 255, 255]
        );
        assert_eq!(
            only_pixel(&decode("image/jpeg", &cmyk_jpeg([0, 0, 0, 255]))),
            [0, 0, 0, 255]
        );
    }

    #[test]
    fn an_embedded_icc_profile_takes_precedence_over_the_srgb_default() {
        let source = jpeg_rgb(1, 1, &[[128, 128, 128]]);
        let tagged = jpeg_with_icc_profile(&source, &linear_rgb_icc_profile());

        assert_grey_within(only_pixel(&decode("image/jpeg", &source)), 128, 2);
        assert_grey_within(only_pixel(&decode("image/jpeg", &tagged)), 188, 2);
    }

    #[test]
    fn malformed_multipart_icc_metadata_is_ignored() {
        // jpeg_read_icc_profile returns no profile for an incomplete or
        // duplicated sequence; the reference draws the image untagged.
        let source = jpeg_rgb(1, 1, &[[128, 128, 128]]);
        let profile = linear_rgb_icc_profile();
        let mut missing_part = b"ICC_PROFILE\0\x01\x02".to_vec();
        missing_part.extend_from_slice(&profile);
        let missing_part = jpeg_with_app_segment(&source, 0xe2, &missing_part);
        let mut duplicate_part = b"ICC_PROFILE\0\x01\x01".to_vec();
        duplicate_part.extend_from_slice(&profile);
        let duplicate = jpeg_with_app_segment(&source, 0xe2, &duplicate_part);
        let duplicate = jpeg_with_app_segment(&duplicate, 0xe2, &duplicate_part);
        let mut zero_index = b"ICC_PROFILE\0\x00\x01".to_vec();
        zero_index.extend_from_slice(&profile);
        let zero_index = jpeg_with_app_segment(&source, 0xe2, &zero_index);

        let untagged = only_pixel(&decode("image/jpeg", &source));
        for malformed in [missing_part, duplicate, zero_index] {
            assert_eq!(only_pixel(&decode("image/jpeg", &malformed)), untagged);
        }
    }

    #[test]
    fn a_profile_that_does_not_parse_or_fit_the_model_is_ignored() {
        let source = jpeg_rgb(1, 1, &[[128, 128, 128]]);
        let untagged = only_pixel(&decode("image/jpeg", &source));
        let truncated = linear_rgb_icc_profile()[..100].to_vec();
        let mut gray = linear_rgb_icc_profile();
        gray[16..20].copy_from_slice(b"GRAY");
        for profile in [truncated, gray] {
            let tagged = jpeg_with_icc_profile(&source, &profile);
            assert_eq!(only_pixel(&decode("image/jpeg", &tagged)), untagged);
        }
        // Blink keeps only a CMYK profile on a CMYK image.
        let cmyk = cmyk_jpeg([255, 0, 0, 0]);
        let tagged = jpeg_with_icc_profile(&cmyk, &linear_rgb_icc_profile());
        assert_eq!(
            only_pixel(&decode("image/jpeg", &tagged)),
            only_pixel(&decode("image/jpeg", &cmyk))
        );
    }

    #[test]
    fn exif_orientation_swaps_the_dimensions_and_turns_the_pixels() {
        // 2x1 red|blue turned by orientation 6 (90 degrees clockwise) reads
        // red over blue. The encoder keeps 4:4:4 chroma at quality 100, so
        // neighbouring pixels do not bleed into each other.
        let source = jpeg_rgb(2, 1, &[[240, 20, 20], [20, 20, 240]]);
        let asset = decode("image/jpeg", &jpeg_with_app1_exif(&source, 6));

        assert_eq!((asset.width, asset.height), (1, 2));
        let top = pixel(&asset, 0, 0);
        let bottom = pixel(&asset, 0, 1);
        assert!(top[0] > top[2], "top pixel: {top:?}");
        assert!(bottom[2] > bottom[0], "bottom pixel: {bottom:?}");
        let validated = resolve_for_svg("image/jpeg", &jpeg_with_app1_exif(&source, 6));
        assert_eq!((validated.asset.width, validated.asset.height), (2, 1));
    }

    #[test]
    fn a_signature_only_gain_map_index_is_not_a_valid_container() {
        let base = jpeg_rgb(1, 1, &[JFIF_BLUE]);
        let gain_map = jpeg_rgb(1, 1, &[[255, 255, 255]]);
        let xmp = b"http://ns.adobe.com/xap/1.0/\0<x:xmpmeta hdrgm:Version=\"1.0\"/>";
        let container = jpeg_with_app_segment(&base, 0xe2, b"MPF\0fixture-index");
        let mut container = jpeg_with_app_segment(&container, 0xe1, xmp);
        container.extend_from_slice(&gain_map);

        assert_eq!(
            failure_for("image/jpeg", &container, OutputFormat::Svg),
            ResolveError::JpegDecodeFailed
        );
    }

    #[test]
    fn data_after_the_first_image_is_ignored() {
        // Anything after the first EOI, another image included, is not read.
        let source = jpeg_rgb(1, 1, &[JFIF_BLUE]);
        let expected = only_pixel(&decode("image/jpeg", &source));
        let mut concatenated = source.clone();
        concatenated.extend_from_slice(&jpeg_rgb(1, 1, &[[255, 255, 255]]));
        let mut unterminated = source.clone();
        let second = jpeg_rgb(1, 1, &[[255, 255, 255]]);
        unterminated.extend_from_slice(&second[..second.len() - 2]);
        let mut garbage = source.clone();
        garbage.extend_from_slice(b"garbage\0\0\xff");
        for jpeg in [concatenated, unterminated, garbage] {
            assert_eq!(only_pixel(&decode("image/jpeg", &jpeg)), expected);
        }
    }

    /// The offset of the first `marker` segment of `jpeg`.
    fn marker_at(jpeg: &[u8], marker: u8) -> usize {
        let mut offset = 2;
        loop {
            assert_eq!(jpeg[offset], 0xff);
            if jpeg[offset + 1] == marker {
                return offset;
            }
            offset += 2 + usize::from(u16::from_be_bytes([jpeg[offset + 2], jpeg[offset + 3]]));
        }
    }

    fn inserted(jpeg: &[u8], at: usize, bytes: &[u8]) -> Vec<u8> {
        [&jpeg[..at], bytes, &jpeg[at..]].concat()
    }

    #[test]
    fn junk_bytes_and_stray_markers_between_segments_are_skipped() {
        // libjpeg's next_marker skips non-marker bytes and stuffed zeros;
        // read_markers ignores RSTn and TEM outside a scan.
        let source = jpeg_rgb(2, 2, &[JFIF_BLUE, [200, 20, 20], [20, 200, 20], [9, 9, 9]]);
        let expected = pixels(&decode("image/jpeg", &source));
        let sos = marker_at(&source, 0xda);
        let dqt = marker_at(&source, 0xdb);
        for (at, bytes) in [
            (sos, b"junk".as_slice()),
            (dqt, b"\x00\x12\xff\x00"),
            (sos, b"\xff\xd0"),
            (dqt, b"\xff\x01"),
            (dqt, b"\xff\xff\xff"),
        ] {
            let jpeg = inserted(&source, at, bytes);
            assert_eq!(pixels(&decode("image/jpeg", &jpeg)), expected, "{bytes:?}");
        }
    }

    #[test]
    fn a_missing_huffman_table_is_the_standard_table() {
        // libjpeg-turbo's jpeg_std_huff_table, as for MJPEG: the fixture
        // encoder writes exactly the standard tables.
        let source = jpeg_rgb(2, 2, &[JFIF_BLUE, [200, 20, 20], [20, 200, 20], [9, 9, 9]]);
        let dht = marker_at(&source, 0xc4);
        let sos = marker_at(&source, 0xda);
        let mut stripped = source[..dht].to_vec();
        let mut offset = dht;
        while offset < sos {
            let end = offset
                + 2
                + usize::from(u16::from_be_bytes([source[offset + 2], source[offset + 3]]));
            if source[offset + 1] != 0xc4 {
                stripped.extend_from_slice(&source[offset..end]);
            }
            offset = end;
        }
        stripped.extend_from_slice(&source[sos..]);
        assert!(stripped.len() < source.len());
        assert_eq!(
            pixels(&decode("image/jpeg", &stripped)),
            pixels(&decode("image/jpeg", &source))
        );
    }

    #[test]
    fn a_missing_quantization_table_is_a_decode_failure() {
        // latch_quant_tables: JERR_NO_QUANT_TABLE; the reference paints nothing.
        let source = jpeg_rgb(1, 1, &[JFIF_BLUE]);
        let dqt = marker_at(&source, 0xdb);
        let sof = marker_at(&source, 0xc0);
        let jpeg = [&source[..dqt], &source[sof..]].concat();
        assert_eq!(
            failure_for("image/jpeg", &jpeg, OutputFormat::Svg),
            ResolveError::JpegDecodeFailed
        );
    }

    fn restart_jpeg() -> Vec<u8> {
        let mut bytes = Vec::new();
        let mut encoder = jpeg_encoder::Encoder::new(&mut bytes, 100);
        encoder.set_restart_interval(1);
        let rgb: Vec<u8> = (0..32 * 16 * 3).map(|i| (i * 7 % 251) as u8).collect();
        encoder
            .encode(&rgb, 32, 16, jpeg_encoder::ColorType::Rgb)
            .expect("restart JPEG encodes");
        bytes
    }

    #[test]
    fn restart_markers_must_follow_the_restart_interval() {
        let source = restart_jpeg();
        let sos = marker_at(&source, 0xda);
        let restarts: Vec<usize> = (sos..source.len() - 1)
            .filter(|&i| source[i] == 0xff && (0xd0..=0xd7).contains(&source[i + 1]))
            .collect();
        assert!(restarts.len() > 2);
        decode("image/jpeg", &source);
        // Missing restart markers, one dropped, and one out of sequence leave
        // the reference resynchronizing: it paints garbled rows.
        let mut missing = source.clone();
        for &at in restarts.iter().rev() {
            missing.drain(at..at + 2);
        }
        let mut dropped = source.clone();
        dropped.drain(restarts[1]..restarts[1] + 2);
        let mut swapped = source.clone();
        swapped[restarts[1] + 1] = swapped[restarts[0] + 1];
        for jpeg in [missing, dropped, swapped] {
            assert_eq!(
                failure_for("image/jpeg", &jpeg, OutputFormat::Svg),
                ResolveError::JpegDecodeFailed
            );
        }
        // A zero interval disables restart processing.
        let rgb = jpeg_rgb(1, 1, &[JFIF_BLUE]);
        let dri_zero = inserted(&rgb, marker_at(&rgb, 0xda), b"\xff\xdd\x00\x04\x00\x00");
        assert_eq!(
            only_pixel(&decode("image/jpeg", &dri_zero)),
            only_pixel(&decode("image/jpeg", &rgb))
        );
    }

    #[test]
    fn only_a_complete_progression_draws_without_its_eoi() {
        let mut bytes = Vec::new();
        let mut encoder = jpeg_encoder::Encoder::new(&mut bytes, 100);
        encoder.set_progressive(true);
        let rgb: Vec<u8> = (0..16 * 16 * 3).map(|i| (i * 5 % 253) as u8).collect();
        encoder
            .encode(&rgb, 16, 16, jpeg_encoder::ColorType::Rgb)
            .expect("progressive JPEG encodes");
        let expected = pixels(&decode("image/jpeg", &bytes));
        let unterminated = &bytes[..bytes.len() - 2];
        assert_eq!(pixels(&decode("image/jpeg", unterminated)), expected);
        // Ending after an earlier scan leaves coefficients unread.
        let scans: Vec<usize> = (2..bytes.len() - 1)
            .filter(|&i| bytes[i] == 0xff && bytes[i + 1] == 0xda)
            .collect();
        assert!(scans.len() > 2);
        assert_eq!(
            failure_for(
                "image/jpeg",
                &bytes[..scans[scans.len() - 1]],
                OutputFormat::Svg
            ),
            ResolveError::JpegDecodeFailed
        );
        assert_eq!(
            failure_for("image/jpeg", &bytes[..scans[1] + 2], OutputFormat::Svg),
            ResolveError::JpegDecodeFailed
        );
    }

    /// A 1x1 three-component JPEG with component ids `ids`, and optionally an
    /// Adobe transform and no JFIF segment.
    fn three_component(ids: [u8; 3], adobe: Option<u8>, jfif: bool) -> Vec<u8> {
        let mut jpeg = jpeg_rgb(1, 1, &[[200, 60, 30]]);
        if !jfif {
            jpeg = jpeg_without_app0(&jpeg);
        }
        let sof = marker_at(&jpeg, 0xc0);
        let sos = marker_at(&jpeg, 0xda);
        for (index, id) in ids.into_iter().enumerate() {
            jpeg[sof + 10 + 3 * index] = id;
            jpeg[sos + 5 + 2 * index] = id;
        }
        if let Some(transform) = adobe {
            let mut segment = b"Adobe\x00\x64\x00\x00\x00\x00".to_vec();
            segment.push(transform);
            jpeg = jpeg_with_app_segment(&jpeg, 0xee, &segment);
        }
        jpeg
    }

    #[test]
    fn jfif_precedes_the_adobe_transform_and_component_ids() {
        // jdapimin.c default_decompress_parms for three components: JFIF
        // means YCbCr; else the Adobe transform (0 RGB, other YCbCr); else ids
        // 'R','G','B' mean RGB.
        let ycbcr = only_pixel(&decode(
            "image/jpeg",
            &three_component([1, 2, 3], None, true),
        ));
        let rgb = only_pixel(&decode(
            "image/jpeg",
            &three_component([1, 2, 3], Some(0), false),
        ));
        assert_ne!(ycbcr, rgb);
        for (jpeg, expected) in [
            (three_component([1, 2, 3], Some(0), true), ycbcr),
            (three_component(*b"RGB", None, true), ycbcr),
            (three_component(*b"RGB", Some(1), false), ycbcr),
            (three_component([1, 2, 3], Some(7), false), ycbcr),
            (three_component(*b"RGB", Some(0), true), ycbcr),
            (three_component(*b"RGB", None, false), rgb),
            (three_component([4, 5, 6], Some(0), false), rgb),
        ] {
            assert_eq!(only_pixel(&decode("image/jpeg", &jpeg)), expected);
        }
    }

    #[test]
    fn a_four_component_adobe_transform_other_than_zero_is_ycck() {
        // libjpeg reads four components as YCCK for any Adobe transform but 0.
        let cmyk = cmyk_jpeg([255, 0, 0, 0]);
        let segment = |transform: u8| {
            let mut segment = b"Adobe\x00\x64\x00\x00\x00\x00".to_vec();
            segment.push(transform);
            segment
        };
        let plain = only_pixel(&decode("image/jpeg", &cmyk));
        let as_cmyk = jpeg_with_app_segment(&jpeg_without_adobe(&cmyk), 0xee, &segment(0));
        assert_eq!(only_pixel(&decode("image/jpeg", &as_cmyk)), plain);
        let ycck = only_pixel(&decode(
            "image/jpeg",
            &jpeg_with_app_segment(&jpeg_without_adobe(&cmyk), 0xee, &segment(2)),
        ));
        assert_ne!(ycck, plain);
        for transform in [1, 9] {
            let jpeg = jpeg_with_app_segment(&jpeg_without_adobe(&cmyk), 0xee, &segment(transform));
            assert_eq!(
                only_pixel(&decode("image/jpeg", &jpeg)),
                ycck,
                "{transform}"
            );
        }
    }

    /// `jpeg` without its Adobe APP14 segments.
    fn jpeg_without_adobe(jpeg: &[u8]) -> Vec<u8> {
        let sos = marker_at(jpeg, 0xda);
        let mut out = jpeg[..2].to_vec();
        let mut offset = 2;
        while offset < sos {
            let end =
                offset + 2 + usize::from(u16::from_be_bytes([jpeg[offset + 2], jpeg[offset + 3]]));
            if jpeg[offset + 1] != 0xee {
                out.extend_from_slice(&jpeg[offset..end]);
            }
            offset = end;
        }
        out.extend_from_slice(&jpeg[sos..]);
        out
    }

    #[test]
    fn an_exif_header_with_a_fill_byte_is_read() {
        // Blink accepts "Exif\0" followed by 0x00 or 0xff.
        let source = jpeg_rgb(2, 1, &[[240, 20, 20], [20, 20, 240]]);
        let exif = jpeg_with_app1_exif(&source, 6);
        let app1 = marker_at(&exif, 0xe1);
        for (fill, turned) in [(0xff, true), (0x01, false)] {
            let mut variant = exif.clone();
            variant[app1 + 9] = fill;
            let asset = decode("image/jpeg", &variant);
            assert_eq!(
                (asset.width, asset.height),
                if turned { (1, 2) } else { (2, 1) }
            );
        }
    }

    #[test]
    fn truncated_and_malformed_streams_are_decode_failures() {
        let mut truncated = jpeg_rgb(1, 1, &[JFIF_BLUE]);
        truncated.truncate(truncated.len() - 2);
        let malformed_segment = b"\xff\xd8\xff\xe0\0\x01\xff\xd9".to_vec();

        for jpeg in [truncated, malformed_segment] {
            assert_eq!(
                failure_for("image/jpeg", &jpeg, OutputFormat::Svg),
                ResolveError::JpegDecodeFailed
            );
        }
    }
}

// --- GIF -------------------------------------------------------------------

mod gif_codec {
    use ::gif::{DisposalMethod, Frame, Repeat};

    use super::*;

    #[test]
    fn a_single_frame_gif_decodes_its_palette_and_is_not_animated() {
        let gif = gif(&[gif_frame(&[0, 1], 2, 1, &RED_BLUE_PALETTE, None)]);
        let resolution = resolve_for_png("image/gif", &gif);

        assert_eq!(resolution.media_kind, MediaKind::Gif);
        assert!(!resolution.animated);
        assert_eq!(resolution.asset.media_type, "image/gif");
        assert_eq!(pixels(&resolution.asset), [RED, BLUE]);
    }

    #[test]
    fn the_transparent_index_and_frame_offset_compose_onto_a_clear_screen() {
        let mut first = gif_frame(&[1], 1, 1, &[0, 0, 255, 255, 0, 0], Some(0));
        first.left = 1;
        let gif = gif_with_screen(2, 1, &[], None, &[first]);

        let asset = decode("image/gif", &gif);

        assert_eq!((asset.width, asset.height), (2, 1));
        assert_eq!(pixels(&asset), [[0, 0, 0, 0], RED]);
    }

    #[test]
    fn a_gif87a_with_a_frame_local_palette_decodes() {
        let mut gif = gif(&[gif_frame(&[1], 1, 1, &[0, 255, 0, 255, 0, 255], None)]);
        let control = gif_graphic_control_offset(&gif);
        gif.drain(control..control + 8);
        gif[..6].copy_from_slice(b"GIF87a");

        assert_eq!(only_pixel(&decode("image/gif", &gif)), [255, 0, 255, 255]);
    }

    #[test]
    fn an_animated_gif_flattens_to_its_first_frame_and_reports_animation() {
        let gif = gif(&[
            gif_frame(&[0], 1, 1, &RED_BLUE_PALETTE, None),
            gif_frame(&[1], 1, 1, &RED_BLUE_PALETTE, None),
        ]);
        let resolution = resolve_for_png("image/gif", &gif);

        assert!(resolution.animated);
        assert_eq!(only_pixel(&resolution.asset), RED);
    }

    #[test]
    fn loop_metadata_alone_does_not_make_a_gif_animated() {
        let gif = gif_with_screen(
            1,
            1,
            &[],
            Some(Repeat::Infinite),
            &[gif_frame(&[0], 1, 1, &RED_BLUE_PALETTE, None)],
        );

        assert!(!resolve_for_png("image/gif", &gif).animated);
    }

    #[test]
    fn a_global_palette_interlacing_and_every_disposal_method_are_accepted() {
        let frames: Vec<Frame<'static>> = [
            DisposalMethod::Any,
            DisposalMethod::Keep,
            DisposalMethod::Background,
            DisposalMethod::Previous,
        ]
        .into_iter()
        .map(|dispose| {
            let mut frame = gif_global_palette_frame(&[0, 1, 1, 0], 2, 2);
            frame.dispose = dispose;
            frame.interlaced = true;
            frame
        })
        .collect();
        let gif = gif_with_screen(2, 2, &RED_BLUE_PALETTE, None, &frames);

        let resolution = resolve_for_png("image/gif", &gif);

        assert!(resolution.animated);
        assert_eq!(pixels(&resolution.asset), [RED, BLUE, BLUE, RED]);
        resolve_for_svg("image/gif", &gif);
    }

    #[test]
    fn a_truncated_later_frame_keeps_the_first_while_a_truncated_first_frame_fails() {
        // The reference (Wuffs) draws a complete first frame whatever follows.
        let mut animated = gif(&[
            gif_frame(&[0], 1, 1, &RED_BLUE_PALETTE, None),
            gif_frame(&[1], 1, 1, &RED_BLUE_PALETTE, None),
        ]);
        animated.truncate(animated.len() - 3);
        let resolution = resolve_for_png("image/gif", &animated);
        assert!(resolution.animated);
        assert_eq!(only_pixel(&resolution.asset), RED);

        // A first frame whose data ends before its pixels are complete is the
        // declared divergence: the reference paints it partially.
        let mut single = gif(&[gif_frame(&[0, 1, 0, 1], 4, 1, &RED_BLUE_PALETTE, None)]);
        let descriptor = gif_image_descriptor_offset(&single);
        single.truncate(descriptor + 12);
        assert_eq!(
            failure_for("image/gif", &single, OutputFormat::Svg),
            ResolveError::GifDecodeFailed
        );
    }

    #[test]
    fn out_of_palette_background_and_transparent_indices_are_ignored() {
        let mut invalid_background = gif_with_screen(
            1,
            1,
            &RED_BLUE_PALETTE,
            None,
            &[gif_global_palette_frame(&[0], 1, 1)],
        );
        invalid_background[11] = 2;

        let mut invalid_transparency = gif(&[gif_frame(&[0], 1, 1, &RED_BLUE_PALETTE, Some(0))]);
        let control = gif_graphic_control_offset(&invalid_transparency);
        invalid_transparency[control + 6] = 2;

        // A background index and a transparent index past the palette are
        // ignored. (Extension block sizes are covered by the reference record.)
        for gif in [invalid_background, invalid_transparency] {
            assert_eq!(only_pixel(&decode("image/gif", &gif)), RED);
        }
    }

    #[test]
    fn reserved_disposal_methods_and_descriptor_bits_are_ignored() {
        let mut disposal = gif(&[gif_frame(&[0], 1, 1, &RED_BLUE_PALETTE, None)]);
        let control = gif_graphic_control_offset(&disposal);
        disposal[control + 3] = 4 << 2;

        let mut reserved = gif(&[gif_frame(&[0], 1, 1, &RED_BLUE_PALETTE, None)]);
        let descriptor = gif_image_descriptor_offset(&reserved);
        reserved[descriptor + 9] |= 0x08;

        for gif in [disposal, reserved] {
            assert_eq!(only_pixel(&decode("image/gif", &gif)), RED);
        }
    }
}

// --- WebP ------------------------------------------------------------------

mod webp_codec {
    use super::*;

    const TRANSLUCENT_BLUE: [u8; 4] = [24, 96, 208, 128];

    #[test]
    fn a_lossless_webp_keeps_its_alpha_straight() {
        let webp = webp_rgba(2, 1, &[TRANSLUCENT_BLUE, [200, 100, 50, 0]]);
        let resolution = resolve_for_png("image/webp", &webp);

        assert_eq!(resolution.media_kind, MediaKind::WebP);
        assert!(!resolution.animated);
        assert_eq!(resolution.asset.media_type, "image/webp");
        assert_eq!(pixels(&resolution.asset), [TRANSLUCENT_BLUE, [0, 0, 0, 0]]);
    }

    #[test]
    fn untransformed_webp_truncates_only_the_draw_time_copy() {
        let source = [[1, 128, 128, 1], [255, 129, 65, 128]];
        // The EXIF orientation is ignored, as the reference ignores it.
        let webp = webp_with_metadata(2, 1, &source, None, Some(exif_orientation_with_header(6)));
        let asset = decode("image/webp", &webp);
        assert_eq!((asset.width, asset.height), (2, 1));
        assert_eq!(pixels(&asset), source);
        assert_eq!(
            asset.premultiplied_rgba.as_deref().unwrap().as_slice(),
            &[0, 0, 0, 1, 128, 64, 32, 128]
        );
        // A real color transform remains on the established rounded path.
        let tagged = webp_with_metadata(
            1,
            1,
            &[[128, 128, 128, 1]],
            Some(linear_rgb_icc_profile()),
            None,
        );
        let asset = decode("image/webp", &tagged);
        let pixel = only_pixel(&asset);
        assert_eq!(pixel[3], 1);
        assert!(pixel[..3].iter().all(|c| c.abs_diff(188) <= 2));
        assert_eq!(
            asset.premultiplied_rgba.as_deref().unwrap().as_slice(),
            &[1, 1, 1, 1]
        );
    }

    #[test]
    fn a_lossy_webp_decodes_to_opaque_srgb() {
        let asset = decode("image/webp", &lossy_red_webp());

        assert_eq!((asset.width, asset.height), (2, 2));
        assert_eq!(pixels(&asset), [[255, 0, 2, 255]; 4]);
    }

    #[test]
    fn an_animated_webp_flattens_to_its_first_frame_and_reports_animation() {
        let webp = animated_webp(
            1,
            1,
            &[
                (0, 0, webp_rgba(1, 1, &[RED])),
                (0, 0, webp_rgba(1, 1, &[BLUE])),
            ],
        );
        let resolution = resolve_for_png("image/webp", &webp);

        assert!(resolution.animated);
        assert_eq!(only_pixel(&resolution.asset), RED);
    }

    #[test]
    fn the_first_animation_frame_keeps_its_alpha() {
        let webp = animated_webp(
            1,
            1,
            &[
                (0, 0, webp_rgba(1, 1, &[TRANSLUCENT_BLUE])),
                (0, 0, webp_rgba(1, 1, &[BLUE])),
            ],
        );
        let resolution = resolve_for_png("image/webp", &webp);

        assert!(resolution.animated);
        assert_eq!(only_pixel(&resolution.asset), TRANSLUCENT_BLUE);
    }

    #[test]
    fn exif_orientation_is_ignored_as_the_reference_ignores_it() {
        // Blink's WebP decoder never applies EXIF orientation.
        let webp = webp_with_metadata(
            4,
            1,
            &[RED, RED, BLUE, BLUE],
            None,
            Some(exif_orientation_with_header(6)),
        );

        let asset = decode("image/webp", &webp);
        assert_eq!((asset.width, asset.height), (4, 1));
        assert_eq!(pixels(&asset), [RED, RED, BLUE, BLUE]);

        let validated = resolve_for_svg("image/webp", &webp);
        assert_eq!((validated.asset.width, validated.asset.height), (4, 1));
        assert!(validated.asset.rgba.is_none());
    }

    #[test]
    fn an_embedded_icc_profile_takes_precedence_over_the_srgb_default() {
        let untagged = webp_rgba(1, 1, &[GREY_128]);
        let tagged = webp_with_metadata(1, 1, &[GREY_128], Some(linear_rgb_icc_profile()), None);

        assert_grey_within(only_pixel(&decode("image/webp", &untagged)), 128, 1);
        assert_grey_within(only_pixel(&decode("image/webp", &tagged)), 188, 2);
    }

    #[test]
    fn truncated_and_malformed_containers_are_decode_failures() {
        let mut truncated = webp_rgba(1, 1, &[[24, 96, 208, 255]]);
        truncated.truncate(truncated.len() - 2);
        let malformed = b"RIFF\x04\0\0\0WEBP".to_vec();

        for webp in [truncated, malformed] {
            assert_eq!(
                failure_for("image/webp", &webp, OutputFormat::Svg),
                ResolveError::WebPDecodeFailed
            );
        }
    }

    #[test]
    fn later_frames_outside_the_canvas_are_rejected_but_reserved_frame_bits_and_the_alpha_flag_are_not(
    ) {
        let animated = animated_webp(
            1,
            1,
            &[
                (0, 0, webp_rgba(1, 1, &[RED])),
                (0, 0, webp_rgba(1, 1, &[[0, 0, 255, 128]])),
            ],
        );
        let second_frame = webp_second_frame_payload_offset(&animated);

        let mut outside_canvas = animated.clone();
        outside_canvas[second_frame] = 1;
        assert_eq!(
            failure_for("image/webp", &outside_canvas, OutputFormat::Svg),
            ResolveError::WebPDecodeFailed
        );
        // libwebp's demuxer ignores reserved ANMF bits, and Blink draws an
        // animation whatever its VP8X alpha flag says.
        let mut reserved_frame_flag = animated.clone();
        reserved_frame_flag[second_frame + 15] |= 0x80;
        let mut missing_alpha_flag = animated;
        missing_alpha_flag[20] &= !0x10;
        for webp in [reserved_frame_flag, missing_alpha_flag] {
            let resolution = resolve_for_png("image/webp", &webp);
            assert!(resolution.animated);
            assert_eq!(only_pixel(&resolution.asset), RED);
        }
    }
}

// --- BMP -------------------------------------------------------------------

mod bmp_codec {
    use super::*;

    #[test]
    fn bottom_up_rows_come_out_top_down() {
        let resolution = resolve_for_png("image/bmp", &bmp24(1, 2, &[[255, 0, 0], [0, 0, 255]]));

        assert_eq!(resolution.media_kind, MediaKind::Bmp);
        assert!(!resolution.animated);
        assert_eq!(resolution.asset.media_type, "image/bmp");
        assert_eq!((resolution.asset.width, resolution.asset.height), (1, 2));
        assert_eq!(pixels(&resolution.asset), [RED, BLUE]);
    }

    #[test]
    fn legacy_bi_rgb_32_ignores_its_reserved_byte_while_alpha_bitfields_read_it() {
        let legacy = bmp32([24, 96, 208, 0], false);
        let bitfields = bmp32([24, 96, 208, 128], true);

        assert_eq!(
            only_pixel(&decode("image/bmp", &legacy)),
            [24, 96, 208, 255]
        );
        assert_eq!(
            only_pixel(&decode("image/bmp", &bitfields)),
            [24, 96, 208, 128]
        );
    }

    #[test]
    fn rle8_delta_skips_stay_transparent() {
        let asset = decode("image/bmp", &bmp_rle8());

        assert_eq!((asset.width, asset.height), (2, 1));
        assert_eq!(pixels(&asset), [[0, 0, 0, 0], BLUE]);
    }

    #[test]
    fn a_52_byte_alpha_bitfields_header_has_no_alpha_mask_and_draws_opaque() {
        assert_eq!(
            only_pixel(&decode("image/bmp", &bmp32_without_alpha_mask_storage())),
            [24, 96, 208, 255]
        );
    }

    #[test]
    fn an_all_zero_alpha_channel_draws_opaque() {
        assert_eq!(
            only_pixel(&decode("image/bmp", &bmp32([24, 96, 208, 0], true))),
            [24, 96, 208, 255]
        );
    }

    #[test]
    fn file_size_and_reserved_fields_are_ignored() {
        let mut bmp = bmp24(1, 1, &[[24, 96, 208]]);
        bmp[2..10].copy_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);

        assert_eq!(only_pixel(&decode("image/bmp", &bmp)), [24, 96, 208, 255]);
    }

    #[test]
    fn palette_indices_beyond_the_used_colors_draw_opaque_black() {
        let asset = decode("image/bmp", &bmp_rle8_with_colors_used(1));

        assert_eq!(pixels(&asset), [[0, 0, 0, 0], [0, 0, 0, 255]]);
    }

    #[test]
    fn dimensions_of_65536_are_rejected_for_every_target() {
        let bmp = bmp24(65536, 1, &vec![[24, 96, 208]; 65536]);

        assert_eq!(
            failure_for("image/bmp", &bmp, OutputFormat::Svg),
            ResolveError::BmpDecodeFailed
        );
    }

    #[test]
    fn an_os2_core_header_admits_only_the_universal_depths() {
        let mut bmp = bmp24(1, 1, &[[24, 96, 208]]);
        // Rewrite the 40-byte header as a 12-byte OS/2 1.x header.
        let core = [12, 0, 0, 0, 1, 0, 1, 0, 1, 0, 24, 0];
        bmp.splice(14..54, core);
        bmp[10..14].copy_from_slice(&26_u32.to_le_bytes());
        assert_eq!(only_pixel(&decode("image/bmp", &bmp)), [24, 96, 208, 255]);

        bmp[24] = 32;
        bmp.push(0);
        assert_eq!(
            failure_for("image/bmp", &bmp, OutputFormat::Svg),
            ResolveError::BmpDecodeFailed
        );
    }

    #[test]
    fn an_embedded_profile_that_does_not_parse_fails_every_target() {
        let bmp = bmp_v5_embedded_profile(b"not an icc profile");

        assert_eq!(
            failure_for("image/bmp", &bmp, OutputFormat::Svg),
            ResolveError::BmpDecodeFailed
        );
    }

    #[test]
    fn calibrated_rgb_with_unusable_endpoints_draws_as_srgb() {
        assert_eq!(
            only_pixel(&decode("image/bmp", &bmp_v4_calibrated([0; 6], 2 << 16))),
            [24, 96, 208, 255]
        );
    }

    #[test]
    fn calibrated_rgb_converts_through_its_endpoints_and_gamma() {
        // Rec. 709 primaries with a 2.2 decoding gamma: a mid grey stays grey
        // but is re-encoded from gamma 2.2 to the sRGB curve.
        let primaries =
            [0.64, 0.33, 0.30, 0.60, 0.15, 0.06].map(|v| (v * f64::from(1 << 30)) as u32);
        let [red, green, blue, alpha] = only_pixel(&decode(
            "image/bmp",
            &bmp_v4_calibrated_pixel(primaries, 144_179, [128, 128, 128]),
        ));

        assert_eq!(alpha, 255);
        assert!(
            red.max(green).max(blue) - red.min(green).min(blue) <= 1,
            "{red} {green} {blue}"
        );
        assert!((126..=129).contains(&red), "{red}");
    }

    #[test]
    fn a_truncated_bmp_is_a_decode_failure_under_its_sniffed_kind() {
        let mut bmp = bmp24(1, 1, &[[24, 96, 208]]);
        bmp.truncate(bmp.len() - 1);

        assert_eq!(
            failure_for("image/png", &bmp, OutputFormat::Svg),
            ResolveError::BmpDecodeFailed
        );
    }

    /// The one-pixel [`bmp_rle8`] fixture with `colors` palette entries in use.
    fn bmp_rle8_with_colors_used(colors: u32) -> Vec<u8> {
        let mut bmp = bmp_rle8();
        bmp[46..50].copy_from_slice(&colors.to_le_bytes());
        bmp
    }

    /// A one-pixel 24-bit BMP with a `header_size`-byte V4/V5 header of colour
    /// space `space`, the `extra` header fields from offset 60 and `trailer`
    /// after the pixel row.
    fn bmp_v4_like(
        header_size: u32,
        space: &[u8; 4],
        extra: &[u8],
        pixel: [u8; 3],
        trailer: &[u8],
    ) -> Vec<u8> {
        let pixel_offset = 14 + header_size;
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"BM");
        bytes.extend_from_slice(&(pixel_offset + 4).to_le_bytes());
        bytes.extend_from_slice(&[0; 4]);
        bytes.extend_from_slice(&pixel_offset.to_le_bytes());
        let mut header = vec![0_u8; header_size as usize];
        header[0..4].copy_from_slice(&header_size.to_le_bytes());
        header[4..8].copy_from_slice(&1_i32.to_le_bytes());
        header[8..12].copy_from_slice(&1_i32.to_le_bytes());
        header[12..14].copy_from_slice(&1_u16.to_le_bytes());
        header[14..16].copy_from_slice(&24_u16.to_le_bytes());
        header[56..60].copy_from_slice(space);
        header[60..60 + extra.len()].copy_from_slice(extra);
        bytes.extend_from_slice(&header);
        bytes.extend_from_slice(&[pixel[2], pixel[1], pixel[0], 0]);
        bytes.extend_from_slice(trailer);
        bytes
    }

    /// A V4 calibrated-RGB BMP of the [24, 96, 208] pixel with the red, green
    /// and blue endpoints' 2.30 X/Y values and a 16.16 gamma for every channel.
    fn bmp_v4_calibrated(primaries: [u32; 6], gamma: u32) -> Vec<u8> {
        bmp_v4_calibrated_pixel(primaries, gamma, [24, 96, 208])
    }

    fn bmp_v4_calibrated_pixel(primaries: [u32; 6], gamma: u32, pixel: [u8; 3]) -> Vec<u8> {
        let mut extra = [0_u8; 48];
        for (index, offset) in [0, 4, 12, 16, 24, 28].into_iter().enumerate() {
            extra[offset..offset + 4].copy_from_slice(&primaries[index].to_le_bytes());
        }
        for offset in [36, 40, 44] {
            extra[offset..offset + 4].copy_from_slice(&gamma.to_le_bytes());
        }
        bmp_v4_like(108, &0_u32.to_le_bytes(), &extra, pixel, &[])
    }

    /// A V5 BMP whose `MBED` colour space names `profile`, stored after the
    /// pixel row.
    fn bmp_v5_embedded_profile(profile: &[u8]) -> Vec<u8> {
        let mut extra = [0_u8; 64];
        // The profile offset counts from the info header: header plus pixel row.
        extra[52..56].copy_from_slice(&(124_u32 + 4).to_le_bytes());
        extra[56..60].copy_from_slice(&(profile.len() as u32).to_le_bytes());
        bmp_v4_like(124, b"DEBM", &extra, [24, 96, 208], profile)
    }
}

// --- ICO -------------------------------------------------------------------

mod ico_codec {
    use super::*;

    /// A 32 bpp `BI_RGB` icon DIB `width` wide whose header stores
    /// `stored_height` (colour rows plus mask rows, negative for top-down),
    /// followed by `rows` of BGRA pixels in storage order and `mask`.
    fn dib32_rows(width: i32, stored_height: i32, rows: &[[u8; 4]], mask: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&40_u32.to_le_bytes());
        bytes.extend_from_slice(&width.to_le_bytes());
        bytes.extend_from_slice(&stored_height.to_le_bytes());
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&32_u16.to_le_bytes());
        bytes.extend_from_slice(&[0; 24]);
        for [r, g, b, a] in rows {
            bytes.extend_from_slice(&[*b, *g, *r, *a]);
        }
        bytes.extend_from_slice(mask);
        bytes
    }

    /// The offset of directory entry `index`'s field at `field`.
    fn entry_field(index: usize, field: usize) -> usize {
        6 + index * 16 + field
    }

    /// A PNG whose `iCCP` profile Skia snaps to sRGB: sRGB colorants with the
    /// sRGB parametric curve.
    fn srgb_parametric_icc_png(color: [u8; 4]) -> Vec<u8> {
        let mut profile = linear_rgb_icc_profile();
        profile.truncate(296);
        profile.extend_from_slice(b"para\0\0\0\0\0\x04\0\0");
        for value in [0x0002_6666_u32, 0xf2a7, 0x0d59, 0x13d0, 0x0a5b, 0, 0] {
            profile.extend_from_slice(&value.to_be_bytes());
        }
        let size = u32::try_from(profile.len()).expect("profile size fits");
        profile[..4].copy_from_slice(&size.to_be_bytes());
        for tag in 4..7 {
            let at = 132 + tag * 12 + 8;
            profile[at..at + 4].copy_from_slice(&40_u32.to_be_bytes());
        }
        let chunk = [
            b"srgb\0\0".as_slice(),
            &miniz_oxide::deflate::compress_to_vec_zlib(&profile, 6),
        ]
        .concat();
        png_with_chunks_before_idat(&solid_png(1, 1, color), &[(*b"iCCP", chunk)])
    }

    #[test]
    fn the_largest_entry_is_selected_before_the_deepest() {
        let ico = ico(&[
            (1, 1, 32, solid_png(1, 1, BLUE)),
            (2, 1, 8, solid_png(2, 1, RED)),
        ]);
        let resolution = resolve_for_png("image/x-icon", &ico);

        assert_eq!(resolution.media_kind, MediaKind::Ico);
        assert_eq!(resolution.asset.media_type, "image/x-icon");
        assert_eq!((resolution.asset.width, resolution.asset.height), (2, 1));
        assert_eq!(pixels(&resolution.asset), [RED, RED]);
    }

    #[test]
    fn equal_areas_prefer_depth_and_exact_ties_the_first_directory_entry() {
        let ico = ico(&[
            (1, 1, 8, solid_png(1, 1, BLUE)),
            (1, 1, 32, solid_png(1, 1, RED)),
            (1, 1, 32, solid_png(1, 1, GREEN)),
        ]);

        assert_eq!(only_pixel(&decode("image/x-icon", &ico)), RED);
    }

    #[test]
    fn a_missing_bit_count_ranks_by_the_colour_count() {
        // Entry one's zero bit count means 16 colours, four bits; entry zero
        // declares two.
        let mut ico = ico(&[
            (1, 1, 2, solid_png(1, 1, BLUE)),
            (1, 1, 0, solid_png(1, 1, RED)),
        ]);
        ico[entry_field(1, 2)] = 16;

        assert_eq!(only_pixel(&decode("image/x-icon", &ico)), RED);
    }

    #[test]
    fn a_cursor_decodes_like_an_icon_but_ranks_by_the_colour_count() {
        // A cursor's bit-count field holds its hotspot: both entries rank as
        // eight bits, so the first wins the tie.
        let mut cursor = ico(&[
            (1, 1, 8, solid_png(1, 1, BLUE)),
            (1, 1, 32, solid_png(1, 1, RED)),
        ]);
        cursor[2] = 2;
        let resolution = resolve_for_png("image/x-icon", &cursor);

        assert_eq!(resolution.media_kind, MediaKind::Ico);
        assert_eq!(only_pixel(&resolution.asset), BLUE);
    }

    #[test]
    fn a_dib_entry_keeps_meaningful_alpha_and_falls_back_to_the_and_mask() {
        let translucent = ico(&[(1, 1, 32, dib32([24, 96, 208, 128], false))]);
        let masked = ico(&[(1, 1, 32, dib32([24, 96, 208, 0], true))]);
        let unmasked = ico(&[(1, 1, 32, dib32([24, 96, 208, 0], false))]);

        assert_eq!(
            only_pixel(&decode("image/x-icon", &translucent)),
            [24, 96, 208, 128]
        );
        assert_eq!(only_pixel(&decode("image/x-icon", &masked)), [0, 0, 0, 0]);
        assert_eq!(
            only_pixel(&decode("image/x-icon", &unmasked)),
            [24, 96, 208, 255]
        );
    }

    #[test]
    fn a_non_zero_alpha_channel_ignores_the_and_mask() {
        let ico = ico(&[(1, 1, 32, dib32([24, 96, 208, 128], true))]);

        assert_eq!(
            only_pixel(&decode("image/x-icon", &ico)),
            [24, 96, 208, 128]
        );
    }

    #[test]
    fn a_dib_without_alpha_reads_its_and_mask() {
        // A 24 bpp DIB 1x1: one padded colour row, then one mask row.
        let mut dib = dib32([24, 96, 208, 0], true);
        dib[14..16].copy_from_slice(&24_u16.to_le_bytes());
        dib.splice(40..44, [208, 96, 24, 0]);
        let masked = ico(&[(1, 1, 24, dib.clone())]);
        dib[44] = 0;
        let unmasked = ico(&[(1, 1, 24, dib)]);

        assert_eq!(only_pixel(&decode("image/x-icon", &masked)), [0, 0, 0, 0]);
        assert_eq!(
            only_pixel(&decode("image/x-icon", &unmasked)),
            [24, 96, 208, 255]
        );
    }

    #[test]
    fn a_top_down_dib_and_its_mask_run_from_the_top_row() {
        let dib = dib32_rows(
            1,
            -4,
            &[[255, 0, 0, 0], [0, 0, 255, 0]],
            &[0x80, 0, 0, 0, 0, 0, 0, 0],
        );
        let asset = decode("image/x-icon", &ico(&[(1, 2, 32, dib)]));

        assert_eq!(pixels(&asset), [[0, 0, 0, 0], BLUE]);
    }

    #[test]
    fn an_odd_stored_height_halves_down() {
        let dib = dib32_rows(1, 5, &[BLUE, RED], &[0; 8]);
        let asset = decode("image/x-icon", &ico(&[(1, 2, 32, dib)]));

        assert_eq!(pixels(&asset), [RED, BLUE]);
    }

    #[test]
    fn the_dib_size_must_equal_the_directory_size_but_its_depth_need_not() {
        let deeper = ico(&[(1, 1, 24, dib32([24, 96, 208, 255], false))]);
        let larger = ico(&[(2, 1, 32, dib32([24, 96, 208, 255], false))]);

        assert_eq!(
            only_pixel(&decode("image/x-icon", &deeper)),
            [24, 96, 208, 255]
        );
        assert_eq!(
            failure_for("image/x-icon", &larger, OutputFormat::Svg),
            ResolveError::IcoDecodeFailed
        );
    }

    #[test]
    fn an_rle_dib_reads_its_mask_from_the_end_of_bitmap_marker() {
        // 16x1 RLE8, blue palette entry one: a 16-pixel run, then the
        // end-of-bitmap marker `00 01`, whose second byte masks pixel 15.
        let mut dib = Vec::new();
        dib.extend_from_slice(&40_u32.to_le_bytes());
        dib.extend_from_slice(&16_i32.to_le_bytes());
        dib.extend_from_slice(&2_i32.to_le_bytes());
        dib.extend_from_slice(&1_u16.to_le_bytes());
        dib.extend_from_slice(&8_u16.to_le_bytes());
        dib.extend_from_slice(&1_u32.to_le_bytes());
        dib.extend_from_slice(&[0; 12]);
        dib.extend_from_slice(&2_u32.to_le_bytes());
        dib.extend_from_slice(&[0; 4]);
        dib.extend_from_slice(&[0, 0, 255, 0, 255, 0, 0, 0]);
        dib.extend_from_slice(&[16, 1, 0, 1, 0, 0]);
        let asset = decode("image/x-icon", &ico(&[(16, 1, 8, dib)]));

        assert_eq!(pixel(&asset, 14, 0), BLUE);
        assert_eq!(pixel(&asset, 15, 0), [0, 0, 0, 0]);
    }

    #[test]
    fn a_truncated_and_mask_fails_only_where_it_is_read() {
        let mut read = dib32([24, 96, 208, 0], false);
        read.truncate(44);
        let mut unread = dib32([24, 96, 208, 128], false);
        unread.truncate(44);

        assert_eq!(
            failure("image/x-icon", &ico(&[(1, 1, 32, read)])),
            ResolveError::IcoDecodeFailed
        );
        assert_eq!(
            only_pixel(&decode("image/x-icon", &ico(&[(1, 1, 32, unread)]))),
            [24, 96, 208, 128]
        );
    }

    #[test]
    fn the_selected_entry_sets_the_dimensions_for_the_svg_target_too() {
        let ico = ico(&[(2, 1, 32, solid_png(2, 1, RED))]);
        let resolution = resolve_for_svg("image/x-icon", &ico);

        assert_eq!((resolution.asset.width, resolution.asset.height), (2, 1));
        assert!(resolution.asset.rgba.is_none());
    }

    #[test]
    fn an_unselected_entry_is_never_read() {
        let mut malformed = solid_png(1, 1, BLUE);
        let last = malformed.len() - 1;
        malformed[last] ^= 1;
        let ico = ico(&[
            (2, 1, 32, solid_png(2, 1, RED)),
            (1, 1, 8, malformed),
            (1, 1, 8, b"garbage".to_vec()),
        ]);

        assert_eq!(pixels(&decode("image/x-icon", &ico)), [RED, RED]);
    }

    #[test]
    fn any_entry_offset_inside_the_directory_fails_the_icon() {
        let mut ico = ico(&[
            (2, 1, 32, solid_png(2, 1, RED)),
            (1, 1, 8, solid_png(1, 1, BLUE)),
        ]);
        let at = entry_field(1, 12);
        ico[at..at + 4].copy_from_slice(&37_u32.to_le_bytes());

        assert_eq!(
            failure_for("image/x-icon", &ico, OutputFormat::Svg),
            ResolveError::IcoDecodeFailed
        );
    }

    #[test]
    fn reserved_fields_planes_and_the_byte_size_are_ignored() {
        let mut ico = ico(&[(1, 1, 32, dib32([24, 96, 208, 128], false))]);
        ico[entry_field(0, 3)] = 9;
        ico[entry_field(0, 4)] = 7;
        for byte_size in [0_u32, 10, 5000] {
            let at = entry_field(0, 8);
            ico[at..at + 4].copy_from_slice(&byte_size.to_le_bytes());

            assert_eq!(
                only_pixel(&decode("image/x-icon", &ico)),
                [24, 96, 208, 128]
            );
        }
    }

    #[test]
    fn a_png_entry_runs_to_its_iend_whatever_its_byte_size() {
        let mut ico = ico(&[(1, 1, 32, solid_png(1, 1, RED))]);
        let at = entry_field(0, 8);
        ico[at..at + 4].copy_from_slice(&4_u32.to_le_bytes());
        ico.extend_from_slice(b"trailing octets");

        assert_eq!(only_pixel(&decode("image/x-icon", &ico)), RED);
    }

    #[test]
    fn a_png_entry_must_report_the_directory_size() {
        let ico = ico(&[(1, 1, 32, solid_png(2, 1, RED))]);

        assert_eq!(
            failure_for("image/x-icon", &ico, OutputFormat::Svg),
            ResolveError::IcoDecodeFailed
        );
    }

    #[test]
    fn a_png_entry_draws_without_its_exif_orientation() {
        let png = png_with_chunks_before_idat(
            &png_rgba(2, 1, &[RED, BLUE]),
            &[(*b"eXIf", exif_orientation_tiff(6))],
        );
        let asset = decode("image/x-icon", &ico(&[(2, 1, 32, png)]));

        assert_eq!(pixels(&asset), [RED, BLUE]);
    }

    #[test]
    fn a_png_entry_is_colour_managed_as_the_same_png_standalone() {
        // Declared divergence: the reference paints a transforming entry
        // transparent.
        let linear = png_with_chunks_before_idat(
            &png_rgba(1, 1, &[GREY_128]),
            &[(*b"gAMA", 100_000_u32.to_be_bytes().to_vec())],
        );
        let snapped = srgb_parametric_icc_png(GREY_128);

        assert_eq!(
            only_pixel(&decode("image/x-icon", &ico(&[(1, 1, 32, linear.clone())]))),
            only_pixel(&decode("image/png", &linear))
        );
        assert_eq!(
            only_pixel(&decode(
                "image/x-icon",
                &ico(&[(1, 1, 32, snapped.clone())])
            )),
            GREY_128
        );
        assert_eq!(only_pixel(&decode("image/png", &snapped)), GREY_128);
    }

    #[test]
    fn overlapping_dib_bitfield_masks_are_rejected() {
        let ico = ico(&[(1, 1, 32, dib_with_overlapping_masks())]);

        assert_eq!(
            failure_for("image/x-icon", &ico, OutputFormat::Svg),
            ResolveError::IcoDecodeFailed
        );
    }
}

// --- Payload authority and sniffing ---------------------------------------

mod payload_authority {
    use super::*;

    /// Resolves the `asset` record under `authority` with `supplemental`
    /// beside `scene`, for the PNG target from a fresh cache.
    fn resolve_authority(
        scene: &SceneDocument,
        supplemental: &SupplementalAssets,
        authority: Authority,
    ) -> Result<Resolution, ResolutionFailure> {
        resolve_with(
            &mut Cache::default(),
            scene,
            supplemental,
            OutputFormat::Png,
            authority,
            |_| Ok(()),
        )
    }

    #[test]
    fn payloads_without_a_known_signature_are_unsupported() {
        let avif_ftyp = b"\x00\x00\x00\x1cftypavif\x00\x00\x00\x00avifmif1miaf";

        assert_eq!(failure("image/avif", avif_ftyp), ResolveError::Unsupported);
        assert_eq!(
            failure("image/png", b"not a png"),
            ResolveError::Unsupported
        );
    }

    #[test]
    fn a_supplemental_record_carries_only_its_advisory_media_type_as_a_claim() {
        let supplemental = SupplementalAssets::builder()
            .asset_with_media_type("asset", png_rgba(1, 1, &[RED]), "image/jpeg")
            .build()
            .expect("supplemental mapping is valid");

        let resolution = resolve_authority(&empty_scene(), &supplemental, Authority::Supplemental)
            .expect("the supplemental PNG decodes");

        assert_eq!(resolution.media_kind, MediaKind::Png);
        assert_eq!(resolution.claims.mime_type.as_deref(), Some("image/jpeg"));
        assert_eq!(resolution.claims.data_url_media_type, None);
        assert_eq!(only_pixel(&resolution.asset), RED);
    }

    #[test]
    fn a_missing_authority_or_record_is_missing() {
        let supplemental = SupplementalAssets::default();

        assert_eq!(
            asset_error(resolve_authority(
                &empty_scene(),
                &supplemental,
                Authority::Missing
            )),
            ResolveError::Missing
        );
        assert_eq!(
            asset_error(resolve_authority(
                &empty_scene(),
                &supplemental,
                Authority::Scene
            )),
            ResolveError::Missing
        );
        assert_eq!(
            asset_error(resolve_authority(
                &empty_scene(),
                &supplemental,
                Authority::Supplemental
            )),
            ResolveError::Missing
        );
    }

    #[test]
    fn malformed_payloads_and_forbidden_locations_have_distinct_recovery() {
        let null_record = SceneDocument::parse(
            br#"{"type":"excalidraw","version":2,"elements":[],"appState":{},"files":{"asset":null}}"#,
        )
        .expect("null file record remains admitted scene content");
        let bad_base64 = SceneDocument::parse(
            br#"{"type":"excalidraw","version":2,"elements":[],"appState":{},"files":{
                "asset":{"id":"asset","mimeType":"image/png","dataURL":"data:image/png;base64,not-base64","created":1}
            }}"#,
        )
        .expect("scene is admitted");
        let no_scheme = SceneDocument::parse(
            br#"{"type":"excalidraw","version":2,"elements":[],"appState":{},"files":{
                "asset":{"id":"asset","mimeType":"image/png","dataURL":"https://example.com/a.png","created":1}
            }}"#,
        )
        .expect("scene is admitted");

        for scene in [null_record, bad_base64] {
            assert_eq!(
                asset_error(resolve_authority(
                    &scene,
                    &SupplementalAssets::default(),
                    Authority::Scene
                )),
                ResolveError::Malformed
            );
        }
        let rejected =
            resolve_authority(&no_scheme, &SupplementalAssets::default(), Authority::Scene)
                .unwrap();
        assert_eq!(rejected.svg_failures.len(), 1);
        assert_eq!(rejected.svg_failures[0].error, ResolveError::Forbidden);
        assert!(
            !rejected.svg_failures[0].placeholder,
            "the cache-only call supplies no safe viewport"
        );
    }
}

// --- The transaction sink ----------------------------------------------------

mod transactions {
    //! The shape of what a resolution asks the meter for: which events are
    //! acquired, released and aborted, in what order, and that a cache hit
    //! asks for nothing. Events are named through `MediaKind`'s own accessors
    //! and no byte or work amount is pinned, so this holds the protocol stable
    //! without pinning the trace itself (ADR-0002).

    use super::*;

    /// One transaction as `(kind, event)`, so a sequence reads as a ledger.
    fn describe(transaction: &IntrinsicTransaction) -> (&'static str, &'static str) {
        match transaction {
            IntrinsicTransaction::Acquire { event, .. } => ("acquire", event),
            IntrinsicTransaction::ReleaseTemporary { event, .. } => ("release", event),
            IntrinsicTransaction::Abort { event, .. } => ("abort", event),
            IntrinsicTransaction::RejectLocalWork { event, .. } => ("reject", event),
            IntrinsicTransaction::PreflightNested { .. } => ("preflight", "nested"),
            IntrinsicTransaction::DiscoverNestedOccurrence { .. } => ("discover", "occurrence"),
            IntrinsicTransaction::DiscoverNestedPixels { .. } => ("discover", "pixels"),
        }
    }

    /// Resolves `scene` for `format` through `cache`, collecting every
    /// transaction the resolution issues.
    fn resolve_collecting(
        cache: &mut Cache,
        scene: &SceneDocument,
        format: OutputFormat,
    ) -> (
        Result<Resolution, ResolutionFailure>,
        Vec<(&'static str, &'static str)>,
    ) {
        let mut events = Vec::new();
        let result = resolve_with(
            cache,
            scene,
            &SupplementalAssets::default(),
            format,
            Authority::Scene,
            |transaction| {
                events.push(describe(&transaction));
                Ok(())
            },
        );
        (result, events)
    }

    /// The kinds of `events` in order, without their event names.
    fn kinds(events: &[(&'static str, &'static str)]) -> Vec<&'static str> {
        events.iter().map(|(kind, _)| *kind).collect()
    }

    /// The event names `events` acquired, in order.
    fn acquired(events: &[(&'static str, &'static str)]) -> Vec<&'static str> {
        events
            .iter()
            .filter(|(kind, _)| *kind == "acquire")
            .map(|(_, event)| *event)
            .collect()
    }

    #[test]
    fn the_direct_svg_target_acquires_only_the_validated_source() {
        let scene = file_record_scene("image/png", &png_rgba(1, 1, &[RED]));

        let (result, events) = resolve_collecting(&mut Cache::default(), &scene, OutputFormat::Svg);

        result.expect("the PNG validates");
        assert_eq!(kinds(&events), ["acquire", "release"]);
        assert_eq!(acquired(&events), [MediaKind::Png.source_event()]);
    }

    #[test]
    fn the_png_target_acquires_source_colour_and_decoded_pixels_in_turn() {
        let scene = file_record_scene("image/jpeg", &jpeg_rgb(1, 1, &[JFIF_BLUE]));

        let (result, events) = resolve_collecting(&mut Cache::default(), &scene, OutputFormat::Png);

        result.expect("the JPEG decodes");
        assert_eq!(
            kinds(&events),
            ["acquire", "release", "acquire", "release", "acquire", "release"]
        );
        assert_eq!(
            acquired(&events),
            [
                MediaKind::Jpeg.source_event(),
                MediaKind::Jpeg.color_event(),
                MediaKind::Jpeg.decoded_event(),
            ]
        );
    }

    #[test]
    fn identical_bytes_resolve_from_the_cache_without_a_transaction() {
        let mut cache = Cache::default();
        let scene = file_record_scene(
            "image/gif",
            &gif(&[gif_frame(&[0], 1, 1, &RED_BLUE_PALETTE, None)]),
        );

        let (first, _) = resolve_collecting(&mut cache, &scene, OutputFormat::Png);
        let (second, events) = resolve_collecting(&mut cache, &scene, OutputFormat::Png);

        let first = first.expect("the GIF decodes");
        let second = second.expect("the GIF resolves again");
        assert!(events.is_empty(), "cached resolution issued {events:?}");
        assert_eq!(first.asset.identity, second.asset.identity);
        assert!(Arc::ptr_eq(
            first.asset.rgba.as_ref().expect("first decode"),
            second.asset.rgba.as_ref().expect("second decode")
        ));
    }

    #[test]
    fn a_validation_failure_aborts_what_it_acquired() {
        let mut png = png_rgba(1, 1, &[RED]);
        mutate_first_png_chunk(&mut png, *b"IDAT", |data| data[0] ^= 0xff);
        let scene = file_record_scene("image/png", &png);

        let (result, events) = resolve_collecting(&mut Cache::default(), &scene, OutputFormat::Png);

        assert_eq!(asset_error(result), ResolveError::DecodeFailed);
        assert_eq!(
            events,
            [
                ("acquire", MediaKind::Png.source_event()),
                ("abort", MediaKind::Png.source_event()),
            ]
        );
    }

    #[test]
    fn a_header_that_does_not_parse_fails_before_anything_is_acquired() {
        let mut truncated = webp_rgba(1, 1, &[RED]);
        truncated.truncate(truncated.len() - 2);
        let scene = file_record_scene("image/webp", &truncated);

        let (result, events) = resolve_collecting(&mut Cache::default(), &scene, OutputFormat::Png);

        assert_eq!(asset_error(result), ResolveError::WebPDecodeFailed);
        assert!(events.is_empty(), "header failure issued {events:?}");
    }

    #[test]
    fn a_refused_transaction_stops_the_resolution_with_the_meter_error() {
        let scene = file_record_scene("image/bmp", &bmp24(1, 1, &[[24, 96, 208]]));
        let refuse = |_| {
            Err(RenderError {
                category: crate::RenderErrorCategory::ResourceLimit,
                code: crate::ErrorCode::REQUEST_INVALID_RESOURCE_LIMIT,
                message: "refused by the test".into(),
                format: OutputFormat::Png,
                element_id: None,
                json_pointer: "".into(),
                details: None,
                report: crate::CompatibilityReport::empty(),
            })
        };

        let result = resolve_with(
            &mut Cache::default(),
            &scene,
            &SupplementalAssets::default(),
            OutputFormat::Png,
            Authority::Scene,
            refuse,
        );

        match result {
            Err(ResolutionFailure::Transaction(error)) => {
                assert_eq!(error.message.as_ref(), "refused by the test");
            }
            Err(other) => panic!("the meter's refusal is passed through: {other:?}"),
            Ok(_) => panic!("the meter's refusal stops the resolution"),
        }
    }
}
