//! The direct SVG backend: `SvgPlan` lowers the resolved scene
//! into `SvgElement`s in the units SVG emits, `measure` and `encode_exact` run
//! the writer twice through a counting and a real `SvgSink` so the artifact is
//! sized before it is allocated, and the plan carries the font bundle, the
//! artifact metadata and the mandatory notices the writer embeds. `sink` is
//! that writer abstraction; `write` spells the document.

mod sink;
mod write;

pub(super) use sink::SvgSink;
pub(super) use write::freedraw_path_data;
#[cfg(test)]
pub(super) use write::push_rough_path;

use std::sync::Arc;

use crate::normalization::NormalizedElementSource;
use crate::{color::CanonicalColor, ColorMode, RenderRequest, Sizing};

use super::metadata::ArtifactMetadata;
use super::notices::MandatoryNotices;
use super::plan::PlannedElement;
use super::resolve::{
    DarkModeImageFilter, FallbackColors, FallbackSource, ResolvedClip, ResolvedElement,
    ResolvedImageAsset, ResolvedImageProjection, ResolvedLabelMask, ResolvedScene,
    ResolvedTransform, StrokeStyle,
};
use super::rough::{Drawing, Fill};
use super::svg_fonts::Bundle;
use super::text::{ResolvedLabel, ResolvedText};
use super::FormatEncodeError;

use sink::CountingWriter;

/// A transform in the units SVG emits: degrees of rotation, logical pixels for
/// translation and centre. `ResolvedTransform` carries radians, so the
/// conversion happens once here rather than at each element variant — the same
/// coordinate-space boundary `PngTransform` marks for device pixels.
#[derive(Clone, Copy, Debug)]
struct SvgTransform {
    translation_x: f64,
    translation_y: f64,
    rotation_degrees: f64,
    center_x: f64,
    center_y: f64,
}

impl SvgTransform {
    fn new(transform: &ResolvedTransform) -> Self {
        Self {
            translation_x: transform.translation_x,
            translation_y: transform.translation_y,
            rotation_degrees: rotation_degrees(transform.angle_radians),
            center_x: transform.center_x,
            center_y: transform.center_y,
        }
    }
}

/// `radians` in degrees. An angle so large that the product overflows is the
/// same turn as its reduction into (-π, π], which the canvas draws for it.
fn rotation_degrees(radians: f64) -> f64 {
    let degrees = 180.0 * radians / std::f64::consts::PI;
    if degrees.is_finite() || !radians.is_finite() {
        return degrees;
    }
    let (sin, cos) = crate::portable_math::sin_cos(radians);
    180.0 * crate::portable_math::atan2(sin, cos) / std::f64::consts::PI
}

#[derive(Debug)]
pub(super) struct SvgPlan {
    logical_width: f64,
    logical_height: f64,
    output_width: f64,
    output_height: f64,
    background: Option<CanonicalColor>,
    fallback_colors: FallbackColors,
    font_bundle: Bundle,
    mandatory_notices: Option<MandatoryNotices>,
    metadata: Option<ArtifactMetadata>,
    elements: Vec<SvgElement>,
    /// Whether the root carries [`THEME_FILTER`]: a dark-mode export, whose
    /// colors stay unlowered beneath it.
    dark_mode: bool,
    /// Every distinct image asset the plan draws, in first-use paint order,
    /// each defined once in `<defs>` and referenced by every element that
    /// draws it.
    image_symbols: Vec<SvgImageSymbol>,
}

/// One shared image definition, the reference export's `<symbol
/// id="image-…"><image href="data:…" width="100%" height="100%"/></symbol>`
/// under its `reuseImages` default: the asset's data URL is written once and
/// each element draws it through a `<use>` sized to its own content
/// rectangle. Identity is the semantic asset identity, so occurrences that
/// decode to the same validated payload share one definition whatever their
/// file ids say; the definition is keyed, like a clip, by the source index of
/// the first element in paint order that draws it, since an element or file
/// id is untrusted input and not a usable XML ID.
#[derive(Debug)]
struct SvgImageSymbol {
    source_index: usize,
    asset: Arc<ResolvedImageAsset>,
}

/// The Filter Effects `saturate` matrix for `s = 1.25`, row-major as a
/// `feColorMatrix type="matrix"` lists it: `0.213 + 0.787s`,
/// `0.715 - 0.715s`, `0.072 - 0.072s` on the red row, `0.213 - 0.213s`,
/// `0.715 + 0.285s`, `0.072 - 0.072s` on the green row and `0.213 - 0.213s`,
/// `0.715 - 0.715s`, `0.072 + 0.928s` on the blue row, alpha untouched. It is
/// the matrix `color::saturate` evaluates for PDF and PNG; spelled out here
/// because `type="saturate"` cannot carry an amount above 1 portably (#161).
/// A macro rather than a `const` so `concat!` can splice the literal.
macro_rules! saturate_1_25_matrix {
    () => {
        "1.19675 -0.17875 -0.018 0 0 -0.05325 1.07125 -0.018 0 0 -0.05325 -0.17875 1.232 0 0 0 0 0 1 0"
    };
}

