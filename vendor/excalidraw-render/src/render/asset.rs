//! Image assets: `Cache` is the deep module — one semantic asset
//! identity per compatibility revision, media kind and byte string; validation,
//! color normalization and decoding each charged once through the caller's
//! `IntrinsicTransaction`s and shared across every occurrence and every target
//! of a frame batch — and this file keeps the media kinds, the error and
//! transaction vocabulary, the per-kind dispatch (`Cache::resolve`, the nested
//! raster drive and the color-normalization selection), and the thin adapters
//! over the `bmp` and `ico` format modules. `payload` fetches the bytes from a
//! scene file record or a supplemental asset, `png`, `jpeg`, `gif` and `webp`
//! validate and decode their formats, `orientation` reads TIFF orientation and
//! stands decoded pixels upright, `color` normalizes them to straight and
//! premultiplied sRGB, and `svg` sanitizes nested SVG.

mod bmp;
mod color;
mod gif;
mod hdr;
mod icc_lut;
mod icc_matrix;
mod ico;
mod jpeg;
mod jpeg_markers;
mod orientation;
mod payload;
mod png;
mod skia_srgb;
pub(super) mod svg;
mod svg_coordinates;
#[cfg(test)]
mod tests;
mod webp;
#[cfg(test)]
pub(crate) use color::{with_cms_failure, with_cms_failure_after, CmsSite};

pub(super) use payload::data_url_decoded_len;

use color::{finish_rgba, prepare_png_color_normalization, ColorNormalization};
use gif::{
    decode_gif_default_frame, gif_animation_validation_work, read_gif_dimensions, validate_gif,
    GifSemantics, GIF87A_SIGNATURE, GIF89A_SIGNATURE,
};
use jpeg::{
    decode_static_jpeg, jpeg_color_normalization, read_jpeg_dimensions, validate_static_jpeg,
    JpegSemantics, JPEG_SIGNATURE,
};
use payload::{payload, Payload};
pub(super) use png::encode_normalized_png;
use png::{
    decode_static_png, png_animation_validation_work, read_png_ihdr, validate_static_png,
    PngSemantics, PNG_SIGNATURE,
};
use webp::{
    decode_webp_default_frame, read_webp_dimensions, validate_webp, webp_animation_validation_work,
    webp_icc_profile, WebPSemantics, WEBP_RIFF_SIGNATURE, WEBP_SIGNATURE,
};

use std::collections::BTreeMap;
use std::sync::Arc;

use sha2::{Digest as _, Sha256};

use crate::{
    CompatibilityRevision, OutputFormat, RenderError, ResourceLimits, SceneDocument,
    SupplementalAssets,
};

use super::resolve::ResolvedImageAsset;

const BMP_SIGNATURE: &[u8; 2] = b"BM";
const ICO_SIGNATURE: &[u8; 4] = b"\0\0\x01\0";
const CUR_SIGNATURE: &[u8; 4] = b"\0\0\x02\0";
const IDENTITY_DOMAIN: &[u8] = b"excalidraw-render:semantic-asset:v1\0";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum MediaKind {
    Png,
    Jpeg,
    Gif,
    WebP,
    Bmp,
    Ico,
    Svg,
}

impl MediaKind {
    const fn tag(self) -> u64 {
        match self {
            Self::Png => 1,
            Self::Jpeg => 2,
            Self::Gif => 3,
            Self::WebP => 4,
            Self::Bmp => 5,
            Self::Ico => 6,
            Self::Svg => 8,
        }
    }

    pub(super) const fn media_type(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
            Self::Gif => "image/gif",
            Self::WebP => "image/webp",
            Self::Bmp => "image/bmp",
            Self::Ico => "image/x-icon",
            Self::Svg => "image/svg+xml",
        }
    }

    const fn source_event(self) -> &'static str {
        match self {
            Self::Png => "semantic_png_source",
            Self::Jpeg => "semantic_jpeg_source",
            Self::Gif => "semantic_gif_source",
            Self::WebP => "semantic_webp_source",
            Self::Bmp => "semantic_bmp_source",
            Self::Ico => "semantic_ico_source",
            Self::Svg => "semantic_svg_source",
        }
    }

    const fn color_event(self) -> &'static str {
        match self {
            Self::Png => "png_color_normalize",
            Self::Jpeg => "jpeg_color_normalize",
            Self::Gif => "gif_color_normalize",
            Self::WebP => "webp_color_normalize",
            Self::Bmp => "bmp_color_normalize",
            Self::Ico => "ico_color_normalize",
            Self::Svg => "svg_color_normalize",
        }
    }

    const fn decoded_event(self) -> &'static str {
        match self {
            Self::Png => "decoded_png_rgba",
            Self::Jpeg => "decoded_jpeg_rgba",
            Self::Gif => "decoded_gif_rgba",
            Self::WebP => "decoded_webp_rgba",
            Self::Bmp => "decoded_bmp_rgba",
            Self::Ico => "decoded_ico_rgba",
            Self::Svg => "decoded_svg_rgba",
        }
    }
}

pub(super) fn is_persistent_event(event: &str) -> bool {
    matches!(
        event,
        "semantic_png_source"
            | "decoded_png_rgba"
            | "semantic_jpeg_source"
            | "decoded_jpeg_rgba"
            | "semantic_gif_source"
            | "decoded_gif_rgba"
            | "semantic_webp_source"
            | "decoded_webp_rgba"
            | "semantic_bmp_source"
            | "decoded_bmp_rgba"
            | "semantic_ico_source"
            | "decoded_ico_rgba"
            | "semantic_svg_source"
            | "decoded_svg_rgba"
    )
}