/// The reference export's dark-mode filter, `THEME_FILTER`. Its SVG carries it
/// on the root element, where the browser applies it to the composited image;
/// the root filter here is that same attribute value, so every viewer that
/// filters the reference's SVG filters this one identically.
pub(super) const THEME_FILTER: &str = "invert(93%) hue-rotate(180deg)";

/// The id of [`IMAGE_INVERT_FILTER_DEFINITION`].
const IMAGE_INVERT_FILTER_ID: &str = "dark-mode-image-invert";

/// The `<defs>` filter standing in for the reference's `IMAGE_INVERT_FILTER`
/// (`invert(100%) hue-rotate(180deg) saturate(1.25)`), which it puts on a
/// raster image's `<use>` under the root `THEME_FILTER`. A nested-SVG image
/// gets no filter of its own and inverts with the root. The definition spells
/// the CSS filter functions as the Filter Effects primitives they are defined
/// as, in sRGB like the functions, so a viewer without filter-function support
/// still applies them.
const IMAGE_INVERT_FILTER_DEFINITION: &str = concat!(
    "<filter id=\"dark-mode-image-invert\" color-interpolation-filters=\"sRGB\">",
    // invert(100%)
    "<feComponentTransfer>",
    "<feFuncR type=\"table\" tableValues=\"1 0\"/>",
    "<feFuncG type=\"table\" tableValues=\"1 0\"/>",
    "<feFuncB type=\"table\" tableValues=\"1 0\"/>",
    "</feComponentTransfer>",
    // hue-rotate(180deg) saturate(1.25). The saturate step is spelled as the
    // explicit Filter Effects saturate matrix for s = 1.25 rather than
    // `type="saturate" values="1.25"`: the primitive's SVG 1.1 domain is
    // [0, 1], and usvg/resvg clamp a larger value to 1, the identity, where
    // the CSS function the reference uses keeps any positive amount (#161).
    // Every viewer evaluates a literal matrix as written.
    "<feColorMatrix type=\"hueRotate\" values=\"180\"/>",
    "<feColorMatrix type=\"matrix\" values=\"",
    saturate_1_25_matrix!(),
    "\"/>",
    "</filter>",
);

/// One SVG plan element: the shared header in SVG units and the kind.
type SvgElement = PlannedElement<SvgTransform, ResolvedClip, SvgElementKind>;

/// What differs per element kind in the SVG plan; everything else an element
/// carries is the [`PlannedElement`] header.
#[derive(Debug)]
enum SvgElementKind {
    RoughOutline {
        source: NormalizedElementSource,
        drawing: Drawing,
        stroke_color: CanonicalColor,
        fill_color: Option<CanonicalColor>,
        stroke_width: f64,
        stroke_style: StrokeStyle,
        source_stroke_width: f64,
        background_color: CanonicalColor,
        label_mask: Option<ResolvedLabelMask>,
        generated_label: Option<ResolvedText>,
        generated_label_color: Option<CanonicalColor>,
        generated_label_offset_y: f64,
    },
    Freedraw {
        source: NormalizedElementSource,
        path_data: String,
        background_fill: Option<Fill>,
        color: CanonicalColor,
        background_color: Option<CanonicalColor>,
        source_stroke_width: f64,
    },
    /// A frame's outline and name. The writer emits no opacity for it: the
    /// reference SVG's frame `<rect>` has no opacity attribute, while its
    /// canvas sets `globalAlpha` from the element's render opacity, which PNG
    /// and PDF follow (format-specific parity, spec 8.1). The frame's opacity
    /// still multiplies its children's, resolved before the plan.
    Frame {
        source: NormalizedElementSource,
        name: Option<SvgFrameName>,
        /// The outline's stroke when the frame paints one: `FRAME_STYLE.strokeColor`
        /// for the color mode, as the reference SVG strokes every frame kind.
        outline_stroke_color: Option<CanonicalColor>,
    },
    Text {
        source: NormalizedElementSource,
        text: ResolvedText,
        color: CanonicalColor,
    },
    Image {
        source: NormalizedElementSource,
        /// The source index keying the `<symbol>` in `<defs>` that holds this
        /// element's asset: the first element in paint order that draws it.
        symbol_source_index: usize,
        /// Whether the element's `<use>` references the dark-mode image
        /// inversion defined in `<defs>`: a raster image in dark mode.
        image_invert_filter: bool,
        /// The image's rounded-corner clip radius when it has a roundness
        /// record; the SVG defines the clip as a `<rect rx ry>`.
        corner_radius: Option<f64>,
        /// The reference export's rounded viewport and content placement.
        projection: ResolvedImageProjection,
        scale: [i8; 2],
    },
    Fallback {
        source: FallbackSource,
        clip_index: usize,
        label: Option<String>,
    },
}