#[derive(Clone, Copy, Debug)]
pub(super) enum Authority {
    Scene,
    Supplemental,
    Missing,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ResolveError {
    Missing,
    Forbidden,
    Malformed,
    Unsupported,
    DecodeFailed,
    JpegDecodeFailed,
    GifDecodeFailed,
    WebPDecodeFailed,
    BmpDecodeFailed,
    IcoDecodeFailed,
    SvgDecodeFailed,
    AssetBytes { attempted: u64 },
    Dimensions { attempted: u64 },
    Pixels { attempted: u64 },
    AssetWork { attempted: u64 },
    SvgComplexity { attempted: u64 },
    SvgDepth { attempted: u64 },
    Coordinate { value_bits: u64 },
    Memory,
}

#[derive(Debug)]
pub(super) enum ResolutionFailure {
    Asset(ResolveError),
    Transaction(RenderError),
    Memory,
}

impl From<ResolveError> for ResolutionFailure {
    fn from(error: ResolveError) -> Self {
        Self::Asset(error)
    }
}

#[derive(Debug)]
pub(super) enum IntrinsicTransaction {
    PreflightNested {
        asset_entries: u64,
        asset_bytes: u64,
        decoded_pixels: u64,
        total_work_units: u64,
    },
    DiscoverNestedOccurrence {
        asset_bytes: u64,
    },
    DiscoverNestedPixels {
        decoded_pixels: u64,
    },
    RejectLocalWork {
        event: &'static str,
        identity: [u8; 32],
        total_work_units: u64,
    },
    Acquire {
        event: &'static str,
        identity: [u8; 32],
        persistent_working_bytes: u64,
        temporary_working_bytes: u64,
        decoded_pixels: u64,
        total_work_units: u64,
    },
    ReleaseTemporary {
        event: &'static str,
        identity: [u8; 32],
        bytes: u64,
    },
    Abort {
        event: &'static str,
        identity: [u8; 32],
        bytes: u64,
    },
}

pub(super) struct Claims {
    pub(super) mime_type: Option<String>,
    pub(super) data_url_media_type: Option<String>,
}

pub(super) struct Resolution {
    pub(super) asset: Arc<ResolvedImageAsset>,
    pub(super) svg_tree: Option<Arc<usvg::Tree>>,
    pub(super) claims: Claims,
    pub(super) animated: bool,
    pub(super) media_kind: MediaKind,
    pub(super) svg_removals: Vec<svg::Removal>,
    pub(super) svg_failures: Vec<svg::Failure>,
    pub(super) svg_media_claims: Vec<svg::MediaClaim>,
}

struct ValidatedRaster {
    identity: [u8; 32],
    bytes: Arc<Vec<u8>>,
    media_kind: MediaKind,
    source_width: u32,
    source_height: u32,
    width: u32,
    height: u32,
    semantics: RasterSemantics,
}

enum RasterSemantics {
    Png(PngSemantics),
    Jpeg(JpegSemantics),
    Gif(GifSemantics),
    WebP(WebPSemantics),
    Bmp(bmp::Info),
    Ico(IcoSemantics),
}

impl RasterSemantics {
    const fn orientation(&self) -> Option<u8> {
        match self {
            Self::Png(semantics) => semantics.orientation,
            Self::Jpeg(semantics) => semantics.orientation,
            Self::Gif(_) => None,
            Self::WebP(_) => None,
            Self::Bmp(_) => None,
            Self::Ico(_) => None,
        }
    }