/// A frame's name as its own unrotated, unclipped, fully opaque text element
/// above the frame, the way the reference export synthesizes it.
#[derive(Debug)]
struct SvgFrameName {
    label: ResolvedLabel,
    transform: SvgTransform,
    color: CanonicalColor,
}

impl SvgPlan {
    pub(super) fn new(
        scene: ResolvedScene,
        request: &RenderRequest,
        mut font_bundle: Bundle,
    ) -> Result<Self, FormatEncodeError> {
        let scale = match request.sizing() {
            Sizing::Scale(scale) => scale,
            Sizing::MaxDimension(_) => 1.0,
        };
        let mandatory_notices = font_bundle.take_mandatory_notices();
        let mut elements = Vec::new();
        elements
            .try_reserve_exact(scene.elements.len())
            .map_err(|_| FormatEncodeError::MemoryUnavailable {
                subject_kind: "format_plan",
                subject: "svg_elements",
            })?;
        let mut fallback_index = 0_usize;
        let mut image_symbols = Vec::<SvgImageSymbol>::new();
        for element in scene.elements {
            match element {
                ResolvedElement::Outline(outline) => {
                    if outline.source.id().is_empty() {
                        return Err(FormatEncodeError::Encoding(format!(
                            "outline at {} has no normalized identity",
                            outline.source.pointer()
                        )));
                    }
                    let frame_clips = if outline.generated_label.is_some() {
                        Vec::new()
                    } else {
                        outline.frame_clips
                    };
                    elements.push(SvgElement {
                        frame_clips,
                        transform: SvgTransform::new(&outline.transform),
                        opacity: if outline.generated_label.is_some() {
                            outline.element_opacity
                        } else {
                            outline.opacity
                        },
                        link: outline.link,
                        width: outline.width,
                        height: outline.height,
                        kind: SvgElementKind::RoughOutline {
                            source: outline.source,
                            drawing: outline.drawing,
                            stroke_color: outline.stroke_color,
                            fill_color: outline.fill_color,
                            stroke_width: outline.stroke_width,
                            stroke_style: outline.stroke_style,
                            source_stroke_width: outline.source_stroke_width,
                            background_color: outline.background_color,
                            label_mask: outline.label_mask,
                            generated_label: outline.generated_label,
                            generated_label_color: outline.generated_label_color,
                            generated_label_offset_y: outline.generated_label_offset_y,
                        },
                    });
                }
                ResolvedElement::Freedraw(freedraw) => {
                    elements.push(SvgElement {
                        frame_clips: freedraw.frame_clips,
                        transform: SvgTransform::new(&freedraw.transform),
                        opacity: freedraw.opacity,
                        link: freedraw.link,
                        width: freedraw.width,
                        height: freedraw.height,
                        kind: SvgElementKind::Freedraw {
                            source: freedraw.source,
                            path_data: freedraw_path_data(&freedraw.path)?,
                            background_fill: freedraw.background_fill,
                            color: freedraw.color,
                            background_color: freedraw.background_color,
                            source_stroke_width: freedraw.source_stroke_width,
                        },
                    });
                }
                ResolvedElement::Frame(frame) => {
                    elements.push(SvgElement {
                        frame_clips: frame.frame_clips,
                        transform: SvgTransform::new(&frame.transform),
                        opacity: frame.opacity,
                        link: frame.link,
                        width: frame.width,
                        height: frame.height,
                        kind: SvgElementKind::Frame {
                            source: frame.source,
                            name: frame.name.map(|name| SvgFrameName {
                                transform: SvgTransform::new(&name.transform),
                                color: name.color,
                                label: name.label,
                            }),
                            outline_stroke_color: frame.outline.map(|outline| outline.stroke_color),
                        },
                    });
                    fallback_index = fallback_index.saturating_add(1);
                }
                ResolvedElement::Text(text) => {
                    elements.push(SvgElement {
                        frame_clips: text.frame_clips,
                        transform: SvgTransform::new(&text.transform),
                        opacity: text.opacity,
                        link: text.link,
                        width: text.width,
                        height: text.height,
                        kind: SvgElementKind::Text {
                            source: text.source,
                            text: text.text,
                            color: text.color,
                        },
                    });
                }
                ResolvedElement::Image(image) => {
                    // Elements are visited in paint order, so the first
                    // element drawing an asset keys its shared definition.
                    let symbol_source_index = match image_symbols
                        .iter()
                        .find(|symbol| symbol.asset.identity == image.asset.identity)
                    {
                        Some(symbol) => symbol.source_index,
                        None => {
                            image_symbols.try_reserve(1).map_err(|_| {
                                FormatEncodeError::MemoryUnavailable {
                                    subject_kind: "format_plan",
                                    subject: "svg_image_symbols",
                                }
                            })?;
                            image_symbols.push(SvgImageSymbol {
                                source_index: image.source.source_index(),
                                asset: Arc::clone(&image.asset),
                            });
                            image.source.source_index()
                        }
                    };
                    elements.push(SvgElement {
                        frame_clips: image.frame_clips,
                        transform: SvgTransform::new(&image.transform),
                        opacity: image.opacity,
                        link: image.link,
                        width: image.width,
                        height: image.height,
                        kind: SvgElementKind::Image {
                            source: image.source,
                            symbol_source_index,
                            image_invert_filter: image.dark_mode_filter
                                == Some(DarkModeImageFilter::RasterImage),
                            corner_radius: image.corner_clip.map(|clip| clip.radius),
                            projection: image.projection,
                            scale: image.scale,
                        },
                    });
                }
                ResolvedElement::ImageFallback(_) => {}
                ResolvedElement::Fallback(fallback) => {
                    elements.push(SvgElement {
                        frame_clips: fallback.frame_clips,
                        transform: SvgTransform::new(&fallback.transform),
                        opacity: fallback.opacity,
                        link: fallback.link,
                        width: fallback.width,
                        height: fallback.height,
                        kind: SvgElementKind::Fallback {
                            source: fallback.source,
                            clip_index: fallback_index,
                            label: fallback.label.map(|label| label.display_text),
                        },
                    });
                    fallback_index = fallback_index.saturating_add(1);
                }
            }
        }
        Ok(Self {
            logical_width: scene.logical_width,
            logical_height: scene.logical_height,
            output_width: scene.logical_width * scale,
            output_height: scene.logical_height * scale,
            background: scene.include_background.then_some(scene.background_color),
            fallback_colors: scene.fallback_colors,
            font_bundle,
            mandatory_notices,
            metadata: None,
            elements,
            dark_mode: request.color_mode() == ColorMode::Dark,
            image_symbols,
        })
    }