    const fn animated(&self) -> bool {
        match self {
            Self::Png(semantics) => semantics.animation_frames > 1,
            Self::Gif(semantics) => semantics.animation_frames > 1,
            Self::Jpeg(_) => false,
            Self::WebP(semantics) => semantics.animation_frames > 1,
            Self::Bmp(_) => false,
            Self::Ico(_) => false,
        }
    }
}

#[derive(Clone)]
enum IcoSemantics {
    /// The PNG entry's octets within the file and its semantics, without an
    /// orientation: the reference draws an icon's PNG unrotated.
    Png {
        offset: usize,
        length: usize,
        semantics: PngSemantics,
    },
    /// The DIB entry, read in place with offsets into the whole file.
    Dib(bmp::Info),
}

struct DecodedRaster {
    rgba: Arc<Vec<u8>>,
    premultiplied_rgba: Arc<Vec<u8>>,
}

#[derive(Default)]
pub(super) struct Cache {
    validated: BTreeMap<[u8; 32], Arc<ValidatedRaster>>,
    decoded: BTreeMap<[u8; 32], Arc<DecodedRaster>>,
    svg: BTreeMap<[u8; 32], Arc<svg::SanitizedSvg>>,
    normalized_nested_rasters: BTreeMap<[u8; 32], Arc<NormalizedNestedRaster>>,
    svg_discovery_owners: std::collections::BTreeSet<String>,
    svg_diagnostic_owners: std::collections::BTreeSet<String>,
}

impl Cache {
    #[cfg(test)]
    #[allow(clippy::too_many_arguments)]
    pub(super) fn resolve<F>(
        &mut self,
        scene: &SceneDocument,
        supplemental: &SupplementalAssets,
        revision: CompatibilityRevision,
        format: OutputFormat,
        limits: &ResourceLimits,
        file_id: &str,
        authority: Authority,
        transact: F,
    ) -> Result<Resolution, ResolutionFailure>
    where
        F: FnMut(IntrinsicTransaction) -> Result<(), RenderError>,
    {
        self.resolve_with_viewport(
            scene,
            supplemental,
            revision,
            format,
            limits,
            file_id,
            authority,
            None,
            transact,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn resolve_with_viewport<F>(
        &mut self,
        scene: &SceneDocument,
        supplemental: &SupplementalAssets,
        revision: CompatibilityRevision,
        format: OutputFormat,
        limits: &ResourceLimits,
        file_id: &str,
        authority: Authority,
        fallback_dimensions: Option<(f64, f64)>,
        mut transact: F,
    ) -> Result<Resolution, ResolutionFailure>
    where
        F: FnMut(IntrinsicTransaction) -> Result<(), RenderError>,
    {
        let payload = match payload(
            scene,
            supplemental,
            file_id,
            authority,
            limits.max_asset_bytes(),
        ) {
            Ok(payload) => payload,
            Err(ResolveError::Forbidden) => {
                return root_svg_recovery(
                    ResolveError::Forbidden,
                    Claims {
                        mime_type: None,
                        data_url_media_type: None,
                    },
                    fallback_dimensions,
                    limits,
                    revision,
                    &mut transact,
                )
            }
            Err(error) => return Err(error.into()),
        };
        if svg::looks_like_svg(&payload.bytes) {
            return self.resolve_svg(
                revision,
                format,
                limits,
                file_id,
                authority,
                payload,
                fallback_dimensions,
                &mut transact,
            );
        }
        let media_kind = sniff_media_kind(&payload.bytes)?;
        let identity = semantic_identity(revision, media_kind, &payload.bytes);
        let (source_width, source_height) = match media_kind {
            MediaKind::Png => read_png_ihdr(&payload.bytes)?,
            MediaKind::Jpeg => read_jpeg_dimensions(&payload.bytes)?,
            MediaKind::Gif => read_gif_dimensions(&payload.bytes)?,
            MediaKind::WebP => read_webp_dimensions(&payload.bytes)?,
            MediaKind::Bmp => read_bmp_dimensions(&payload.bytes)?,
            MediaKind::Ico => read_ico_dimensions(&payload.bytes)?,
            MediaKind::Svg => unreachable!("SVG assets use the sanitizer path"),
        };
        let pixels = u64::from(source_width)
            .checked_mul(u64::from(source_height))
            .ok_or(ResolveError::Pixels {
                attempted: u64::MAX,
            })?;
        let payload_bytes = u64::try_from(payload.bytes.len()).unwrap_or(u64::MAX);
        let animation_validation_work =
            match media_kind {
                MediaKind::Png => png_animation_validation_work(&payload.bytes, pixels)?,
                MediaKind::Jpeg => 0,
                MediaKind::Gif => gif_animation_validation_work(&payload.bytes, pixels)?,
                MediaKind::WebP => webp_animation_validation_work(&payload.bytes)?,
                MediaKind::Bmp => bmp::validation_work(&payload.bytes)
                    .map_err(|_| ResolveError::BmpDecodeFailed)?,
                MediaKind::Ico => ico::validation_work(&payload.bytes)
                    .map_err(|_| ResolveError::IcoDecodeFailed)?,
                MediaKind::Svg => unreachable!("SVG assets use the sanitizer path"),
            };
        let validation_working_bytes = if self.validated.contains_key(&identity) {
            0
        } else {
            512_u64.saturating_add(logical_aligned_bytes(
                u64::try_from(payload.bytes.len()).unwrap_or(u64::MAX),
            ))
        };
        let validated = if let Some(validated) = self.validated.get(&identity) {
            Arc::clone(validated)
        } else {
            let validation_work = payload_bytes.checked_add(animation_validation_work).ok_or(
                ResolveError::AssetWork {
                    attempted: u64::MAX,
                },
            )?;
            if validation_work > limits.max_asset_work_units() {
                transact(IntrinsicTransaction::RejectLocalWork {
                    event: media_kind.source_event(),
                    identity,
                    total_work_units: validation_work,
                })
                .map_err(ResolutionFailure::Transaction)?;
                return Err(ResolveError::AssetWork {
                    attempted: validation_work,
                }
                .into());
            }
            let decoder_workspace = decoder_workspace_bytes(source_width, source_height)?
                .checked_add(if media_kind == MediaKind::Ico {
                    ico::directory_workspace(&payload.bytes)
                        .map_err(|_| ResolveError::IcoDecodeFailed)?
                } else {
                    0
                })
                .ok_or(ResolveError::Memory)?;
            transact(IntrinsicTransaction::Acquire {
                event: media_kind.source_event(),
                identity,
                persistent_working_bytes: validation_working_bytes,
                temporary_working_bytes: decoder_workspace,
                decoded_pixels: 0,
                total_work_units: validation_work,
            })
            .map_err(ResolutionFailure::Transaction)?;
            let validation = match media_kind {
                MediaKind::Png => {
                    validate_static_png(&payload.bytes).map(|(width, height, semantics)| {
                        (width, height, RasterSemantics::Png(semantics))
                    })
                }
                MediaKind::Jpeg => {
                    validate_static_jpeg(&payload.bytes).map(|(width, height, semantics)| {
                        (width, height, RasterSemantics::Jpeg(semantics))
                    })
                }
                MediaKind::Gif => validate_gif(&payload.bytes).map(|(width, height, semantics)| {
                    (width, height, RasterSemantics::Gif(semantics))
                }),
                MediaKind::WebP => {
                    validate_webp(&payload.bytes).map(|(width, height, semantics)| {
                        (width, height, RasterSemantics::WebP(semantics))
                    })
                }
                MediaKind::Bmp => validate_bmp(&payload.bytes).map(|(width, height, semantics)| {
                    (width, height, RasterSemantics::Bmp(semantics))
                }),
                MediaKind::Ico => validate_ico(&payload.bytes).map(|(width, height, semantics)| {
                    (width, height, RasterSemantics::Ico(semantics))
                }),
                MediaKind::Svg => unreachable!("SVG assets use the sanitizer path"),
            };
            let validation = match validation {
                Ok(validation) => validation,
                Err(error) => {
                    transact(IntrinsicTransaction::Abort {
                        event: media_kind.source_event(),
                        identity,
                        bytes: validation_working_bytes.saturating_add(decoder_workspace),
                    })
                    .map_err(ResolutionFailure::Transaction)?;
                    return Err(error.into());
                }
            };
            transact(IntrinsicTransaction::ReleaseTemporary {
                event: "raster_validation_workspace",
                identity,
                bytes: decoder_workspace,
            })
            .map_err(ResolutionFailure::Transaction)?;
            let (width, height, semantics) = validation;
            let validated = Arc::new(ValidatedRaster {
                identity,
                bytes: Arc::new(payload.bytes),
                media_kind,
                source_width,
                source_height,
                width,
                height,
                semantics,
            });
            self.validated.insert(identity, Arc::clone(&validated));
            validated
        };

        let decoded = if format == OutputFormat::Svg {
            None
        } else if let Some(decoded) = self.decoded.get(&identity) {
            Some(Arc::clone(decoded))
        } else {
            if let Some(dimension) = [validated.width, validated.height]
                .into_iter()
                .find(|side| *side > limits.max_raster_dimension())
            {
                return Err(ResolveError::Dimensions {
                    attempted: u64::from(dimension),
                }
                .into());
            }
            if pixels > limits.max_decoded_pixels_per_asset() {
                return Err(ResolveError::Pixels { attempted: pixels }.into());
            }
            let work = payload_bytes
                .checked_mul(5)
                .and_then(|work| {
                    work.checked_add(if media_kind == MediaKind::Png {
                        animation_validation_work
                    } else {
                        0
                    })
                })
                .and_then(|work| {
                    pixels
                        .checked_mul(4)
                        .and_then(|pixel_work| work.checked_add(pixel_work))
                })
                .ok_or(ResolveError::AssetWork {
                    attempted: u64::MAX,
                })?;
            if work > limits.max_asset_work_units() {
                transact(IntrinsicTransaction::RejectLocalWork {
                    event: validated.media_kind.decoded_event(),
                    identity,
                    total_work_units: work,
                })
                .map_err(ResolutionFailure::Transaction)?;
                return Err(ResolveError::AssetWork { attempted: work }.into());
            }
            let decoder_workspace = decoder_workspace_bytes(validated.width, validated.height)?;
            transact(IntrinsicTransaction::Acquire {
                event: validated.media_kind.color_event(),
                identity,
                persistent_working_bytes: 0,
                temporary_working_bytes: decoder_workspace,
                decoded_pixels: 0,
                total_work_units: payload_bytes,
            })
            .map_err(ResolutionFailure::Transaction)?;
            let color_normalization = match prepare_color_normalization(
                &validated.bytes,
                &validated.semantics,
                decoder_workspace,
            ) {
                Ok(normalization) => normalization,
                Err(error) => {
                    transact(IntrinsicTransaction::Abort {
                        event: validated.media_kind.color_event(),
                        identity,
                        bytes: decoder_workspace,
                    })
                    .map_err(ResolutionFailure::Transaction)?;
                    return Err(error.into());
                }
            };
            transact(IntrinsicTransaction::ReleaseTemporary {
                event: "raster_color_normalize_workspace",
                identity,
                bytes: decoder_workspace,
            })
            .map_err(ResolutionFailure::Transaction)?;
            let decode_working_bytes = 256_u64
                .checked_add(pixels.checked_mul(4).ok_or(ResolveError::Memory)?)
                .ok_or(ResolveError::Memory)?;
            transact(IntrinsicTransaction::Acquire {
                event: validated.media_kind.decoded_event(),
                identity,
                persistent_working_bytes: decode_working_bytes,
                temporary_working_bytes: decoder_workspace,
                decoded_pixels: pixels,
                total_work_units: payload_bytes
                    .saturating_mul(3)
                    .saturating_add(pixels.saturating_mul(4)),
            })
            .map_err(ResolutionFailure::Transaction)?;
            let decoded = match validated.media_kind {
                MediaKind::Png => decode_static_png(
                    &validated.bytes,
                    pixels,
                    color_normalization,
                    validated.semantics.orientation(),
                ),
                MediaKind::Jpeg => decode_static_jpeg(
                    &validated.bytes,
                    pixels,
                    color_normalization,
                    validated.semantics.orientation(),
                ),
                MediaKind::Gif => decode_gif_default_frame(
                    &validated.bytes,
                    validated.width,
                    validated.height,
                    pixels,
                ),
                MediaKind::WebP => decode_webp_default_frame(
                    &validated.bytes,
                    validated.source_width,
                    validated.source_height,
                    pixels,
                    color_normalization,
                ),
                MediaKind::Bmp => {
                    decode_bmp(&validated.bytes, color_normalization, &validated.semantics)
                }
                MediaKind::Ico => decode_ico(
                    &validated.bytes,
                    pixels,
                    color_normalization,
                    &validated.semantics,
                ),
                MediaKind::Svg => unreachable!("SVG assets use the sanitizer path"),
            };
            let decoded = match decoded {
                Ok(decoded) => Arc::new(decoded),
                Err(error) => {
                    transact(IntrinsicTransaction::Abort {
                        event: validated.media_kind.decoded_event(),
                        identity,
                        bytes: decode_working_bytes.saturating_add(decoder_workspace),
                    })
                    .map_err(ResolutionFailure::Transaction)?;
                    return Err(error.into());
                }
            };
            transact(IntrinsicTransaction::ReleaseTemporary {
                event: "raster_decoder_workspace",
                identity,
                bytes: decoder_workspace,
            })
            .map_err(ResolutionFailure::Transaction)?;
            self.decoded.insert(identity, Arc::clone(&decoded));
            Some(decoded)
        };

        let (width, height) = if format == OutputFormat::Svg {
            (validated.source_width, validated.source_height)
        } else {
            (validated.width, validated.height)
        };
        Ok(Resolution {
            asset: Arc::new(ResolvedImageAsset {
                identity: validated.identity,
                source_bytes: Arc::clone(&validated.bytes),
                media_type: validated.media_kind.media_type(),
                width,
                height,
                rgba: decoded.as_ref().map(|decoded| Arc::clone(&decoded.rgba)),
                premultiplied_rgba: decoded
                    .as_ref()
                    .map(|decoded| Arc::clone(&decoded.premultiplied_rgba)),
                sanitized_svg: None,
                svg_effects: Vec::new(),
                svg_fallback_pointer: None,
            }),
            svg_tree: None,
            claims: payload.claims,
            animated: validated.semantics.animated(),
            media_kind: validated.media_kind,
            svg_removals: Vec::new(),
            svg_failures: Vec::new(),
            svg_media_claims: Vec::new(),
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn resolve_svg<F>(
        &mut self,
        revision: CompatibilityRevision,
        format: OutputFormat,
        limits: &ResourceLimits,
        file_id: &str,
        authority: Authority,
        payload: Payload,
        fallback_dimensions: Option<(f64, f64)>,
        transact: &mut F,
    ) -> Result<Resolution, ResolutionFailure>
    where
        F: FnMut(IntrinsicTransaction) -> Result<(), RenderError>,
    {
        let source_identity = semantic_identity(revision, MediaKind::Svg, &payload.bytes);
        let mut pending_nested_rasters = BTreeMap::new();
        let (sanitized, discovered_live) = if let Some(svg) = self.svg.get(&source_identity) {
            (Arc::clone(svg), false)
        } else {
            let source_len = u64::try_from(payload.bytes.len()).unwrap_or(u64::MAX);
            if source_len > limits.max_asset_work_units() {
                transact(IntrinsicTransaction::RejectLocalWork {
                    event: MediaKind::Svg.source_event(),
                    identity: source_identity,
                    total_work_units: source_len,
                })
                .map_err(ResolutionFailure::Transaction)?;
                return Err(ResolveError::AssetWork {
                    attempted: source_len,
                }
                .into());
            }
            let temporary = 1_048_576;
            transact(IntrinsicTransaction::Acquire {
                event: MediaKind::Svg.source_event(),
                identity: source_identity,
                persistent_working_bytes: 0,
                temporary_working_bytes: temporary,
                decoded_pixels: 0,
                total_work_units: source_len,
            })
            .map_err(ResolutionFailure::Transaction)?;
            let sanitized = match svg::sanitize(
                &payload.bytes,
                limits,
                revision,
                format,
                &self.normalized_nested_rasters,
                &mut pending_nested_rasters,
                transact,
            ) {
                Ok(svg) => Arc::new(svg),
                Err(ResolutionFailure::Asset(error)) => {
                    transact(IntrinsicTransaction::Abort {
                        event: MediaKind::Svg.source_event(),
                        identity: source_identity,
                        bytes: temporary,
                    })
                    .map_err(ResolutionFailure::Transaction)?;
                    if matches!(
                        error,
                        ResolveError::SvgComplexity { .. } | ResolveError::SvgDepth { .. }
                    ) {
                        return root_svg_recovery(
                            error,
                            payload.claims,
                            fallback_dimensions,
                            limits,
                            revision,
                            transact,
                        );
                    }
                    return Err(error.into());
                }
                Err(ResolutionFailure::Transaction(error)) => {
                    return Err(ResolutionFailure::Transaction(error));
                }
                Err(ResolutionFailure::Memory) => return Err(ResolutionFailure::Memory),
            };
            if sanitized.work_units > limits.max_asset_work_units() {
                transact(IntrinsicTransaction::RejectLocalWork {
                    event: "derived_sanitized_svg",
                    identity: source_identity,
                    total_work_units: sanitized.work_units.saturating_sub(source_len),
                })
                .map_err(ResolutionFailure::Transaction)?;
                transact(IntrinsicTransaction::Abort {
                    event: MediaKind::Svg.source_event(),
                    identity: source_identity,
                    bytes: temporary,
                })
                .map_err(ResolutionFailure::Transaction)?;
                return Err(ResolveError::AssetWork {
                    attempted: sanitized.work_units,
                }
                .into());
            }
            transact(IntrinsicTransaction::Acquire {
                event: "derived_sanitized_svg",
                identity: source_identity,
                persistent_working_bytes: sanitized.working_bytes,
                temporary_working_bytes: 0,
                decoded_pixels: 0,
                total_work_units: sanitized.work_units.saturating_sub(source_len),
            })
            .map_err(ResolutionFailure::Transaction)?;
            transact(IntrinsicTransaction::ReleaseTemporary {
                event: "svg_sanitizer_workspace",
                identity: source_identity,
                bytes: temporary,
            })
            .map_err(ResolutionFailure::Transaction)?;
            for admission in &sanitized.nested_admissions {
                let transaction = match admission {
                    svg::NestedAdmission::Recovery { .. } => continue,
                    svg::NestedAdmission::Occurrence { asset_bytes } => {
                        IntrinsicTransaction::DiscoverNestedOccurrence {
                            asset_bytes: *asset_bytes,
                        }
                    }
                    svg::NestedAdmission::DecodedPixels { decoded_pixels } => {
                        IntrinsicTransaction::DiscoverNestedPixels {
                            decoded_pixels: *decoded_pixels,
                        }
                    }
                    svg::NestedAdmission::Work {
                        identity,
                        total_work_units,
                    } => IntrinsicTransaction::Acquire {
                        event: "nested_asset_intrinsic_work",
                        identity: *identity,
                        persistent_working_bytes: 0,
                        temporary_working_bytes: 0,
                        decoded_pixels: 0,
                        total_work_units: *total_work_units,
                    },
                };
                transact(transaction).map_err(ResolutionFailure::Transaction)?;
            }
            self.normalized_nested_rasters
                .append(&mut pending_nested_rasters);
            self.svg.insert(source_identity, Arc::clone(&sanitized));
            (sanitized, true)
        };
        if self.svg_discovery_owners.insert(file_id.to_owned()) && !discovered_live {
            for admission in &sanitized.nested_admissions {
                svg::replay_recovery(admission, transact)?;
                if let svg::NestedAdmission::Occurrence { asset_bytes } = admission {
                    transact(IntrinsicTransaction::DiscoverNestedOccurrence {
                        asset_bytes: *asset_bytes,
                    })
                    .map_err(ResolutionFailure::Transaction)?;
                }
            }
        }
        let identity = semantic_identity(revision, MediaKind::Svg, &sanitized.bytes);
        let report_svg_diagnostics = self.svg_diagnostic_owners.insert(file_id.to_owned());
        Ok(Resolution {
            asset: Arc::new(ResolvedImageAsset {
                identity,
                source_bytes: Arc::new(sanitized.bytes.clone()),
                media_type: MediaKind::Svg.media_type(),
                width: sanitized.width,
                height: sanitized.height,
                rgba: None,
                premultiplied_rgba: None,
                sanitized_svg: Some(Arc::new(sanitized.bytes.clone())),
                svg_effects: sanitized.effects.clone(),
                svg_fallback_pointer: match authority {
                    Authority::Scene => Some(format!(
                        "/files/{}/dataURL",
                        file_id.replace('~', "~0").replace('/', "~1")
                    )),
                    Authority::Supplemental | Authority::Missing => Some(String::new()),
                },
            }),
            svg_tree: Some(Arc::clone(&sanitized.tree)),
            claims: payload.claims,
            animated: false,
            media_kind: MediaKind::Svg,
            svg_removals: if report_svg_diagnostics {
                sanitized.removals.clone()
            } else {
                Vec::new()
            },
            svg_failures: if report_svg_diagnostics {
                sanitized.failures.clone()
            } else {
                Vec::new()
            },
            svg_media_claims: if report_svg_diagnostics {
                sanitized.media_claims.clone()
            } else {
                Vec::new()
            },
        })
    }
}

fn root_svg_recovery(
    error: ResolveError,
    claims: Claims,
    viewport: Option<(f64, f64)>,
    limits: &ResourceLimits,
    revision: CompatibilityRevision,
    transact: &mut dyn FnMut(IntrinsicTransaction) -> Result<(), RenderError>,
) -> Result<Resolution, ResolutionFailure> {
    use base64::Engine;
    let url = match viewport {
        Some((w, h)) => svg::resource_placeholder_url(w, h, limits, revision, 0, transact)?,
        None => None,
    };
    let placeholder = url.is_some();
    let mut bytes = Vec::new();
    if let Some(url) = url {
        let encoded = url
            .strip_prefix("data:image/svg+xml;base64,")
            .ok_or(ResolveError::SvgDecodeFailed)?;
        bytes
            .try_reserve_exact(encoded.len())
            .map_err(|_| ResolutionFailure::Memory)?;
        bytes.resize(encoded.len(), 0);
        let n = base64::engine::general_purpose::STANDARD
            .decode_slice(encoded, &mut bytes)
            .map_err(|_| ResolveError::SvgDecodeFailed)?;
        bytes.truncate(n);
    } else {
        let empty = b"<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"1\" height=\"1\"/>";
        bytes
            .try_reserve_exact(empty.len())
            .map_err(|_| ResolutionFailure::Memory)?;
        bytes.extend_from_slice(empty);
    }
    let identity = semantic_identity(revision, MediaKind::Svg, &bytes);
    transact(IntrinsicTransaction::Acquire {
        event: "nested_recovery_viewer",
        identity,
        persistent_working_bytes: 512 + (bytes.len() as u64).div_ceil(64) * 64,
        temporary_working_bytes: 1_048_576,
        decoded_pixels: 0,
        total_work_units: bytes.len() as u64,
    })
    .map_err(ResolutionFailure::Transaction)?;
    let tree = usvg::Tree::from_data(&bytes, &svg::viewer_options()?)
        .map_err(|_| ResolveError::SvgDecodeFailed)?;
    transact(IntrinsicTransaction::ReleaseTemporary {
        event: "nested_recovery_viewer",
        identity,
        bytes: 1_048_576,
    })
    .map_err(ResolutionFailure::Transaction)?;
    let mut failures = Vec::new();
    failures
        .try_reserve_exact(1)
        .map_err(|_| ResolutionFailure::Memory)?;
    failures.push(svg::Failure {
        may_lose_semantics: true,
        path: Vec::new(),
        error,
        placeholder,
    });
    let bytes = Arc::new(bytes);
    Ok(Resolution {
        asset: Arc::new(ResolvedImageAsset {
            identity,
            source_bytes: Arc::clone(&bytes),
            media_type: MediaKind::Svg.media_type(),
            width: tree.size().to_int_size().width(),
            height: tree.size().to_int_size().height(),
            rgba: None,
            premultiplied_rgba: None,
            sanitized_svg: Some(bytes),
            svg_effects: Vec::new(),
            svg_fallback_pointer: None,
        }),
        svg_tree: Some(Arc::new(tree)),
        claims,
        animated: false,
        media_kind: MediaKind::Svg,
        svg_removals: Vec::new(),
        svg_failures: failures,
        svg_media_claims: Vec::new(),
    })
}

struct NormalizedNestedRaster {
    source_media_kind: MediaKind,
    normalized_png: Vec<u8>,
}

struct ValidatedNestedRaster {
    media_kind: MediaKind,
    source_width: u32,
    source_height: u32,
    width: u32,
    height: u32,
    pixels: u64,
    validation_work: u64,
    semantics: RasterSemantics,
}

fn normalize_nested_raster(
    bytes: &[u8],
    validated: ValidatedNestedRaster,
    format: OutputFormat,
) -> Result<NormalizedNestedRaster, ResolveError> {
    let color_normalization = prepare_color_normalization(
        bytes,
        &validated.semantics,
        decoder_workspace_bytes(validated.width, validated.height)?,
    )?;
    let webp_draw_time = validated.media_kind == MediaKind::WebP
        && format == OutputFormat::Png
        && matches!(color_normalization, ColorNormalization::None);
    let decoded = match validated.media_kind {
        MediaKind::Png => decode_static_png(
            bytes,
            validated.pixels,
            color_normalization,
            validated.semantics.orientation(),
        ),
        MediaKind::Jpeg => decode_static_jpeg(
            bytes,
            validated.pixels,
            color_normalization,
            validated.semantics.orientation(),
        ),
        MediaKind::Gif => {
            decode_gif_default_frame(bytes, validated.width, validated.height, validated.pixels)
        }
        MediaKind::WebP => decode_webp_default_frame(
            bytes,
            validated.source_width,
            validated.source_height,
            validated.pixels,
            color_normalization,
        ),
        MediaKind::Bmp => decode_bmp(bytes, color_normalization, &validated.semantics),
        MediaKind::Ico => decode_ico(
            bytes,
            validated.pixels,
            color_normalization,
            &validated.semantics,
        ),
        MediaKind::Svg => unreachable!("nested SVG uses recursive sanitization"),
    }?;
    let normalized_png = if webp_draw_time {
        // Nested PNG rendering consumes a normalized PNG through resvg. Encode
        // a straight representation that reconstructs the WebP draw-time copy,
        // rather than discarding its codec-specific alpha quantization. PDF must
        // continue to receive the original canonical straight samples.
        let mut pixels = try_copy(&decoded.premultiplied_rgba)?;
        for pixel in pixels.as_chunks_mut::<4>().0 {
            let alpha = u16::from(pixel[3]);
            for channel in &mut pixel[..3] {
                *channel = (u16::from(*channel) * 255 + alpha / 2)
                    .checked_div(alpha)
                    .unwrap_or(0) as u8;
            }
        }
        encode_normalized_png(validated.width, validated.height, &pixels)?
    } else {
        encode_normalized_png(validated.width, validated.height, &decoded.rgba)?
    };
    Ok(NormalizedNestedRaster {
        source_media_kind: validated.media_kind,
        normalized_png,
    })
}

fn validate_nested_raster(
    bytes: &[u8],
    limits: &ResourceLimits,
) -> Result<(MediaKind, u64, u64), ResolveError> {
    let validated = validate_nested_raster_semantics(bytes, limits)?;
    Ok((
        validated.media_kind,
        validated.pixels,
        validated.validation_work,
    ))
}

fn validate_nested_raster_semantics(
    bytes: &[u8],
    limits: &ResourceLimits,
) -> Result<ValidatedNestedRaster, ResolveError> {
    let media_kind = sniff_media_kind(bytes)?;
    let (source_width, source_height) = match media_kind {
        MediaKind::Png => read_png_ihdr(bytes)?,
        MediaKind::Jpeg => read_jpeg_dimensions(bytes)?,
        MediaKind::Gif => read_gif_dimensions(bytes)?,
        MediaKind::WebP => read_webp_dimensions(bytes)?,
        MediaKind::Bmp => read_bmp_dimensions(bytes)?,
        MediaKind::Ico => read_ico_dimensions(bytes)?,
        MediaKind::Svg => unreachable!("nested SVG uses recursive sanitization"),
    };
    let source_pixels = u64::from(source_width)
        .checked_mul(u64::from(source_height))
        .ok_or(ResolveError::Memory)?;
    let animation_work = match media_kind {
        MediaKind::Png => png_animation_validation_work(bytes, source_pixels)?,
        MediaKind::Jpeg => 0,
        MediaKind::Gif => gif_animation_validation_work(bytes, source_pixels)?,
        MediaKind::WebP => webp_animation_validation_work(bytes)?,
        MediaKind::Bmp => bmp::validation_work(bytes).map_err(|_| ResolveError::BmpDecodeFailed)?,
        MediaKind::Ico => ico::validation_work(bytes).map_err(|_| ResolveError::IcoDecodeFailed)?,
        MediaKind::Svg => unreachable!("nested SVG uses recursive sanitization"),
    };
    let work = u64::try_from(bytes.len())
        .unwrap_or(u64::MAX)
        .checked_add(animation_work)
        .ok_or(ResolveError::AssetWork {
            attempted: u64::MAX,
        })?;
    if work > limits.max_asset_work_units() {
        return Err(ResolveError::AssetWork { attempted: work });
    }
    let (width, height, semantics) = match media_kind {
        MediaKind::Png => validate_static_png(bytes)
            .map(|(width, height, semantics)| (width, height, RasterSemantics::Png(semantics)))?,
        MediaKind::Jpeg => validate_static_jpeg(bytes)
            .map(|(width, height, semantics)| (width, height, RasterSemantics::Jpeg(semantics)))?,
        MediaKind::Gif => validate_gif(bytes)
            .map(|(width, height, semantics)| (width, height, RasterSemantics::Gif(semantics)))?,
        MediaKind::WebP => validate_webp(bytes)
            .map(|(width, height, semantics)| (width, height, RasterSemantics::WebP(semantics)))?,
        MediaKind::Bmp => validate_bmp(bytes)
            .map(|(width, height, semantics)| (width, height, RasterSemantics::Bmp(semantics)))?,
        MediaKind::Ico => validate_ico(bytes)
            .map(|(width, height, semantics)| (width, height, RasterSemantics::Ico(semantics)))?,
        MediaKind::Svg => unreachable!("nested SVG uses recursive sanitization"),
    };
    let pixels = u64::from(width)
        .checked_mul(u64::from(height))
        .ok_or(ResolveError::Memory)?;
    Ok(ValidatedNestedRaster {
        media_kind,
        source_width,
        source_height,
        width,
        height,
        pixels,
        validation_work: work,
        semantics,
    })
}

/// Classify a coordinate-rejected image without allocating or decoding its
/// payload. Scene authority wins even when its record is malformed.
pub(super) fn rejected_image_may_have_semantics(
    scene: &SceneDocument,
    supplemental: &SupplementalAssets,
    file_id: &str,
    linked: bool,
) -> bool {
    if let Some(record) = (0..scene.file_record_count())
        .filter_map(|i| scene.file_record_at(i))
        .find(|r| r.key() == file_id)
    {
        return match record.data_url() {
            crate::scene::SourceField::Value(url) => {
                linked || svg::rejected_payload_may_have_semantics(url)
            }
            _ => false,
        };
    }
    supplemental.get(file_id).is_some_and(|asset| {
        linked || sniff_media_kind(&asset.bytes()[..asset.bytes().len().min(48)]).is_err()
    })
}

fn sniff_media_kind(bytes: &[u8]) -> Result<MediaKind, ResolveError> {
    if bytes.starts_with(PNG_SIGNATURE) {
        Ok(MediaKind::Png)
    } else if bytes.starts_with(JPEG_SIGNATURE) {
        Ok(MediaKind::Jpeg)
    } else if bytes.starts_with(GIF87A_SIGNATURE) || bytes.starts_with(GIF89A_SIGNATURE) {
        Ok(MediaKind::Gif)
    } else if bytes.starts_with(WEBP_RIFF_SIGNATURE) && bytes.get(8..12) == Some(WEBP_SIGNATURE) {
        Ok(MediaKind::WebP)
    } else if bytes.starts_with(BMP_SIGNATURE) {
        Ok(MediaKind::Bmp)
    } else if bytes.starts_with(ICO_SIGNATURE) || bytes.starts_with(CUR_SIGNATURE) {
        Ok(MediaKind::Ico)
    } else {
        Err(ResolveError::Unsupported)
    }
}

fn read_bmp_dimensions(bytes: &[u8]) -> Result<(u32, u32), ResolveError> {
    bmp::dimensions(bytes).map_err(|_| ResolveError::BmpDecodeFailed)
}

fn validate_bmp(bytes: &[u8]) -> Result<(u32, u32, bmp::Info), ResolveError> {
    let info = bmp::validate(bytes).map_err(|_| ResolveError::BmpDecodeFailed)?;
    // Blink fails the whole image when an embedded profile does not parse,
    // before it reports a size; direct SVG must reject it as well.
    if let Some(profile) = info.embedded_profile(bytes) {
        bmp_embedded_normalization(profile)?;
    }
    Ok((info.width, info.height, info))
}

/// The normalization an `MBED` profile selects. A profile that does not parse
/// fails the image; one Skia snaps to sRGB draws untransformed; a parsed
/// non-RGB profile keeps the project's strict unsupported recovery.
fn bmp_embedded_normalization(profile: &[u8]) -> Result<ColorNormalization, ResolveError> {
    match color::parse_icc_profile(profile) {
        Ok(ColorNormalization::Profile(profile))
            if profile.color_space != moxcms::DataColorSpace::Rgb =>
        {
            Err(ResolveError::Unsupported)
        }
        Ok(ColorNormalization::Profile(profile)) if skia_srgb::snaps_to_srgb(&profile) => {
            Ok(ColorNormalization::None)
        }
        Ok(ColorNormalization::Lut(lut)) if lut.first().is_none_or(|lut| lut.gray()) => {
            Err(ResolveError::Unsupported)
        }
        Err(ResolveError::Memory) => Err(ResolveError::Memory),
        Err(_) => Err(ResolveError::BmpDecodeFailed),
        other => other,
    }
}

fn decode_bmp(
    bytes: &[u8],
    color_normalization: ColorNormalization,
    semantics: &RasterSemantics,
) -> Result<DecodedRaster, ResolveError> {
    let RasterSemantics::Bmp(info) = semantics else {
        return Err(ResolveError::BmpDecodeFailed);
    };
    let mut rgba = bmp::decode(bytes, info).map_err(|_| ResolveError::BmpDecodeFailed)?;
    color::normalize_rgba(&mut rgba, color_normalization)?;
    finish_rgba(rgba)
}

fn read_ico_dimensions(bytes: &[u8]) -> Result<(u32, u32), ResolveError> {
    let selected = ico::selected_entry(bytes).map_err(|_| ResolveError::IcoDecodeFailed)?;
    match ico::payload(bytes, selected).map_err(|_| ResolveError::IcoDecodeFailed)? {
        ico::Payload::Png => {
            let payload =
                ico::png_bytes(bytes, selected).map_err(|_| ResolveError::IcoDecodeFailed)?;
            let dimensions = read_png_ihdr(payload).map_err(|_| ResolveError::IcoDecodeFailed)?;
            if dimensions != (selected.width, selected.height) {
                return Err(ResolveError::IcoDecodeFailed);
            }
        }
        ico::Payload::Dib => {
            ico::dib_dimensions(bytes, selected).map_err(|_| ResolveError::IcoDecodeFailed)?;
        }
    }
    Ok((selected.width, selected.height))
}

/// Validates the entry the reference selects; the other entries' images are
/// never read.
fn validate_ico(bytes: &[u8]) -> Result<(u32, u32, IcoSemantics), ResolveError> {
    let selected = ico::selected_entry(bytes).map_err(|_| ResolveError::IcoDecodeFailed)?;
    let semantics =
        match ico::payload(bytes, selected).map_err(|_| ResolveError::IcoDecodeFailed)? {
            ico::Payload::Png => {
                let payload =
                    ico::png_bytes(bytes, selected).map_err(|_| ResolveError::IcoDecodeFailed)?;
                let (_, _, mut semantics) =
                    validate_static_png(payload).map_err(|_| ResolveError::IcoDecodeFailed)?;
                if read_png_ihdr(payload).map_err(|_| ResolveError::IcoDecodeFailed)?
                    != (selected.width, selected.height)
                {
                    return Err(ResolveError::IcoDecodeFailed);
                }
                semantics.orientation = None;
                IcoSemantics::Png {
                    offset: selected.offset,
                    length: payload.len(),
                    semantics,
                }
            }
            ico::Payload::Dib => {
                let info = ico::validate_dib(bytes, selected)
                    .map_err(|_| ResolveError::IcoDecodeFailed)?;
                // As for a BMP, an embedded profile that does not parse fails the
                // image.
                if info.embedded_profile(bytes).is_some() {
                    bmp_color_normalization(bytes, &info, ResolveError::IcoDecodeFailed)?;
                }
                IcoSemantics::Dib(info)
            }
        };
    Ok((selected.width, selected.height, semantics))
}

fn decode_ico(
    bytes: &[u8],
    pixels: u64,
    color_normalization: ColorNormalization,
    semantics: &RasterSemantics,
) -> Result<DecodedRaster, ResolveError> {
    let RasterSemantics::Ico(semantics) = semantics else {
        return Err(ResolveError::IcoDecodeFailed);
    };
    match semantics {
        IcoSemantics::Png {
            offset,
            length,
            semantics,
        } => {
            let payload = bytes
                .get(*offset..offset + length)
                .ok_or(ResolveError::IcoDecodeFailed)?;
            decode_static_png(payload, pixels, color_normalization, semantics.orientation)
                .map_err(|_| ResolveError::IcoDecodeFailed)
        }
        IcoSemantics::Dib(info) => {
            let mut rgba = bmp::decode(bytes, info).map_err(|_| ResolveError::IcoDecodeFailed)?;
            color::normalize_rgba(&mut rgba, color_normalization)?;
            finish_rgba(rgba)
        }
    }
}

fn prepare_color_normalization(
    bytes: &[u8],
    semantics: &RasterSemantics,
    workspace_bytes: u64,
) -> Result<ColorNormalization, ResolveError> {
    match semantics {
        RasterSemantics::Png(semantics) => {
            prepare_png_color_normalization(bytes, semantics, workspace_bytes)
        }
        RasterSemantics::Jpeg(_) => jpeg_color_normalization(bytes),
        RasterSemantics::Gif(_) => Ok(ColorNormalization::None),
        RasterSemantics::WebP(_) => match webp_icc_profile(bytes)? {
            // Blink keeps only a profile that parses as an RGB profile and
            // otherwise ignores it; only a physical allocation failure stays.
            Some(profile) => match color::parse_icc_profile(profile) {
                // Skia draws a profile it snaps to sRGB untransformed.
                Ok(ColorNormalization::Profile(profile))
                    if profile.color_space != moxcms::DataColorSpace::Rgb
                        || skia_srgb::snaps_to_srgb(&profile) =>
                {
                    Ok(ColorNormalization::None)
                }
                Ok(ColorNormalization::Lut(lut)) if lut.first().is_some_and(|lut| lut.gray()) => {
                    Ok(ColorNormalization::None)
                }
                Err(ResolveError::Memory) => Err(ResolveError::Memory),
                Err(_) => Ok(ColorNormalization::None),
                other => other,
            },
            None => Ok(ColorNormalization::None),
        },
        RasterSemantics::Bmp(info) => {
            bmp_color_normalization(bytes, info, ResolveError::BmpDecodeFailed)
        }
        // Declared divergence: the reference paints a PNG entry whose colour
        // needs a real transform fully transparent; the project renders it
        // colour-managed as the same PNG renders standalone.
        RasterSemantics::Ico(IcoSemantics::Png {
            offset,
            length,
            semantics,
        }) => {
            let payload = bytes
                .get(*offset..offset + length)
                .ok_or(ResolveError::IcoDecodeFailed)?;
            prepare_png_color_normalization(payload, semantics, workspace_bytes)
                .map_err(|_| ResolveError::IcoDecodeFailed)
        }
        RasterSemantics::Ico(IcoSemantics::Dib(info)) => {
            bmp_color_normalization(bytes, info, ResolveError::IcoDecodeFailed)
        }
    }
}

/// The normalization a DIB's V4/V5 colour space selects, for a BMP or an ICO
/// entry; a failing embedded profile reports as `failure`.
fn bmp_color_normalization(
    bytes: &[u8],
    info: &bmp::Info,
    failure: ResolveError,
) -> Result<ColorNormalization, ResolveError> {
    match &info.color_space {
        bmp::ColorSpace::Srgb => Ok(ColorNormalization::None),
        bmp::ColorSpace::Calibrated { to_xyz_d50, gammas } => {
            color::calibrated_rgb_normalization(to_xyz_d50, *gammas)
        }
        bmp::ColorSpace::Embedded { .. } => match info.embedded_profile(bytes) {
            Some(profile) => bmp_embedded_normalization(profile).map_err(|error| {
                if matches!(error, ResolveError::BmpDecodeFailed) {
                    failure
                } else {
                    error
                }
            }),
            None => Err(failure),
        },
    }
}

fn decoder_workspace_bytes(width: u32, height: u32) -> Result<u64, ResolveError> {
    1_048_576_u64
        .checked_add(32_u64.saturating_mul(u64::from(width.max(height))))
        .ok_or(ResolveError::Memory)
}

fn semantic_identity(
    revision: CompatibilityRevision,
    media_kind: MediaKind,
    bytes: &[u8],
) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(IDENTITY_DOMAIN);
    hasher.update(revision.get().to_le_bytes());
    hasher.update(media_kind.tag().to_le_bytes());
    hasher.update(u64::try_from(bytes.len()).unwrap_or(u64::MAX).to_le_bytes());
    hasher.update(bytes);
    hasher.finalize().into()
}

fn try_copy(bytes: &[u8]) -> Result<Vec<u8>, ResolveError> {
    let mut copy = Vec::new();
    copy.try_reserve_exact(bytes.len())
        .map_err(|_| ResolveError::Memory)?;
    copy.extend_from_slice(bytes);
    Ok(copy)
}

fn try_owned(value: &str) -> Result<String, ResolveError> {
    let mut owned = String::new();
    owned
        .try_reserve_exact(value.len())
        .map_err(|_| ResolveError::Memory)?;
    owned.push_str(value);
    Ok(owned)
}

fn logical_aligned_bytes(bytes: u64) -> u64 {
    bytes.saturating_add(63) / 64 * 64
}