    /// Whether any image references the dark-mode image inversion, which
    /// `<defs>` then defines once.
    fn uses_image_invert_filter(&self) -> bool {
        self.elements.iter().any(|element| {
            matches!(
                element.kind,
                SvgElementKind::Image {
                    image_invert_filter: true,
                    ..
                }
            )
        })
    }

    pub(super) fn measure(&self) -> Result<usize, FormatEncodeError> {
        let mut output = CountingWriter::default();
        self.write(&mut output)?;
        if output.overflowed {
            Err(svg_size_error())
        } else {
            Ok(output.len)
        }
    }

    pub(super) fn encode_exact(&self, measured: usize) -> Result<Vec<u8>, FormatEncodeError> {
        let mut svg = String::new();
        svg.try_reserve_exact(measured)
            .map_err(|_| FormatEncodeError::MemoryUnavailable {
                subject_kind: "artifact",
                subject: "svg_output",
            })?;
        self.write(&mut svg)?;
        if svg.len() != measured {
            return Err(FormatEncodeError::Encoding(
                "measured and encoded SVG lengths differ".to_owned(),
            ));
        }
        Ok(svg.into_bytes())
    }

    pub(super) fn set_metadata(&mut self, metadata: ArtifactMetadata) {
        self.metadata = Some(metadata);
        self.mandatory_notices = None;
    }

    pub(super) fn take_mandatory_notices(&mut self) -> Option<MandatoryNotices> {
        self.mandatory_notices.take()
    }

    pub(super) fn source_count(&self) -> usize {
        self.elements.len()
    }

    pub(super) fn source(&self, index: usize) -> Option<(Option<&str>, &str)> {
        self.elements.get(index).map(|element| match &element.kind {
            SvgElementKind::RoughOutline { source, .. }
            | SvgElementKind::Freedraw { source, .. }
            | SvgElementKind::Frame { source, .. }
            | SvgElementKind::Text { source, .. }
            | SvgElementKind::Image { source, .. } => (Some(source.id()), source.pointer()),
            SvgElementKind::Fallback { source, .. } => {
                (source.id.as_deref(), source.pointer.as_str())
            }
        })
    }
}

fn svg_size_error() -> FormatEncodeError {
    FormatEncodeError::Encoding("SVG plan exceeds serializer capacity".to_owned())
}
